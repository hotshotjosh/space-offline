#!/usr/bin/env python3
"""Tests for tools/econ-report.py (FE-MEASURE): invented rows only.

    tools/analysis.sh python3 tools/test_econ_report.py
"""
import importlib.util
import io
import os
import contextlib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("econ_report", os.path.join(HERE, "econ-report.py"))
er = importlib.util.module_from_spec(spec)
spec.loader.exec_module(er)

HEADER = "day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h"


def row(day, kind, key, n=0, hours=24, isk=0, isk2=0, per_h=0, scope="all"):
    return dict(zip(HEADER.split("\t"), map(str, (day, scope, kind, key, n, hours, isk, isk2, per_h))))


def basket(day, idx):
    return row(day, "pbasket", "armour", n=3, isk=idx, isk2=idx, per_h=10)


class Shock(unittest.TestCase):
    def test_a_step_with_overshoot(self):
        rows = [basket(d, v) for d, v in
                [(1, 1000), (2, 1000), (3, 1000), (4, 1400), (5, 1300), (6, 1200), (7, 1100), (8, 1100), (9, 1100)]]
        s = er.shock(rows, "armour", 4)
        self.assertEqual(s["pre"], 1000)
        self.assertEqual(s["settled"], 1100)
        self.assertEqual(s["peak"], 1400)
        self.assertAlmostEqual(s["overshoot"], 3.0)  # (1400 - 1100) / (1100 - 1000)
        self.assertEqual(s["days_to_half"], 0)
        self.assertEqual(s["half_life"], 2)  # 1200 is within half the peak gap of 1100

    def test_too_short_is_none_not_a_crash(self):
        s = er.shock([basket(1, 1000)], "armour", 1)
        self.assertIsNone(s["pre"])
        self.assertIsNone(er.shock([], "armour", 1)["peak"])

    def test_a_fall_mirrors_a_rise(self):
        rows = [basket(d, v) for d, v in [(1, 1000), (2, 1000), (3, 600), (4, 700), (5, 800), (6, 800), (7, 800)]]
        s = er.shock(rows, "armour", 3)
        self.assertEqual((s["pre"], s["settled"], s["peak"]), (1000, 800, 600))
        self.assertAlmostEqual(s["overshoot"], 1.0)


class Tape(unittest.TestCase):
    def test_the_tables_print_and_a_missing_day_is_a_dash(self):
        rows = [basket(1, 1000), basket(2, 1100), row(2, "book", "armour", n=5, isk=7, per_h=-167),
                row(1, "level", "all", isk=1000, isk2=900, per_h=500),
                row(2, "audit", "pinned", n=1)]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            er.tape(rows, "all", None)
        text = out.getvalue()
        self.assertIn("basket price index, history", text)
        self.assertIn("1100", text)
        self.assertIn("-167", text)
        self.assertIn("day 2 pinned 1", text)
        self.assertIn("day 1 level 1000 bots 900", text)

    def test_the_old_tables_still_read_the_old_rows(self):
        path = os.path.join(os.environ.get("TMPDIR", "/tmp"), "econ-report-test.tsv")
        with open(path, "w") as f:
            f.write(HEADER + "\n" + "\t".join(["1", "all", "isk_src", "bounties", "4", "0", "400", "0", "0"]) + "\n")
        try:
            self.assertEqual(er.read(path)[0]["key"], "bounties")
        finally:
            os.remove(path)


def healthy(days=6):
    """Invented rows: one basket flat at 1000, level flat, balanced flows."""
    rows = []
    for d in range(1, days + 1):
        rows += [row(d, "pbasket", "armour", n=3, isk=1000, isk2=1000, per_h=10),
                 row(d, "book", "armour", per_h=0),
                 row(d, "level", "all", isk=1000, isk2=1000000),
                 row(d, "isk_src", "bounties", isk=500000), row(d, "isk_sink", "taxes", isk=480000),
                 row(d, "audit", "pinned", n=0)]
    return rows


def verdicts(rows, warmup=2):
    return {k: (v, d) for k, _, v, d in er.judge(rows, warmup)}


