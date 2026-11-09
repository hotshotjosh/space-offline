#!/usr/bin/env python3
"""The bots' economy behaviours, as one PASS/FAIL line each with the counts read (#2, #23).

    python3 -I tools/bot-econ-report.py [RUN_DIR] [--closed] [--belts N] [--strict] [--judges] [--content DIR] [--scale N]

RUN_DIR (default `server/target/soak`) holds `bots.log`, `server.log` and, when the
server ran with EVE_ECON_TSV (tools/soak.sh sets it), `econ.tsv`. One line per item:

#23 mining (the dry-belt and spread work)
  dry       a miner's dry belt, then its next warp to another belt
            `[mine] NAME: belt ID dry, K of N belts left` (eve-bot driver.rs, #23 T3), then the
            same bot's act line `warp  CmdWarpToStuff item ID2 ...` with ID2 != ID (a warp the
            bot follows with `CmdDock ID2` is the warp home, not counted)
  spread    offline miners spread over the belts: distinct `belt=ID` in the server's
            `[settle] NAME (CHAR) miner at ST ... belt=ID` lines (#23 T4) >= min(miners, belts) / 2
            [A] per settling station (belts = the largest N of the dry lines, or --belts; with neither, the distinct
            belts seen, and then two or more miners must show two belts)
  deploy    act `rare  ...: deployed to STATION (system S, ...) -> ok` (rare_ore.rs)
  carry     act `rare  ...: carried N (> 0) units home to STATION -> ok` (rare_ore.rs)
  miner     econ.tsv `career miner` rows from day 3 have hours > 0 and per_h > 0
#2 economy items (PASS on one ok line each)
  haul      act `haul ... -> ok` (driver.rs Act::Haul)
  geode     econ act note `opened N geode(s) of type T` (nes_use.rs)
  plex      econ act note `bought|sold N PLEX ...` (nes_use.rs)
  skin      econ act note `wearing SKIN S on ship H` (nes_use.rs)
  injector  econ act note `injected about P points` or `sold N injector(s)` / `sold the injector`
  pi        act `pi ... -> ok`, or a `[settle] ... pi_farmer` line
  build     econ.tsv `build` row with hours > 0, or an `isk_sink industry fees` row with isk > 0
  careers   `[settle]` lines for at least 3 careers
  gm-setup  with --closed: no `gm-setup` act line (and some act lines to read)

Soak-D judges (#155 #156 #158 #159 #142 #144 #2 #23; specs/soakD.md section 3), on with `--judges` or when
server.log carries the S1 day-line fields (`held=`, `tx=`). Without either the report is exactly as before.
tools/soak.sh passes `--judges --content out/content --scale TIME_SCALE`. With the judges on, every line they
cover is replaced by the strict reading (the old lenient pi and geode readings do not run). The carry deadline
is `carry_days` society days of 86400 s each, divided by --scale, against the bots' wall-clock act times.
They read world.dat, world.dat.ledger/ and, with `--content DIR` (out/content), industry/blueprints.tsv and
econ/prices.tsv; `--scale N` is the soak's --time-scale (margin-real and the carry deadline depend on it):
  pick        builder-cycle `chose N at X ISK/h`: >= 1 pick and none <= 0 (#158)
  order-cap   MarketExceededOrderCount per builder-cycle line (#158)
  washes      econ.tsv `audit washes` n = 0 on every day (#158)
  ledger      `[soc] NAME progress .. career builder .. ledgers X`, first two lines per bot skipped (#158)
  installs    builders install online/offline or say `no job pays` (#158)
  margin-ref  per manufacturing job, margin at reference prices, ME 0 [A] (#142)
  margin-real realised margin of builders' product sales (#142; a negative at --scale > 1 is INCONCLUSIVE)
  reserve     a pool founder's bot-side debits never leave it under its pledges (#159)
  stall       a founded corp, or a founder sitting over the line with pledges while online (#155)
  geode, plex, skin, injector, haul, pi, deploy, carry   the stricter #2 / #144 / #23 readings
NO-SUPPLY (nothing to use yet) and INCONCLUSIVE are never a pass: exit 0, but --strict exits 1 on them.

A line whose source line does not exist in the logs says FAIL-NO-SIGNAL and names the line
the bot or server needs to print, never PASS. Exit status: 1 if any line is FAIL, or with
--strict any FAIL-NO-SIGNAL; 2 for a missing run dir or bots.log; else 0. Standard library only.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import glob
import math
import os
import re
import statistics

PASS, FAIL, NO_SIGNAL, NA = "PASS", "FAIL", "FAIL-NO-SIGNAL", "n/a"
NO_SUPPLY, INCONCLUSIVE = "NO-SUPPLY", "INCONCLUSIVE"

# An act line (driver.rs `record_as`): `NAME +MM:SS.s KIND  WHAT -> TAIL`.
ACT = re.compile(r"^(?P<bot>\S+) \+\d+:[\d.]+ (?P<kind>\S+)\s+(?P<what>.*) -> (?P<tail>.*)$")
WARP = re.compile(r"CmdWarpToStuff item (?P<id>\d+)\b")
DRY = re.compile(r"\[mine\] (?P<bot>[^:\s]+): belt (?P<belt>\d+) dry, (?P<left>\d+) of (?P<n>\d+) belts left")
# econ_agents.rs `Report::line`: `[settle] NAME (CHAR) CAREER at ST span=.. notes: ..` and ` belt=ID`.
SETTLE = re.compile(r"^\[settle\] (?P<name>.+?) \((?P<char>\d+)\) (?P<career>[a-z_]+) at (?P<station>\d+)\b(?P<rest>.*)$")
BELT = re.compile(r"\bbelt=(?P<id>\d+)\b")

GEODE = re.compile(r"\bopened \d+ geode")
PLEX = re.compile(r"\b(?:bought|sold) (?:\d+ )?(?:surplus )?PLEX\b")
SKIN = re.compile(r"\bwearing SKIN\b")
INJECT = re.compile(r"\binjected about \d+ points|\bsold (?:\d+ injector\(s\)|the injector\b)")
DEPLOY = re.compile(r"\bdeployed to \d+")
CARRY = re.compile(r"\bcarried [1-9]\d* units home\b")
DOCK = re.compile(r"CmdDock (?P<id>\d+)\b")

# Soak-D judges (specs/soakD.md section 3).
PROGRESS = re.compile(r"^\[soc\] (?P<bot>\S+) progress\b.*?\bcareer (?P<career>\S+).*?\bwallet (?P<wallet>-?[\d.]+) ledgers (?P<ledgers>\S+)")
SOC_FOUND_OK = re.compile(r"^\[soc\] (?P<bot>\S+) corp found (?P<what>.+): ok$")
SOC_FOUND_REFUSED = re.compile(r"^\[soc\] (?P<bot>\S+) corp found (?P<what>.+): refused (?P<key>\S+)$")
SOC_SUMMARY = re.compile(r"^\[soc\] bots \d+ .*?\bcorps (\d+)\b")
CHOSE = re.compile(r"\bchose (\d+) at (-?\d+) ISK/h")
INSTALLED = re.compile(r"\binstalled (\d+)\b")
NO_JOB_PAYS = re.compile(r"\bno job pays\b")
CAP_KEY = "MarketExceededOrderCount"
SETTLE_JOB = re.compile(r"\bjob (\d+): \d+ runs of blueprint \d+")
DAY_S1 = re.compile(r"\[econ\] day \d+ .*\bheld=\S+ .*\btx=\d+|\[econ\] day \d+ .*\btx=\d+ .*\bheld=\S+")
GEODE_DROP = re.compile(r"\[econ-mod\] geode: tier (\d+) for (\d+)")
GEODE_OPEN = re.compile(r"\[econ-mod\] geode: tier (\d+) opened by (\d+), (\d+) prizes?(?:: (.*))?$")
INJECTOR_DROP = re.compile(r"\[econ-mod\] injector: (large|small) for (\d+)")
HYPERCORE_DROP = re.compile(r"\[econ-mod\] hypercores: hypercores=(\d+) for (\d+)")
ACT_TIME = re.compile(r"^\S+ \+(\d+):([\d.]+) ")
PI_COLONIES = re.compile(r"\b(\d+) colonies\b")
PI_SOLD = re.compile(r"\bsold (\d+) into bids, listed (\d+), kept (\d+)")
STAY_WALLET = re.compile(r"\bstays: wallet (-?[\d.]+) below (-?[\d.]+)")
FILETIME_S = 10_000_000  # FILETIME ticks per second
MFG = "1"  # industry activity id of manufacturing

MIN_CAREERS = 3
MINER_FROM_DAY = 3

class Bands:
    """The [A] numbers of the soak-D judges (specs/soakD.md section 3). Tests pass their own."""

    cap_share = 0.10  # builder lines with a cap refusal, of all builder cycles
    ledger_ok = 0.25  # share of `ledgers -` at or under which the ledger works
    ledger_bad = 0.75  # ... and at or over which it does not
    installers = 3  # distinct builders that install online
    ref_median = 0.03  # margin-ref: median job margin at least
    ref_floor = -0.01  # margin-ref: any job below this fails
    real_fail = -0.10  # margin-real: aggregate at or under this fails, at scale 1 only
    real_fills = 10  # margin-real: product sale fills needed
    real_builders = 2  # ... and builders selling
    line = 1_849_800.0  # CORP_FEE + FOUND_RESERVE
    stall_secs = 1800.0  # a founder over the line with pledges this long, while acting
    reserve_tol = 1.0  # ISK
    pi_floor = 0.9  # P1+ sale VWAP over ref_price
    plex_wallet = 3_000_000.0  # tunables::SEED_PRICE
    idle_turns = 3  # econ turns idle with the item held
    carry_days = 2  # society days a deployed miner may take to carry
    day_secs = 86400.0  # one society day at scale 1

    def __init__(self, **kw):
        for k, v in kw.items():
            if not hasattr(Bands, k):
                raise TypeError(k)
            setattr(self, k, v)


NEED = {
    "dry": "[mine] NAME: belt ID dry, K of N belts left (eve-bot driver.rs after the brain marks a belt dry, #23 T3)",
    "spread": "[settle] ... belt=ID (eve-server econ_agents.rs Report::line, #23 T4)",
}


def spread_need(miners, belts):
    """[A] distinct belts wanted for `miners` offline miners among `belts` belts."""
    return min(miners, belts) / 2


def spread_ok(miners, distinct, belts):
    return distinct >= spread_need(miners, belts) and (miners < 2 or distinct >= 2)


class Run:
    """What the report reads from a run dir."""

    def __init__(self):
        self.acts = []  # (bot, kind, what, tail)
        self.dry = []  # (line index, bot, belt, left, n)
        self.warps = []  # (line index, bot, item id)
        self.docks = []  # (line index, bot, station id)
        self.settles = []  # (name, career, belt or None, station)
        self.tsv = None  # econ.tsv rows (dicts) or None
        self.ff = None  # tools/ff_verdict FfRun of server.log, or None (not a fast-forward run)
        # Soak-D judges: read always, used only when `judges` is set.
        self.judges = False
        self.act_t = []  # seconds of each act, parallel to acts (None when unparsed)
        self.progress = []  # (act index at the line, bot, career, wallet, ledgers)
        self.found_ok = []  # (bot, what)
        self.found_refused = []  # (bot, what, key)
        self.corps = None  # `corps N` of the last `[soc] bots` summary
        self.settle_notes = []  # (name, career, rest)
        self.geode_drops = []  # (tier, char)
        self.geode_opens = []  # (tier, char, prizes, roles list)
        self.injector_drops = []  # (size, char)
        self.hypercores = 0  # sum of the hypercores=N drops
        self.hypercore_drops = 0  # drop lines
        self.s1 = False  # a day line carried held= and tx=
        self.journal = []  # (id, time, ref, o1, o2, arg, amount, balance, desc)
        self.trades = []  # (id, time, type, qty, price, station, buyer, seller)
        self.jobs = []  # dicts: id activity bp owner runs cost product
        self.balances = {}  # character id -> ISK (world.dat)
        self.have_ledger = False
        self.have_world = False
        self.bps = None  # {bp: {"mat": [(type, qty)], "prod": (type, qty)}} or None
        self.prices = None  # {type: (ref_price, source)} or None
        self.summary_errors = None  # marketProxy.Place* usererror total from summary.tsv

    def feed_bots(self, lines):
        for i, line in enumerate(lines):
            line = line.rstrip("\n")
            m = DRY.search(line)
            if m:
                self.dry.append((i, m["bot"], int(m["belt"]), int(m["left"]), int(m["n"])))
                continue
            m = PROGRESS.match(line)
            if m:
                self.progress.append((len(self.acts), m["bot"], m["career"], float(m["wallet"]), m["ledgers"]))
                continue
            m = SOC_FOUND_OK.match(line)
            if m:
                self.found_ok.append((m["bot"], m["what"]))
                continue
            m = SOC_FOUND_REFUSED.match(line)
            if m:
                self.found_refused.append((m["bot"], m["what"], m["key"]))
                continue
            m = SOC_SUMMARY.match(line)
            if m:
                self.corps = int(m.group(1))
                continue
            m = ACT.match(line)
            if m:
                t = ACT_TIME.match(line)
                self.act_t.append(int(t.group(1)) * 60 + float(t.group(2)) if t else None)
                self.acts.append((m["bot"], m["kind"], m["what"], m["tail"]))
                if m["kind"] == "warp":
                    w = WARP.search(m["what"])
                    if w:
                        self.warps.append((i, m["bot"], int(w["id"])))
                d = DOCK.search(m["what"])
                if d:
                    self.docks.append((i, m["bot"], int(d["id"])))

    def feed_server(self, lines):
        for line in lines:
            line = line.rstrip("\n")
            m = SETTLE.match(line)
            if m:
                b = BELT.search(m["rest"])
                self.settles.append((m["name"], m["career"], int(b["id"]) if b else None, m["station"]))
                self.settle_notes.append((m["name"], m["career"], m["rest"]))
                continue
            if "[econ-mod]" in line:
                m = GEODE_OPEN.search(line)
                if m:
                    roles = [r.strip() for r in (m.group(4) or "").split(",") if r.strip()]
                    self.geode_opens.append((int(m.group(1)), int(m.group(2)), int(m.group(3)), roles))
                    continue
                m = GEODE_DROP.search(line)
                if m:
                    self.geode_drops.append((int(m.group(1)), int(m.group(2))))
                    continue
                m = INJECTOR_DROP.search(line)
                if m:
                    self.injector_drops.append((m.group(1), int(m.group(2))))
                    continue
                m = HYPERCORE_DROP.search(line)
                if m:
                    self.hypercores += int(m.group(1))
                    self.hypercore_drops += 1
                    continue
            if "[econ] day" in line and DAY_S1.search(line):
                self.s1 = True

    def feed_ledger(self, directory):
        """The journal and mkttrade rows of `directory/world.dat.ledger/*.tsv`; False when there are none."""
        files = sorted(glob.glob(os.path.join(directory, "world.dat.ledger", "*.tsv")))
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                self.feed_ledger_lines(f.read().splitlines())
        return bool(files)

    def feed_ledger_lines(self, lines):
        self.have_ledger = True
        for line in lines:
            p = line.rstrip("\n").split("\t")
            try:
                if p[0] == "journal" and len(p) >= 9:
                    self.journal.append((int(p[1]), int(p[2]), int(p[3]), int(p[4]), int(p[5]), int(p[6]),
                                         float(p[7]), float(p[8]), p[9] if len(p) > 9 else ""))
                elif p[0] == "mkttrade" and len(p) >= 9:
                    self.trades.append((int(p[1]), int(p[2]), int(p[3]), int(p[4]), float(p[5]), int(p[6]),
                                        int(p[7]), int(p[8])))
            except ValueError:
                pass
        self.journal.sort(key=lambda r: r[0])

    def feed_world(self, path):
        with open(path, encoding="utf-8", errors="replace") as f:
            self.feed_world_lines(f.read().splitlines())

    def feed_world_lines(self, lines):
        """`job` and `balance` rows of world.dat. Job fields, counted from 1 after the tag: 1 id, 2 activity,
        4 blueprint type, 9 owner, 18 runs, 21 cost, 24 product type."""
        self.have_world = True
        for line in lines:
            p = line.rstrip("\n").split("\t")
            try:
                if p[0] == "balance" and len(p) >= 3:
                    self.balances[int(p[1])] = float(p[2])
                elif p[0] == "job" and len(p) >= 25:
                    self.jobs.append({"id": int(p[1]), "activity": p[2], "bp": int(p[4]), "owner": int(p[9]),
                                      "runs": int(p[18]), "cost": float(p[21]), "product": int(p[24]),
                                      "start": int(p[15])})
            except ValueError:
                pass

    def feed_content(self, directory):
        """industry/blueprints.tsv (manufacturing only) and econ/prices.tsv of a content bundle."""
        self.bps, self.prices = {}, {}
        with open(os.path.join(directory, "industry", "blueprints.tsv"), encoding="utf-8") as f:
            for line in f:
                p = line.rstrip("\n").split("\t")
                if len(p) < 5 or p[0].startswith("#") or p[1] != MFG:
                    continue
                e = self.bps.setdefault(int(p[0]), {"mat": [], "prod": None})
                if p[2] == "material":
                    e["mat"].append((int(p[3]), int(p[4])))
                elif p[2] == "product":
                    e["prod"] = (int(p[3]), int(p[4]))
        with open(os.path.join(directory, "econ", "prices.tsv"), encoding="utf-8") as f:
            for line in f:
                p = line.rstrip("\n").split("\t")
                if len(p) >= 3 and not p[0].startswith("#"):
                    try:
                        self.prices[int(p[0])] = (float(p[1]), p[2])
                    except ValueError:
                        pass

    def feed_tsv(self, lines):
        header = None
        self.tsv = []
        for line in lines:
            parts = line.rstrip("\n").split("\t")
            if header is None:
                header = parts
            elif len(parts) == len(header) and parts[0] != header[0]:
                self.tsv.append(dict(zip(header, parts)))

    def belt_warps(self):
        """The warps that go to a belt: a warp the same bot follows with a dock
        on the same id, before its next warp, is the warp home (brain.rs go_home)."""
        out = []
        for k, (idx, bot, item) in enumerate(self.warps):
            nxt = next((w[0] for w in self.warps[k + 1:] if w[1] == bot), None)
            if any(d[1] == bot and d[2] == item and d[0] > idx and (nxt is None or d[0] < nxt)
                   for d in self.docks):
                continue
            out.append((idx, bot, item))
        return out

    def ok_acts(self, kind, pattern=None):
        return [a for a in self.acts if a[1] == kind and a[3] == "ok" and (pattern is None or pattern.search(a[2]))]


def _need(detail_line):
    return f"0 lines read; needs {detail_line}"

# ---- soak-D judges (specs/soakD.md section 3) -----------------------------------------------------


def _builder_cycles(run):
    """[(bot, whole line text)] of builder-cycle act lines."""
    return [(a[0], a[2] + " -> " + a[3]) for a in run.acts if a[1] == "market" and a[2].startswith("builder cycle:")]


def _wallet_of(row):
    """The wallet a journal row belongs to: the payer (owner 1) for a debit, the payee (owner 2) for a credit."""
    return row[3] if row[6] < 0 else row[4]


def j_pick(run, b):
    picks = [(bot, int(m.group(1)), int(m.group(2))) for bot, text in _builder_cycles(run) for m in CHOSE.finditer(text)]
    if not picks:
        return (NO_SIGNAL, _need("market  builder cycle: ... chose N at X ISK/h (eve-bot builder.rs, #158)"))
    bad = [p for p in picks if p[2] <= 0]
    return (FAIL if bad else PASS,
            f"{len(picks)} picks by {len({p[0] for p in picks})} builders, {len(bad)} at or under 0 ISK/h")


def j_order_cap(run, b):
    cycles = _builder_cycles(run)
    if not cycles:
        return (NO_SIGNAL, _need("market  builder cycle: ... lines (eve-bot builder.rs, #158)"))
    counts = [text.count(CAP_KEY) for _, text in cycles]
    many = sum(1 for c in counts if c > 1)
    some = sum(1 for c in counts if c >= 1)
    extra = "" if run.summary_errors is None else f"; summary.tsv marketProxy.Place* errors {run.summary_errors}"
    detail = f"{some} of {len(cycles)} builder cycles hit the cap, {many} more than once{extra}"
    if many:
        return FAIL, detail
    return (PASS if some <= b.cap_share * len(cycles) else INCONCLUSIVE), detail


def j_washes(run, b):
    if run.tsv is None:
        return NO_SIGNAL, "no econ.tsv (the server needs EVE_ECON_TSV)"
    rows = [r for r in run.tsv if r["scope"] == "all" and r["kind"] == "audit" and r["key"] == "washes"]
    if not rows:
        return NO_SIGNAL, "no `audit washes` row in econ.tsv"
    bad = sorted((int(r["day"]), r["n"]) for r in rows if float(r["n"]) > 0)
    return (FAIL if bad else PASS), (f"{len(rows)} days read" + (f"; washes on days {bad}" if bad else ", none"))


def j_ledger(run, b):
    seen, rows = {}, []
    for _pos, bot, career, _w, ledgers in run.progress:
        seen[bot] = seen.get(bot, 0) + 1
        if career == "builder" and seen[bot] > 2:
            rows.append(ledgers)
    if not rows:
        return NO_SIGNAL, _need("[soc] NAME progress .. career builder .. ledgers X, a third line per builder (#158)")
    dash = sum(1 for x in rows if x == "-")
    share = dash / len(rows)
    detail = f"{dash} of {len(rows)} builder progress lines say `ledgers -` ({share:.0%})"
    if share <= b.ledger_ok:
        return PASS, detail
    return (FAIL if share >= b.ledger_bad else INCONCLUSIVE), detail


def j_installs(run, b):
    cycles = _builder_cycles(run)
    online = {}
    nopay = set()
    for bot, text in cycles:
        online[bot] = online.get(bot, 0) + sum(int(x) for x in INSTALLED.findall(text))
        if NO_JOB_PAYS.search(text):
            nopay.add(bot)
    offline = [(n, SETTLE_JOB.search(r)) for n, c, r in run.settle_notes if c == "builder"]
    off_names = {n for n, m in offline if m}
    for n, c, r in run.settle_notes:
        if c == "builder" and NO_JOB_PAYS.search(r):
            nopay.add(n)
    if not cycles and not off_names and not nopay:
        return NO_SIGNAL, _need("builder cycle lines or [settle] .. builder notes (#158)")
    installers = {bot for bot, n in online.items() if n > 0}
    builders = set(online) | off_names | nopay
    unexplained = builders - installers - off_names - nopay
    detail = (f"{sum(online.values())} online installs by {len(installers)} builders, {len(off_names)} builders "
              f"installed offline, {len(nopay)} said no job pays, {len(unexplained)} of {len(builders)} builders neither")
    if len(installers) >= b.installers or (installers and not unexplained):
        return PASS, detail
    if not installers and not off_names and not nopay:
        return FAIL, detail
    return INCONCLUSIVE, detail


def _need_content(run):
    if run.bps is None or run.prices is None:
        return "--content DIR (out/content: industry/blueprints.tsv, econ/prices.tsv)"
    return None


def j_margin_ref(run, b):
    need = _need_content(run)
    if need:
        return NO_SIGNAL, "needs " + need
    if not run.have_world:
        return NO_SIGNAL, _need("world.dat job rows")
    offline = {int(m.group(1)) for _, c, r in run.settle_notes for m in [SETTLE_JOB.search(r)] if m and c == "builder"}
    margins, skipped = [], 0
    for j in run.jobs:
        if j["activity"] != MFG:
            continue
        bp = run.bps.get(j["bp"])
        if not bp or not bp["prod"] or bp["prod"][0] not in run.prices or any(t not in run.prices for t, _ in bp["mat"]):
            skipped += 1
            continue
        mats = sum(run.prices[t][0] * q * j["runs"] for t, q in bp["mat"])  # ME 0 [A]
        out = run.prices[bp["prod"][0]][0] * bp["prod"][1] * j["runs"]
        cost = mats + j["cost"]
        if cost <= 0:
            skipped += 1
            continue
        margins.append(((out - cost) / cost, j["id"] in offline))
    if not margins:
        return NO_SIGNAL, f"no manufacturing job priced ({skipped} skipped for a missing blueprint or price)"
    vals = [m for m, _ in margins]
    med = statistics.median(vals)
    detail = (f"{len(vals)} jobs ({sum(1 for _, o in margins if o)} offline), min {min(vals):+.1%}, median {med:+.1%}, "
              f"{skipped} skipped; ME 0 assumed")
    if min(vals) < b.ref_floor:
        return FAIL, detail
    if min(vals) >= 0 and med >= b.ref_median:
        return PASS, detail
    return INCONCLUSIVE, detail


def _vwap(trades):
    q = sum(t[3] for t in trades)
    return sum(t[3] * t[4] for t in trades) / q if q else None


def j_margin_real(run, b, scale):
    need = _need_content(run)
    if need:
        return NO_SIGNAL, "needs " + need
    if not (run.have_world and run.have_ledger):
        return NO_SIGNAL, _need("world.dat job rows and world.dat.ledger mkttrade rows")
    products = {}  # builder -> product -> [unit cost parts]
    for j in run.jobs:
        bp = run.bps.get(j["bp"])
        if j["activity"] != MFG or not bp or not bp["prod"]:
            continue
        units = bp["prod"][1] * j["runs"]
        day = j["start"] // (86400 * FILETIME_S)
        mat_cost, ok = 0.0, True
        for t, q in bp["mat"]:
            own = [x for x in run.trades if x[2] == t and x[6] == j["owner"]]
            same_day = [x for x in run.trades if x[2] == t and x[1] // (86400 * FILETIME_S) == day]
            price = _vwap(own) or _vwap(same_day) or _vwap([x for x in run.trades if x[2] == t])
            if price is None:
                ok = False
                break
            mat_cost += price * q * j["runs"]
        if ok and units:
            e = products.setdefault((j["owner"], bp["prod"][0]), [0.0, 0])
            e[0] += mat_cost + j["cost"]
            e[1] += units
    rev = cost = 0.0
    fills = 0
    sellers = set()
    for (owner, ptype), (c, units) in products.items():
        sales = [x for x in run.trades if x[7] == owner and x[2] == ptype]
        if not sales:
            continue
        qty = sum(x[3] for x in sales)
        rev += sum(x[3] * x[4] for x in sales)
        cost += qty * c / units
        fills += len(sales)
        sellers.add(owner)
    if fills < b.real_fills or len(sellers) < b.real_builders:
        return NO_SIGNAL, f"{fills} product sale fills by {len(sellers)} builders (need {b.real_fills} fills, {b.real_builders} builders)"
    agg = (rev - cost) / rev if rev else 0.0
    detail = f"aggregate {agg:+.1%} over {fills} fills by {len(sellers)} builders, revenue-weighted; time scale x{scale:g}"
    if agg >= 0:
        return PASS, detail
    if agg <= b.real_fail and scale <= 1:
        return FAIL, detail
    return INCONCLUSIVE, detail + (" (a negative at x>1 is inconclusive: products come faster than wall-paced demand)" if scale > 1 else "")


def founder_windows(run):
    """{founder: [rows]}: each pledge recipient's journal rows from its first pledge credit on."""
    founders = {}
    for r in run.journal:
        if r[2] == 10 and r[6] > 0 and "co-founding pledge" in r[8]:
            founders.setdefault(r[4], None)
    out = {}
    for f in founders:
        rows = [r for r in run.journal if _wallet_of(r) == f]
        first = next(i for i, r in enumerate(rows) if r[2] == 10 and r[6] > 0 and "co-founding pledge" in r[8])
        out[f] = rows[first:]
    return out


