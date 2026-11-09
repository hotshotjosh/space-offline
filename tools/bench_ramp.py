#!/usr/bin/env python3
"""Ramp bots into ONE system and find where the server (and eve-bots) bend.

Driven by tools/bench-ramp.sh (builds, makes the accounts, then runs this);
see docs/guides/bench-ramp.md. Std-only. The pure parts (roster, tick-line
parsing, step summary, knee and collapse) are unit-tested in
tools/test_bench_ramp.py; `run()` is the orchestrator.

    bench_ramp.py --bin DIR --out RUNDIR --world WORLD --tokens FILE ...
    bench_ramp.py --summarize RUNDIR     # redo steps.tsv / summary from logs
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.append(str(Path(__file__).resolve().parent))
import soak_summary as ss  # noqa: E402  (p95, the bots' act-line shape)

TICK_S = 0.25       # push.rs TICK
WINDOW_TICKS = 40   # push.rs REPORT_EVERY
BUDGET_MS = TICK_S * 1000
KNEE_FRACTION = 0.5  # of the tick budget
COLLAPSE_WINDOWS = 3  # consecutive windows with a tick over budget
MIN_WINDOWS = 6      # a step with fewer tick-health windows is flagged (hold 60 s)
HALF_SHIFT_PCT = 30.0  # work p95 moving more than this between a step's halves is flagged
HALF_SHIFT_FLOOR = 0.1  # ... but only when the step's own p95 is above this share of the tick budget
HALF_SHIFT_SKIP = 3    # windows dropped from the start of a step. The tick lines come every 10 s at a phase
                       # unrelated to the step start, so three skipped windows cover any burst within the
                       # first 20 s of a step (logins, undock, warp); a later one (a fleet joining at 25-30 s)
                       # can land in the first kept window and flag a settled step [A]

# The tick-health line, with the lane's trailing counters optional (an older
# server prints none of them).
TICK = re.compile(
    r"\[world\] tick health: (?P<ticks>\d+) ticks, late mean (?P<late_mean>[\d.]+) ms max (?P<late_max>[\d.]+) ms, "
    r"over budget (?P<over>\d+), world lock wait mean (?P<wait_mean>[\d.]+) ms max (?P<wait_max>[\d.]+) ms, "
    r"world lock held mean (?P<held_mean>[\d.]+) ms max (?P<held_max>[\d.]+) ms, work max (?P<work_max>[\d.]+) ms, "
    r"skipped (?P<skipped>\d+)"
    r"(?:, work p95 (?P<work_p95>[\d.]+) ms, out (?P<out_frames>\d+) frames (?P<out_bytes>\d+) bytes, "
    r"destiny (?P<dest_frames>\d+) frames (?P<dest_bytes>\d+) bytes"
    r"(?: parts (?P<dest_parts>\d+) merged (?P<dest_merged>\d+))?)?"
    r"(?:, slice_max (?P<slice_max>[\d.]+) ms)?"
)
LIVE_PILOTS = re.compile(r"^\[live\].*? pilots (\d+) \(in space (\d+)\)")
# The `[live]` line's world size: "systems hot N warm M ... | npcs A rocks B".
LIVE_WORLD = re.compile(r"^\[live\].*? systems hot (\d+) warm (\d+) .*? npcs (\d+)(?: rocks (\d+))?")
# The bots' `[stats]` line (eve-bot/src/stats.rs `Reporter::line`): the process
# figures, then one `name X.Yms/s n=N Zus` section per phase.
STATS_HEAD = re.compile(
    r"^\[stats\] bots=(?P<bots>\d+) cpu=(?P<cpu>[\d.]+)% rss=(?P<rss>\d+)MB \(\d+kB/bot\) "
    r"threads=(?P<threads>\d+) in=(?P<kbps>\d+)kB/s"
)
STATS_SECTION = re.compile(r"\| (?P<name>[a-z]+) (?P<ms>[\d.]+)ms/s n=\d+ \d+us")
STATS_PHASES = ("decode", "observe", "destiny", "refresh", "decide", "absorb", "log")

COLUMNS = [
    "step", "target_bots", "pilots", "in_space", "windows",
    "work_p95_ms", "work_max_ms", "held_mean_ms", "held_p95_ms", "held_max_ms",
    "wait_max_ms", "late_max_ms", "over_budget", "skipped",
    "srv_cpu_pct", "srv_rss_mb", "srv_threads",
    "bots_cpu_pct", "bots_rss_mb", "bots_threads",
    "out_kBps", "destiny_kBps", "destiny_frames_per_tick", "destiny_bytes_per_bot_s",
    "act_failed", "act_refused", "usererrors",
    # PA-3 lock slices: the longest single write-lock slice (max and p95 over windows; 0 before PA-3)
    "slice_max_ms", "slice_p95_ms",
    # PA-0 profile (live_status::prof): ms per tick, means over the step
    "take_due_ms", "filter_ms", "audience_ms", "session_changes_ms", "save_max_ms",
    "clock_occ_pct", "payload_occ_pct", "payload_turns_per_s", "ro_guess_turn_pct",
    "writer_frames_per_s", "writer_pass_max_ms", "deflate_ms_total", "longest_write_ms",
    # PA-12a read turns (payload_occ_pct above is the EXCLUSIVE occupancy from here on)
    "ro_turns_per_s", "ro_served_turn_pct", "ro_fallbacks",
    # SCALE-BENCH: the world's size from the last `[live]` line of the step
    "hot_systems", "warm_systems", "npcs", "rocks",
    # SCALE-BENCH: the bots' own `[stats]` lines (--stats), means over the step; 0 without them
    "stats_n", "stats_cpu_pct", "stats_in_kBps",
    "decode_ms_s", "observe_ms_s", "destiny_ms_s", "refresh_ms_s", "decide_ms_s", "absorb_ms_s", "log_ms_s",
    # SCALE-BENCH: is the hold long enough? work p95 of the step's first half and its second half, % apart
    "p95_half_shift_pct", "half_p95_max_ms",
    # SCALE-BENCH: kit shopping that failed (`MISSING` lines of the bots' log in the step)
    "missing_lines",
]


# ---- the scenario ---------------------------------------------------------

def parse_mix(text):
    """`miners=60,ratters=25,fighters=15` -> {kind: weight}; kinds checked."""
    mix = {}
    for part in text.split(","):
        k, _, v = part.partition("=")
        k = k.strip()
        if k not in KINDS or not v.strip().isdigit() or int(v) < 0:
            raise ValueError(f"--mix {text!r}: want {'|'.join(KINDS)}=N pairs")
        mix[k] = int(v)
    if sum(mix.values()) == 0:
        raise ValueError("--mix: all weights are zero")
    return {k: v for k, v in mix.items() if v}


# kind -> (group prefix, role, skills, ship, side)
KINDS = {
    "miners": ("bm", "mine", "role", "Hulk", "civ"),
    "ratters": ("br", "sites", "role", "Vexor", "civ"),
    "fighters": ("bf", "hunter", "role", "Rifter", "red"),
    # SCALE-BENCH: the other side of a fight (`--mix fighters=1,blues=1 --brawl`): a
    # different corporation, so `hostile_to` counts it hostile to the reds.
    "blues": ("bg", "hunter", "role", "Rifter", "blu"),
}
SPREAD_GROUP = 5     # miners per fleet in --grid spread: one boss (a belt) per five
FLEET_CAP = 250      # Fleets::MAX_MEMBERS_IN_FLEET is 256


def split_counts(total, mix):
    """Largest-remainder split of `total` bots over the mix."""
    weight = sum(mix.values())
    base = {k: total * v // weight for k, v in mix.items()}
    rest = total - sum(base.values())
    order = sorted(mix, key=lambda k: (-(total * mix[k] % weight), k))
    for k in order[:rest]:
        base[k] += 1
    return base


def roster(total, mix, grid, systems=None, one_side=False):
    """The roster TSV text for `total` bots, and the bot names (in order).

    --grid same: one group per kind (one fleet, so the miners follow one boss
    to one belt; a fight or a site is one place too), split at the fleet cap.
    --grid spread: miners in fleets of five, each boss picking a belt of its
    own; the other kinds as in `same` (they roam by themselves).

    `systems` (--systems K, a list of K system ids): the spread miners' fleets
    are seated round-robin in them, by the roster's own `system` column (a
    group is one fleet, so one home, one boss picking a belt). None or one
    system: no column, the run's `--roster-home` applies, as before.

    `one_side` (--prep): every hostile-side row is seated on the fighters' side
    so nobody fights while the kits are bought (the bots re-/devcorp at each
    run's login, so the ramp's own roster puts them back on their side)."""
    counts = split_counts(total, mix)
    rows, names = [], []
    for kind, n in counts.items():
        if n == 0:
            continue
        prefix, role, skills, ship, side = KINDS[kind]
        if one_side and side == "blu":
            side = KINDS["fighters"][4]
        size = SPREAD_GROUP if (grid == "spread" and kind == "miners") else FLEET_CAP
        gi = 0
        while n > 0:
            c = min(n, size)
            gi += 1
            group = f"{prefix}{gi}x"
            row = f"{group}\t{side}\t{role}\t{c}\t{skills}\t{ship}"
            if systems and len(systems) > 1 and grid == "spread" and kind == "miners":
                row += f"\t{systems[(gi - 1) % len(systems)]}"
            rows.append(row)
            names += [f"{group}{i}" for i in range(1, c + 1)]
            n -= c
    return "# bench-ramp roster (generated)\n" + "\n".join(rows) + "\n", names


def steps_of(n0, add, nmax):
    """The bot counts of each step: n0, n0+add, ... capped at nmax."""
    out, n = [], n0
    while n < nmax:
        out.append(n)
        n += add
    out.append(nmax)
    return out


LOW_SEC = 0.45  # below this a system is low or null: the server's SecurityClass::from_level (celestial.rs)


def read_universe(content):
    """(security {id: float}, names {name: id}, gates {id: [id]}, stations {id}) from
    a content bundle directory (`universe/systems.tsv`, `universe/celestials.tsv`
    gate rows, `stations/stations.tsv`). Read at run time from `out/content`;
    nothing of it is kept in git."""
    base = Path(content)
    sec, names, gates, stations = {}, {}, {}, set()
    for l in (base / "universe" / "systems.tsv").read_text().splitlines():
        f = l.split("\t")
        if l.startswith("#") or len(f) < 18:
            continue
        sec[int(f[0])] = float(f[5])
        names[f[17]] = int(f[0])
    with open(base / "universe" / "celestials.tsv") as fh:
        for l in fh:
            if l.startswith("gate\t"):
                f = l.rstrip("\n").split("\t")
                gates.setdefault(int(f[7]), []).append(int(f[9]))
    for l in (base / "stations" / "stations.tsv").read_text().splitlines():
        f = l.split("\t")
        if not l.startswith("#") and len(f) > 1:
            stations.add(int(f[1]))
    return sec, names, gates, stations


def pick_systems(universe, home, k):
    """The ids of K systems for a spread run: the home first (as the one the run
    is named for), then the nearest low or null systems with a station by jumps
    (breadth first over the gate graph; ties by id, so a rerun picks the same).
    SystemExit when the graph holds fewer than K."""
    sec, names, gates, stations = universe
    start = names.get(home) if not str(home).isdigit() else int(home)
    if start is None or start not in sec:
        raise SystemExit(f"bench-ramp: --system {home!r} is not a system of the content bundle")
    out, seen, frontier = [start], {start}, [start]
    while frontier and len(out) < k:
        nxt = []
        for sid in frontier:
            for n in gates.get(sid, []):
                if n not in seen:
                    seen.add(n)
                    nxt.append(n)
        for n in sorted(nxt):
            if len(out) < k and sec.get(n, 1.0) < LOW_SEC and n in stations:
                out.append(n)
        frontier = sorted(nxt)
    if len(out) < k:
        raise SystemExit(f"bench-ramp: only {len(out)} low/null systems with a station within reach of "
                         f"{home}; --systems {k} is too many")
    return out[:k]


# ---- reading the logs -----------------------------------------------------

def parse_ticks(lines):
    """One dict of floats per `tick health` line."""
    out = []
    for l in lines:
        m = TICK.search(l)
        if m:
            out.append({k: (float(v) if v is not None else None) for k, v in m.groupdict().items()})
    return out


def last_pilots(lines):
    got = None
    for l in lines:
        m = LIVE_PILOTS.match(l)
        if m:
            got = (int(m.group(1)), int(m.group(2)))
    return got


def live_world(lines):
    """(hot, warm, npcs, rocks) of the last `[live]` line, else zeros."""
    got = (0, 0, 0, 0)
    for l in lines:
        m = LIVE_WORLD.match(l)
        if m:
            got = (int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4) or 0))
    return got


def parse_stats(lines):
    """One dict per bots `[stats]` line: cpu, rss, threads, kbps and the ms/s of
    each phase (zero for a phase the line lacks)."""
    out = []
    for l in lines:
        m = STATS_HEAD.match(l)
        if not m:
            continue
        d = {"cpu": float(m["cpu"]), "rss": int(m["rss"]), "threads": int(m["threads"]), "kbps": int(m["kbps"])}
        d.update({p: 0.0 for p in STATS_PHASES})
        for sm in STATS_SECTION.finditer(l):
            if sm["name"] in d:
                d[sm["name"]] = float(sm["ms"])
        out.append(d)
    return out


def stats_columns(bot_lines):
    st = parse_stats(bot_lines)
    n = len(st)

    def mean(k):
        return sum(x[k] for x in st) / n if n else 0.0
    return {
        "stats_n": n, "stats_cpu_pct": mean("cpu"), "stats_in_kBps": mean("kbps"),
        **{f"{p}_ms_s": mean(p) for p in STATS_PHASES},
    }


def half_p95s(vals, skip=HALF_SHIFT_SKIP):
    """(first, second) half p95s of the step's windows after the first `skip`
    (the step's batch bursts there); None when fewer than four are left."""
    vals = vals[skip:]
    if len(vals) < 4:
        return None
    h = len(vals) // 2
    return ss.p95(vals[:h]), ss.p95(vals[-h:])


def half_shift_pct(vals, skip=HALF_SHIFT_SKIP):
    """How far the p95 of the second half of the step's windows is from the p95
    of the first half, in percent of the first, after the first `skip` windows
    (the batch burst); 0 when fewer than four windows are left."""
    h = half_p95s(vals, skip)
    if not h:
        return 0.0
    a, b = h
    return abs(b - a) / a * 100 if a > 0 else 0.0


def bot_acts(lines):
    failed = refused = 0
    for l in lines:
        m = ss.BOTLINE.match(l)
        if not m or " -> " not in l:
            continue
        outcome = l.rsplit(" -> ", 1)[-1]
        failed += outcome.startswith("FAILED")
        refused += outcome.startswith("refused")
    return failed, refused


def summarize_step(idx, target, server_lines, bot_lines, srv, bots, bot_count):
    """One step's row (dict of COLUMNS). `srv`/`bots` are dicts with cpu_pct,
    rss_mb, threads (the step's means / peaks, from /proc); `bot_count` the
    bots started so far (for per-bot figures)."""
    w = parse_ticks(server_lines)
    n = len(w)
    secs = max(1.0, n * WINDOW_TICKS * TICK_S)
    held_max = [x["held_max"] for x in w]
    work_p95 = [x["work_p95"] if x["work_p95"] is not None else x["work_max"] for x in w]
    pil = last_pilots(server_lines) or (0, 0)
    have_out = [x for x in w if x["out_bytes"] is not None]
    out_b = sum(x["out_bytes"] for x in have_out)
    dest_b = sum(x["dest_bytes"] for x in have_out)
    dest_f = sum(x["dest_frames"] for x in have_out)
    ticks = sum(x["ticks"] for x in w) or 1
    failed, refused = bot_acts(bot_lines)
    usererr = sum(1 for l in server_lines if ss.USERERR.match(l))
    row = {
        "step": idx, "target_bots": target, "pilots": pil[0], "in_space": pil[1], "windows": n,
        "work_p95_ms": ss.p95(work_p95), "work_max_ms": max([x["work_max"] for x in w] or [0]),
        "held_mean_ms": (sum(x["held_mean"] * x["ticks"] for x in w) / ticks) if w else 0,
        "held_p95_ms": ss.p95(held_max), "held_max_ms": max(held_max or [0]),
        "wait_max_ms": max([x["wait_max"] for x in w] or [0]),
        "late_max_ms": max([x["late_max"] for x in w] or [0]),
        "over_budget": int(sum(x["over"] for x in w)), "skipped": int(sum(x["skipped"] for x in w)),
        "srv_cpu_pct": srv.get("cpu_pct", 0), "srv_rss_mb": srv.get("rss_mb", 0),
        "srv_threads": srv.get("threads", 0),
        "bots_cpu_pct": bots.get("cpu_pct", 0), "bots_rss_mb": bots.get("rss_mb", 0),
        "bots_threads": bots.get("threads", 0),
        "out_kBps": out_b / secs / 1024, "destiny_kBps": dest_b / secs / 1024,
        "destiny_frames_per_tick": dest_f / ticks,
        "destiny_bytes_per_bot_s": (dest_b / secs / bot_count) if bot_count else 0,
        "act_failed": failed, "act_refused": refused, "usererrors": usererr,
    }
    slices = [x["slice_max"] for x in w if x["slice_max"] is not None]
    row["slice_max_ms"] = max(slices or [0.0])
    row["slice_p95_ms"] = ss.p95(slices) if slices else 0.0
    row.update(prof_columns(ss.parse_prof(server_lines), secs))
    hot, warm, npcs, rocks = live_world(server_lines)
    row.update({"hot_systems": hot, "warm_systems": warm, "npcs": npcs, "rocks": rocks})
    row.update(stats_columns(bot_lines))
    row["p95_half_shift_pct"] = half_shift_pct(work_p95)
    row["half_p95_max_ms"] = max(half_p95s(work_p95) or (0.0, 0.0))
    row["missing_lines"] = sum(1 for l in bot_lines if "MISSING" in l)
    row["_over_runs"] = longest_over_run(w)
    return row


def prof_columns(prof, secs):
    """The PA-0 profile columns of a step (zeros for an older server)."""
    def mean(vals):
        return sum(vals) / len(vals) if vals else 0.0

    def mx(vals):
        return max(vals) if vals else 0.0
    sec, lk, me, wr = prof["sections"], prof["lock"], prof["methods"], prof["writers"]
    turns = sum(w[4] for w in lk)
    mturns = sum(w[0] for w in me)
    return {
        "take_due_ms": mean([w[0] for w in sec]), "filter_ms": mean([w[2] for w in sec]),
        "audience_ms": mean([w[4] for w in sec]), "session_changes_ms": mean([w[6] for w in sec]),
        "save_max_ms": mx([w[9] for w in sec]),
        "clock_occ_pct": mean([w[1] for w in lk]), "payload_occ_pct": mean([w[3] for w in lk]),
        "payload_turns_per_s": turns / secs,
        "ro_guess_turn_pct": (sum(w[3] * w[0] for w in me) / mturns) if mturns else 0.0,
        "writer_frames_per_s": mean([w[1] for w in wr]), "writer_pass_max_ms": mx([w[6] for w in wr]),
        "deflate_ms_total": sum(w[8] for w in wr), "longest_write_ms": mx([w[12] for w in wr]),
        "ro_turns_per_s": sum(w[1] for w in prof.get("read", []) if w[0] == "lock") / secs,
        "ro_served_turn_pct": mean([w[1] for w in prof.get("read", []) if w[0] == "methods"]),
        "ro_fallbacks": sum(w[3] for w in prof.get("read", []) if w[0] == "methods"),
    }


def longest_over_run(windows):
    best = run = 0
    for x in windows:
        run = run + 1 if x["over"] > 0 else 0
        best = max(best, run)
    return best


def knee(rows, fraction=KNEE_FRACTION):
    """(row, reason) of the first step past the knee, else (None, '')."""
    limit = BUDGET_MS * fraction
    for r in rows:
        why = []
        if r["held_p95_ms"] > limit:
            why.append(f"lock held p95 {r['held_p95_ms']:.1f} ms > {limit:.0f} ms")
        if r["work_p95_ms"] > limit:
            why.append(f"tick work p95 {r['work_p95_ms']:.1f} ms > {limit:.0f} ms")
        if r["over_budget"] > 0:
            why.append(f"{r['over_budget']} ticks over budget")
        if r["skipped"] > 0:
            why.append(f"{r['skipped']} ticks skipped")
        if why:
            return r, "; ".join(why)
    return None, ""


def collapse(rows):
    """A reason to stop after the last of `rows`, else ''."""
    if not rows:
        return ""
    last = rows[-1]
    if last.get("_over_runs", 0) >= COLLAPSE_WINDOWS:
        return f"over budget in {last['_over_runs']} consecutive windows"
    if last["skipped"] >= WINDOW_TICKS // 2:
        return f"{last['skipped']} ticks skipped in one step"
    if len(rows) >= 2:
        prev = rows[-2]
        if prev["pilots"] > 0 and last["pilots"] < 0.9 * prev["pilots"]:
            return f"pilots fell {prev['pilots']} -> {last['pilots']}: bots disconnecting"
    return ""


def fmt(v):
    return f"{v:.2f}" if isinstance(v, float) else str(v)


def write_tsv(path, rows):
    with open(path, "w") as f:
        f.write("\t".join(COLUMNS) + "\n")
        for r in rows:
            f.write("\t".join(fmt(r[c]) for c in COLUMNS) + "\n")


def final_summary(rows, stopped, args_line="", prep=False):
    lines = [f"bench-ramp: {len(rows)} step(s){(' ' + args_line) if args_line else ''}"]
    if rows:
        lines.append("bench-ramp: step  bots  pilots  work_p95  held_p95  held_max  srv_cpu%  srv_MB  bots_cpu%  bots_MB  out_kB/s")
        for r in rows:
            lines.append(
                f"bench-ramp: {r['step']:4d} {r['target_bots']:5d} {r['pilots']:7d} {r['work_p95_ms']:9.1f} "
                f"{r['held_p95_ms']:9.1f} {r['held_max_ms']:9.1f} {r['srv_cpu_pct']:9.1f} {r['srv_rss_mb']:7.0f} "
                f"{r['bots_cpu_pct']:10.1f} {r['bots_rss_mb']:8.0f} {r['out_kBps']:9.1f}")
    if rows and any(r["stats_n"] for r in rows):
        lines.append("bench-ramp: bots step  bots  stats_cpu%  decode  observe  destiny  refresh  decide  absorb  log (ms/s)")
        for r in rows:
            lines.append(
                f"bench-ramp: bots {r['step']:4d} {r['target_bots']:5d} {r['stats_cpu_pct']:10.1f} "
                + " ".join(f"{r[p + '_ms_s']:7.1f}" for p in STATS_PHASES))
    if rows and any(r["hot_systems"] + r["warm_systems"] for r in rows):
        lines.append("bench-ramp: world step  bots  hot  warm  npcs  rocks")
        for r in rows:
            lines.append(f"bench-ramp: world {r['step']:4d} {r['target_bots']:5d} {r['hot_systems']:4d} "
                         f"{r['warm_systems']:5d} {r['npcs']:5d} {r['rocks']:6d}")
    miss = sum(r["missing_lines"] for r in rows)
    if miss:
        lines.append(f"bench-ramp: WARNING {miss} MISSING lines in bots.log: some bots could not buy their kit "
                     f"and sit docked (they add connections, not grid load); see docs/guides/bench-ramp.md")
    for r in rows:
        if r["windows"] < MIN_WINDOWS and not prep:
            lines.append(f"bench-ramp: WARNING step {r['step']} ({r['target_bots']} bots) has {r['windows']} "
                         f"windows (< {MIN_WINDOWS}): the hold is too short to trust its p95")
        if (r["p95_half_shift_pct"] > HALF_SHIFT_PCT
                and r["half_p95_max_ms"] > HALF_SHIFT_FLOOR * BUDGET_MS):
            lines.append(f"bench-ramp: WARNING step {r['step']} ({r['target_bots']} bots): work p95 moved "
                         f"{r['p95_half_shift_pct']:.0f}% between its halves (> {HALF_SHIFT_PCT:.0f}%): still settling (halves' p95 up to {r['half_p95_max_ms']:.0f} ms)")
    k, why = knee(rows)
    if k:
        lines.append(f"bench-ramp: KNEE at {k['target_bots']} bots (step {k['step']}): {why}")
    else:
        top = rows[-1]["target_bots"] if rows else 0
        lines.append(f"bench-ramp: no knee up to {top} bots (limit {BUDGET_MS * KNEE_FRACTION:.0f} ms, "
                     f"no late/skipped ticks)")
    lines.append(f"bench-ramp: {'STOPPED EARLY: ' + stopped if stopped else 'ramp finished'}")
    return lines


# ---- /proc ----------------------------------------------------------------

HZ = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100


def proc_stat(pid):
    """(cpu seconds, rss MB, threads) or None."""
    try:
        st = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        cpu = (int(st[11]) + int(st[12])) / HZ
        rss = thr = 0
        for l in Path(f"/proc/{pid}/status").read_text().splitlines():
            if l.startswith("VmRSS:"):
                rss = int(l.split()[1]) / 1024
            elif l.startswith("Threads:"):
                thr = int(l.split()[1])
        return cpu, rss, thr
    except (OSError, IndexError, ValueError):
        return None


class Sampler:
    """CPU% (of one core) over an interval, RSS and threads peaks, for a pid."""

    def __init__(self, pid):
        self.pid, self.last, self.peak_rss, self.peak_thr = pid, None, 0.0, 0
        self.begin()

    def begin(self):
        s = proc_stat(self.pid)
        self.t0 = time.monotonic()
        self.c0 = s[0] if s else 0.0
        self.peak_rss, self.peak_thr = (s[1], s[2]) if s else (0.0, 0)

    def poll(self):
        s = proc_stat(self.pid)
        if s:
            self.peak_rss, self.peak_thr = max(self.peak_rss, s[1]), max(self.peak_thr, s[2])
            self.last = s

    def end(self):
        self.poll()
        dt = max(1e-6, time.monotonic() - self.t0)
        cpu = ((self.last[0] - self.c0) / dt * 100) if self.last else 0.0
        return {"cpu_pct": cpu, "rss_mb": self.peak_rss, "threads": self.peak_thr}


# ---- the login gate (#252, eve-bots --login-rate) ---------------------------

# The server prints this once per character that enters the game
# (eve-server/src/dispatch.rs, "session change"); the log has no clock, so the
# runner stamps each line with the time it read it (LogTail).
# [A] The stamp is the read time, up to one poll (0.5 s) after the login; the
# curve is therefore only good to about a second. Falsifier: a steady gate run
# whose logins.tsv alternates 0 and 4 in a second (then stamp finer, or have
# the server print a clock).
LOGIN_LINE = re.compile(r"^\[svc\] session change: character (\d+) entering the game")
GATE_JITTER_S = 10  # mirrors eve-bot login_gate::JITTER_MAX_S: a bot's first arrival at the gate is 0-10 s
BURST_WINDOW_S = 5  # bursts are judged over this many seconds, because of the stamping skew above


def gate_hold_needed(bots, rate):
    """The shortest hold that lets every bot through the gate and then run 30 s."""
    return int(bots / rate) + GATE_JITTER_S + 30


def count_logins(lines):
    return sum(1 for l in lines if LOGIN_LINE.match(l))


def login_ids(events):
    """The distinct character ids seen entering the game."""
    ids = set()
    for _, lines in events:
        for l in lines:
            m = LOGIN_LINE.match(l)
            if m:
                ids.add(m.group(1))
    return ids


def per_second(events):
    """`events` = [(t_s, lines)] (the time the lines were read, seconds since
    the bots started) -> the logins in each whole second, index 0 = [0, 1)."""
    counts = []
    for t, lines in events:
        n = count_logins(lines)
        if n == 0:
            continue
        sec = max(0, int(t))
        counts.extend([0] * (sec + 1 - len(counts)))
        counts[sec] += n
    return counts


def burst_limit(rate, window=BURST_WINDOW_S):
    """What a bucket of burst 1 lets through in `window` seconds: window * rate + 1."""
    return window * rate + 1


def bursts(counts, rate, window=BURST_WINDOW_S):
    """[(first second, n)] for every `window`-second stretch holding more logins
    than the gate allows (approximate: stamps are up to one poll late)."""
    lim = burst_limit(rate, window)
    pad = counts + [0] * max(0, window - len(counts))
    return [(i, sum(pad[i:i + window])) for i in range(len(pad) - window + 1)
            if sum(pad[i:i + window]) > lim]


def login_windows(events):
    """One row per tick-health line read, with the logins since the previous
    one: the tick p95 during the login curve. Rows are dicts."""
    rows, last_t, since = [], 0.0, 0
    for t, lines in events:
        since += count_logins(lines)
        for w in parse_ticks(lines):
            rows.append({"t_s": t, "logins": since, "logins_per_s": since / max(1e-6, t - last_t),
                         "work_p95_ms": w["work_p95"] if w["work_p95"] is not None else w["work_max"],
                         "late_max_ms": w["late_max"], "over_budget": w["over"]})
            last_t, since = t, 0
    return rows


def gate_summary(events, rate, want):
    """The login-gate lines for summary.txt."""
    counts = per_second(events)
    total, bad = len(login_ids(events)), bursts(counts, rate)
    wins = login_windows(events)
    live = [w for w in wins if w["logins"] > 0]
    lines = [f"bench-ramp: login gate {rate:g}/s: {total} of {want} logins seen in {len(counts)} s, "
             f"peak {max(counts, default=0)}/s (a 1-burst bucket allows about {rate:g}/s)"]
    if live:
        worst = max(live, key=lambda w: w["work_p95_ms"])
        lines.append(f"bench-ramp: login gate: {len(live)} tick window(s) with logins, work p95 worst "
                     f"{worst['work_p95_ms']:.1f} ms at t={worst['t_s']:.0f} s, late max "
                     f"{max(w['late_max_ms'] for w in live):.1f} ms")
    else:
        lines.append("bench-ramp: login gate: no tick window overlapped a login")
    if bad:
        lines.append(f"bench-ramp: WARNING login gate: {len(bad)} {BURST_WINDOW_S} s window(s) above "
                     f"{burst_limit(rate):g} logins (approximate; first: t={bad[0][0]} s, {bad[0][1]}): "
                     f"a burst got through")
    if total < want:
        lines.append(f"bench-ramp: WARNING login gate: only {total} of {want} logins within the hold")
    return lines


def write_logins(path, events):
    counts = per_second(events)
    cum = 0
    with open(path, "w") as f:
        f.write("t_s\tlogins\tcumulative\n")
        for i, n in enumerate(counts):
            cum += n
            f.write(f"{i}\t{n}\t{cum}\n")


class LogTail:
    """Reads a growing log by complete lines, stamping each read with the time."""

    def __init__(self, path, t0):
        self.path, self.t0, self.pos, self.rest, self.events = path, t0, 0, b"", []

    def poll(self, now=None):
        now = time.monotonic() if now is None else now
        try:
            with open(self.path, "rb") as f:
                f.seek(self.pos)
                data = f.read()
        except OSError:
            return
        self.pos += len(data)
        data = self.rest + data
        head, _, self.rest = data.rpartition(b"\n")
        if head or data.endswith(b"\n"):
            # keep only what the gate report reads: logins and tick-health lines
            keep = [l for l in head.decode(errors="replace").splitlines() if LOGIN_LINE.match(l) or TICK.search(l)]
            if keep:
                self.events.append((now - self.t0, keep))


# ---- the run --------------------------------------------------------------

def lines_between(path, a, b):
    try:
        with open(path, "rb") as f:
            f.seek(a)
            return f.read(max(0, b - a)).decode(errors="replace").splitlines()
    except OSError:
        return []


def size(path):
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def ready(lines, want):
    """--prep: every bot is in the game and most are out of the station
    (their kits are bought and fitted, which is what the prep is for)."""
    got = last_pilots(lines)
    return bool(got) and got[0] >= want and got[1] >= 0.85 * want


def bot_extra_args(a, env):
    """The extra eve-bots words: `--brawl`, `--stats SECS` (default 10; not added
    when the env words carry their own `--stats`), `--quiet`, then the env's."""
    words = []
    if a.brawl:
        words.append("--brawl")
    if a.stats > 0 and "--stats" not in env.split():
        words += ["--stats", str(a.stats)]
    if a.quiet:
        words.append("--quiet")
    return words + env.split()


def run(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    systems = None
    if a.systems > 1:
        systems = pick_systems(read_universe(a.content), a.system, a.systems)
        (out / "systems.txt").write_text("\n".join(map(str, systems)) + "\n")
    text, names = roster(a.max, parse_mix(a.mix), a.grid, systems, one_side=a.prep)
    (out / "roster.tsv").write_text(text)
    gate = a.login_rate > 0
    steps = [a.max] if gate else steps_of(a.start, a.add, a.max)
    slog, blog = out / "server.log", out / "bots.log"
    world = Path(a.world)
    stop_file = Path(str(world) + ".stop")
    for stale in (stop_file,):
        stale.unlink(missing_ok=True)
    env = dict(os.environ, EVE_GIT_SHA="bench")
    srv_args = [f"{a.bin}/{os.environ.get('EVE_SERVER_BIN', 'eve-offline-server')}", "--dev", "--world", str(world), "--world-accounts",
                "--status", str(out / "status.tsv"),
                "--bind", f"127.0.0.1:{a.port},signup=on,capture=off,cap={max(2048, 2 * a.max)},"
                f"chat=127.0.0.1:{a.port + 1}"]
    srv_args.append("--turn-sched")  # TT-A: cpu / runq in the [turn-slow] lines
    if a.content:
        srv_args += ["--content", a.content]
    if a.authored:
        srv_args += ["--authored", a.authored]
    # Extra eve-server flags for a run (e.g. "--tick-threads 1").
    srv_args += os.environ.get("EVE_BENCH_SERVER_ARGS", "").split()
    final_hold = a.final_hold or a.hold
    total_secs = a.hold * len(steps) + (final_hold - a.hold) + 60
    bot_args = [f"{a.bin}/eve-bots", "--connect", f"127.0.0.1:{a.port}", "--tokens", a.tokens,
                "--roster", str(out / "roster.tsv"), "--roster-home", a.system, "--society-pvp",
                "--chat", "on"]
    if gate:
        # #252: all bots at once at a human pace; the gate (not --ramp) spaces the logins.
        bot_args += ["--pace", "human", "--login-rate", f"{a.login_rate:g}"]
    else:
        bot_args += ["--ramp", f"{a.start},{a.add},{a.hold},{a.stagger_ms}"]
    bot_args += ["--run", str(total_secs)]
    if a.content:
        bot_args += ["--content", a.content]
    # Extra eve-bots flags for a profile run (e.g. "--stats 10 --quiet").
    bot_args += bot_extra_args(a, os.environ.get("EVE_BENCH_BOT_ARGS", ""))
    print(f"bench-ramp: server {a.port}, world {world}, {len(steps)} steps {steps}, hold {a.hold}s"
          f"{', login gate ' + format(a.login_rate, 'g') + '/s' if gate else ''}", flush=True)
    sp = subprocess.Popen(srv_args, stdout=open(slog, "w"), stderr=subprocess.STDOUT, env=env)
    bp = None
    proc_log = open(os.devnull, "w")
    rows, stopped = [], ""
    try:
        for _ in range(180):
            if "listening" in slog.read_text(errors="replace"):
                break
            if sp.poll() is not None:
                raise SystemExit(f"bench-ramp: the server died on startup; see {slog}")
            time.sleep(1)
        bp = subprocess.Popen(bot_args, stdout=open(blog, "w"), stderr=subprocess.STDOUT)
        t_bots = time.monotonic()
        ssamp, bsamp = Sampler(sp.pid), Sampler(bp.pid)
        tail = LogTail(slog, t_bots) if gate else None
        seen = [None, time.monotonic()]
        proc_log.close()
        proc_log = open(out / "proc.tsv", "w")  # closed in the finally below
        proc_log.write("t_s\tsrv_cpu_s\tsrv_rss_mb\tsrv_threads\tbots_cpu_s\tbots_rss_mb\tbots_threads\n")
        proc_last = 0.0
        for i, target in enumerate(steps):
            # Step i is bots' batch i (it logs in at i*hold); measure to (i+1)*hold.
            end = t_bots + (i + 1) * a.hold + ((final_hold - a.hold) if i == len(steps) - 1 else 0)
            s0, b0 = size(slog), size(blog)
            ssamp.begin()
            bsamp.begin()
            while time.monotonic() < end:
                time.sleep(min(0.5 if gate else 2.0, max(0.0, end - time.monotonic())))
                if tail:
                    tail.poll()
                ssamp.poll()
                bsamp.poll()
                if time.monotonic() - proc_last >= 10 and ssamp.last and bsamp.last:
                    proc_last = time.monotonic()
                    proc_log.write(f"{proc_last - t_bots:.0f}\t" + "\t".join(
                        f"{v:.1f}" for x in (ssamp.last, bsamp.last) for v in x) + "\n")
                    proc_log.flush()
                if sp.poll() is not None or bp.poll() is not None:
                    stopped = "server exited" if sp.poll() is not None else "bots exited"
                    break
                if a.prep:
                    got = last_pilots(lines_between(slog, s0, size(slog)))
                    if got != seen[0]:
                        seen[:] = [got, time.monotonic()]
                    if ready(lines_between(slog, s0, size(slog)), target) or (
                            got and got[0] >= target and time.monotonic() - seen[1] > 90):
                        break  # all in, and nobody new has left the station for 90 s
            if tail:
                tail.poll()
            s1, b1 = size(slog), size(blog)
            row = summarize_step(i, target, lines_between(slog, s0, s1), lines_between(blog, b0, b1),
                                 ssamp.end(), bsamp.end(), target)
            rows.append(row)
            write_tsv(out / "steps.tsv", rows)
            print(f"bench-ramp: step {i} bots {target}: pilots {row['pilots']}, work p95 {row['work_p95_ms']:.1f} ms, "
                  f"held p95 {row['held_p95_ms']:.1f} max {row['held_max_ms']:.1f} ms, srv {row['srv_cpu_pct']:.0f}% "
                  f"{row['srv_rss_mb']:.0f} MB, bots {row['bots_cpu_pct']:.0f}% {row['bots_rss_mb']:.0f} MB, "
                  f"out {row['out_kBps']:.0f} kB/s", flush=True)
            stopped = stopped or collapse(rows)
            if stopped:
                break
    finally:
        proc_log.close()
        if bp and bp.poll() is None:
            bp.send_signal(signal.SIGTERM)
            try:
                bp.wait(20)
            except subprocess.TimeoutExpired:
                bp.kill()
        if sp.poll() is None:
            stop_file.touch()
            try:
                sp.wait(60)
            except subprocess.TimeoutExpired:
                sp.send_signal(signal.SIGTERM)
                sp.wait(20)
    extra = " ".join(os.environ.get(v, "") for v in ("EVE_BENCH_SERVER_ARGS", "EVE_BENCH_BOT_ARGS")).strip()
    where = f"{a.system} x{a.systems}" if a.systems > 1 else a.system
    lines = final_summary(rows, stopped, prep=a.prep, args_line=f"({a.grid}, mix {a.mix}, {where}{', ' + extra if extra else ''})")
    if gate and tail:
        write_logins(out / "logins.tsv", tail.events)
        (out / "login-windows.tsv").write_text(
            "t_s\tlogins\tlogins_per_s\twork_p95_ms\tlate_max_ms\tover_budget\n" + "".join(
                f"{w['t_s']:.0f}\t{w['logins']}\t{w['logins_per_s']:.2f}\t{w['work_p95_ms']:.1f}\t"
                f"{w['late_max_ms']:.1f}\t{w['over_budget']:.0f}\n" for w in login_windows(tail.events)))
        lines += gate_summary(tail.events, a.login_rate, a.max)
    (out / "summary.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


def build_parser():
    p = argparse.ArgumentParser()
    p.add_argument("--bin")
    p.add_argument("--out")
    p.add_argument("--world")
    p.add_argument("--tokens")
    p.add_argument("--content")
    p.add_argument("--authored")
    p.add_argument("--port", type=int, default=26500)
    p.add_argument("--start", type=int, default=50)
    p.add_argument("--add", type=int, default=50)
    p.add_argument("--max", type=int, default=300)
    p.add_argument("--hold", type=int, default=75)
    p.add_argument("--stagger-ms", type=int, default=50)
    p.add_argument("--grid", choices=["same", "spread"], default="same")
    p.add_argument("--mix", default="miners=100")
    p.add_argument("--system", default="Amamake")
    p.add_argument("--systems", type=int, default=1,
                   help="spread: seat the miners' fleets over K low/null systems with a station (default 1)")
    p.add_argument("--brawl", action="store_true", help="eve-bots --brawl (hunters close on the nearest foe)")
    p.add_argument("--stats", type=int, default=10, help="eve-bots --stats SECS (0: off); the [stats] columns")
    p.add_argument("--quiet", action="store_true", help="eve-bots --quiet (no act lines: act_* columns read 0)")
    p.add_argument("--final-hold", type=int, default=0,
                   help="seconds to hold the LAST step (default --hold); e.g. 600 for a 10 minute RSS series")
    p.add_argument("--login-rate", type=float, default=0.0,
                   help="#252: all --max bots at once, Human pace, eve-bots --login-rate R (no --ramp); "
                        "records logins per second (logins.tsv) next to the tick health. 0: the ramp.")
    p.add_argument("--prep", action="store_true",
                   help="warm the world in --world (kits bought) and stop when the bots are out")
    return p


def check_args(a):
    """The argument rules: an error message, or None. Pure (touches nothing)."""
    parse_mix(a.mix)
    if not (a.bin and a.out and a.world and a.tokens):
        return "--bin, --out, --world and --tokens are required"
    if a.systems < 1 or a.stats < 0 or (a.final_hold and a.final_hold < 20):
        return "want --systems >= 1, --stats >= 0, --final-hold 0 (off) or >= 20"
    if a.systems > 1 and (a.grid != "spread" or not a.content):
        return "--systems K > 1 needs --grid spread and --content"
    if a.login_rate < 0 or a.login_rate != a.login_rate or a.login_rate == float("inf"):
        return "want --login-rate >= 0 (0: the ramp)"
    if a.login_rate > 0:
        if a.prep:
            return "--login-rate and --prep do not go together"
        if a.max < 1:
            return "want --max >= 1"
        need = gate_hold_needed(a.max, a.login_rate)
        if a.final_hold and a.final_hold < need:
            return f"--final-hold {a.final_hold} is below the {need} s the gate needs (omit it in this mode)"
        if a.hold < need:
            return f"--login-rate {a.login_rate:g} with {a.max} bots needs --hold >= {need} (bots/rate + {GATE_JITTER_S} s jitter + 30 s)"
        return None
    if a.start < 1 or a.add < 1 or a.max < a.start or a.hold < 20:
        return "want 1 <= --start <= --max, --add >= 1, --hold >= 20"
    return None


def main(argv):
    p = build_parser()
    a = p.parse_args(argv)
    err = check_args(a)
    if err:
        p.error(err)
    return run(a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