class Judge(unittest.TestCase):
    def test_a_healthy_run_passes_every_check(self):
        v = verdicts(healthy())
        self.assertEqual({k: x[0] for k, x in v.items()},
                         {"baskets": "PASS", "level": "PASS", "net": "PASS", "audit": "PASS", "demand": "N/A"})

    def test_a_basket_out_of_band_fails(self):
        out = int(1000 * er.BASKET_BAND[1] * 1.5)
        rows = healthy() + [row(7, "pbasket", "armour", n=3, isk=out, isk2=out, per_h=10)]
        self.assertEqual(verdicts(rows)["baskets"][0], "FAIL")
        self.assertIn("armour day 7", verdicts(rows)["baskets"][1])

    def test_a_thin_basket_is_na_never_pass(self):
        rows = [r for r in healthy() if r["kind"] != "pbasket"]
        rows += [row(d, "pbasket", "ammo", n=1, isk=0, isk2=0) for d in range(1, 7)]
        self.assertEqual(verdicts(rows)["baskets"][0], "N/A")

    def test_a_basket_unpriced_after_warmup_is_thin_not_pass(self):
        rows = [r for r in healthy() if r["kind"] != "pbasket"]
        rows += [row(d, "pbasket", "ammo", n=1, isk=1000 if d <= 2 else 0, isk2=0) for d in range(1, 7)]
        self.assertEqual(verdicts(rows)["baskets"][0], "N/A")

    def test_level_drift(self):
        rows = healthy() + [row(7, "level", "all", isk=int(1000 * (1 + er.LEVEL_DRIFT * 1.2)), isk2=1000000)]
        self.assertEqual(verdicts(rows)["level"][0], "FAIL")

    def test_net_outside_band_fails_either_way(self):
        rows = [r for r in healthy() if not (r["day"] == "5" and r["kind"] == "isk_sink")]
        self.assertEqual(verdicts(rows)["net"][0], "FAIL")  # +50% of bot ISK
        low = int((0.02 - er.NET_BAND[0] + 0.01) * 1000000)  # healthy() nets +2% of bot ISK
        rows = healthy() + [row(7, "isk_sink", "taxes", isk=low), row(7, "level", "all", isk=1000, isk2=1000000)]
        self.assertEqual(verdicts(rows)["net"][0], "FAIL")  # -10% of bot ISK
        self.assertEqual(verdicts(healthy())["net"][0], "PASS")

    def test_net_is_na_without_bot_isk(self):
        rows = [r for r in healthy() if r["kind"] != "level"]
        self.assertEqual(verdicts(rows)["net"][0], "N/A")

    def test_an_audit_finding_fails(self):
        rows = healthy() + [row(4, "audit", "washes", n=2)]
        self.assertEqual(verdicts(rows)["audit"][0], "FAIL")

    def test_demand_pass_and_fail(self):
        hot = er.PRESSURE + 50
        up = healthy() + [row(4, "book", "armour", per_h=hot)]
        up = [r for r in up if not (r["day"] == "5" and r["kind"] == "pbasket")]
        up.append(row(5, "pbasket", "armour", n=3, isk=1000, isk2=int(1000 * (1 + er.SPOT_RISE * 2)), per_h=10))
        self.assertEqual(verdicts(up)["demand"][0], "PASS")
        flat = healthy() + [row(4, "book", "armour", per_h=hot)]
        self.assertEqual(verdicts(flat)["demand"][0], "FAIL")
        low = healthy() + [row(4, "book", "armour", per_h=er.PRESSURE - 1)]
        self.assertEqual(verdicts(low)["demand"][0], "N/A")

    def test_too_short_is_na_and_main_exit_codes(self):
        self.assertEqual({v for v, _ in verdicts(healthy(2)).values()}, {"N/A"})
        import tempfile
        for rows, want in ((healthy(), 0), (healthy() + [row(4, "audit", "washes", n=1)], 1),
                           (healthy(2), 3)):
            with tempfile.TemporaryDirectory() as dir_:
                path = os.path.join(dir_, "econ.tsv")
                with open(path, "w") as f:
                    f.write(HEADER + "\n")
                    for r in rows:
                        f.write("\t".join(r[k] for k in HEADER.split("\t")) + "\n")
                import sys
                old = sys.argv
                sys.argv = ["econ-report.py", path, "--judge"]
                try:
                    with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as cm:
                        er.main()
                finally:
                    sys.argv = old
                self.assertEqual(cm.exception.code, want)