BOT_SIDE = {2, 42, 46, 54, 127}
OFFLINE_SIDE = {56, 57, 58, 59, 60, 135}


def j_reserve(run, b):
    if not run.have_ledger:
        return NO_SIGNAL, _need("world.dat.ledger journal rows")
    wins = founder_windows(run)
    if not wins:
        return NO_SIGNAL, "no co-founding pledge rows in the journal"
    bad, offline = [], []
    for f, rows in wins.items():
        reserved = 0.0
        for r in rows:
            if r[2] == 39:
                break
            if r[2] == 10 and "co-founding pledge" in r[8] and r[6] > 0:
                reserved += r[6]
            elif r[2] == 10 and r[6] < 0:
                reserved += r[6]  # a refund from the founder
            elif r[6] < 0 and r[7] < reserved - b.reserve_tol:
                (bad if r[2] in BOT_SIDE else offline if r[2] in OFFLINE_SIDE else []).append((f, r[0], r[2], r[6]))
    detail = f"{len(wins)} pool founders"
    if offline:
        detail += f"; {len(offline)} offline-capable debits under the reserve (reported, not judged; draft D4)"
    if bad:
        return FAIL, detail + "; bot-side debits under the reserve: " + ", ".join(f"{f} ref {ref} {a:.0f} (row {i})" for f, i, ref, a in bad[:5])
    return PASS, detail + ", no bot-side debit left a balance under its pledges"


