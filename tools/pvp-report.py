#!/usr/bin/env python3
"""The bots' PvP behaviours, as one PASS/FAIL line each with the counts read (#12).

    python3 -I tools/pvp-report.py [server/target/soak/bots.log ..] [--strict]

Reads the bots' log (`tools/soak.sh` writes `server/target/soak/bots.log`; the
act lines go to stdout and the `[warp_bubble]` notes to stderr, both land in
it). One line per behaviour:

  probes    dictors launch probes     act line `kit  launch probe from ID -> ok`
  field     HICs run a field          act line `kit  field on ID -> ok`
  bubble    bubble holds              `[warp_bubble] NAME: held` then `free after N s`
  tackler   kill the tackler first    `[counter] NAME: tackler-first ... ok=1|0`
  dictor    pick the dictor when held `[counter] NAME: dictor-held ... ok=1|0`
  bomb      keep off a smartbomb      `[counter] NAME: bomb-keepoff ... ok=1|0`
  mjd       answer an MJD with a scram `[counter] NAME: mjd-scram ... ok=1|0`
  gate      a gang holds a gate       `[camp] NAME: holding gate=ID system=S ok=1|0`
  capital   capital sourced           `[cap] NAME hull=TYPE from=hangar|market|aboard` before `capital: siege on` (n/a with no siege line)

The first three read the act and `[warp_bubble]` lines. The last five read the
`[counter]` and `[camp]` lines that counter.rs `log_decision` prints (#136), at
the engaged target (`counter::orbit`), the drive answer (`counter::mjd_scram`)
and the post (`camp::post`), once per change of target and outcome. A log with
none of them says FAIL-NO-SIGNAL, never PASS. Exit status: 1 if any line is FAIL, or with --strict any FAIL-NO-SIGNAL;
else 0. Only the standard library.

The capital line reads the `[cap]` lines and the `capital: siege on` act note (also inside a
longer note, `capital pair: ...: capital: siege on, ...`). A bot's `[cap]` line and its siege
must be in the same file, or the files given in time order. A bot whose only source is
`from=aboard` (the hull was aboard at login) counts as n/a ("aboard at login"), never sourced.
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

PASS, FAIL, NO_SIGNAL, NA = "PASS", "FAIL", "FAIL-NO-SIGNAL", "n/a"

# An act line (driver.rs `record_as`): `NAME +MM:SS.s KIND  WHAT -> TAIL`.
ACT = re.compile(r"^(?P<bot>\S+) \+\d+:[\d.]+ (?P<kind>\S+)\s+(?P<what>.*) -> (?P<tail>.*)$")
# warp_bubble.rs `decide`: held / free after N s / held over the budget.
BUBBLE = re.compile(r"\[warp_bubble\] (?P<bot>[^:\s]+): (?P<what>held over the budget|held|free after \d+ s)")
# The proposed lines: `[counter] NAME: KIND ... ok=1`, `[camp] NAME: holding ... ok=1`.
SIGNAL = re.compile(r"\[(?P<tag>counter|camp)\] (?P<bot>[^:\s]+): (?P<kind>[\w-]+)(?P<rest>.*)$")

# capital.rs `log_source`: `[cap] NAME hull=TYPE from=hangar|market`; and the act note
# `capital: siege on, ...` (capital_fight.rs) a siege pilot leaves.
CAP = re.compile(r"\[cap\] (?P<bot>\S+) hull=\d+ from=(?P<src>hangar|market|aboard)\b")
SIEGE_ON = re.compile(r"capital: siege on\b")

# [A] a counter or gate behaviour passes when at least this share of its
# outcome-marked lines say ok=1. Falsified by a soak whose fights show the
# bots dropping the counter while the share stays above it.
MIN_OK_SHARE = 0.5

COUNTER = (
    ("tackler", "kill the tackler first", "counter", "tackler-first",
     "[counter] NAME: tackler-first target=ID ok=1|0 (a tackler or jammer is on the bot: ok=1 when the engaged target is one of them)"),
    ("dictor", "pick the dictor when held", "counter", "dictor-held",
     "[counter] NAME: dictor-held target=ID ok=1|0 (held in a bubble: ok=1 when the engaged target is a bubbler hull)"),
    ("bomb", "keep off a smartbomb", "counter", "bomb-keepoff",
     "[counter] NAME: bomb-keepoff target=ID range=M ok=1|0 (range is the orbit chosen: ok=1 at or above BOMB_SAFE)"),
    ("mjd", "answer an MJD with a scram", "counter", "mjd-scram",
     "[counter] NAME: mjd-scram target=ID ok=1|0 (a drive is spooling: ok=1 when a lock or scram went at it)"),
    ("gate", "a gang holds a gate", "camp", "holding",
     "[camp] NAME: holding gate=ID system=S ok=1|0 (the camper was posted to its gate and its warp has ended, so it holds there; ok=0: no gate to take; gate=none)"),
)


class Log:
    """What the report reads from the log files."""

    def __init__(self):
        self.launch_ok = []  # bot names, one per launch that was not refused
        self.launch_refused = 0
        self.field_on = []
        self.field_off = 0
        self.field_refused = 0
        self.held = []
        self.free = 0
        self.over_budget = 0
        self.signals = {}  # (tag, kind) -> [ok bot names]
        self.bad = {}  # (tag, kind) -> count of ok=0
        self.cap_sourced = set()  # bots with a hangar or market `[cap]` line so far
        self.cap_aboard = set()  # bots with only a from=aboard line so far
        self.sieged = []  # bots whose `capital: siege on` came with no earlier [cap] line
        self.aboard_sieged = []  # sieged bots whose only source line is from=aboard
        self.siege_lines = 0

    def feed(self, lines):
        for line in lines:
            line = line.rstrip("\n")
            m = CAP.search(line)
            if m:
                (self.cap_aboard if m["src"] == "aboard" else self.cap_sourced).add(m["bot"])
                continue
            m = ACT.match(line)
            if m and SIEGE_ON.search(m["what"] + " " + m["tail"]):
                self.siege_lines += 1
                bot = m["bot"]
                if bot not in self.cap_sourced:
                    if bot in self.cap_aboard:
                        if bot not in self.aboard_sieged:
                            self.aboard_sieged.append(bot)
                    elif bot not in self.sieged:
                        self.sieged.append(bot)
            if m and m["kind"] == "kit":
                self.act(m["bot"], m["what"], m["tail"])
                continue
            m = BUBBLE.search(line)
            if m:
                w = m["what"]
                if w == "held":
                    self.held.append(m["bot"])
                elif w == "held over the budget":
                    self.over_budget += 1
                else:
                    self.free += 1
                continue
            m = SIGNAL.search(line)
            if m:
                key = (m["tag"], m["kind"])
                if re.search(r"\bok=0\b", m["rest"]):
                    self.bad[key] = self.bad.get(key, 0) + 1
                elif re.search(r"\bok=1\b", m["rest"]):
                    self.signals.setdefault(key, []).append(m["bot"])

    def act(self, bot, what, tail):
        refused = tail != "ok" or "refused" in what
        if what.startswith("launch probe from"):
            if refused:
                self.launch_refused += 1
            else:
                self.launch_ok.append(bot)
        elif what.startswith("field on"):
            if refused:
                self.field_refused += 1
            else:
                self.field_on.append(bot)
        elif what.startswith("field off") and not refused:
            self.field_off += 1


def verdicts(log):
    """[(key, behaviour, verdict, detail)] in report order."""
    out = []
    n = len(log.launch_ok)
    out.append(("probes", "dictors launch probes", PASS if n else FAIL,
                f"{n} launches by {len(set(log.launch_ok))} bots, {log.launch_refused} refused"))
    n = len(log.field_on)
    out.append(("field", "HICs run a field", PASS if n else FAIL,
                f"{n} field-on by {len(set(log.field_on))} bots, {log.field_off} field-off, "
                f"{log.field_refused} refused"))
    held = len(log.held)
    ok = held > 0 and log.free > 0 and log.over_budget == 0
    out.append(("bubble", "bubble holds", PASS if ok else FAIL,
                f"{held} held by {len(set(log.held))} bots, {log.free} freed, "
                f"{log.over_budget} over the budget"))
    for key, name, tag, kind, line in COUNTER:
        good = len(log.signals.get((tag, kind), []))
        bad = log.bad.get((tag, kind), 0)
        if good + bad == 0:
            out.append((key, name, NO_SIGNAL, f"0 lines read; the bot needs to log {line}"))
        else:
            share = good / (good + bad)
            out.append((key, name, PASS if good and share >= MIN_OK_SHARE else FAIL,
                        f"{good} ok, {bad} not ok ({share:.0%} ok)"))
    if not log.siege_lines:
        out.append(("capital", "capital sourced", NA, "no `capital: siege on` line"))
    else:
        bots = len(log.cap_sourced)
        aboard = f"; {len(log.aboard_sieged)} aboard at login" if log.aboard_sieged else ""
        if log.sieged:
            v = FAIL
        elif log.aboard_sieged and not bots:
            v = NA
        else:
            v = PASS
        out.append(("capital", "capital sourced", v,
                    f"{log.siege_lines} siege lines, {bots} sourced bots{aboard}"
                    + (f"; no earlier [cap] line for {', '.join(log.sieged)}" if log.sieged else "")))
    return out


def report(log):
    return [f"{v:<14} {name}: {detail}" for _, name, v, detail in verdicts(log)]


def status(log, strict=False):
    bad = {FAIL, NO_SIGNAL} if strict else {FAIL}
    return 1 if any(v in bad for _, _, v, _ in verdicts(log)) else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description="PASS/FAIL per PvP behaviour from the soak's bot log")
    ap.add_argument("logs", nargs="*", help="default: server/target/soak/bots.log")
    ap.add_argument("--strict", action="store_true", help="FAIL-NO-SIGNAL also exits 1")
    a = ap.parse_args(argv)
    here = os.path.dirname(os.path.abspath(__file__))
    logs = a.logs or [os.path.join(here, "..", "server", "target", "soak", "bots.log")]
    log = Log()
    for path in logs:
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                log.feed(f)
        except OSError as e:
            print(f"pvp-report: {path}: {e}", file=_sys.stderr)
            return 2
    print("\n".join(report(log)))
    return status(log, a.strict)


if __name__ == "__main__":
    _sys.exit(main())
