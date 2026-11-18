#!/usr/bin/env python3
"""How long the server lags the Tranquility client build (tracking-tq.md, "Metrics").

`docs/status/tq-builds.tsv` holds one row per TQ build: build number, line, tier and the
UTC time of each step (seen, imported, provisional, served, pinned, retired). This tool
records a step, checks each build against its tier's target and prints the "Tracking TQ"
block that `tools/status-page.py` puts on the status page. Stdlib only; it never touches
the network (the tracker count reads a file the caller made).

    python3 -I tools/tq-lag.py --record STEP BUILD [--at UTC] [--line L] [--tier T] [--issue N]
    python3 -I tools/tq-lag.py --check [--now UTC]
    python3 -I tools/tq-lag.py --md [--intake-tsv FILE]

  --record   set one cell (STEP: seen, imported, provisional, served, pinned, retired). A cell
             never moves backwards: an earlier time than the one there is refused. `seen` may
             start a row (--line and --tier fill it in). --at defaults to now.
  --check    print each unserved build's lag against its tier's target; exit 3 when one is over.
  --md       print the block. Open `tq-intake` issues are counted per area from --intake-tsv,
             the output of `tools/tracker.py issues` (already filtered to that label); without
             it the block says "tracker not asked" and never prints a zero.

Columns: build line tier seen_utc seen_by imported_utc provisional_utc served_utc pinned_utc
retired_utc issue note. `-` is a step not reached. `note` carries an `[A]` for any time that is
an assumption, with what would falsify it. Times are `YYYY-MM-DDTHH:MM:SSZ`.

Lag: build lag (h) = served - seen; login lag (h) = (provisional or served) - seen.
Targets (hours from `seen`, plan 3.4): T0 provisional 2 and served 24; T1 and T2 served 24;
T3 none (best effort). A build with no tier or no `seen` time has no target.

Exit: 0 ok; 3 a build is over target (--check); 1 the file is malformed; 2 usage or a refusal.
"""

from __future__ import annotations

import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import datetime as dt
import os
import re
import statistics
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TSV = os.path.join(ROOT, "docs", "status", "tq-builds.tsv")
COLUMNS = ["build", "line", "tier", "seen_utc", "seen_by", "imported_utc", "provisional_utc",
           "served_utc", "pinned_utc", "retired_utc", "issue", "note"]
STEPS = ["seen", "imported", "provisional", "served", "pinned", "retired"]
TIERS = ["T0", "T1", "T2", "T3"]
SEEN_BY = ["notes", "esi", "launcher", "report", "-"]
# hours from `seen`: (provisional target, served target); None is no target
TARGETS = {"T0": (2, 24), "T1": (None, 24), "T2": (None, 24), "T3": (None, None)}
TIME_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
FMT = "%Y-%m-%dT%H:%M:%SZ"


class Bad(Exception):
    pass


def parse_time(s: str) -> dt.datetime:
    return dt.datetime.strptime(s, FMT).replace(tzinfo=dt.timezone.utc)


def fmt_time(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime(FMT)


def read(path: str) -> tuple[list[str], list[dict]]:
    """(the comment lines, the rows). Raises Bad on anything unsound."""
    comments, rows, header = [], [], False
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f.read().splitlines(), 1):
            if line.startswith("#") or not line.strip():
                if not header:
                    comments.append(line)
                continue
            cells = line.split("\t")
            if not header:
                if cells != COLUMNS:
                    raise Bad(f"{path}:{n}: the header must be {' '.join(COLUMNS)}")
                header = True
                continue
            if len(cells) != len(COLUMNS):
                raise Bad(f"{path}:{n}: {len(cells)} columns, want {len(COLUMNS)}")
            r = dict(zip(COLUMNS, cells))
            r["line_no"] = n
            if not r["build"].isdigit():
                raise Bad(f"{path}:{n}: build '{r['build']}' is not a number")
            if r["tier"] != "-" and r["tier"] not in TIERS:
                raise Bad(f"{path}:{n}: tier '{r['tier']}' is not T0-T3 or -")
            if r["seen_by"] not in SEEN_BY:
                raise Bad(f"{path}:{n}: seen_by '{r['seen_by']}' is not one of {' '.join(SEEN_BY)}")
            for c in COLUMNS:
                if c.endswith("_utc") and r[c] != "-":
                    try:
                        if not TIME_RE.match(r[c]):
                            raise ValueError(r[c])
                        parse_time(r[c])
                    except ValueError:
                        raise Bad(f"{path}:{n}: {c} '{r[c]}' is not a real UTC time (YYYY-MM-DDTHH:MM:SSZ) or -")
            if any(x["build"] == r["build"] for x in rows):
                raise Bad(f"{path}:{n}: build {r['build']} twice")
            rows.append(r)
    if not header:
        raise Bad(f"{path}: no header")
    return comments, rows