def j_stall(run, b):
    if run.found_refused:
        keys = sorted({k for _, _, k in run.found_refused})
        return FAIL, f"{len(run.found_refused)} `corp found` refusals: {', '.join(keys[:5])}"
    wins = founder_windows(run) if run.have_ledger else {}
    founded_ref = {r[3] for r in run.journal if r[2] == 39}
    stalled, peak = [], 0.0
    for f, rows in wins.items():
        reserved, since = 0.0, None
        for r in rows:
            if r[2] == 39:
                break
            if r[2] == 10 and "co-founding pledge" in r[8] and r[6] > 0:
                reserved += r[6]
            elif r[2] == 10 and r[6] < 0:
                reserved += r[6]
            if r[2] in (2, 42, 46) and since is not None and (r[1] - since) / FILETIME_S >= b.stall_secs:
                stalled.append((f, r[0]))
                break
            peak = max(peak, r[7])
            since = (since if since is not None else r[1]) if (r[7] >= b.line and reserved > 0) else None
    if stalled:
        return FAIL, "stalled founders (over the line with pledges, acting, no registration fee): " + ", ".join(
            f"{f} (row {i})" for f, i in stalled[:5])
    if not run.found_ok and not run.have_ledger and run.corps is None:
        return NO_SIGNAL, _need("`[soc] NAME corp found ..` lines and world.dat.ledger (#155)")
    if run.found_ok and (run.corps or 0) >= 1:
        return PASS, f"{len(run.found_ok)} founded, last summary corps {run.corps}, no stall"
    if run.found_ok or founded_ref:
        return INCONCLUSIVE, f"{len(run.found_ok)} founded and {len(founded_ref)} registration fees, but the last summary shows corps {run.corps}"
    if peak < b.line:
        return INCONCLUSIVE, f"no pool reached the line ({b.line:.0f}): peak founder wallet {peak:.0f}"
    return INCONCLUSIVE, "a founder reached the line and nobody founded, but no stall proved either"