class ScaleLine(unittest.TestCase):
    def log(self, text):
        import tempfile
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        path = os.path.join(d.name, "server.log")
        with open(path, "w") as f:
            f.write(text)
        return path

    def test_the_startup_line_is_printed(self):
        path = self.log("noise\n[econ] scale bounty=2.5 mission=1 npc_buy=1 insurance=1 isk=2\nmore\n")
        self.assertEqual(er.scale_line(path),
                         "income multipliers: bounty=2.5 mission=1 npc_buy=1 insurance=1 isk=2")

    def test_the_last_line_wins_and_a_log_without_one_says_so(self):
        path = self.log("[econ] scale isk=1\n[econ] scale isk=3\n")
        self.assertEqual(er.scale_line(path), "income multipliers: isk=3")
        self.assertIn("not in the log", er.scale_line(self.log("nothing\n")))


FF_CLEAN = "[ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0"
FF_STALLS = "[ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=3"
FF_DOG = "[ff] watchdog exit real_s=60.0 virt_s=3601.25"


class FastForward(unittest.TestCase):
    def judge_cli(self, log_text):
        import sys
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            tsv = os.path.join(d, "econ.tsv")
            with open(tsv, "w") as f:
                f.write(HEADER + "\n")
                for r in [basket(day, 1000) for day in range(1, 8)]:
                    f.write("\t".join(r[k] for k in HEADER.split("\t")) + "\n")
            argv = ["econ-report.py", tsv, "--judge"]
            if log_text is not None:
                log = os.path.join(d, "server.log")
                with open(log, "w") as f:
                    f.write(log_text)
                argv += ["--log", log]
            old = sys.argv
            sys.argv = argv
            out = io.StringIO()
            try:
                with contextlib.redirect_stdout(out), self.assertRaises(SystemExit) as cm:
                    er.main()
            finally:
                sys.argv = old
            return cm.exception.code, out.getvalue()

    def test_a_log_without_ff_reports_as_before(self):
        rows = [basket(day, 1000) for day in range(1, 8)]
        want = "".join("%-5s %s: %s\n" % (v, name, detail) for _, name, v, detail in er.judge(rows, 2))
        code, text = self.judge_cli("[econ] scale isk=1\n")
        self.assertEqual(text, "income multipliers: isk=1\n" + want)
        self.assertNotIn("ff:", text)
        self.assertIn(code, (0, 1, 3))

    def test_a_watchdog_exit_makes_every_check_n_a_and_exits_3(self):
        code, text = self.judge_cli(FF_CLEAN + "\n" + FF_DOG + "\n")
        self.assertEqual(code, 3)
        self.assertIn("ff: mode=lockstep", text)
        for line in text.splitlines():
            if line.startswith(("PASS", "FAIL")):
                self.fail(line)
        self.assertIn("N/A", text)

    def test_a_clean_ff_line_keeps_the_old_verdicts(self):
        base_code, base = self.judge_cli("")
        code, text = self.judge_cli(FF_CLEAN + "\n")
        self.assertEqual(code, base_code)
        self.assertEqual([l for l in text.splitlines() if not l.startswith("ff:")], base.splitlines())


# ---- #156 money identity (invented source, ids and rows) -----------------------------------------

import subprocess
import sys
import tempfile

MINI_LEDGER = """
pub const REFS: &[(i64, Flow, &str)] = &[
    (1, Flow::Player, "player trading"),
    (2, Flow::Market, "market transaction"),
    (
        3,
        Flow::System,
        "grant",
    ),
    (42, Flow::Held, "escrow"),
    (10_004, Flow::System, "start"),
];
"""
MINI_EXCHANGE = ("pub const NPC_ORDER_BASE: i64 = 5_000;\npub const MAX_STATIONS: usize = 1 << 2;\n"
                 "pub const STATION_STRIDE: i64 = 1 << 3;\n"
                 "pub const NPC_ORDER_END: i64 = NPC_ORDER_BASE + MAX_STATIONS as i64 * STATION_STRIDE;\n"
                 "pub const DEV_ORDER_BASE: i64 = 1 << 20;\n")
