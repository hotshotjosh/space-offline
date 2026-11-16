#!/usr/bin/env python3
"""The society gate GC and the sov gate GD, as pass/fail with the numbers (SOC-REPORT).

    tools/analysis.sh python3 tools/society-report.py out/soak/bots.log out/soak/server.log
        [--game-day-secs 3600] [--tick-secs 60] [--json]

Reads the bots' `[soc]` lines and the server's `[sov]` lines (any file order,
any prefix before the tag) and judges GC-SOAK and GD-SOAK of
docs/archive/plan-deep-queue.md. A gate with no instrument in the logs says
`n/a` and why, never a pass. Exit status 1 if any gate FAILs, 0 otherwise.
Only the standard library.

GC (R-SOCIETY) passes when every criterion holds:
  corps, alliances   the last `[soc] bots ..` summary's `corps N alliances N`
  blue and red       the peak `blue N red N` over the summaries
  loss moves R       a `[soc] relation loss .. before=X after=Y` line with X != Y
  no flapping        no pair changes sign (blue<->red, a neutral gap ignored)
                     twice inside one game day

FOUND (#155) is judged when the logs hold a `corp found` or `corp pledge` line: PASS with
a `found .. : ok` and the last summary's corps >= 1, FAIL on a refusal otherwise, else n/a.

REL (#95, SOAK-REL) is judged when the logs hold a `relation pair`, `engage` or `betrayal` line:
  no friendly engage  FAIL if any `engage` line has stance friendly or allied; n/a with none
  betrayal sours      every `betrayal` line has stance=hostile (betray() flips the stance and
                      leaves R alone, so the stance is judged; before/after are detail only);
                      n/a when no line carries a stance
  memory fades        for a pair seen on two `relation pair` lines t0 < t1 (game days), with
                      line 1's touched= stamp before t0 (nothing added to the pair between
                      the lines; `touched=none` is skipped too), |r1| <= |r0| * 0.5^((t1-t0)/half)
                      * 1.1 [A]; `half` is the line's own (the corp matrix's loyalty-scaled
                      half-life); n/a when no interval qualifies

GD (sov) passes when every criterion holds:
  claims             distinct hubs launched or captured
  contested          a campaign that began AND had a node taken on the same hub
                     (a node appeared in between is shown, not required)
  change of owner    a system captured by one alliance and later by another

Lines read:

  [soc] bots N online N mix .. corps N alliances N blue N red N [day N]
  [soc] NAME corp stance O -> T: blue -> red (R x)         (society::Stances)
  [soc] relation loss victim=V killer=K before=X after=Y   (society::loss_line)
  [soc] relation pair viewer=V target=T r=X stance=S day=D t=T half=H touched=U|none  (society::rel_line; once a game day per pair, REL)
  [soc] engage shooter=G target=H stance=S                (society::engage_line; a bot's lock on a pilot, REL)
  [soc] betrayal victim=V killer=K before=X after=Y stance=S  (society::betrayal_line, REL)
  [soc] NAME corp found NAME [TICKER] ID: ok              (corpmind.rs Plan::Found; FOUND gate, #155)
  [soc] NAME corp found NAME [TICKER]: refused KEY        (a refusal fails FOUND unless a corp was founded)
  [soc] NAME corp pledge AMT to ID: ok                    (Plan::Pledge; counted in the FOUND detail)
  [soc] NAME [corp ]advert posted corp=ID                  (corp_ads::tend; informational)
  [soc] NAME [corp ]advert removed corp=ID                 (corp_ads::tend; informational)
  [soc] NAME [corp ]applied corp=ID via=finder             (corp_ads::applied_line; informational)
  [sov] hub H launched in S by A at ..
  [sov] hub H in S captured by alliance A (corp C)
  [sov] campaign in S begins on hub H
  [sov] command node N appears in S for the campaign on hub H
  [sov] a node of hub H was taken by the W: ..

The clock for flaps: a `day N` field on the summary lines when present, else the
summary's position (`--tick-secs`, the summary period, over `--game-day-secs`,
the real seconds per game day). With neither, flaps are `n/a`.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import json
import os
import re
import sys

# Gate thresholds (docs/archive/plan-deep-queue.md, GC-SOAK and GD-SOAK).
MIN_CORPS = 3
MIN_ALLIANCES = 2
MIN_BLUE = 1
MIN_RED = 1
MAX_FLIPS_PER_DAY = 1
MIN_CLAIMS = 2
MIN_CONTESTED = 1
MIN_OWNER_CHANGES = 1
DEFAULT_TICK_SECS = 60.0  # driver::SOC_SUMMARY_EVERY

PASS, FAIL, NA = "PASS", "FAIL", "n/a"
TAGS = ("[soc]", "[sov]")

SUMMARY = re.compile(r"^\[soc\] bots \d+ .*?\bcorps (\d+) alliances (\d+) blue (\d+) red (\d+)(?: day (\d+))?")
STANCE = re.compile(r"^\[soc\] .*?\bcorp stance (\S+) -> (\S+): (\w+) -> (\w+) \(R ")
# #152, informational: the driver echoes a corp turn's line as `[soc] NAME corp LINE`.
ADVERT = re.compile(r"^\[soc\] .*?\badvert (posted|removed) corp=(\d+)")
FINDER = re.compile(r"^\[soc\] .*?\bapplied corp=(\d+) via=finder\b")
# #155: the founder's turn lines, echoed as `[soc] NAME corp LINE` (corpmind.rs `Plan::Found`, `Plan::Pledge`).
FOUND_OK = re.compile(r"^\[soc\] (?P<bot>\S+) corp found (?P<what>.+): ok$")
FOUND_REFUSED = re.compile(r"^\[soc\] (?P<bot>\S+) corp found (?P<what>.+): refused (?P<key>\S+)$")
PLEDGE_OK = re.compile(r"^\[soc\] (?P<bot>\S+) corp pledge (?P<amount>\d+(?:\.\d+)?) to (?P<to>\d+): ok$")
LOSS = re.compile(r"^\[soc\] relation loss\b")
REL_PAIR = re.compile(r"^\[soc\] relation pair\b")
ENGAGE = re.compile(r"^\[soc\] engage\b")
BETRAYAL = re.compile(r"^\[soc\] betrayal victim=")
FADE_SLACK = 1.1  # [A] see gate_rel
TOKEN = re.compile(r"(\w+)=(\S+)")
LAUNCHED = re.compile(r"^\[sov\] hub (\d+) launched in (\S+) by ")
CAPTURED = re.compile(r"^\[sov\] hub (\d+) in (\S+) captured by alliance (\S+) ")
BEGINS = re.compile(r"^\[sov\] campaign in (\S+) begins on hub (\d+)")
NODE = re.compile(r"^\[sov\] command node \d+ appears in \S+ for the campaign on hub (\d+)")
TAKEN = re.compile(r"^\[sov\] a node of hub (\d+) was taken by")


def num(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return None


class Log:
    """Everything the report reads from a run's files."""

    def __init__(self):
        self.summaries = []  # (corps, alliances, blue, red, day or None)
        self.stances = []  # (tick, day or None, owner, target, from, to)
        self.losses = []  # kv dicts
        self.seq = 0  # position of the last REL-relevant line, to order lines without a day
        self.relpairs = []  # (viewer, target, r, t, half, touched or None)
        self.engages = []  # kv dicts
        self.betrayals = []  # (seq, kv dict)
        self.launched = []  # (hub, system)
        self.captured = []  # (hub, system, alliance)
        self.begun = []  # hub ids
        self.nodes = []  # hub ids
        self.taken = []  # hub ids
        self.adverts_posted = []  # corp ids
        self.adverts_removed = []  # corp ids
        self.applied_finder = []  # corp ids
        self.founded = []  # (bot, what)
        self.found_refused = []  # (bot, what, key)
        self.pledges = []  # (bot, amount, pool founder id)

    def feed(self, lines):
        for line in lines:
            line = line.rstrip("\n")
            # A line may carry a prefix (a timestamp, a bot name).
            for tag in TAGS:
                i = line.find(tag)
                if i >= 0:
                    line = line[i:]
                    break
            else:
                continue
            m = SUMMARY.match(line)
            if m:
                day = int(m.group(5)) if m.group(5) else None
                self.summaries.append((*(int(m.group(i)) for i in range(1, 5)), day))
                continue
            m = STANCE.match(line)
            if m:
                tick = len(self.summaries)
                day = self.summaries[-1][4] if self.summaries else None
                self.stances.append((tick, day, m.group(1), m.group(2), m.group(3), m.group(4)))
                continue
            m = ADVERT.match(line)
            if m:
                (self.adverts_posted if m.group(1) == "posted" else self.adverts_removed).append(m.group(2))
                continue
            m = FINDER.match(line)
            if m:
                self.applied_finder.append(m.group(1))
                continue
            m = FOUND_OK.match(line)
            if m:
                self.founded.append((m["bot"], m["what"]))
                continue
            m = FOUND_REFUSED.match(line)
            if m:
                self.found_refused.append((m["bot"], m["what"], m["key"]))
                continue
            m = PLEDGE_OK.match(line)
            if m:
                self.pledges.append((m["bot"], float(m["amount"]), m["to"]))
                continue
            if LOSS.match(line):
                self.seq += 1
                kv = dict(TOKEN.findall(line))
                kv["_seq"] = self.seq
                self.losses.append(kv)
                continue
            if REL_PAIR.match(line):
                self.seq += 1
                kv = dict(TOKEN.findall(line))
                r, t, half = num(kv.get("r")), num(kv.get("t")), num(kv.get("half"))
                if None not in (r, t, half) and "viewer" in kv and "target" in kv:
                    touched = None if kv.get("touched") == "none" else num(kv.get("touched"))
                    self.relpairs.append((kv["viewer"], kv["target"], r, t, half, touched))
                continue
            if ENGAGE.match(line):
                self.engages.append(dict(TOKEN.findall(line)))
                continue
            if BETRAYAL.match(line):
                self.seq += 1
                self.betrayals.append((self.seq, dict(TOKEN.findall(line))))
                continue
            m = LAUNCHED.match(line)
            if m:
                self.launched.append((m.group(1), m.group(2)))
                continue
            m = CAPTURED.match(line)
            if m:
                self.captured.append((m.group(1), m.group(2), m.group(3)))
                continue
            m = BEGINS.match(line)
            if m:
                self.begun.append(m.group(2))
                continue
            m = NODE.match(line)
            if m:
                self.nodes.append(m.group(1))
                continue
            m = TAKEN.match(line)
            if m:
                self.taken.append(m.group(1))


