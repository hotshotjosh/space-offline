#!/usr/bin/env python3
"""Sample processes and summarise a battle run (issue #100, lane battle-100t).

    battle_summary.py sample --out procs.tsv PID...
    battle_summary.py summarize --server-log F --bots-log F [--procs F] [--result result.json]
                                --per-side N --out DIR

`sample` writes one row per pid every 10 s (cpu % of one core, RSS MB, threads,
from /proc) and exits when the first pid is gone.

`summarize` cuts the server log into windows (every line from one
`[world] tick health` line up to the next, so window i is about i x 10 s) and
writes battle.tsv (one row per window), summary.txt and stalls/w<idx>.txt (the
raw block of every window with a tick over LATE_MS late or any skipped tick).
The verdict is the rule of docs/guides/bench-ramp.md "A battle with a client":
skipped 0, no window with late max over LATE_MS, capped 0, clock slice wait
under LATE_MS; judged on the fight windows (in space at least FIGHT_FRACTION of
2 x per_side) and on the whole run. Standard library only.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins (see
# `tools/asteroids.py`, `tools/dis.py`).
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

sys.path.append(str(Path(__file__).resolve().parent))
import bench_ramp  # noqa: E402  (TICK and LIVE_PILOTS: one parser for the tick line)

TICK_MS = 250.0       # push.rs TICK: the tick period
LATE_MS = 100.0       # push.rs LATE_BUDGET: a tick this late is over budget
LOCK_MS = 25.0        # liveness.rs: the clock hold per tick before DEGRADED counts
KNEE_MS = 125.0       # tools/bench_ramp.py: work_p95 or held_p95 knee
CEILING_MS = 1000.0   # tools/client-ci.sh: late max that fails the run
FIGHT_FRACTION = 0.75  # in_space / (2 x per_side) that makes a window a fight window
SAMPLE_S = 10         # `sample`: seconds between rows

TICK = bench_ramp.TICK
LIVE_PILOTS = bench_ramp.LIVE_PILOTS
LOCK = re.compile(
    r"\[world\] tick lock: .*?payload held [\d.]+ ms occupancy [\d.]+% turns \d+ hold max (?P<payload_hold_max>[\d.]+) ms"
    r".*?clock slice wait max (?P<slice_wait>[\d.]+) ms \(slice \d+\), waiters max (?P<waiters>\d+), "
    r"other held max (?P<other_held>[\d.]+) ms \((?P<other_site>[^)]*)\) wait max [\d.]+ ms \([^)]*\), "
    r"gate raised (?P<raised>\d+) held back (?P<held_back>\d+) turns max (?P<gate_max>[\d.]+) ms capped (?P<capped>\d+)")
STAGES = re.compile(r"\[world\] tick stages: .*clock cpu max (?P<cpu>[\d.]+) ms runq max (?P<runq>[\d.]+) ms")
TURNS = re.compile(r"^\[turns\] .*?hold_max=(?P<hold_max>[\d.]+)ms")
SLOW = re.compile(r"^\[turn-slow\] worst label=(?P<label>\S+) hold=(?P<hold>[\d.]+)ms")
OUTBOX = re.compile(r"^\[push\] (?P<who>.*?): outbox over cap")
STATS = re.compile(r"\[stats\]")

COLUMNS = ["idx", "secs", "late_max", "over", "skipped", "held_max", "wait_max", "work_p95", "slice_max",
           "slice_wait", "waiters", "other_held", "other_site", "capped", "gate_raised", "gate_held_back",
           "clock_cpu", "clock_runq", "turns_hold_max", "slow_label", "slow_hold",
           "pilots", "in_space", "destiny_fps", "out_kBps"]


def quantile(values, q):
    v = sorted(values)
    if not v:
        return None
    return v[min(len(v) - 1, int(q * len(v)))]


def num(text):
    return float(text) if text is not None else None


def read_lines(path):
    with open(path, errors="replace") as f:
        return f.read().splitlines()


def windows(lines):
    """`[{'lines': [...], 'tick': match}]`; lines before the first tick line are dropped."""
    out = []
    for ln in lines:
        m = TICK.search(ln)
        if m:
            out.append({"lines": [ln], "tick": m})
        elif out:
            out[-1]["lines"].append(ln)
    return out


def digest(idx, w, last_live):
    t = w["tick"]
    row = {k: None for k in COLUMNS}
    secs = int(t["ticks"]) * TICK_MS / 1000.0
    row.update(idx=idx, secs=secs, late_max=num(t["late_max"]), over=int(t["over"]), skipped=int(t["skipped"]),
               held_max=num(t["held_max"]), wait_max=num(t["wait_max"]), work_p95=num(t["work_p95"]),
               slice_max=num(t["slice_max"]), pilots=last_live[0], in_space=last_live[1])
    if t["dest_frames"] is not None and secs > 0:
        row["destiny_fps"] = round(int(t["dest_frames"]) / secs, 1)
    if t["out_bytes"] is not None and secs > 0:
        row["out_kBps"] = round(int(t["out_bytes"]) / 1000.0 / secs, 1)
    for ln in w["lines"][1:]:
        m = LOCK.search(ln)
        if m:
            row.update(slice_wait=num(m["slice_wait"]), waiters=int(m["waiters"]), other_held=num(m["other_held"]),
                       other_site=m["other_site"], capped=int(m["capped"]), gate_raised=int(m["raised"]),
                       gate_held_back=int(m["held_back"]))
            row["lock_payload_hold_max"] = num(m["payload_hold_max"])
        m = STAGES.search(ln)
        if m:
            row.update(clock_cpu=num(m["cpu"]), clock_runq=num(m["runq"]))
        m = TURNS.search(ln)
        if m:
            row["turns_hold_max"] = num(m["hold_max"])
        m = SLOW.search(ln)
        if m:
            row.update(slow_label=m["label"], slow_hold=num(m["hold"]))
        m = LIVE_PILOTS.search(ln)
        if m:
            row.update(pilots=int(m[1]), in_space=int(m[2]))
    return row


def cell(v):
    return "" if v is None else str(v)


def is_stall(row):
    return (row["late_max"] or 0) > LATE_MS or (row["skipped"] or 0) > 0


def judge(rows):
    """`(label, reasons)` by the rule in the module docstring."""
    if not rows:
        return "NO WINDOWS", []
    why = []
    skipped = sum(r["skipped"] or 0 for r in rows)
    late = max((r["late_max"] or 0) for r in rows)
    capped = sum(r["capped"] or 0 for r in rows)
    sw = max((r["slice_wait"] or 0) for r in rows)
    if skipped:
        why.append("skipped ticks %d" % skipped)
    if late > LATE_MS:
        why.append("late max %.1f ms over %.0f" % (late, LATE_MS))
    if capped:
        why.append("capped %d" % capped)
    if sw >= LATE_MS:
        why.append("clock slice wait max %.1f ms, %.0f or more" % (sw, LATE_MS))
    missing = sum(r["slice_wait"] is None for r in rows)
    if why:
        if missing:
            why.append("capped and slice wait not measured in %d windows" % missing)
        return "STALLS", why
    if missing:
        return "INCOMPLETE", ["no tick lock data in %d of %d windows: capped and the clock slice wait were not measured"
                              % (missing, len(rows))]
    return "PASS", []


def section(title, rows):
    out = ["== %s (%d windows) ==" % (title, len(rows))]
    if not rows:
        out.append("  none")
        return out
    held = [r["held_max"] for r in rows if r["held_max"] is not None]
    late = max(rows, key=lambda r: r["late_max"] or 0)
    sw = max(rows, key=lambda r: r["slice_wait"] or 0)
    ph = [r["turns_hold_max"] for r in rows if r["turns_hold_max"] is not None]
    out += [
        "  skipped ticks total: %d" % sum(r["skipped"] or 0 for r in rows),
        "  over-budget ticks total: %d" % sum(r["over"] or 0 for r in rows),
        "  longest stall (late max): %.1f ms in window %d (about %d s), other held max %s ms (%s)"
        % (late["late_max"] or 0, late["idx"], late["idx"] * 10, cell(late["other_held"]), cell(late["other_site"])),
        "  clock hold (held max per window): p50 %s ms, p95 %s ms, max %s ms (budget %.0f ms per tick; knee %.0f ms)"
        % (quantile(held, 0.5), quantile(held, 0.95), max(held) if held else None, LOCK_MS, KNEE_MS),
        "  clock slice wait max: %s ms (window %d); waiters max %s"
        % (cell(sw["slice_wait"]), sw["idx"], max((r["waiters"] or 0) for r in rows)),
        "  payload turn hold max: %s ms ([turns]); %s ms (tick lock line)"
        % (max(ph) if ph else None, max((r.get("lock_payload_hold_max") or 0) for r in rows)),
        "  gate: raised %d, held back %d turns, capped %d"
        % (sum(r["gate_raised"] or 0 for r in rows), sum(r["gate_held_back"] or 0 for r in rows),
           sum(r["capped"] or 0 for r in rows)),
        "  peak pilots %s, peak in_space %s" % (max((r["pilots"] or 0) for r in rows), max((r["in_space"] or 0) for r in rows)),
        "  clock cpu max %s ms, runq max %s ms" % (max((r["clock_cpu"] or 0) for r in rows), max((r["clock_runq"] or 0) for r in rows)),
    ]
    label, why = judge(rows)
    out.append("  verdict: %s%s" % (label,
                                    (" (" + "; ".join(why) + ")") if why else ""))
    if late["late_max"] and late["late_max"] >= CEILING_MS:
        out.append("  THE CLOCK DID NOT KEEP TIME: late max %.0f ms is at or over the %.0f ms ceiling" % (late["late_max"], CEILING_MS))
    return out


def procs_peaks(path):
    peaks = {}
    for ln in read_lines(path)[1:]:
        f = ln.split("\t")
        if len(f) < 5:
            continue
        try:
            cpu, rss, thr = float(f[2]), float(f[3]), float(f[4])
        except ValueError:
            continue
        p = peaks.setdefault(f[1], [0.0, 0.0, 0.0])
        p[0], p[1], p[2] = max(p[0], cpu), max(p[1], rss), max(p[2], thr)
    return peaks


def summarize(a):
    srv = read_lines(a.server_log)
    wins = windows(srv)
    rows, live = [], (None, None)
    for i, w in enumerate(wins):
        for ln in w["lines"]:
            m = LIVE_PILOTS.search(ln)
            if m:
                live = (int(m[1]), int(m[2]))
        rows.append(digest(i, w, live))
    target = 2 * a.per_side
    fight = [r for r in rows if (r["in_space"] or 0) >= FIGHT_FRACTION * target]
    out_dir = Path(a.out)
    (out_dir / "stalls").mkdir(parents=True, exist_ok=True)
    cols = COLUMNS + ["lock_payload_hold_max"]
    with open(out_dir / "battle.tsv", "w") as f:
        f.write("\t".join(cols) + "\n")
        for r in rows:
            f.write("\t".join(cell(r.get(c)) for c in cols) + "\n")
    for i, (r, w) in enumerate(zip(rows, wins)):
        if is_stall(r):
            (out_dir / "stalls" / ("w%d.txt" % i)).write_text("\n".join(w["lines"]) + "\n")
    lines = ["battle summary: %d windows, target %d pilots (2 x %d), fight windows need in_space >= %d"
             % (len(rows), target, a.per_side, int(FIGHT_FRACTION * target))]
    lines += section("fight windows", fight) + section("whole run", rows)
    alarms = {"DEGRADED": 0, "outbox over cap": 0, "outbox over cap (cipilot)": 0, "write failed": 0,
              "panicked at": 0, "TAINTED": 0}
    for ln in srv:
        if "[live] DEGRADED" in ln:
            alarms["DEGRADED"] += 1
        m = OUTBOX.search(ln)
        if m:
            alarms["outbox over cap (cipilot)" if "cipilot" in m["who"] else "outbox over cap"] += 1
        if "[push] write failed" in ln:
            alarms["write failed"] += 1
        if "panicked at" in ln:
            alarms["panicked at"] += 1
        if "[world] TAINTED" in ln:
            alarms["TAINTED"] += 1
    lines.append("== alarms ==")
    lines += ["  %s: %d" % kv for kv in alarms.items()]
    if a.bots_log and os.path.exists(a.bots_log):
        bl = read_lines(a.bots_log)
        lines.append("== bots log ==\n  [stats] lines: %d, MISSING lines: %d"
                     % (sum(1 for x in bl if STATS.search(x)), sum(1 for x in bl if "MISSING" in x)))
    if a.procs and os.path.exists(a.procs):
        lines.append("== processes (peak cpu % of one core, RSS MB, threads) ==")
        for name, (c, r, t) in sorted(procs_peaks(a.procs).items()):
            lines.append("  %s: cpu %.0f, rss %.0f, threads %.0f" % (name, c, r, t))
    if a.result and os.path.exists(a.result):
        with open(a.result) as f:
            res = json.load(f)
        lines.append("== client (%s, ok=%s) ==" % (res.get("scenario"), res.get("ok")))
        for c in res.get("checks", []):
            lines.append("  [%s] %s -- %s" % ("ok" if c.get("ok") else "FAIL", c.get("name"), str(c.get("detail"))[:300]))
        if res.get("error"):
            lines.append("  error: " + str(res["error"])[:300])
    (out_dir / "summary.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


def read_proc(pid):
    """`(cpu_ticks, rss_mb, threads, comm)` or None when the pid is gone."""
    try:
        with open("/proc/%d/stat" % pid) as f:
            st = f.read()
        rest = st[st.rindex(")") + 2:].split()      # comm may hold spaces; fields after it are fixed
        if rest[0] in "ZX":                          # a zombie is gone for our purposes
            return None
        ticks = int(rest[11]) + int(rest[12])        # utime + stime
        comm = st[st.index("(") + 1:st.rindex(")")]
        rss = threads = None
        with open("/proc/%d/status" % pid) as f:
            for ln in f:
                if ln.startswith("VmRSS:"):
                    rss = int(ln.split()[1]) / 1024.0
                elif ln.startswith("Threads:"):
                    threads = int(ln.split()[1])
        return ticks, rss or 0.0, threads or 0, comm
    except (OSError, ValueError, IndexError):
        return None


def sample(a):
    hz = os.sysconf("SC_CLK_TCK")
    last = {}
    for p in a.pids:
        r = read_proc(p)
        if r:
            last[p] = r[0]
    with open(a.out, "w") as f:
        f.write("time\tproc\tcpu_pct_one_core\trss_mb\tthreads\n")
        f.flush()
        while read_proc(a.pids[0]) is not None:
            time.sleep(SAMPLE_S)
            for p in a.pids:
                r = read_proc(p)
                if r is None or p not in last:
                    continue
                cpu = (r[0] - last[p]) * 100.0 / (hz * SAMPLE_S)
                last[p] = r[0]
                f.write("%s\t%s\t%.0f\t%.0f\t%d\n" % (time.strftime("%H:%M:%S"), r[3], cpu, r[1], r[2]))
            f.flush()
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sample")
    s.add_argument("--out", required=True)
    s.add_argument("pids", type=int, nargs="+")
    m = sub.add_parser("summarize")
    m.add_argument("--server-log", required=True)
    m.add_argument("--bots-log")
    m.add_argument("--procs")
    m.add_argument("--result")
    m.add_argument("--per-side", type=int, required=True)
    m.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    return sample(a) if a.cmd == "sample" else summarize(a)


if __name__ == "__main__":
    sys.exit(main())
