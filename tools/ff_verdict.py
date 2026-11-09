"""Read the `[ff]` lines of a fast-forward run and decide whether the reports may judge it.

The line format is pinned in specs/ff-now.md section 2 (printed by the ticker or the
scheduler):

    [ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=0
    [ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0
    [ff] stall real_s=10.0 virt_s=3601.25 running=bot-ore-3 links=127.0.0.1:40122(c2s 812/790)
    [ff] watchdog exit real_s=60.0 virt_s=3601.25

Fields are `key=value`; unknown keys are ignored. The last summary line counts. A log with
a `watchdog exit`, or with `stalls>0` on its last summary, gets the verdict `n/a`
(fast-forward spec P3). A log with no `[ff]` line gives None: the reports then behave as
they always did.

Stdlib only. Library use: `ff_verdict.parse(lines)`, `ff_verdict.gate(run)`.
"""
import os

NA = "n/a"
OK = "ok"
MARK = "[ff] "


class FfRun:
    def __init__(self):
        self.summary = None  # key -> value of the last summary line
        self.stall_events = 0  # `[ff] stall` lines seen
        self.watchdog = False  # a `[ff] watchdog exit` line was seen


def _fields(text):
    out = {}
    for tok in text.split():
        k, eq, v = tok.partition("=")
        if eq and k:
            out[k] = v
    return out


def parse(lines):
    """FfRun from the `[ff]` lines among `lines`, or None when there are none."""
    run = None
    for line in lines:
        i = line.find(MARK)
        if i < 0:
            continue
        rest = line[i + len(MARK):].strip()
        if run is None:
            run = FfRun()
        if rest.startswith("watchdog exit"):
            run.watchdog = True
        elif rest.split(None, 1)[:1] == ["stall"]:
            run.stall_events += 1
        else:
            f = _fields(rest)
            if "mode" in f:
                run.summary = f
    return run


def parse_paths(paths):
    """parse() over the readable files among `paths` (None entries and missing files are skipped)."""
    lines = []
    for p in paths:
        if not p:
            continue
        try:
            with open(p, errors="replace") as f:
                lines.extend(f.read().splitlines())
        except OSError:
            pass
    return parse(lines)


def _stalls(run):
    try:
        return int((run.summary or {}).get("stalls", "0"))
    except ValueError:
        return 0


def gate(run):
    """(status, detail): NA when the run must not be judged, else OK."""
    if run is None:
        return OK, "not a fast-forward run"
    if run.watchdog:
        return NA, "fast-forward watchdog exit: the run did not finish"
    n = _stalls(run)
    if n > 0:
        return NA, f"fast-forward run stalled ({n} stalls on the last summary)"
    return OK, "fast-forward run clean"


def describe(run):
    """The `ff: mode=... ratio=... stalls=...` line the reports print, or None."""
    if run is None:
        return None
    s = run.summary or {}
    return "ff: mode=%s ratio=%s stalls=%s" % (s.get("mode", "?"), s.get("ratio", "?"), s.get("stalls", "?"))


def banner(run):
    """The describe() line plus the gate's reason when it is n/a; None for a non-ff run."""
    d = describe(run)
    if d is None:
        return None
    status, detail = gate(run)
    return d if status == OK else f"{d}; verdict {NA}: {detail}"
