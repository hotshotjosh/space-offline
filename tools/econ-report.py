#!/usr/bin/env python3
"""Turn a soak's economy TSV into tables (X-ECONREPORT).

The server appends one block of rows per rollover to the file named by the
environment variable EVE_ECON_TSV (`server/crates/eve-server/src/econ_report.rs`),
and `/econ tsv` prints the same rows.  Columns:
day scope kind key n hours isk isk2 per_h.

    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv [--scope all|<regionID>] [--log server.log]

`--log FILE` first prints the income multipliers the run used (#141), read
from the server's startup line `[econ] scale bounty=.. mission=.. npc_buy=..
insurance=.. isk=..`.

Prints, for the scope (default `all`): net ISK/h by career per day, net ISK/h
by ship tier per day (does income grow as bots fly better ships and train?),
and ISK sources and sinks by class per day.  `career` and `tier` rows: n is
bots, hours their active game hours, isk the wallet delta, isk2 the earnings,
per_h = isk / hours.  Only the standard library.

FE-MEASURE adds the tape (R-FLUIDECON section 4); `--tape` prints it and
`--shock DAY` reads one basket across a price shock:

    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv --tape [--basket armour]
    tools/analysis.sh python3 tools/econ-report.py out/soak/econ.tsv --shock 5 --basket armour

Tape kinds (same nine columns; "x" is price over P_ref times 1000):
  price   key basket:type  n fills, isk window VWAP, isk2 P_ref, per_h index x
  spot    key basket:type  isk best ask, isk2 best bid, per_h spot x
  pbasket key basket       n types, isk history x, isk2 spot x, per_h units
  pindex  key basket       isk trading-index x
  book    key basket       n bid units, isk ask units, per_h pressure x 1000
  stock   key basket       n held by bots, isk on sale, per_h days of volume x 100
  build   key basket       n jobs, hours job-hours, per_h share x 1000
  level   key all          isk price level x, isk2 bot ISK, per_h bots / humans x 1000
  churn   key class        n destroyed, isk minted, per_h destroyed / minted x 100
  audit   key detector     n findings (outlier, pairs, round_trips, pinned, washes)

`--identity RUN_DIR` (#156, soak D) checks the money identity of a run dir (world.dat, world.dat.ledger/,
server.log): wallets + corp wallets + open-bid escrow = sum of faucets - sum of sinks over every journal row,
by the server's own rule (econ_ledger.rs `classify`). The ref table (`REFS`) and `NPC_ORDER_BASE` are parsed
from the Rust source at run time (`--src DIR` names the eve-server src dir), never copied. When the day lines
carry the S1 fields `held=` and `tx=`, each day is checked too, and the server's net against the net of the
journal ids in (tx of the day before, tx of the day]; a day line's `late=` (#209: rows booked to an
earlier closed day after its line printed) counts in its net. A per-day PASS needs at least 3 such day lines
(spec section 4), else the verdict is inconclusive. Corp journal System rows (a tax skim) must each have the
mirror row the server writes in the character journal. Exit 1 on FAIL, 3 when inconclusive, 0 on PASS.

`--judge [--warmup K]` (#94) judges a run from the tape, one PASS/FAIL/N/A line each, and
exits 1 on any FAIL, 3 when every check is N/A (N/A is never a pass).  The first K days (default 2) are the warm-up;
its last day with a trade is the reference.  After it: (a) every basket's history index
(pbasket isk) stays within 0.5x..2.0x of the reference; (c) the level row (isk) drifts
less than 25%; (e) each day's net (isk_src - isk_sink, scope all) over bot ISK (level
isk2) is within -2%..+4%; (f) no audit row has n > 0; demand: every day whose basket book
pressure (book per_h) is at least 400 has that basket's spot index (pbasket isk2) up at
least 15% the next day.  A basket with no trade (history index 0) is thin: N/A.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import glob
import os
import re
import sys


def read(path):
    f = sys.stdin if path == "-" else open(path)
    rows = []
    with f:
        header = f.readline().rstrip("\n").split("\t")
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == len(header) and parts[0] != header[0]:
                rows.append(dict(zip(header, parts)))
    return rows


SCALE_PREFIX = "[econ] scale "


def scale_line(path):
    """The income multipliers a run used (#141): the last `[econ] scale k=v ...`
    line the server logged at startup, else a note that the log has none."""
    last = None
    with open(path, errors="replace") as f:
        for line in f:
            i = line.find(SCALE_PREFIX)
            if i >= 0:
                last = line[i + len(SCALE_PREFIX):].strip()
    if last is None:
        return "income multipliers: not in the log (a server before #141 logs none)"
    return "income multipliers: " + last

def _ff():
    """tools/ff_verdict.py, loaded by path: `python3 -I` leaves tools/ off sys.path."""
    import importlib.util
    import os

    spec = importlib.util.spec_from_file_location("ff_verdict", os.path.join(os.path.dirname(os.path.abspath(__file__)), "ff_verdict.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def table(title, days, keys, cell):
    print(title)
    if not keys:
        print("  (no rows)\n")
        return
    width = max(10, *(len(k) for k in keys)) + 1
    print("  " + "day".rjust(5) + "".join(k.rjust(width) for k in keys))
    for d in days:
        print("  " + str(d).rjust(5) + "".join(cell(d, k).rjust(width) for k in keys))
    print()


def series(rows, kind, key, col):
    """{day: float(col)} of one kind/key; a day with no row is missing."""
    return {int(r["day"]): float(r[col]) for r in rows if r["kind"] == kind and r["key"] == key}


def shock(rows, basket, day):
    """Pre/post ratios of a basket's history index across `day`.

    Returns a dict: pre (mean of the days before), post (the day after the
    shock .. the last), peak, overshoot (peak / settled - 1), days_to_half
    (days until the series covers half the way from pre to its settled
    level), half_life (days for the overshoot to halve after the peak), or
    None entries when the series is too short."""
    ser = series(rows, "pbasket", basket, "isk")
    ser = {d: v for d, v in ser.items() if v > 0}
    before = [v for d, v in sorted(ser.items()) if d < day]
    after = [(d, v) for d, v in sorted(ser.items()) if d >= day]
    out = {"pre": None, "settled": None, "peak": None, "overshoot": None,
           "days_to_half": None, "half_life": None}
    if not before or not after:
        return out
    pre = sum(before) / len(before)
    settled = sum(v for _, v in after[-3:]) / len(after[-3:])
    out["pre"], out["settled"] = pre, settled
    sign = 1.0 if settled >= pre else -1.0
    peak_day, peak = max(after, key=lambda dv: sign * dv[1])
    out["peak"] = peak
    if settled != pre:
        out["overshoot"] = sign * (peak - settled) / abs(settled - pre)
        half = pre + (settled - pre) / 2.0
        for d, v in after:
            if sign * (v - half) >= 0:
                out["days_to_half"] = d - day
                break
    gap = abs(peak - settled)
    if gap > 0:
        for d, v in after:
            if d > peak_day and abs(v - settled) <= gap / 2.0:
                out["half_life"] = d - peak_day
                break
    return out


def tape(rows, scope, only):
    days = sorted({int(r["day"]) for r in rows})
    baskets = sorted({r["key"] for r in rows if r["kind"] == "pbasket"})
    if only:
        baskets = [b for b in baskets if b == only]
    for kind, col, title, fmt in (
        ("pbasket", "isk", "basket price index, history (1000 = P_ref)", "%.0f"),
        ("pbasket", "isk2", "basket price index, spot", "%.0f"),
        ("pindex", "isk", "basket trading index (mktidx)", "%.0f"),
        ("book", "per_h", "book pressure (bids - asks) / (bids + asks) x 1000", "%.0f"),
        ("stock", "per_h", "bot stock, days of volume x 100", "%.0f"),
        ("build", "per_h", "builder share of job-hours x 1000", "%.0f"),
        ("churn", "per_h", "destroyed / minted x 100", "%.0f"),
    ):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel} if kind in ("churn",) else
                      {k for _, k in sel if not only or k == only})
        table("%s, scope %s" % (title, scope), days, keys,
              lambda d, k, sel=sel, col=col, fmt=fmt: fmt % float(sel[(d, k)][col]) if (d, k) in sel else "-")
    lv = {(int(r["day"])): r for r in rows if r["kind"] == "level"}
    if lv:
        print("price level x, bot ISK, bots/humans x 1000")
        for d in days:
            if d in lv:
                r = lv[d]
                print("  day %s level %s bots %s ratio %s" % (d, r["isk"], r["isk2"], r["per_h"]))
        print()
    au = [r for r in rows if r["kind"] == "audit" and float(r["n"]) > 0]
    print("audit findings" + ("" if au else ": none"))
    for r in au:
        print("  day %s %s %s" % (r["day"], r["key"], r["n"]))


PASS, FAIL, NA = "PASS", "FAIL", "N/A"
# [A] thresholds from the soak plan (#94); falsified by a healthy run that trips one.
BASKET_BAND = (0.5, 2.0)
LEVEL_DRIFT = 0.25
NET_BAND = (-0.02, 0.04)
PRESSURE = 400.0
SPOT_RISE = 0.15


def _by_day(rows, kind, key, col):
    return {d: v for d, v in series(rows, kind, key, col).items()}


def judge(rows, warmup=2):
    """[(key, name, verdict, detail)] for the #94 checks over `rows` (scope already chosen)."""
    days = sorted({int(r["day"]) for r in rows})
    warm, after = days[:warmup], days[warmup:]
    out = []
    if not after:
        for key, name in (("baskets", "basket indices stay in band"), ("level", "price level drift"),
                          ("net", "daily net over bot ISK"), ("audit", "no audit findings"),
                          ("demand", "spot moves with demand")):
            out.append((key, name, NA, "%d days read, warm-up %d: no day after it" % (len(days), warmup)))
        return out
    # (a) baskets
    names = sorted({r["key"] for r in rows if r["kind"] == "pbasket"})
    bad, ok, thin = [], 0, []
    for b in names:
        hist = _by_day(rows, "pbasket", b, "isk")
        ref = next((hist[d] for d in reversed(warm) if hist.get(d, 0) > 0), None)
        if ref is None:
            thin.append(b)
            continue
        checked = 0
        for d in after:
            v = hist.get(d)
            if v is None or v <= 0:
                continue
            checked += 1
            ratio = v / ref
            if not BASKET_BAND[0] <= ratio <= BASKET_BAND[1]:
                bad.append("%s day %d x%.2f" % (b, d, ratio))
        if checked:
            ok += 1
        else:
            thin.append(b)
    if bad:
        out.append(("baskets", "basket indices stay in band", FAIL, "; ".join(bad[:8])))
    elif ok:
        out.append(("baskets", "basket indices stay in band", PASS,
                    "%d baskets within %.1fx..%.1fx of the warm-up day%s" % (
                        ok, BASKET_BAND[0], BASKET_BAND[1], ", thin: " + ",".join(thin) if thin else "")))
    else:
        out.append(("baskets", "basket indices stay in band", NA,
                    "no basket traded in the warm-up" + (": " + ",".join(thin) if thin else "")))
    # (c) level
    lvl = _by_day(rows, "level", "all", "isk")
    ref = next((lvl[d] for d in reversed(warm) if lvl.get(d, 0) > 0), None)
    seen = [(d, lvl[d]) for d in after if lvl.get(d, 0) > 0]
    if ref is None or not seen:
        out.append(("level", "price level drift", NA, "no level row with a value in warm-up and after"))
    else:
        d, v = max(seen, key=lambda dv: abs(dv[1] / ref - 1))
        drift = abs(v / ref - 1)
        out.append(("level", "price level drift", PASS if drift < LEVEL_DRIFT else FAIL,
                    "max %.1f%% on day %d (limit %.0f%%)" % (drift * 100, d, LEVEL_DRIFT * 100)))
    # (e) daily net
    src = {}
    for r in rows:
        if r["kind"] in ("isk_src", "isk_sink"):
            sign = 1.0 if r["kind"] == "isk_src" else -1.0
            src[int(r["day"])] = src.get(int(r["day"]), 0.0) + sign * float(r["isk"])
    bots = _by_day(rows, "level", "all", "isk2")
    nets = [(d, src[d] / bots[d]) for d in after if d in src and bots.get(d, 0) > 0]
    if not nets:
        out.append(("net", "daily net over bot ISK", NA, "no day after warm-up with flow rows and bot ISK"))
    else:
        offs = ["day %d %+.1f%%" % (d, n * 100) for d, n in nets if not NET_BAND[0] <= n <= NET_BAND[1]]
        out.append(("net", "daily net over bot ISK", FAIL if offs else PASS,
                    ("outside %+.0f%%..%+.0f%%: %s" % (NET_BAND[0] * 100, NET_BAND[1] * 100, "; ".join(offs[:8])))
                    if offs else "%d days within %+.0f%%..%+.0f%%" % (len(nets), NET_BAND[0] * 100, NET_BAND[1] * 100)))
    # (f) audit
    au = [r for r in rows if r["kind"] == "audit"]
    hits = ["day %s %s n=%s" % (r["day"], r["key"], r["n"]) for r in au if float(r["n"]) > 0]
    if not au:
        out.append(("audit", "no audit findings", NA, "no audit rows"))
    else:
        out.append(("audit", "no audit findings", FAIL if hits else PASS,
                    "; ".join(hits[:8]) if hits else "%d audit rows, none above 0" % len(au)))
    # demand
    events, misses = 0, []
    for b in names:
        book = _by_day(rows, "book", b, "per_h")
        spot = _by_day(rows, "pbasket", b, "isk2")
        hist = _by_day(rows, "pbasket", b, "isk")
        for d in sorted(book):
            if book[d] < PRESSURE or hist.get(d, 0) <= 0 or spot.get(d, 0) <= 0 or spot.get(d + 1, 0) <= 0:
                continue
            events += 1
            rise = spot[d + 1] / spot[d] - 1
            if rise < SPOT_RISE:
                misses.append("%s day %d spot %+.0f%%" % (b, d, rise * 100))
    if not events:
        out.append(("demand", "spot moves with demand", NA, "no day with pressure >= %.0f on a traded basket" % PRESSURE))
    else:
        out.append(("demand", "spot moves with demand", FAIL if misses else PASS,
                    ("%d of %d pressure days did not rise %.0f%%: %s" % (len(misses), events, SPOT_RISE * 100, "; ".join(misses[:8])))
                    if misses else "%d pressure days, each up >= %.0f%% the next day" % (events, SPOT_RISE * 100)))
    return out


# ---- the money identity (#156, J2) ------------------------------------------------

DEFAULT_SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "server", "crates", "eve-server", "src")
REFS_BLOCK = re.compile(r"pub const REFS:[^=]*=\s*&\[(.*?)\n\];", re.S)
REFS_ROW = re.compile(r"\(\s*([0-9_]+)\s*,\s*Flow::(\w+)\s*,")
CONST = re.compile(r"^\s*(?:pub\s+)?const\s+(\w+):\s*[\w]+\s*=\s*(.+?);\s*$", re.M)
SAFE_EXPR = re.compile(r"^[0-9\s()+*<-]*$")
NPC_CORP = re.compile(r"pub fn is_npc_corporation\([^)]*\)\s*->\s*bool\s*\{\s*\(([0-9_]+)\.\.=([0-9_]+)\)")
SYSTEM, MARKET, PLAYER, HELD = "System", "Market", "Player", "Held"
FAUCET, SINK, MOVED, UNCLASSIFIED = "faucet", "sink", "moved", "unclassified"