MINI_WAR = "pub fn is_npc_corporation(id: i64) -> bool {\n    (100..=199).contains(&id)\n}\n"
BOT_A, BOT_B, NPC = 9001, 9002, 150


def jrow(i, ref, o1, o2, arg, amount):
    return "journal\t%d\t%d\t%d\t%d\t%d\t%d\t%s\t0\t" % (i, 1000 + i, ref, o1, o2, arg, amount)


def make_run(tmp, journal_rows, balances, orders=(), corp_cents=(), day_lines=(), src=True, corp_jrn=()):
    srcdir = os.path.join(tmp, "src")
    os.makedirs(srcdir, exist_ok=True)
    for name, text in (("econ_ledger.rs", MINI_LEDGER), ("econ_exchange.rs", MINI_EXCHANGE), ("war.rs", MINI_WAR)):
        with open(os.path.join(srcdir, name), "w") as f:
            f.write(text)
    run = os.path.join(tmp, "run")
    os.makedirs(os.path.join(run, "world.dat.ledger"), exist_ok=True)
    with open(os.path.join(run, "world.dat.ledger", "2000-01-01.tsv"), "w") as f:
        f.write("\n".join(journal_rows) + "\n")
    with open(os.path.join(run, "world.dat"), "w") as f:
        for who, v in balances.items():
            f.write("balance\t%d\t%s\n" % (who, v))
        for oid, bid, escrow in orders:
            f.write("mktorder\t%d\t%d\t7\t1\t2\t3\t10\t5\t5\t1\t%d\t0\t9\t-1\t%s\t0\n" % (oid, BOT_A, bid, escrow))
        for corp, key, cents in corp_cents:
            f.write("delta\tcorpwal\t2\t%d\t%d\t%d\n" % (corp, key, cents))
        for line in corp_jrn:
            f.write(line + "\n")
    with open(os.path.join(run, "server.log"), "w") as f:
        f.write("\n".join(day_lines) + "\n")
    return run, srcdir


# A grant to A, A sells to the NPC (ref 2, arg an NPC order), A trades with B (ref 2, player order, two rows),
# B places a bid with escrow 500 (ref 42 is moved into the order).
BASE_ROWS = [
    jrow(1, 3, 1, BOT_A, 0, 10000),
    jrow(2, 3, 1, BOT_B, 0, 10000),
    jrow(3, 2, BOT_A, NPC, 5001, 300),
    jrow(4, 2, BOT_A, BOT_B, 70, -200),
    jrow(5, 2, BOT_B, BOT_A, 70, 200),
    jrow(6, 42, BOT_B, 1, 71, -500),
]
# Wallets: A 10000+300-200 = 10100; B 10000+200-500 = 9700; escrow 500.
BASE_BAL = {BOT_A: 10100, BOT_B: 9700}
BASE_ORDERS = [(71, 1, 500)]