def read(paths):
    log = Log()
    for p in paths:
        with open(p, errors="replace") as f:
            log.feed(f)
    return log

def _ff():
    """tools/ff_verdict.py, loaded by path: `python3 -I` leaves tools/ off sys.path."""
    import importlib.util
    
    spec = importlib.util.spec_from_file_location("ff_verdict", os.path.join(os.path.dirname(os.path.abspath(__file__)), "ff_verdict.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# ---- GC ---------------------------------------------------------------------


def flaps(stances, day_of):
    """Per directed pair: the most sign changes (blue<->red, a neutral gap
    ignored) inside one game day. `day_of(entry)` is the entry's time in game
    days. Returns {(owner, target): max in a window}."""
    by_pair = {}
    for e in stances:
        by_pair.setdefault((e[2], e[3]), []).append(e)
    out = {}
    for pair, es in by_pair.items():
        last = None
        times = []
        for e in es:
            to = e[5]
            if to not in ("blue", "red"):
                continue
            if last is not None and to != last:
                times.append(day_of(e))
            last = to
        best = 0
        for i, t in enumerate(times):
            best = max(best, sum(1 for u in times[i:] if u - t < 1.0))
        out[pair] = best
    return out


def gate_gc(log, game_day_secs=None, tick_secs=DEFAULT_TICK_SECS):
    """The criteria as (name, status, detail) rows, and the gate's verdict."""
    rows = []
    if not log.summaries:
        return NA, "no `[soc] bots .. corps N alliances N` line: the society was off (needs --society)", rows
    last, peak_blue, peak_red = log.summaries[-1], max(s[2] for s in log.summaries), max(s[3] for s in log.summaries)
    rows.append(("corps", PASS if last[0] >= MIN_CORPS else FAIL, f"{last[0]} (need {MIN_CORPS})"))
    rows.append(("alliances", PASS if last[1] >= MIN_ALLIANCES else FAIL, f"{last[1]} (need {MIN_ALLIANCES})"))
    rows.append(("blue pair", PASS if peak_blue >= MIN_BLUE else FAIL, f"peak {peak_blue} (need {MIN_BLUE})"))
    rows.append(("red pair", PASS if peak_red >= MIN_RED else FAIL, f"peak {peak_red} (need {MIN_RED})"))
    if not log.losses:
        rows.append(("loss moves R", NA, "no `[soc] relation loss` line (society::loss_line is not wired into the hall yet)"))
    else:
        moved = [l for l in log.losses if num(l.get("before")) is not None and num(l.get("after")) is not None
                 and num(l["before"]) != num(l["after"])]
        rows.append(("loss moves R", PASS if moved else FAIL, f"{len(moved)} of {len(log.losses)} losses moved a relation"))
    if all(s[4] is not None for s in log.summaries):
        day_of = lambda e: float(e[1] if e[1] is not None else 0)  # noqa: E731
        clock = "summary `day` field"
    elif game_day_secs:
        day_of = lambda e: e[0] * tick_secs / game_day_secs  # noqa: E731
        clock = f"summary ticks of {tick_secs:g}s over {game_day_secs:g}s game days"
    else:
        day_of = None
    if not log.stances:
        rows.append(("no flapping", NA, "no `[soc] .. corp stance` lines: run the soak with echo on"))
    elif day_of is None:
        rows.append(("no flapping", NA, "no game clock: pass --game-day-secs, or log a `day N` summary field"))
    else:
        fl = flaps(log.stances, day_of)
        worst = max(fl.values(), default=0)
        bad = sorted(p for p, n in fl.items() if n > MAX_FLIPS_PER_DAY)
        detail = f"{len(fl)} pairs seen, most sign changes in a game day {worst} (limit {MAX_FLIPS_PER_DAY}); clock: {clock}"
        if bad:
            detail += "; flapping: " + ", ".join(f"{o}->{t}" for o, t in bad[:5])
        rows.append(("no flapping", FAIL if bad else PASS, detail))
    return verdict(rows), f"{len(log.summaries)} summaries, last: corps {last[0]} alliances {last[1]}", rows


# ---- REL --------------------------------------------------------------------


def fade_checks(log):
    """(compared, failures): consecutive `relation pair` lines of one pair with t1 > t0, and the later
    line's `touched` stamp before t0 (the matrix only decayed in between)."""
    by_pair = {}
    for v, t, r, at, half, touched in log.relpairs:
        by_pair.setdefault((v, t), []).append((r, at, half, touched))
    compared, bad = 0, []
    for (v, t), rows in by_pair.items():
        for (r0, t0, half0, _), (r1, t1, _, touched1) in zip(rows, rows[1:]):
            k = t1 - t0
            if k <= 0 or half0 <= 0 or touched1 is None or touched1 >= t0:
                continue
            compared += 1
            if abs(r1) > abs(r0) * 0.5 ** (k / half0) * FADE_SLACK:
                bad.append(f"{v}->{t} {r0:g} to {r1:g} over {k:.3f}d (half {half0:g}d)")
    return compared, bad


def gate_rel(log):
    """#95: the relation rows. n/a when the run printed no relation, engage or betrayal line."""
    rows = []
    if not (log.relpairs or log.engages or log.betrayals):
        return NA, "no `relation pair`, `engage` or `betrayal` line: the run predates SOAK-REL or had no society", rows
    if not log.engages:
        rows.append(("no friendly engage", NA, "no `[soc] engage` line"))
    else:
        friendly = [e for e in log.engages if e.get("stance") in ("friendly", "allied")]
        detail = f"{len(log.engages)} engages, {len(friendly)} against a friendly or allied group"
        if friendly:
            detail += ": " + ", ".join(f"{e.get('shooter')}->{e.get('target')}" for e in friendly[:5])
        rows.append(("no friendly engage", FAIL if friendly else PASS, detail))
    judged = [kv for _, kv in log.betrayals if kv.get("stance")]
    if not judged:
        rows.append(("betrayal sours", NA, "no `[soc] betrayal` line with stance="))
    else:
        bad = [kv for kv in judged if kv["stance"] != "hostile"]
        detail = f"{len(judged)} betrayals, {len(bad)} whose stance is not hostile"
        if bad:
            detail += ": " + ", ".join(
                f"{kv.get('victim')}->{kv.get('killer')} {kv['stance']} (R {kv.get('before')} to {kv.get('after')})"
                for kv in bad[:5]
            )
        else:
            detail += "; R " + ", ".join(f"{kv.get('before')} to {kv.get('after')}" for kv in judged[:3])
        rows.append(("betrayal sours", FAIL if bad else PASS, detail))
    if not log.relpairs:
        rows.append(("memory fades", NA, "no `[soc] relation pair` line"))
    else:
        compared, bad = fade_checks(log)
        if not compared:
            rows.append(("memory fades", NA, "no pair with two lines and no add between them"))
        else:
            detail = f"{compared} intervals, {len(bad)} faded less than their half-life allows (x{FADE_SLACK:g})"
            if bad:
                detail += ": " + "; ".join(bad[:5])
            rows.append(("memory fades", FAIL if bad else PASS, detail))
    return verdict(rows), f"{len(log.relpairs)} pair lines, {len(log.engages)} engages, {len(log.betrayals)} betrayals", rows


# ---- GD ---------------------------------------------------------------------


def owner_changes(captured):
    """Captures of a system by an alliance other than its previous capturer."""
    owner, n = {}, 0
    for _hub, system, alliance in captured:
        if system in owner and owner[system] != alliance:
            n += 1
        owner[system] = alliance
    return n


def gate_gd(log):
    rows = []
    if not (log.launched or log.captured or log.begun):
        return NA, "no `[sov]` hub or campaign line: sov was off or the server log was not given", rows
    claims = {h for h, _ in log.launched} | {h for h, _, _ in log.captured}
    rows.append(("claims", PASS if len(claims) >= MIN_CLAIMS else FAIL, f"{len(claims)} hubs (need {MIN_CLAIMS})"))
    contested = sorted(set(log.begun) & set(log.taken))
    rows.append(("contested campaign", PASS if len(contested) >= MIN_CONTESTED else FAIL,
                 f"{len(set(log.begun))} began, {len(set(log.nodes))} with a node, {len(set(log.taken))} with a node taken; "
                 f"{len(contested)} began and had a node fight (need {MIN_CONTESTED})"))
    if not log.captured:
        rows.append(("change of owner", NA, "no `[sov] .. captured by alliance` line"))
    else:
        n = owner_changes(log.captured)
        rows.append(("change of owner", PASS if n >= MIN_OWNER_CHANGES else FAIL,
                     f"{n} (need {MIN_OWNER_CHANGES}) over {len(log.captured)} captures"))
    return verdict(rows), f"{len(claims)} claims, {len(set(log.begun))} campaigns", rows


def gate_found(log):
    """#155: a bot corp was founded. PASS: a `found .. : ok` line and the last summary has
    corps >= 1; else FAIL if any `found .. : refused KEY`; else n/a (no pool reached the line)."""
    rows = []
    last_corps = log.summaries[-1][0] if log.summaries else 0
    pledged = sum(p[1] for p in log.pledges)
    detail = (f"{len(log.founded)} found ok, {len(log.found_refused)} refused, last summary corps {last_corps}, "
              f"{len(log.pledges)} pledges totalling {pledged:g}")
    rows.append(("founded", PASS if log.founded else (FAIL if log.found_refused else NA),
                 f"{len(log.founded)} `corp found .. : ok` lines"))
    rows.append(("corps in summary", PASS if last_corps >= 1 else (FAIL if log.found_refused else NA),
                 f"last `[soc] bots ..` summary has corps {last_corps}"))
    if log.founded and last_corps >= 1:
        return PASS, detail, rows
    if log.found_refused:
        keys = sorted({k for _, _, k in log.found_refused})
        return FAIL, detail + "; refused: " + ", ".join(keys[:5]), rows
    return NA, detail + "; nothing founded and nothing refused", rows


def verdict(rows):
    if any(s == FAIL for _, s, _ in rows):
        return FAIL
    if any(s == NA for _, s, _ in rows):
        return NA  # a criterion without an instrument is never a pass
    return PASS


# ---- report -----------------------------------------------------------------


def judge(log, game_day_secs=None, tick_secs=DEFAULT_TICK_SECS):
    res = {"GC": gate_gc(log, game_day_secs, tick_secs)}
    # REL (#95) is judged only when the log carries a relation, engage or betrayal line.
    if log.relpairs or log.engages or log.betrayals:
        res["REL"] = gate_rel(log)
    # FOUND is judged only when the log carries a founding line (an older soak reports as before).
    if log.founded or log.found_refused or log.pledges:
        res["FOUND"] = gate_found(log)
    res["GD"] = gate_gd(log)
    return res


def render(res):
    out = []
    for gid, (status, detail, rows) in res.items():
        out.append(f"{gid}  {status}  {detail}")
        for name, s, d in rows:
            out.append(f"      {s:4}  {name}: {d}")
    fails = sorted(g for g, (s, _, _) in res.items() if s == FAIL)
    nas = sorted(g for g, (s, _, _) in res.items() if s == NA)
    out.append(f"gates: {sum(s == PASS for s, _, _ in res.values())} PASS, {len(fails)} FAIL, {len(nas)} n/a")
    return "\n".join(out)


def info(log):
    """Counts that are shown, never judged (#152): adverts and finder applications."""
    return {
        "adverts posted": len(log.adverts_posted),
        "adverts removed": len(log.adverts_removed),
        "corps advertising": len(set(log.adverts_posted)),
        "applied via finder": len(log.applied_finder),
    }


def render_info(counts):
    return "info (not a gate): " + ", ".join(f"{k} {v}" for k, v in counts.items())


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("logs", nargs="+", help="bots.log / server.log files")
    ap.add_argument("--game-day-secs", type=float, default=None, help="real seconds per game day (the flap clock)")
    ap.add_argument("--tick-secs", type=float, default=DEFAULT_TICK_SECS, help="seconds between `[soc]` summaries")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args(argv)
    log = read(a.logs)
    res = judge(log, a.game_day_secs, a.tick_secs)
    ff = _ff()
    ffrun = ff.parse_paths(a.logs)
    if ff.gate(ffrun)[0] == ff.NA:
        res = {g: (NA, f"{ff.gate(ffrun)[1]}; was {s}: {d}", rows) for g, (s, d, rows) in res.items()}
    if a.json:
        print(json.dumps({**({"ff": ff.banner(ffrun)} if ffrun else {}), "info": info(log), **{g: {"status": s, "detail": d, "criteria": [list(r) for r in rows]}
                          for g, (s, d, rows) in res.items()}}, indent=1))
    else:
        if ffrun:
            print(ff.banner(ffrun))
        print(render(res))
        print(render_info(info(log)))
    return 1 if any(s == FAIL for s, _, _ in res.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