def _roles(run, word):
    return sum(1 for _t, _c, _n, roles in run.geode_opens for r in roles if word in r.lower() and not r.endswith("!"))


def j_geode(run, b):
    drops, opens = len(run.geode_drops), run.geode_opens
    if not drops and not opens:
        return NO_SUPPLY, "0 geode drops (`[econ-mod] geode: tier T for C`)"
    empty = [o for o in opens if o[2] < 1]
    lost = [o for o in opens if any(r.endswith("!") for r in o[3])]  # a role whose prize() returned nothing
    if empty or lost:
        return FAIL, (f"{len(empty)} of {len(opens)} geodes opened with 0 prizes, {len(lost)} with a prize that "
                      "came to nothing (`role!`; draft D1)")
    if drops >= 3 and not opens:
        return FAIL, f"{drops} geode drops, none opened"
    if opens and (drops < 2 or len(opens) >= drops / 2):
        return PASS, f"{len(opens)} opened with prizes, {drops} dropped"
    return INCONCLUSIVE, f"{len(opens)} opened of {drops} dropped"


def _idle_after_wallet(run, b):
    """Bots whose wallet reached the PLEX threshold and whose next idle_turns econ turns all said nothing to do."""
    out = set()
    for pos, bot, _c, wallet, _l in run.progress:
        if wallet < b.plex_wallet:
            continue
        nxt = [a for a in run.acts[pos:] if a[0] == bot and a[1] == "econ"][:b.idle_turns]
        if len(nxt) >= b.idle_turns and all("nothing to do" in a[2] for a in nxt):
            out.add(bot)
    return out