class Identity(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.addCleanup(self.td.cleanup)

    def run_id(self, **kw):
        run, src = make_run(self.td.name, kw.pop("rows", BASE_ROWS), kw.pop("balances", BASE_BAL),
                            kw.pop("orders", BASE_ORDERS), **kw)
        return er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, day_frac=0.001, fail_frac=0.01, min_days=1))

    def test_refs_and_the_npc_base_are_parsed_from_the_source_not_copied(self):
        run, src = make_run(self.td.name, BASE_ROWS, BASE_BAL)
        refs, base, corp = er.refs_from_source(src)
        self.assertEqual(refs, {1: "Player", 2: "Market", 3: "System", 42: "Held", 10004: "System"})
        self.assertEqual((base, corp), ((5000, 5032, 1 << 20), (100, 199)))

    def test_the_real_source_parses(self):
        refs, base, corp = er.refs_from_source()
        self.assertIn(2, refs)
        self.assertEqual(refs[2], "Market")
        self.assertTrue(0 < base[0] < base[1] < base[2])
        self.assertEqual(sorted(refs), list(refs))

    def test_a_run_with_an_npc_sale_a_trade_a_bid_and_a_grant_closes_to_zero(self):
        verdict, lines = self.run_id()
        res = er.identity(er.journal(os.path.join(self.td.name, "run", "world.dat.ledger")),
                          er.refs_from_source(os.path.join(self.td.name, "src")),
                          er.world_money(os.path.join(self.td.name, "run", "world.dat")))
        self.assertAlmostEqual(res["residual"], 0.0)
        self.assertEqual(res["unclassified"], 0)
        self.assertAlmostEqual(res["faucet"], 20300)
        self.assertIn("residual +0.00", "\n".join(lines))
        self.assertEqual(verdict, er.NA)  # no S1 fields: the rule is proved, the server is not

    def test_the_same_sale_read_as_moved_money_fails(self):
        wrong = [r for r in BASE_ROWS if not r.startswith("journal\t3\t")] + [jrow(3, 1, BOT_A, BOT_B, 5001, 300)]
        verdict, lines = self.run_id(rows=wrong)
        self.assertEqual(verdict, er.FAIL, lines)

    def test_a_party_in_the_npc_corporation_range_is_a_sink_even_with_a_player_order(self):
        rules = er.refs_from_source(os.path.join(make_run(self.td.name, BASE_ROWS, BASE_BAL)[1]))
        row = (1, 1, 2, BOT_A, NPC, 70, -50.0)
        self.assertEqual(er.classify(row, rules), er.SINK)
        self.assertEqual(er.classify((1, 1, 2, BOT_A, BOT_B, 70, -50.0), rules), er.MOVED)
        self.assertEqual(er.classify((1, 1, 99, BOT_A, BOT_B, 70, -50.0), rules), er.UNCLASSIFIED)

    def test_the_npc_order_rule_is_the_servers_band_or_dev_range(self):
        rules = er.refs_from_source(make_run(self.td.name, BASE_ROWS, BASE_BAL)[1])
        for arg, want in ((4999, er.MOVED), (5000, er.SINK), (5031, er.SINK), (5032, er.MOVED), (9000, er.MOVED),
                          (1 << 20, er.SINK), ((1 << 20) + 5, er.SINK)):
            self.assertEqual(er.classify((1, 1, 2, BOT_A, BOT_B, arg, -5.0), rules), want, arg)

    def test_a_taxed_bounty_closes_because_the_skim_is_mirrored_in_the_character_journal(self):
        # gross 1000: 900 to the bot (ref 3 stands in for a System ref), 100 skimmed into the corp wallet; the
        # server's `post(mirror: true)` journals the skim as +100 under the character and the corp.
        corp = 5
        net, mirror = jrow(1, 3, 1, BOT_A, 0, 900), jrow(2, 3, BOT_A, corp, 0, 100)
        cj = "delta\tcorpwjrn\t2\t5\t000000000001\t1000\t1002\t3\t%d\t%d\t0\t10000\t10000\td:" % (BOT_A, corp)
        run, src = make_run(self.td.name, [net, mirror], {BOT_A: 900}, corp_cents=[(corp, 1000, 10000)], corp_jrn=[cj])
        rules = er.refs_from_source(src)
        rows = er.journal(os.path.join(run, "world.dat.ledger"))
        res = er.identity(rows, rules, er.world_money(os.path.join(run, "world.dat")), (),
                          er.corp_journal(os.path.join(run, "world.dat")))
        self.assertAlmostEqual(res["residual"], 0.0)
        self.assertEqual(res["unmirrored"], [])

    def test_a_skim_without_its_mirror_row_is_flagged_and_never_passes(self):
        corp = 5
        net = jrow(1, 3, 1, BOT_A, 0, 900)
        cj = "delta\tcorpwjrn\t2\t5\t000000000001\t1000\t1002\t3\t%d\t%d\t0\t10000\t10000\td:" % (BOT_A, corp)
        run, src = make_run(self.td.name, [net], {BOT_A: 900}, corp_cents=[(corp, 1000, 10000)], corp_jrn=[cj])
        v, lines = er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, fail_frac=0.01, min_days=1))
        self.assertEqual(v, er.FAIL, lines)
        self.assertTrue(any("no character-journal mirror" in l for l in lines))
        # the same skim with a loose fail line is inconclusive, not a pass
        v2, _ = er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, fail_frac=0.5, min_days=1))
        self.assertEqual(v2, er.NA)

    def test_a_per_day_pass_needs_the_minimum_of_s1_day_lines(self):
        day = ("[econ] day %d faucet=%s sink=0 net=%s M_h=0 M_bots=%s unclassified=0 held=500 tx=%d")
        d1 = day % (1, 20300, 20300, 19800, 7)
        rows = BASE_ROWS + [jrow(8, 3, 1, BOT_A, 0, 100), jrow(9, 3, 1, BOT_A, 0, 100)]
        d2 = day % (2, 100, 100, 19900, 8)
        d3 = day % (3, 100, 100, 20000, 9)
        run, src = make_run(self.td.name, rows, {BOT_A: 10300, BOT_B: 9700}, BASE_ORDERS, day_lines=[d1, d2, d3])
        v, lines = er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, fail_frac=0.01, min_days=3))
        self.assertEqual(v, er.PASS, lines)
        run, src = make_run(self.td.name, rows, {BOT_A: 10300, BOT_B: 9700}, BASE_ORDERS, day_lines=[d1, d2, d3][:2])
        v, lines = er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, fail_frac=0.01, min_days=3))
        self.assertEqual(v, er.NA, lines)
        self.assertTrue(any("need 3" in l for l in lines))
        # a day line without the S1 fields spoils the per-day PASS as well
        run, src = make_run(self.td.name, rows, {BOT_A: 10300, BOT_B: 9700}, BASE_ORDERS,
                            day_lines=[d1, d2, d3, "[econ] day 4 faucet=1 sink=0 net=1 M_h=0 M_bots=1 unclassified=0"])
        v, _ = er.run_identity(run, src, er.Bands(abs_isk=5, run_frac=0, fail_frac=0.01, min_days=3))
        self.assertEqual(v, er.NA)

    def test_corp_wallet_cents_count_as_money(self):
        rows = BASE_ROWS + [jrow(7, 3, 1, BOT_B, 0, 25)]
        v, lines = self.run_id(rows=rows, balances={BOT_A: 10100, BOT_B: 9700}, corp_cents=[(5, 1000, 2500)])
        self.assertNotEqual(v, er.FAIL, lines)

    def test_day_lines_with_the_s1_fields_give_a_pass(self):
        day = ("[econ] day 1 faucet=20300 sink=0 net=20300 M_h=0 M_bots=19800 npc_fills=1 trades=1 "
               "unclassified=0 held=500 tx=7")
        v, lines = self.run_id(day_lines=["noise", day])
        self.assertEqual(v, er.PASS, lines)
        self.assertTrue(any(l.startswith("day 1:") for l in lines))

    def test_a_server_net_that_differs_from_the_journal_is_inconclusive_or_fail(self):
        day = ("[econ] day 1 faucet=20300 sink=0 net=20290 M_h=0 M_bots=19800 npc_fills=1 trades=1 "
               "unclassified=0 held=500 tx=7")
        v, lines = self.run_id(day_lines=[day])
        self.assertNotEqual(v, er.PASS)
        self.assertTrue(any("differs" in l or "residual" in l for l in lines))

    def test_unclassified_on_a_day_line_blocks_a_pass(self):
        day = ("[econ] day 1 faucet=20300 sink=0 net=20300 M_h=0 M_bots=19800 npc_fills=1 trades=1 "
               "unclassified=2 held=500 tx=7")
        v, _ = self.run_id(day_lines=[day])
        self.assertEqual(v, er.NA)

    def test_the_second_day_is_checked_against_the_first(self):
        d1 = "[econ] day 1 faucet=20300 sink=0 net=20300 M_h=0 M_bots=19800 unclassified=0 held=500 tx=7"
        rows = BASE_ROWS + [jrow(8, 3, 1, BOT_A, 0, 100)]
        d2 = "[econ] day 2 faucet=100 sink=0 net=100 M_h=0 M_bots=19900 unclassified=0 held=500 tx=8"
        v, lines = self.run_id(rows=rows, balances={BOT_A: 10200, BOT_B: 9700}, day_lines=[d1, d2])
        self.assertEqual(v, er.PASS, lines)

    def test_a_late_booking_counts_in_the_next_days_net(self):
        # #209: row 9 (50) was booked to day 1 after day 1's line printed, so day 2 prints it as `late=`.
        d1 = "[econ] day 1 faucet=20300 sink=0 net=20300 M_h=0 M_bots=19800 unclassified=0 held=500 tx=7"
        rows = BASE_ROWS + [jrow(8, 3, 1, BOT_A, 0, 100), jrow(9, 3, 1, BOT_A, 0, 50)]
        d2 = "[econ] day 2 faucet=100 sink=0 net=100 M_h=0 M_bots=19950 unclassified=0 held=500 tx=9 late=50.00"
        v, lines = self.run_id(rows=rows, balances={BOT_A: 10250, BOT_B: 9700}, day_lines=[d1, d2])
        self.assertEqual(v, er.PASS, lines)
        day2 = [l for l in lines if l.startswith("day 2:")]
        self.assertEqual(len(day2), 1, lines)
        self.assertIn("residual +0.00", day2[0])
        self.assertNotIn("differs", day2[0])

    def test_cli_exit_codes(self):
        run, src = make_run(self.td.name, BASE_ROWS, BASE_BAL, BASE_ORDERS)
        script = os.path.join(HERE, "econ-report.py")
        done = subprocess.run([sys.executable, "-I", script, "--identity", run, "--src", src],
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 3, done.stdout + done.stderr)
        self.assertIn("money identity", done.stdout)
        missing = subprocess.run([sys.executable, "-I", script, "--identity", os.path.join(run, "nope"), "--src", src],
                                 capture_output=True, text=True)
        self.assertEqual(missing.returncode, 2)

    def test_without_the_flag_the_tsv_is_still_required(self):
        script = os.path.join(HERE, "econ-report.py")
        done = subprocess.run([sys.executable, "-I", script], capture_output=True, text=True)
        self.assertEqual(done.returncode, 2)