class Bands:
    """The [A] bands of the identity judge (docs: specs/soakD.md section 3, #156). Tests pass their own."""

    abs_isk = 1000.0  # whole-run residual allowed, ISK ...
    run_frac = 0.0001  # ... or this share of the money, whichever is larger
    min_days = 3  # S1 day lines a per-day PASS needs (specs/soakD.md section 4)
    day_frac = 0.001  # per-day residual, share of M
    fail_frac = 0.01  # any residual over this share is a FAIL
    net_isk = 1.0  # server net against the journal's, per day

    def __init__(self, **kw):
        for k, v in kw.items():
            if not hasattr(Bands, k):
                raise TypeError(k)
            setattr(self, k, v)


def _int(text):
    return int(text.replace("_", ""))


def _const_value(name, consts, seen=()):
    """The integer value of a Rust `const NAME: T = EXPR;` made of numbers, `<<`, `+`, `*`, `as T` and other consts."""
    if name in seen or name not in consts:
        raise ValueError("cannot evaluate const " + name)
    expr = re.sub(r"\bas\s+\w+", "", consts[name])
    expr = re.sub(r"(?<=\d)_(?=\d)", "", expr)
    for ident in sorted(set(re.findall(r"[A-Za-z]\w*", expr)), key=len, reverse=True):
        expr = re.sub(r"\b%s\b" % ident, str(_const_value(ident, consts, seen + (name,))), expr)
    if not SAFE_EXPR.match(expr):
        raise ValueError("const %s is not a plain integer expression: %s" % (name, consts[name]))
    return int(eval(expr, {"__builtins__": {}}, {}))  # only digits and + * << ( ) pass SAFE_EXPR