def j_item(run, b, key, pat, word):
    hits = run.ok_acts("econ", pat)
    prizes = _roles(run, word)
    supply = prizes
    note = f"{prizes} {word} prizes"
    if key == "plex":
        rich = [p for p in run.progress if p[3] >= b.plex_wallet]
        supply += len({p[1] for p in rich})
        note += f", {len({p[1] for p in rich})} bots reached a wallet of {b.plex_wallet:.0f}"
    if key == "injector":
        supply += len(run.injector_drops)
        note += f", {len(run.injector_drops)} injector drops, {run.hypercores} hypercores in {run.hypercore_drops} drops"
    if hits:
        return PASS, f"{len(hits)} lines by {len({h[0] for h in hits})} bots; {note}"
    if not supply:
        return NO_SUPPLY, note
    if key == "plex":
        idle = _idle_after_wallet(run, b)
        if idle:
            return FAIL, f"{note}; {len(idle)} bots over the threshold stayed idle {b.idle_turns} econ turns"
    return INCONCLUSIVE, f"{note}; no use seen (the holder's age and wallet are not in the logs)"


def j_haul(run, b):
    hits = run.ok_acts("haul")
    freight = [s for s in run.settles if s[1] == "freighter_hauler"]
    if hits:
        return PASS, f"{len(hits)} ok haul acts by {len({h[0] for h in hits})} bots"
    if not freight and not any(a[1] == "haul" for a in run.acts):
        return NO_SUPPLY, "no freighter_hauler career and no haul act (expected in a closed economy)"
    return INCONCLUSIVE, f"{len(freight)} freighter_hauler settle lines, no ok haul act"