class Nes(unittest.TestCase):
    ROWS = [
        row(3, "nes_geode", "miner", n=4, isk=4),
        row(3, "nes_inject", "human", n=2, isk=1000),
        row(3, "plex", "held:miner", n=12),
        row(3, "plex", "avg", isk=1500000),
        row(3, "plex", "in:invented", n=5),
        row(4, "plex", "out:invented", n=2),
        row(3, "career_band", "miner:low", n=1, hours=10, isk=500, isk2=600, per_h=50),
    ]

    def run_nes(self, rows):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            printed = er.nes(rows)
        return printed, buf.getvalue()

    def test_the_nes_tables_print_counts_and_plex(self):
        printed, out = self.run_nes(self.ROWS)
        self.assertTrue(printed)
        self.assertIn("geode opens by career", out)
        self.assertIn("4/4", out)
        self.assertIn("2/1000", out)
        self.assertIn("held:miner", out)
        self.assertIn("1500000", out)
        self.assertIn("out:invented", out)

    def test_the_first_fold_day_is_flagged_cumulative(self):
        printed, out = self.run_nes(self.ROWS + [row(3, "plex", "since_start", n=1)])
        self.assertTrue(printed)
        self.assertIn("day 3 is cumulative", out)
        self.assertNotIn("since_start ", out.split("PLEX per day")[-1])
        _, plain = self.run_nes(self.ROWS)
        self.assertNotIn("cumulative", plain)

    def test_no_rows_prints_nothing(self):
        self.assertEqual(self.run_nes([row(1, "career", "pve")]), (False, ""))


if __name__ == "__main__":
    unittest.main()