def refs_from_source(src_dir=None):
    """({ref: flow name}, (NPC_ORDER_BASE, NPC_ORDER_END, DEV_ORDER_BASE), (npc corp lo, hi)) parsed from the Rust
    source: the server's `is_npc` is `[BASE, END) or >= DEV_ORDER_BASE`."""
    src_dir = src_dir or DEFAULT_SRC
    with open(os.path.join(src_dir, "econ_ledger.rs"), encoding="utf-8") as f:
        text = f.read()
    block = REFS_BLOCK.search(text)
    if not block:
        raise ValueError("no `pub const REFS` table in econ_ledger.rs")
    refs = {_int(a): b for a, b in REFS_ROW.findall(block.group(1))}
    if not refs:
        raise ValueError("REFS table has no rows")
    with open(os.path.join(src_dir, "econ_exchange.rs"), encoding="utf-8") as f:
        consts = dict(CONST.findall(f.read()))
    npc = tuple(_const_value(n, consts) for n in ("NPC_ORDER_BASE", "NPC_ORDER_END", "DEV_ORDER_BASE"))
    corp = (1_000_000, 1_999_999)
    try:
        with open(os.path.join(src_dir, "war.rs"), encoding="utf-8") as f:
            m = NPC_CORP.search(f.read())
        if m:
            corp = (_int(m.group(1)), _int(m.group(2)))
    except OSError:
        pass
    return refs, npc, corp