def j_pi(run, b):
    pis = [a for a in run.acts if a[1] == "pi"]
    colonies = max([int(m.group(1)) for a in pis for m in [PI_COLONIES.search(a[2])] if m] or [0])
    if not pis and not [s for s in run.settles if s[1] == "pi_farmer"]:
        return NO_SIGNAL, _need("act `pi ...: N colonies ...` (eve-bot pi.rs)")
    if colonies < 1:
        return NO_SIGNAL, f"{len(pis)} pi acts, 0 colonies (draft D2: no command centre for sale at the home station)"
    sold = sum(int(m.group(1)) for a in pis for m in [PI_SOLD.search(a[2])] if m)
    kept = sum(int(m.group(3)) for a in pis for m in [PI_SOLD.search(a[2])] if m)
    if run.prices is None or not run.have_ledger:
        return NO_SIGNAL, f"{colonies} colonies, but judging the sales needs --content DIR and world.dat.ledger"
    pi_types = {t for t, (_r, src) in run.prices.items() if src == "pi"}
    sells = {}
    for x in run.trades:
        if x[2] in pi_types and not (1_000_000 <= x[7] <= 1_999_999):
            sells.setdefault(x[2], []).append(x)
    if not sells:
        if kept and not sold:
            return FAIL, f"{colonies} colonies; goods only kept ({kept}), none sold"
        return INCONCLUSIVE, f"{colonies} colonies; no P1+ sale by a bot yet"
    low = [(t, _vwap(v) / run.prices[t][0]) for t, v in sells.items() if run.prices[t][0] > 0 and _vwap(v) < b.pi_floor * run.prices[t][0]]
    if low:
        return FAIL, f"{colonies} colonies; sold under {b.pi_floor:.0%} of the reference: " + ", ".join(f"type {t} at {r:.0%}" for t, r in low[:5])
    return PASS, f"{colonies} colonies; {len(sells)} P1+ types sold at or over {b.pi_floor:.0%} of the reference"


def j_rare(run, b, scale):
    rare = [(i, a) for i, a in enumerate(run.acts) if a[1] == "rare"]
    if not rare:
        return {"deploy": (NO_SIGNAL, _need("rare act `deployed to STATION`")), "carry": (NO_SIGNAL, _need("rare act `carried N units home`"))}
    deploys = [(i, a) for i, a in rare if DEPLOY.search(a[2])]
    bad = [a for _, a in deploys if a[3] != "ok"]
    ok_dep = [(i, a) for i, a in deploys if a[3] == "ok"]
    wallets = [(float(m.group(1)), float(m.group(2))) for _, a in rare for m in [STAY_WALLET.search(a[2])] if m]
    other = len(rare) - len(deploys) - len(wallets)
    if bad:
        d = (FAIL, f"{len(bad)} deploy acts not ok: {bad[0][3][:80]}")
    elif ok_dep:
        d = (PASS, f"{len(ok_dep)} deploys ok by {len({a[0] for _, a in ok_dep})} bots")
    elif wallets and not deploys and other <= 0:
        d = (NO_SIGNAL, f"all {len(wallets)} rare acts are wallet stays: max wallet {max(w for w, _ in wallets):.0f}, gate {max(g for _, g in wallets):.0f} "
                        "(at x>1 sinks bias against; the unpriced-hull gate is open, draft D3)")
    else:
        d = (INCONCLUSIVE, f"{len(rare)} rare acts, none a deploy: {len(wallets)} wallet stays, {other} other stays")
    carries = [(i, a) for i, a in rare if CARRY.search(a[2]) and a[3] == "ok"]
    horizon = max([t for t in run.act_t if t is not None] or [0.0])
    late = []
    for i, a in ok_dep:
        t = run.act_t[i] if i < len(run.act_t) else None
        if t is None or any(c[1][0] == a[0] and c[0] > i for c in carries):
            continue
        if horizon - t > b.carry_days * b.day_secs / max(scale, 1.0):
            late.append(a[0])
    if carries:
        c = (PASS, f"{len(carries)} carries ok by {len({a[0] for _, a in carries})} bots")
    elif late:
        c = (FAIL, f"{len(late)} deployed bots never carried within {b.carry_days} society days: {', '.join(sorted(set(late))[:5])}")
    elif ok_dep:
        c = (INCONCLUSIVE, f"{len(ok_dep)} deploys ok, no carry yet but under {b.carry_days} society days left in the run")
    else:
        c = (NO_SIGNAL, "no deploy, so nothing to carry")
    return {"deploy": d, "carry": c}