def write(path: str, comments: list[str], rows: list[dict]) -> None:
    out = list(comments) + ["\t".join(COLUMNS)]
    out += ["\t".join(r[c] for c in COLUMNS) for r in sorted(rows, key=lambda r: int(r["build"]))]
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    os.replace(tmp, path)


def hours(a: str, b: str) -> float | None:
    """b - a in hours; None when either is `-`."""
    if a == "-" or b == "-":
        return None
    return (parse_time(b) - parse_time(a)).total_seconds() / 3600


def build_lag(r: dict) -> float | None:
    return hours(r["seen_utc"], r["served_utc"])


def login_lag(r: dict) -> float | None:
    return hours(r["seen_utc"], r["provisional_utc"] if r["provisional_utc"] != "-" else r["served_utc"])


def record(rows: list[dict], step: str, build: str, at: str, line: str, tier: str, issue: str) -> str:
    """Set one cell in `rows`; returns a message. Raises Bad on a refusal."""
    if step not in STEPS:
        raise Bad(f"unknown step '{step}' (want {', '.join(STEPS)})")
    if not build.isdigit():
        raise Bad(f"build '{build}' is not a number")
    try:
        if not TIME_RE.match(at):
            raise ValueError(at)
        parse_time(at)
    except ValueError:
        raise Bad(f"time '{at}' is not a real UTC time (YYYY-MM-DDTHH:MM:SSZ)")
    r = next((x for x in rows if x["build"] == build), None)
    if r is None:
        if step != "seen":
            raise Bad(f"build {build} is not in the table: record `seen` first")
        r = {c: "-" for c in COLUMNS}
        r.update(build=build, line=line or "-", tier=tier or "-", note="")
        rows.append(r)
    if line:
        r["line"] = line
    if tier:
        if tier not in TIERS:
            raise Bad(f"tier '{tier}' is not T0-T3")
        r["tier"] = tier
    if issue:
        r["issue"] = issue
    col = step + "_utc"
    if r[col] != "-" and parse_time(at) < parse_time(r[col]):
        raise Bad(f"{col} of {build} is {r[col]}; {at} is earlier, and a cell never moves backwards")
    r[col] = at
    return f"{build} {col} = {at}"


def over_target(r: dict, now: dt.datetime) -> list[str]:
    """What an unserved build is over, as messages; empty when on time or without a target."""
    if r["tier"] not in TARGETS or r["seen_utc"] == "-" or r["served_utc"] != "-" or r["retired_utc"] != "-":
        return []
    prov, served = TARGETS[r["tier"]]
    age = (now - parse_time(r["seen_utc"])).total_seconds() / 3600
    out = []
    if prov is not None and r["provisional_utc"] == "-" and age > prov:
        out.append(f"{age:.1f} h since seen, provisional target {prov} h")
    if served is not None and age > served:
        out.append(f"{age:.1f} h since seen, served target {served} h")
    return out


def check(rows: list[dict], now: dt.datetime, out) -> int:
    code = 0
    for r in sorted(rows, key=lambda r: int(r["build"])):
        if r["served_utc"] != "-" or r["retired_utc"] != "-":
            continue
        prov, served = TARGETS.get(r["tier"], (None, None))
        if r["seen_utc"] == "-" or served is None and prov is None:
            print(f"{r['build']}\t{r['tier']}\tno target", file=out)
            continue
        age = (now - parse_time(r["seen_utc"])).total_seconds() / 3600
        late = over_target(r, now)
        print(f"{r['build']}\t{r['tier']}\t{age:.1f} h since seen\t{'OVER: ' + '; '.join(late) if late else 'on time'}", file=out)
        if late:
            code = 3
    return code


def latest(rows: list[dict], col: str) -> dict | None:
    have = [r for r in rows if r[col] != "-"]
    return max(have, key=lambda r: (r[col], int(r["build"]))) if have else None