def journal(ledger_dir):
    """[(id, time, ref, owner1, owner2, arg, amount)] from every `*.tsv` of the ledger dir, by id."""
    rows = []
    for path in sorted(glob.glob(os.path.join(ledger_dir, "*.tsv"))):
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                p = line.rstrip("\n").split("\t")
                if p[0] == "journal" and len(p) >= 8:
                    try:
                        rows.append((int(p[1]), int(p[2]), int(p[3]), int(p[4]), int(p[5]), int(p[6]), float(p[7])))
                    except ValueError:
                        pass
    rows.sort()
    return rows


def classify(row, rules):
    """FAUCET, SINK, MOVED or UNCLASSIFIED for one journal row, by the server's rule."""
    refs, (npc_base, npc_end, dev_base), (lo, hi) = rules
    _, _, ref, o1, o2, arg, amount = row
    flow = refs.get(ref)
    by_sign = FAUCET if amount >= 0 else SINK
    if flow is None:
        return UNCLASSIFIED
    if flow == SYSTEM:
        return by_sign
    if flow == MARKET and (npc_base <= arg < npc_end or arg >= dev_base or lo <= o1 <= hi or lo <= o2 <= hi):
        return by_sign
    return MOVED


def world_money(world_path):
    """(wallets, corp wallets, bid escrow) in ISK from world.dat: `balance` rows, `corpwal` deltas in cents,
    and the `mktorder` rows with the bid flag (field 12) set, summing the escrow (field 16)."""
    wallets = corp = escrow = 0.0
    with open(world_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            try:
                if p[0] == "balance" and len(p) >= 3:
                    wallets += float(p[2])
                elif p[0] == "delta" and len(p) >= 4 and p[1] == "corpwal":
                    corp += float(p[-1]) / 100.0
                elif p[0] == "mktorder" and len(p) >= 16 and p[11] == "1":
                    escrow += float(p[15])
            except ValueError:
                pass
    return wallets, corp, escrow


def corp_journal(world_path):
    """[(date, ref, owner1, owner2, ISK amount, corp)] of the `corpwjrn` deltas: each is a corp division's journal row
    (`delta corpwjrn 2 CORP SEQ KEY DATE REF O1 O2 REFERENCE CENTS BALANCE d:DESC`)."""
    out = []
    with open(world_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if not line.startswith("delta\tcorpwjrn\t"):
                continue
            p = line.rstrip("\n").split("\t")
            try:
                v = p[3 + int(p[2]):]
                out.append((int(v[1]), int(v[2]), int(v[3]), int(v[4]), int(v[6]) / 100.0, int(p[3])))
            except (ValueError, IndexError):
                pass
    return out


def unmirrored(corp_rows, rows, rules):
    """The corp journal rows of a System flow (a tax skim, a fee) with no character-journal row of the same date,
    ref and amount. The server mirrors them (`service/corp_wallet.rs` `post`, `mirror: true`), so the money identity
    already counts them once; a row missing here would be money the identity cannot see."""
    seen = {}
    for r in rows:
        k = (r[1], r[2], round(abs(r[6]), 2))
        seen[k] = seen.get(k, 0) + 1
    missing = []
    for date, ref, _o1, _o2, amount, corp in corp_rows:
        if rules[0].get(ref) != SYSTEM:
            continue
        k = (date, ref, round(abs(amount), 2))
        if seen.get(k, 0) > 0:
            seen[k] -= 1
        else:
            missing.append((corp, ref, amount))
    return missing


DAY = re.compile(r"\[econ\] day (\d+) (.*)$")


def day_lines(server_log):
    """[(day, {key: value text})] from the server's `[econ] day N k=v ...` lines, in order."""
    out = []
    try:
        with open(server_log, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = DAY.search(line)
                if m:
                    out.append((int(m.group(1)), dict(re.findall(r"(\w+)=(\S+)", m.group(2)))))
    except OSError:
        pass
    return out


def identity(rows, rules, money, days=(), corp_rows=()):
    """The numbers of the identity: whole-run residual and, when the day lines carry `held=` and `tx=`, the days.
    `money` is (wallets, corp, escrow). Returns a dict."""
    fa = si = 0.0
    unclassified = 0
    for r in rows:
        c = classify(r, rules)
        if c == FAUCET:
            fa += r[6]
        elif c == SINK:
            si -= r[6]
        elif c == UNCLASSIFIED:
            unclassified += 1
    total = sum(money)
    out = {"faucet": fa, "sink": si, "money": total, "residual": total - (fa - si), "unclassified": unclassified,
           "days": [], "s1": False, "unclassified_lines": 0, "unmirrored": unmirrored(corp_rows, rows, rules)}
    out["s1"] = bool(days) and all("held" in k and "tx" in k for _, k in days)
    prev_m, prev_tx = 0.0, 0
    for d, k in days:
        out["unclassified_lines"] += int(float(k.get("unclassified", 0)))
        if "held" not in k or "tx" not in k:
            continue
        m = float(k.get("M_h", 0)) + float(k.get("M_bots", 0))
        now = m + float(k["held"])
        tx = int(k["tx"])
        # `late=` (#209): rows booked to an earlier closed day after its line printed. They are in this day's
        # tx range and this day's money growth, so they count in its net here.
        net = float(k.get("net", 0)) + float(k.get("late", 0))
        mine = sum(r[6] for r in rows if prev_tx < r[0] <= tx and classify(r, rules) in (FAUCET, SINK))
        out["days"].append({"day": d, "m": m, "residual": now - prev_m - net, "net": net, "journal_net": mine})
        prev_m, prev_tx = now, tx
    return out


def judge_identity(res, bands=None):
    """(verdict, [lines]) for the #156 identity. PASS, FAIL or N/A (inconclusive)."""
    b = bands or Bands()
    lines = []
    allow = max(b.abs_isk, b.run_frac * res["money"])
    fail_at = b.fail_frac * res["money"]
    run_res = abs(res["residual"])
    lines.append("run: money %.0f, faucets %.0f, sinks %.0f, residual %+.2f ISK (allowed %.0f), %d unclassified journal rows"
                 % (res["money"], res["faucet"], res["sink"], res["residual"], allow, res["unclassified"]))
    verdict = PASS
    if run_res > fail_at and run_res > b.abs_isk:
        verdict = FAIL
    elif run_res > allow:
        verdict = NA
    if res["unclassified"]:
        verdict = FAIL if verdict == FAIL else NA
    if res["unmirrored"]:
        verdict = FAIL if verdict == FAIL else NA
        lines.append("corp journal: %d System-flow rows (tax skims, fees) have no character-journal mirror, first: corp %d ref %d %.2f"
                     % ((len(res["unmirrored"]),) + res["unmirrored"][0]))
    if not res["s1"] or len(res["days"]) < b.min_days:
        if res["days"] or res["unclassified_lines"]:
            lines.append("days: %d S1 day lines (need %d, all with held= and tx=): per-day checks cannot give a PASS"
                         % (len(res["days"]), b.min_days))
    if not res["days"]:
        lines.append("days: no `[econ] day` line with the S1 fields held= and tx=: check 1 alone, which proves the "
                     "rule and not the server")
        return (FAIL if verdict == FAIL else NA), lines
    for dd in res["days"]:
        pct = 100.0 * dd["residual"] / dd["m"] if dd["m"] else 0.0
        diff = dd["net"] - dd["journal_net"]
        bad = []
        if dd["m"] and abs(dd["residual"]) > b.fail_frac * dd["m"]:
            verdict = FAIL
            bad.append("residual over the fail line")
        elif dd["m"] and abs(dd["residual"]) > b.day_frac * dd["m"]:
            verdict = verdict if verdict == FAIL else NA
            bad.append("residual over the day band")
        if abs(diff) > b.net_isk:
            verdict = verdict if verdict == FAIL else NA
            bad.append("server net differs from the journal's")
        lines.append("day %d: M %.0f, residual %+.2f (%+.4f%%), server net %.2f, journal net %.2f%s"
                     % (dd["day"], dd["m"], dd["residual"], pct, dd["net"], dd["journal_net"],
                        "; " + ", ".join(bad) if bad else ""))
    if res["unclassified_lines"]:
        verdict = verdict if verdict == FAIL else NA
        lines.append("day lines: unclassified= sums to %d (need 0)" % res["unclassified_lines"])
    if verdict == PASS and (not res["s1"] or len(res["days"]) < b.min_days):
        verdict = NA
    return verdict, lines


def run_identity(run_dir, src_dir=None, bands=None):
    rules = refs_from_source(src_dir)
    rows = journal(os.path.join(run_dir, "world.dat.ledger"))
    money = world_money(os.path.join(run_dir, "world.dat"))
    res = identity(rows, rules, money, day_lines(os.path.join(run_dir, "server.log")),
                   corp_journal(os.path.join(run_dir, "world.dat")))
    verdict, lines = judge_identity(res, bands)
    lines.insert(0, "money: wallets %.2f, corp wallets %.2f, bid escrow %.2f; %d journal rows" % (*money, len(rows)))
    return verdict, lines


NES_KINDS = (("nes_geode", "geode opens"), ("nes_skin", "SKINs applied"),
             ("nes_inject", "injections (n injectors, isk = points)"),
             ("nes_extract", "extractions (n, isk = points)"), ("nes_crate", "other crate opens"))


def nes(rows):
    """Print the per-day NES tables (#2): the nes_* kinds by career and the PLEX rows.

    Returns True when there was something to print.  nes_* rows: key career, n
    count, isk units.  plex rows: held:<career> n PLEX, avg isk price, in:<src>
    and out:<key> n PLEX moved since the last day line.
    """
    printed = False
    for kind, title in NES_KINDS:
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        if not sel:
            continue
        printed = True
        keys = sorted({k for _, k in sel})
        days = sorted({d for d, _ in sel})
        table("%s by career, per day (count/units)" % title, days, keys,
              lambda d, k, sel=sel: "%s/%s" % (sel[(d, k)]["n"], sel[(d, k)]["isk"]) if (d, k) in sel else "-")
    plex = [r for r in rows if r["kind"] == "plex"]
    first = sorted({int(r["day"]) for r in plex if r["key"] == "since_start"})
    plex = [r for r in plex if r["key"] != "since_start"]
    for d in first:
        printed = True
        print("day %d is cumulative: the first fold counts everything the counters saw since start\n" % d)
    if plex:
        printed = True
        sel = {(int(r["day"]), r["key"]): r for r in plex}
        keys = sorted({k for _, k in sel})
        days = sorted({d for d, _ in sel})

        def cell(d, k):
            r = sel.get((d, k))
            if r is None:
                return "-"
            return "%.0f" % float(r["isk"]) if k == "avg" else r["n"]

        table("PLEX per day (held:career = PLEX held, avg = ISK, in/out = PLEX moved)", days, keys, cell)
    return printed


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("tsv", nargs="?", help="the TSV, or - for stdin")
    ap.add_argument("--identity", metavar="RUN_DIR", help="check the money identity of a soak run dir (#156)")
    ap.add_argument("--src", metavar="DIR", help="eve-server src dir for --identity (default: this tree's)")
    ap.add_argument("--scope", default="all")
    ap.add_argument("--tape", action="store_true", help="print the price tape tables")
    ap.add_argument("--basket", help="only this basket in --tape / --shock")
    ap.add_argument("--shock", type=int, metavar="DAY", help="pre/post ratios across a shock on DAY")
    ap.add_argument("--judge", action="store_true", help="PASS/FAIL the #94 checks; exit 1 on a FAIL")
    ap.add_argument("--warmup", type=int, default=2, metavar="K", help="warm-up days for --judge (default 2)")
    ap.add_argument("--nes", action="store_true", help="print only the PLEX / crate / SKIN / injector tables (#2); they also follow the default tables when rows exist")
    ap.add_argument("--log", metavar="FILE", help="the server log; print the income multipliers its `[econ] scale` line names")
    a = ap.parse_args()
    if a.identity:
        try:
            verdict, lines = run_identity(a.identity, a.src)
        except (OSError, ValueError) as e:
            print("econ-report --identity: %s" % e, file=sys.stderr)
            sys.exit(2)
        print("%-5s money identity (#156)" % verdict)
        for line in lines:
            print("  " + line)
        sys.exit({PASS: 0, FAIL: 1}.get(verdict, 3))
    if not a.tsv:
        ap.error("the TSV is required (or --identity RUN_DIR)")
    ff = _ff()
    ffrun = ff.parse_paths([a.log])
    if a.log:
        print(scale_line(a.log))
    if ffrun:
        print(ff.banner(ffrun))
    rows = [r for r in read(a.tsv) if r["scope"] == a.scope]
    days = sorted({int(r["day"]) for r in rows})
    if a.judge:
        res = judge(rows, a.warmup)
        if ff.gate(ffrun)[0] == ff.NA:
            res = [(k, name, NA, f"{ff.gate(ffrun)[1]}; was {v}: {detail}") for k, name, v, detail in res]
        for _, name, v, detail in res:
            print("%-5s %s: %s" % (v, name, detail))
        if any(v == FAIL for _, _, v, _ in res):
            sys.exit(1)
        # Nothing judged is not a pass: exit 3 when no check reached PASS or FAIL.
        sys.exit(0 if any(v == PASS for _, _, v, _ in res) else 3)
    if a.shock is not None:
        baskets = [a.basket] if a.basket else sorted({r["key"] for r in rows if r["kind"] == "pbasket"})
        for b in baskets:
            s = shock(rows, b, a.shock)
            f = lambda x, p="%.0f": "-" if x is None else p % x
            print("basket %s, shock on day %d: pre %s settled %s peak %s overshoot %s days_to_half %s half_life %s" % (
                b, a.shock, f(s["pre"]), f(s["settled"]), f(s["peak"]),
                f(s["overshoot"], "%.2f"), f(s["days_to_half"]), f(s["half_life"])))
        return
    if a.tape:
        tape(rows, a.scope, a.basket)
        return
    if a.nes:
        if not nes(rows):
            print("no nes_* or plex rows")
        return
    for kind, title in (("career", "net ISK/h by career"), ("tier", "net ISK/h by ship tier"),
                        ("band", "net ISK/h by security band"), ("career_band", "net ISK/h by career and band")):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel})

        def cell(d, k, sel=sel):
            r = sel.get((d, k))
            return "-" if r is None or float(r["hours"]) == 0 else "%.0f (%s)" % (float(r["per_h"]), r["n"])

        table("%s, scope %s (bots in brackets)" % (title, a.scope), days, keys, cell)
    for kind, title in (("isk_src", "ISK sources"), ("isk_sink", "ISK sinks")):
        sel = {(int(r["day"]), r["key"]): r for r in rows if r["kind"] == kind}
        keys = sorted({k for _, k in sel})
        table("%s by class, scope %s" % (title, a.scope), days, keys,
              lambda d, k, sel=sel: "%.0f" % float(sel[(d, k)]["isk"]) if (d, k) in sel else "-")
    goods = [r for r in rows if r["kind"] == "goods"]
    if goods:
        print("goods minted/destroyed per class, per day")
        for r in goods:
            print("  day %s %-10s minted %s destroyed %s" % (r["day"], r["key"], r["isk"], r["isk2"]))
    nes(rows)


if __name__ == "__main__":
    main()