def judged(run, closed, belts, scale, b):
    """Soak-D verdicts as {key: (name, verdict, detail)}: replacements for old keys and new lines."""
    out = {}

    def put(key, name, res):
        out[key] = (name, res[0], res[1])

    put("pick", "builders pick no losing job", j_pick(run, b))
    put("order-cap", "builders stop at the order cap", j_order_cap(run, b))
    put("washes", "no builder self-fills", j_washes(run, b))
    put("ledger", "builder earnings ledgers fill", j_ledger(run, b))
    put("installs", "builders install a paying job or say why not", j_installs(run, b))
    put("margin-ref", "job margin at reference prices", j_margin_ref(run, b))
    put("margin-real", "realised margin of sold products", j_margin_real(run, b, scale))
    put("reserve", "founder reserve held", j_reserve(run, b))
    put("stall", "a bot corp gets founded, no stall", j_stall(run, b))
    put("geode", "geodes opened with prizes", j_geode(run, b))
    put("plex", "PLEX bought or sold", j_item(run, b, "plex", PLEX, "plex"))
    put("skin", "a SKIN worn", j_item(run, b, "skin", SKIN, "skin"))
    put("injector", "injected or an injector sold", j_item(run, b, "injector", INJECT, "injector"))
    put("haul", "freighter haul", j_haul(run, b))
    put("pi", "planetary industry", j_pi(run, b))
    rare = j_rare(run, b, scale)
    put("deploy", "rare-ore miners deploy", rare["deploy"])
    put("carry", "rare-ore miners carry home", rare["carry"])
    return out



def verdicts(run, closed=False, belts=0, scale=1.0, bands=None):
    """[(key, behaviour, verdict, detail)] in report order."""
    out = []

    # --- dry belt, then another belt
    if not run.dry:
        out.append(("dry", "a dry belt, then another belt", NO_SIGNAL, _need(NEED["dry"])))
    else:
        moved = 0
        warps = run.belt_warps()
        for idx, bot, belt, _, _ in run.dry:
            nxt = next((w for w in warps if w[0] > idx and w[1] == bot), None)
            if nxt and nxt[2] != belt:
                moved += 1
        out.append(("dry", "a dry belt, then another belt", PASS if moved else FAIL,
                    f"{len(run.dry)} dry lines by {len({d[1] for d in run.dry})} miners, {moved} followed by a warp to another belt"))

    # --- offline spread
    miners = [(n, b, st) for n, c, b, st in run.settles if c == "miner"]
    with_belt = [(n, b, st) for n, b, st in miners if b is not None]
    if not with_belt:
        why = "no miner [settle] lines" if not miners else "miner [settle] lines carry no belt="
        out.append(("spread", "offline miners spread over belts", NO_SIGNAL, f"{why}; needs {NEED['spread']}"))
    else:
        # A belt id belongs to one system: judge each settling station's miners alone.
        by_station = {}
        for n, b, st in with_belt:
            by_station.setdefault(st, []).append((n, b))
        total = max([belts] + [d[4] for d in run.dry] + [len({b for _, b, _ in with_belt})])
        ok, parts = True, []
        for st in sorted(by_station):
            names = {n for n, _ in by_station[st]}
            seen = {b for _, b in by_station[st]}
            good = spread_ok(len(names), len(seen), total)
            ok = ok and good
            parts.append(f"{len(names)} miners on {len(seen)} distinct belts at {st} (need {spread_need(len(names), total):g})")
        out.append(("spread", "offline miners spread over belts", PASS if ok else FAIL,
                    f"of {total} belts: " + "; ".join(parts)))

    # --- rare ore
    for key, name, pat, line in (
        ("deploy", "rare-ore miners deploy", DEPLOY, "rare act `deployed to STATION`"),
        ("carry", "rare-ore miners carry home", CARRY, "rare act `carried N units home`"),
    ):
        hits = run.ok_acts("rare", pat)
        if hits:
            out.append((key, name, PASS, f"{len(hits)} lines by {len({h[0] for h in hits})} bots"))
        else:
            rare = [a for a in run.acts if a[1] == "rare"]
            out.append((key, name, FAIL if rare else NO_SIGNAL,
                        f"{len(rare)} rare acts, none ok with it" if rare else _need(line)))

    # --- miner career net from day 3
    if run.tsv is None:
        out.append(("miner", "miner net ISK/h > 0 from day 3", NO_SIGNAL, "no econ.tsv (the server needs EVE_ECON_TSV)"))
    else:
        days = {}
        for r in run.tsv:
            if r["scope"] == "all" and r["kind"] == "career" and r["key"] == "miner" and int(r["day"]) >= MINER_FROM_DAY:
                if float(r["hours"]) > 0:
                    days[int(r["day"])] = float(r["per_h"])
        if not days:
            out.append(("miner", "miner net ISK/h > 0 from day 3", NO_SIGNAL,
                        "no `career miner` row with hours > 0 from day 3 in econ.tsv"))
        else:
            bad = sorted(d for d, v in days.items() if v <= 0)
            out.append(("miner", "miner net ISK/h > 0 from day 3", FAIL if bad else PASS,
                        f"{len(days)} days read" + (f"; not positive on days {bad}" if bad else "")))

    # --- #2 items
    hits = run.ok_acts("haul")
    out.append(("haul", "freighter haul", PASS if hits else FAIL,
                f"{len(hits)} ok haul acts by {len({h[0] for h in hits})} bots"
                + ("" if hits else f", {sum(1 for a in run.acts if a[1] == 'haul')} haul acts in all")))
    for key, name, pat in (("geode", "geodes opened", GEODE), ("plex", "PLEX bought or sold", PLEX),
                           ("skin", "a SKIN worn", SKIN), ("injector", "injected or an injector sold", INJECT)):
        hits = run.ok_acts("econ", pat)
        econ = sum(1 for a in run.acts if a[1] == "econ")
        out.append((key, name, PASS if hits else FAIL,
                    f"{len(hits)} lines by {len({h[0] for h in hits})} bots, {econ} econ acts in all"))
    hits = run.ok_acts("pi")
    pi_settle = [s for s in run.settles if s[1] == "pi_farmer"]
    out.append(("pi", "planetary industry", PASS if hits or pi_settle else FAIL,
                f"{len(hits)} ok pi acts, {len(pi_settle)} pi_farmer settle lines"))
    if run.tsv is None:
        out.append(("build", "builders pay job time and fees", NO_SIGNAL, "no econ.tsv (the server needs EVE_ECON_TSV)"))
    else:
        jobs = sum(float(r["hours"]) for r in run.tsv if r["scope"] == "all" and r["kind"] == "build")
        fees = sum(float(r["isk"]) for r in run.tsv
                   if r["scope"] == "all" and r["kind"] == "isk_sink" and r["key"] == "industry fees")
        out.append(("build", "builders pay job time and fees", PASS if jobs > 0 or fees > 0 else FAIL,
                    f"{jobs:.0f} job-hours, {fees:.0f} ISK industry fees"))
    careers = sorted({c for _, c, _, _ in run.settles})
    if not run.settles:
        out.append(("careers", f">= {MIN_CAREERS} careers settled", NO_SIGNAL,
                    _need("[settle] NAME (CHAR) CAREER at STATION ... (server log; agents settle on login after an offline span)")))
    else:
        out.append(("careers", f">= {MIN_CAREERS} careers settled", PASS if len(careers) >= MIN_CAREERS else FAIL,
                    f"{len(run.settles)} lines, careers {','.join(careers)}"))
    if closed:
        gm = [a for a in run.acts if a[1] == "gm-setup"]
        if not run.acts:
            out.append(("gm-setup", "closed economy: no GM setup", NO_SIGNAL, _need("act lines in bots.log")))
        else:
            out.append(("gm-setup", "closed economy: no GM setup", FAIL if gm else PASS,
                        f"{len(gm)} gm-setup acts by {len({a[0] for a in gm})} bots in {len(run.acts)} acts"))
    if run.judges:
        new = judged(run, closed, belts, scale, bands or Bands())
        have = {k for k, *_ in out}
        out = [(k, new[k][0], new[k][1], new[k][2]) if k in new else (k, name, v, d) for k, name, v, d in out]
        out += [(k, *new[k]) for k in new if k not in have]
    if run.ff is not None:
        ff = _ff()
        status, why = ff.gate(run.ff)
        if status == ff.NA:
            out = [(k, name, NA, f"{why}; was {v}: {detail}") for k, name, v, detail in out]
    return out


