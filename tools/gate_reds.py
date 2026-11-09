#!/usr/bin/env python3
"""The gate's red log: failing tests per red, and which of them are flaky.

    python3 -I tools/gate_reds.py record --log L --sha S --tree T --exit N --result red|pass
    python3 -I tools/gate_reds.py report [--since-days 14] [--min 2] [--tree T --brief]
    python3 -I tools/gate_reds.py keep-log --log L --sha S

`tools/land.sh` calls `record` after every gate and `keep-log` after a red. The
file is `var/gate-reds.tsv` (gitignored): line 1 `#gate-reds 1`, line 2 the
column header, then one tab-separated row per failing item. A test is flaky
when a red on a tree is followed by a pass on the same tree, or when a rerun
rescued it. `report` always exits 0 and the recorder never changes land's exit.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins (#126).
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import datetime as dt
import fcntl
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAGIC = "#gate-reds 1"
COLUMNS = "date\tsha\ttree\tresult\ttier\tstage\texit\tkind\ttarget\ttest\tlog"
NFIELDS = 11
KEEP_LOGS = 50

STAGES = (
    ("tree hygiene", "hygiene"),
    ("feature status", "status"),
    ("dependency allowlist", "deps"),
    ("python unit tests", "python"),
    ("build deltas", "deltas"),
    ("shell syntax", "shell"),
    ("toolchain", "toolchain"),
    ("format", "format"),
    ("lint", "lint"),
    ("test", "test"),
)
TIERS = (("docs only", "docs"), ("no rust", "norust"), ("tools only", "tools"))

RE_RUNNING = re.compile(r"^\s*Running (?:unittests )?(.+?) \(\S*deps/(.+)-[0-9a-f]+\)\s*$")
RE_DOCTEST = re.compile(r"^\s*Doc-tests (\S+)")
RE_FAILED = re.compile(r"^test (.+?) \.\.\. FAILED\s*$")
RE_PANIC = re.compile(r"thread '([^']*)'(?: \(\d+\))? panicked")
RE_PYTHON = re.compile(r"^gate: (\S+\.py) failed\s*$")
RE_FORMAT = re.compile(r"^Diff in /repo/(.+?):\d+")
RE_RESULT = re.compile(r"^test result: ")


def clean(s: str) -> str:
    return re.sub(r"[\t\r\n]+", " ", s)


def parse(text: str) -> tuple[str, list[tuple[str, str, str, str]]]:
    """Return (tier, [(stage, kind, target, test)]) for one gate log."""
    tier = "full"
    stage = ""
    target = "-"
    lines = text.splitlines()
    failing = {m.group(1) for m in (RE_FAILED.match(x) for x in lines) if m}
    rows: list[tuple[str, str, str, str]] = []
    seen: set[tuple[str, str, str]] = set()
    compile_rows = 0
    saw_result = False
    pending_e = False

    def add(st: str, kind: str, tgt: str, test: str) -> None:
        key = (kind, tgt, test)
        if key in seen:
            return
        seen.add(key)
        rows.append((st, kind, tgt, test))

    for line in lines:
        if line.startswith("== "):
            body = line[3:]
            hit = False
            for prefix, name in TIERS:
                if body.startswith(prefix):
                    tier = name
                    hit = True
            if hit:
                if tier == "docs":
                    stage = "python"
                continue
            for prefix, name in STAGES:
                if body.startswith(prefix):
                    stage = name
            continue
        m = RE_RUNNING.match(line)
        if m:
            target = f"{m.group(2)}:{m.group(1)}"
            continue
        m = RE_DOCTEST.match(line)
        if m:
            target = f"{m.group(1)}:doctest"
            continue
        if RE_RESULT.match(line):
            saw_result = True
        m = RE_FAILED.match(line)
        if m:
            add(stage or "test", "test", target, m.group(1))
            continue
        m = RE_PANIC.search(line)
        if m and m.group(1) not in failing:
            add(stage or "test", "panic", target, m.group(1))
            continue
        m = RE_PYTHON.match(line)
        if m:
            add(stage or "python", "python", m.group(1), "-")
            continue
        m = RE_FORMAT.match(line)
        if m:
            add(stage or "format", "format", m.group(1), "-")
            continue
        if line.startswith("error: could not compile"):
            compile_rows += 1
            add(stage or "test", "compile", "-", "-")
            pending_e = False
            continue
        if line.startswith("error[E") and not saw_result:
            pending_e = True
    if pending_e and not compile_rows:
        add(stage or "test", "compile", "-", "-")
    if not rows:
        kind = {"status": "status", "deltas": "deltas"}.get(stage, "other")
        rows.append((stage or "-", kind, "-", "-"))
    return tier, rows


def read_rows(path: str) -> list[list[str]]:
    out = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.rstrip("\n")
                if not line or line.startswith("#") or line.startswith("date\t"):
                    continue
                parts = line.split("\t")
                if len(parts) == NFIELDS:
                    out.append(parts)
    except FileNotFoundError:
        pass
    return out


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def cmd_record(a: argparse.Namespace) -> int:
    path = a.file or os.path.join(ROOT, "var", "gate-reds.tsv")
    try:
        with open(a.log, encoding="utf-8", errors="replace") as f:
            text = f.read()
    except OSError as e:
        print(f"gate_reds: cannot read {a.log}: {e}", file=sys.stderr)
        return 1
    tier, parsed = parse(text)
    if a.result == "pass":
        parsed = [("-", "-", "-", "-")]
    date = now()
    rows = []
    for stage, kind, target, test in parsed:
        fields = [date, a.sha, a.tree, a.result, tier, stage, str(a.exit), kind, target, test, a.log]
        rows.append("\t".join(clean(x) for x in fields) + "\n")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path + ".lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        new = not os.path.exists(path) or os.path.getsize(path) == 0
        if not new:
            with open(path, encoding="utf-8") as f:
                first = f.readline().rstrip("\n")
            if first != MAGIC:
                print(f"gate_reds: {path} does not start with '{MAGIC}'; not writing", file=sys.stderr)
                return 1
        data = ("" if not new else MAGIC + "\n" + COLUMNS + "\n") + "".join(rows)
        fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
        try:
            os.write(fd, data.encode())
        finally:
            os.close(fd)
    return 0


def cmd_report(a: argparse.Namespace) -> int:
    path = a.file or os.path.join(ROOT, "var", "gate-reds.tsv")
    rows = read_rows(path)
    cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=a.since_days)

    def when(r: list[str]) -> dt.datetime:
        try:
            return dt.datetime.strptime(r[0], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)
        except ValueError:
            return dt.datetime.min.replace(tzinfo=dt.timezone.utc)

    pass_idx: dict[str, list[int]] = {}
    for i, r in enumerate(rows):
        if r[3] == "pass":
            pass_idx.setdefault(r[2], []).append(i)

    if a.brief:
        if not a.tree:
            return 0
        shown = set()
        for i, r in enumerate(rows):
            if r[3] == "red" and r[2] == a.tree and r[7] != "-" and any(j > i for j in pass_idx.get(a.tree, [])):
                name = r[9] if r[9] != "-" else r[8]
                if name not in shown:
                    shown.add(name)
                    print(f"land: this tree was red before: {name} ({r[0]}); marked flaky")
        return 0

    stats: dict[tuple[str, str], dict] = {}
    for i, r in enumerate(rows):
        if r[3] not in ("red", "rescued") or when(r) < cutoff:
            continue
        s = stats.setdefault((r[8], r[9]), {"reds": 0, "flaky": 0, "trees": set(), "last": "", "panic": False})
        s["reds"] += 1
        s["trees"].add(r[2])
        s["last"] = max(s["last"], r[0])
        if r[7] == "panic":
            s["panic"] = True
        if r[3] == "rescued" or any(j > i for j in pass_idx.get(r[2], [])):
            s["flaky"] += 1
    print("reds flaky trees last target test")
    for (target, test), s in sorted(stats.items(), key=lambda kv: (-kv[1]["flaky"], -kv[1]["reds"], kv[0])):
        flags = []
        if s["flaky"] >= a.min or len(s["trees"]) >= 3:
            flags.append("FLAKY")
        if s["panic"]:
            flags.append("PANIC")
        print(f"{s['reds']} {s['flaky']} {len(s['trees'])} {s['last']} {target} {test}"
              + (" " + " ".join(flags) if flags else ""))
    return 0


def cmd_keep_log(a: argparse.Namespace) -> int:
    d = a.dir or os.path.join(ROOT, "var", "gate-logs")
    os.makedirs(d, exist_ok=True)
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    dest = os.path.join(d, f"{stamp}-{a.sha[:8]}.log")
    with open(a.log, "rb") as src, open(dest, "wb") as out:
        out.write(src.read())
    names = sorted(n for n in os.listdir(d) if n.endswith(".log"))
    for n in names[:-KEEP_LOGS]:
        try:
            os.remove(os.path.join(d, n))
        except OSError:
            pass
    print(dest)
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("record")
    r.add_argument("--log", required=True)
    r.add_argument("--sha", required=True)
    r.add_argument("--tree", required=True)
    r.add_argument("--exit", type=int, required=True)
    r.add_argument("--result", choices=("red", "pass", "rescued"), required=True)
    r.add_argument("--file")
    r.set_defaults(fn=cmd_record)
    q = sub.add_parser("report")
    q.add_argument("--file")
    q.add_argument("--since-days", type=int, default=14)
    q.add_argument("--min", type=int, default=2)
    q.add_argument("--tree")
    q.add_argument("--brief", action="store_true")
    q.set_defaults(fn=cmd_report)
    k = sub.add_parser("keep-log")
    k.add_argument("--log", required=True)
    k.add_argument("--sha", required=True)
    k.add_argument("--dir")
    k.set_defaults(fn=cmd_keep_log)
    a = p.parse_args()
    return a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