def md(rows: list[dict], intake: dict[str, int] | None = None, tracker_line: bool = True) -> str:
    """The "Tracking TQ" block. A pure function of the rows (and the optional intake counts)."""
    seen, served, pinned = latest(rows, "seen_utc"), latest(rows, "served_utc"), latest(rows, "pinned_utc")
    name = lambda r: f"{r['build']} ({r['line']})" if r else "none"
    out = ["## Tracking TQ", "",
           f"TQ build **{name(seen)}** · served **{name(served)}** · pinned **{name(pinned)}**.", ""]
    by_seen = sorted(rows, key=lambda r: (r["seen_utc"], int(r["build"])))[-5:]
    if by_seen:
        out += ["| build | tier | login lag (h) | build lag (h) |", "|---|---|--:|--:|"]
        for r in reversed(by_seen):
            f = lambda v: "-" if v is None else f"{v:.1f}"
            out.append(f"| {r['build']} | {r['tier']} | {f(login_lag(r))} | {f(build_lag(r))} |")
        out.append("")
    meds = []
    for t in TIERS:
        v = [x for x in (build_lag(r) for r in rows if r["tier"] == t) if x is not None]
        if v:
            meds.append(f"{t} {statistics.median(v):.1f} h ({len(v)})")
    out.append("Median build lag per tier: " + (", ".join(meds) if meds else "no served build yet") + ".")
    drift = 0
    if pinned:
        drift = sum(1 for r in rows if r["served_utc"] != "-" and r["served_utc"] > pinned["pinned_utc"])
    out.append(f"Pin drift: {drift} build{'s' if drift != 1 else ''} served since the last re-pin.")
    if tracker_line:
        if intake is None:
            out.append("Open `tq-intake` issues: tracker not asked.")
        else:
            out.append("Open `tq-intake` issues: " + (", ".join(f"{a} {n}" for a, n in sorted(intake.items())) or "none") + ".")
    guess = [r["build"] for r in rows if "[A]" in r["note"]]
    if guess:
        out += ["", "Times that are assumptions, not proved (see `note` in `docs/status/tq-builds.tsv`): "
                + ", ".join(guess) + "."]
    return "\n".join(out) + "\n"


def intake_counts(path: str) -> dict[str, int]:
    """Per-area counts from the output of `tracker.py issues` (header: issue ready prio area ...)."""
    counts: dict[str, int] = {}
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    for l in lines[1:]:
        c = l.split("\t")
        if len(c) >= 4:
            counts[c[3] or "none"] = counts.get(c[3] or "none", 0) + 1
    return counts


def main(argv=None, path: str = TSV, out=None, err=None) -> int:
    out, err = out or sys.stdout, err or sys.stderr
    ap = argparse.ArgumentParser(prog="tq-lag.py", description=__doc__.split("\n")[0])
    ap.add_argument("--record", nargs=2, metavar=("STEP", "BUILD"))
    ap.add_argument("--at")
    ap.add_argument("--line", default="")
    ap.add_argument("--tier", default="")
    ap.add_argument("--issue", default="")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--now")
    ap.add_argument("--md", action="store_true")
    ap.add_argument("--intake-tsv")
    ap.add_argument("--file", default=path)
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    if sum(map(bool, (a.record, a.check, a.md))) != 1:
        print("tq-lag: exactly one of --record, --check, --md", file=err)
        return 2
    try:
        now = parse_time(a.now) if a.now else dt.datetime.now(dt.timezone.utc)
        if a.at and not TIME_RE.match(a.at):
            print(f"tq-lag: --at '{a.at}' is not YYYY-MM-DDTHH:MM:SSZ", file=err)
            return 2
    except ValueError:
        print("tq-lag: --now is not YYYY-MM-DDTHH:MM:SSZ", file=err)
        return 2
    try:
        comments, rows = read(a.file)
    except (Bad, OSError) as e:
        print(f"tq-lag: {e}", file=err)
        return 1
    if a.record:
        try:
            msg = record(rows, a.record[0], a.record[1], a.at or fmt_time(now), a.line, a.tier, a.issue)
        except Bad as e:
            print(f"tq-lag: {e}", file=err)
            return 2
        write(a.file, comments, rows)
        print(msg, file=out)
        return 0
    if a.check:
        return check(rows, now, out)
    intake = None
    if a.intake_tsv:
        try:
            intake = intake_counts(a.intake_tsv)
        except OSError as e:
            print(f"tq-lag: cannot read --intake-tsv ({e.strerror or e}); the intake count is left out", file=err)
            intake = None
    out.write(md(rows, intake))
    return 0


if __name__ == "__main__":
    sys.exit(main())