def _ff():
    """tools/ff_verdict.py, loaded by path: `python3 -I` leaves tools/ off sys.path."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("ff_verdict", os.path.join(os.path.dirname(os.path.abspath(__file__)), "ff_verdict.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def report(run, closed=False, belts=0, scale=1.0, bands=None):
    return [f"{v:<14} {name}: {detail}" for _, name, v, detail in verdicts(run, closed, belts, scale, bands)]


def status(run, closed=False, belts=0, strict=False, scale=1.0, bands=None):
    bad = {FAIL, NO_SIGNAL, NO_SUPPLY, INCONCLUSIVE} if strict else {FAIL}
    return 1 if any(v in bad for _, _, v, _ in verdicts(run, closed, belts, scale, bands)) else 0


def load(directory, content=None, judges=False):
    """A Run from a run dir; OSError when bots.log is missing. `content` is a content bundle dir (--content)."""
    run = Run()
    with open(os.path.join(directory, "bots.log"), encoding="utf-8", errors="replace") as f:
        run.feed_bots(f.read().splitlines())
    for name, feed in (("server.log", run.feed_server), ("econ.tsv", run.feed_tsv)):
        try:
            with open(os.path.join(directory, name), encoding="utf-8", errors="replace") as f:
                feed(f.read().splitlines())
        except OSError:
            pass
    run.ff = _ff().parse_paths([os.path.join(directory, "server.log")])
    run.judges = judges or run.s1
    if run.judges:
        run.feed_ledger(directory)
        try:
            run.feed_world(os.path.join(directory, "world.dat"))
        except OSError:
            pass
        try:
            with open(os.path.join(directory, "summary.tsv"), encoding="utf-8", errors="replace") as f:
                run.summary_errors = sum(int(p[2]) for p in (l.rstrip("\n").split("\t") for l in f)
                                         if len(p) >= 3 and p[0] == "usererror" and p[1].startswith("marketProxy.Place")
                                         and p[2].isdigit())
        except OSError:
            pass
        if content:
            try:
                run.feed_content(content)
            except OSError:
                pass
    return run


def main(argv=None):
    ap = argparse.ArgumentParser(description="PASS/FAIL per bot economy behaviour from a soak run dir")
    ap.add_argument("dir", nargs="?", help="default: server/target/soak")
    ap.add_argument("--closed", action="store_true", help="the run was --closed: judge that no bot used gm-setup")
    ap.add_argument("--belts", type=int, default=0, help="belts in the miners' system, for the spread line")
    ap.add_argument("--strict", action="store_true", help="FAIL-NO-SIGNAL, NO-SUPPLY and INCONCLUSIVE also exit 1")
    ap.add_argument("--judges", action="store_true", help="the soak-D judges (on by itself when server.log has the S1 day-line fields)")
    ap.add_argument("--content", metavar="DIR", help="content bundle (out/content) for margin-ref, margin-real and pi")
    ap.add_argument("--scale", type=float, default=1.0, help="the soak's --time-scale (margin-real FAIL, carry deadline)")
    a = ap.parse_args(argv)
    here = os.path.dirname(os.path.abspath(__file__))
    directory = a.dir or os.path.join(here, "..", "server", "target", "soak")
    try:
        run = load(directory, a.content, a.judges)
    except OSError as e:
        print(f"bot-econ-report: {e}", file=_sys.stderr)
        return 2
    if run.ff is not None:
        print(_ff().banner(run.ff))
    print("\n".join(report(run, a.closed, a.belts, a.scale)))
    return status(run, a.closed, a.belts, a.strict, a.scale)


if __name__ == "__main__":
    _sys.exit(main())
