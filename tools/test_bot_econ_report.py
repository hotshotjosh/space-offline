"""Unit tests for tools/bot-econ-report.py over invented lines and rows (no real ids)."""
import importlib.util
import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("bot_econ_report", os.path.join(HERE, "bot-econ-report.py"))
ber = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ber)

HEADER = "day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h"


def act(bot, kind, what, tail="ok"):
    return f"{bot} +01:02.3 {kind:<11} {what} -> {tail}"


def trow(day, kind, key, n=0, hours=0, isk=0, isk2=0, per_h=0, scope="all"):
    return "\t".join(map(str, (day, scope, kind, key, n, hours, isk, isk2, per_h)))


def settle(name, career, belt=None, ch=5001, st=9):
    tail = f" belt={belt}" if belt is not None else ""
    return f"[settle] {name} ({ch}) {career} at {st} span=3.0h work=1.00h isk=+10.00 goods=[]{tail}"


def run(bots=(), server=(), tsv=None):
    r = ber.Run()
    r.feed_bots(list(bots))
    r.feed_server(list(server))
    if tsv is not None:
        r.feed_tsv([HEADER] + list(tsv))
    return r


def got(r, **kw):
    return {k: (v, d) for k, _, v, d in ber.verdicts(r, **kw)}


class Dry(unittest.TestCase):
    def test_pass_when_the_next_warp_is_another_belt(self):
        r = run([act("Miner1", "warp", "CmdWarpToStuff item 11 minRange=0"),
                 "[mine] Miner1: belt 11 dry, 1 of 2 belts left",
                 act("Miner1", "warp", "CmdWarpToStuff item 12 minRange=0")])
        self.assertEqual(got(r)["dry"][0], ber.PASS)

    def test_fail_when_it_warps_back_or_another_bot_moves(self):
        r = run(["[mine] Miner1: belt 11 dry, 1 of 2 belts left",
                 act("Miner2", "warp", "CmdWarpToStuff item 12 minRange=0"),
                 act("Miner1", "warp", "CmdWarpToStuff item 11 minRange=0")])
        self.assertEqual(got(r)["dry"][0], ber.FAIL)

    def test_the_warp_home_is_not_another_belt(self):
        r = run(["[mine] Miner1: belt 11 dry, 1 of 2 belts left",
                 act("Miner1", "warp", "CmdWarpToStuff item 9 minRange=0"),
                 act("Miner1", "dock", "CmdDock 9")])
        self.assertEqual(got(r)["dry"][0], ber.FAIL)

    def test_no_line_is_no_signal_naming_it(self):
        v, d = got(run())["dry"]
        self.assertEqual(v, ber.NO_SIGNAL)
        self.assertIn("belts left", d)


class Spread(unittest.TestCase):
    def test_pass_and_fail_on_distinct_belts(self):
        good = run(server=[settle("A", "miner", 1, 1), settle("B", "miner", 2, 2), settle("C", "miner", 2, 3), settle("D", "miner", 1, 4)])
        self.assertEqual(got(good, belts=2)["spread"][0], ber.PASS)
        bad = run(server=[settle(n, "miner", 1, i) for i, n in enumerate("ABCD")])
        self.assertEqual(got(bad, belts=2)["spread"][0], ber.FAIL)
        self.assertEqual(got(bad)["spread"][0], ber.FAIL)  # unknown belts: still two miners on one belt

    def test_dry_line_sets_the_belt_count(self):
        r = run(["[mine] M: belt 7 dry, 3 of 4 belts left"],
                [settle(n, "miner", 1, i) for i, n in enumerate("ABCDEF")])
        self.assertEqual(got(r)["spread"][0], ber.FAIL)  # one belt of four for six miners

    def test_belts_of_other_stations_are_not_pooled(self):
        # Four miners at each of two stations, each station's miners on one belt:
        # pooled they show two belts, per station they show one.
        r = run(server=[settle(f"A{i}", "miner", 1, i, st=9) for i in range(4)]
                + [settle(f"B{i}", "miner", 2, 10 + i, st=8) for i in range(4)])
        self.assertEqual(got(r, belts=8)["spread"][0], ber.FAIL)

    def test_the_need_follows_the_rule(self):
        n = 6
        enough = int(ber.spread_need(n, n))
        names = [f"M{i}" for i in range(n)]
        pass_r = run(server=[settle(nm, "miner", i % enough, i) for i, nm in enumerate(names)])
        self.assertEqual(got(pass_r, belts=n)["spread"][0], ber.PASS)
        fail_r = run(server=[settle(nm, "miner", i % (enough - 1), i) for i, nm in enumerate(names)])
        self.assertEqual(got(fail_r, belts=n)["spread"][0], ber.FAIL)

    def test_no_belt_field_is_no_signal(self):
        v, d = got(run(server=[settle("A", "miner")]))["spread"]
        self.assertEqual(v, ber.NO_SIGNAL)
        self.assertIn("belt=ID", d)


class Rare(unittest.TestCase):
    def test_deploy_and_carry(self):
        r = run([act("R1", "rare", "rare ore turn: deployed to 9 (system 4, risk 0.2, rare [1])"),
                 act("R1", "rare", "rare ore turn: carried 40 units home to 8")])
        v = got(r)
        self.assertEqual((v["deploy"][0], v["carry"][0]), (ber.PASS, ber.PASS))

    def test_carried_zero_is_not_a_carry(self):
        r = run([act("R1", "rare", "rare ore turn: carried 0 units home to 8")])
        self.assertEqual(got(r)["carry"][0], ber.FAIL)

    def test_refused_is_not_counted(self):
        r = run([act("R1", "rare", "rare ore turn: deployed to 9 (system 4)", "refused: x")])
        v = got(r)
        self.assertEqual((v["deploy"][0], v["carry"][0]), (ber.FAIL, ber.FAIL))
        self.assertEqual(got(run())["deploy"][0], ber.NO_SIGNAL)


class MinerCareer(unittest.TestCase):
    def test_positive_from_day_three(self):
        r = run(tsv=[trow(1, "career", "miner", 5, 20, per_h=-50), trow(3, "career", "miner", 5, 20, per_h=80),
                     trow(4, "career", "miner", 5, 20, per_h=90)])
        self.assertEqual(got(r)["miner"][0], ber.PASS)

    def test_a_loss_day_fails_and_nothing_is_no_signal(self):
        r = run(tsv=[trow(3, "career", "miner", 5, 20, per_h=80), trow(4, "career", "miner", 5, 20, per_h=-1)])
        self.assertEqual(got(r)["miner"][0], ber.FAIL)
        self.assertEqual(got(run(tsv=[trow(1, "career", "miner", 5, 20, per_h=5)]))["miner"][0], ber.NO_SIGNAL)
        self.assertEqual(got(run())["miner"][0], ber.NO_SIGNAL)


class Economy(unittest.TestCase):
    def test_each_item_passes_on_its_line(self):
        r = run([act("H1", "haul", "freighter cycle: hull, bulk trade, home: hauled 3 loads"),
                 act("E1", "econ", "econ turn: geodes: opened 4 geode(s) of type 9"),
                 act("E2", "econ", "econ turn: bought 2 PLEX under the anchor at 100"),
                 act("E3", "econ", "econ turn: wearing SKIN 5 on ship 6"),
                 act("E4", "econ", "econ turn: sold 1 injector(s) at 90"),
                 act("P1", "pi", "pi: tend colonies: 2 colonies, 0 restarted")],
                tsv=[trow(2, "build", "ships", 3, 12)])
        v = got(r)
        for k in ("haul", "geode", "plex", "skin", "injector", "pi", "build"):
            self.assertEqual(v[k][0], ber.PASS, k)

    def test_injected_and_pi_settle_alternatives(self):
        r = run([act("E4", "econ", "econ turn: injected about 5000 points, 5000 applied")], [settle("F", "pi_farmer")])
        v = got(r)
        self.assertEqual((v["injector"][0], v["pi"][0]), (ber.PASS, ber.PASS))

    def test_absent_items_fail_and_a_fee_sink_passes_build(self):
        r = run([act("H1", "haul", "x", "FAILED: boom")], tsv=[trow(2, "isk_sink", "industry fees", 4, 0, isk=1000)])
        v = got(r)
        for k in ("haul", "geode", "plex", "skin", "injector", "pi"):
            self.assertEqual(v[k][0], ber.FAIL, k)
        self.assertEqual(v["build"][0], ber.PASS)
        self.assertEqual(got(run(tsv=[trow(2, "build", "ships", 0, 0)]))["build"][0], ber.FAIL)
        self.assertEqual(got(run())["build"][0], ber.NO_SIGNAL)

    def test_careers(self):
        three = [settle("A", "miner"), settle("B", "ratter"), settle("C", "trader")]
        self.assertEqual(got(run(server=three))["careers"][0], ber.PASS)
        self.assertEqual(got(run(server=three[:2]))["careers"][0], ber.FAIL)
        self.assertEqual(got(run())["careers"][0], ber.NO_SIGNAL)


class Closed(unittest.TestCase):
    def test_gm_setup_fails_only_when_closed(self):
        r = run([act("B1", "gm-setup", "/give me everything")])
        self.assertNotIn("gm-setup", got(r))
        self.assertEqual(got(r, closed=True)["gm-setup"][0], ber.FAIL)
        self.assertEqual(got(run([act("B1", "mine", "x")]), closed=True)["gm-setup"][0], ber.PASS)
        self.assertEqual(got(run(), closed=True)["gm-setup"][0], ber.NO_SIGNAL)


class Cli(unittest.TestCase):
    def test_main_reads_a_run_dir_and_exit_codes(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(ber.main([d]), 2)  # no bots.log
            with open(os.path.join(d, "bots.log"), "w") as f:
                f.write(act("H1", "haul", "x") + "\n")
            with open(os.path.join(d, "server.log"), "w") as f:
                f.write(settle("A", "miner", 1) + "\n")
            out = io.StringIO()
            with redirect_stdout(out):
                code = ber.main([d])
            self.assertEqual(code, 1)  # geode etc. are FAIL
            self.assertIn("PASS", out.getvalue())
            with redirect_stdout(io.StringIO()):
                self.assertEqual(ber.main([d, "--strict"]), 1)


# ---- soak-D judges (invented rows; invented thresholds, never the [A] defaults) ------------------------

OLD_KEYS = ["dry", "spread", "deploy", "carry", "miner", "haul", "geode", "plex", "skin", "injector", "pi",
            "build", "careers"]
B = ber.Bands(cap_share=0.5, ledger_ok=0.4, ledger_bad=0.6, installers=2, ref_median=0.05, ref_floor=-0.05,
              real_fail=-0.2, real_fills=2, real_builders=2, line=1000.0, stall_secs=60.0, reserve_tol=1.0,
              pi_floor=0.5, plex_wallet=700.0, idle_turns=2, carry_days=1, day_secs=100.0)
T0 = 134_000_000_000_000_000  # an invented FILETIME
SEC = 10_000_000


def cycle(bot, extra="", chose=None):
    c = f"; chose 77 at {chose} ISK/h" if chose is not None else ""
    return act(bot, "market", f"builder cycle: deliver, list, build, buy wants: posted 0 asks{c}{extra}")


def judged_run(bots=(), server=(), tsv=None):
    r = run(bots, server, tsv)
    r.judges = True
    return r


def jrow(i, secs, ref, o1, o2, amount, bal, desc="", arg=0):
    return "journal\t%d\t%d\t%d\t%d\t%d\t%d\t%s\t%s\t%s" % (i, T0 + secs * SEC, ref, o1, o2, arg, amount, bal, desc)


def jobrow(jid, owner, bp=7, runs=2, cost=10, product=900, activity=1, start=0):
    f = ["job", jid, activity, 1, bp, 5, 4, 0, 5, owner, 101, owner, owner, 1, 5, T0 + start, T0 + start, -1, runs, runs,
         runs, cost, 0, 1, product, -1, -1, 5, 4]
    return "\t".join(map(str, f))


def trade(i, typ, qty, price, buyer, seller, day=0):
    return "mkttrade\t%d\t%d\t%d\t%d\t%s\t5\t%d\t%d" % (i, T0 + day * 864_000_000_000, typ, qty, price, buyer, seller)


class AsBefore(unittest.TestCase):
    """A soak without the new fields or lines reports exactly as before."""

    LOG = [act("Miner1", "pi", "pi: tend colonies: 0 colonies, 0 restarted; sold 0 into bids, listed 0, kept 0"),
           act("Miner1", "rare", "rare ore turn: stays at 5: stays: wallet 100 below 200"),
           act("Builder1", "market", "builder cycle: x; chose 7 at -4 ISK/h; MarketExceededOrderCount; MarketExceededOrderCount"),
           "[soc] Builder1 progress class F race G weapon H career builder isk_h - over 0.0h model - wallet 5 ledgers -",
           "[soc] Founder1 corp found Invented [INV]: refused CorpNameTaken",
           act("Miner1", "econ", "econ turn: econ: opened 1 geode(s) of type 5")]

    def test_no_new_keys_and_the_old_verdicts_stand(self):
        r = run(self.LOG, [settle("A", "miner", 1), "[econ-mod] geode: tier 1 opened by 5, 0 prizes",
                           "[econ] day 1 faucet=1 sink=0 net=1 held=5 tx=9"])
        self.assertFalse(r.judges)
        v = ber.verdicts(r)
        self.assertEqual([k for k, *_ in v], OLD_KEYS)
        d = {k: x for k, _, x, _ in v}
        self.assertEqual((d["pi"], d["geode"], d["deploy"]), (ber.PASS, ber.PASS, ber.FAIL))  # an ok pi act with 0 colonies passed before

    def test_pi_acts_with_no_colonies_still_pass_without_the_judges_and_not_with_them(self):
        r = run(self.LOG[:1])
        self.assertEqual(got(r)["pi"][0], ber.PASS)
        r.judges = True
        self.assertNotEqual(got(r, bands=B)["pi"][0], ber.PASS)

    def test_load_turns_the_judges_on_by_the_s1_day_fields_or_the_flag_only(self):
        for server, flag, want in (([], False, False), (["[econ] day 1 faucet=1 net=1"], False, False),
                                   (["[econ] day 1 faucet=1 net=1 held=5 tx=9"], False, True), ([], True, True)):
            with tempfile.TemporaryDirectory() as d:
                with open(os.path.join(d, "bots.log"), "w") as f:
                    f.write(act("H1", "haul", "x") + "\n")
                with open(os.path.join(d, "server.log"), "w") as f:
                    f.write("\n".join(server) + "\n")
                r = ber.load(d, judges=flag)
                self.assertEqual(r.judges, want, (server, flag))
                keys = [k for k, *_ in ber.verdicts(r)]
                self.assertEqual("stall" in keys, want)

    def test_the_cli_prints_the_old_lines_only_for_an_old_dir(self):
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "bots.log"), "w") as f:
                f.write("\n".join(self.LOG) + "\n")
            out = io.StringIO()
            with redirect_stdout(out):
                ber.main([d])
            self.assertEqual(out.getvalue(), "\n".join(ber.report(ber.load(d))) + "\n")
            self.assertEqual(len(out.getvalue().splitlines()), len(OLD_KEYS))


def got(r, **kw):
    return {k: (v, d) for k, _, v, d in ber.verdicts(r, **kw)}


class Builders(unittest.TestCase):
    def test_pick_passes_and_fails_on_a_losing_pick(self):
        self.assertEqual(got(judged_run([cycle("B1", chose=5), cycle("B2", chose=9)]), bands=B)["pick"][0], ber.PASS)
        self.assertEqual(got(judged_run([cycle("B1", chose=5), cycle("B2", chose=0)]), bands=B)["pick"][0], ber.FAIL)
        self.assertEqual(got(judged_run([cycle("B1")]), bands=B)["pick"][0], ber.NO_SIGNAL)

    def test_order_cap_counts_refusals_per_line(self):
        cap = "; list: refused: PlaceMultiSellOrder answered None (MarketExceededOrderCount)"
        one = judged_run([cycle("B1", cap), cycle("B2"), cycle("B3")])
        self.assertEqual(got(one, bands=B)["order-cap"][0], ber.PASS)  # 1 of 3 with the invented 50 % share
        crowded = judged_run([cycle("B1", cap), cycle("B2", cap), cycle("B3")])
        self.assertEqual(got(crowded, bands=B)["order-cap"][0], ber.INCONCLUSIVE)
        twice = judged_run([cycle("B1", cap + cap), cycle("B2")])
        self.assertEqual(got(twice, bands=B)["order-cap"][0], ber.FAIL)
        self.assertEqual(got(judged_run(), bands=B)["order-cap"][0], ber.NO_SIGNAL)

    def test_washes_read_every_day(self):
        ok = judged_run(tsv=[trow(1, "audit", "washes", n=0), trow(2, "audit", "washes", n=0)])
        self.assertEqual(got(ok, bands=B)["washes"][0], ber.PASS)
        bad = judged_run(tsv=[trow(1, "audit", "washes", n=0), trow(2, "audit", "washes", n=3)])
        self.assertEqual(got(bad, bands=B)["washes"][0], ber.FAIL)
        self.assertEqual(got(judged_run(tsv=[]), bands=B)["washes"][0], ber.NO_SIGNAL)

    def test_ledger_skips_each_bots_first_two_lines(self):
        def prog(bot, led):
            return f"[soc] {bot} progress class F race G weapon H career builder isk_h - over 1.0h model - wallet 5 ledgers {led}"

        lines = [prog("B1", "-"), prog("B1", "-"), prog("B1", "10/h"), prog("B1", "12/h"), prog("B1", "-")]
        d = got(judged_run(lines), bands=B)["ledger"]
        self.assertEqual(d[0], ber.PASS)  # 1 of 3 counted lines is `-`
        self.assertIn("1 of 3", d[1])
        bad = [prog("B1", "-")] * 5
        self.assertEqual(got(judged_run(bad), bands=B)["ledger"][0], ber.FAIL)
        self.assertEqual(got(judged_run(bad[:2]), bands=B)["ledger"][0], ber.NO_SIGNAL)

    def test_installs(self):
        inst = lambda b, n: cycle(b, f"; delivered 0 jobs, installed {n}")  # noqa: E731
        self.assertEqual(got(judged_run([inst("B1", 1), inst("B2", 2)]), bands=B)["installs"][0], ber.PASS)
        self.assertEqual(got(judged_run([inst("B1", 0)]), bands=B)["installs"][0], ber.FAIL)
        pay = judged_run([inst("B1", 1), cycle("B2", "; no job pays: 3")])
        self.assertEqual(got(pay, bands=B)["installs"][0], ber.PASS)
        off = judged_run([inst("B1", 0)], [settle("B2", "builder", st=9).replace("goods=[]", "goods=[] notes: job 12: 4 runs of blueprint 7")])
        self.assertEqual(got(off, bands=B)["installs"][0], ber.INCONCLUSIVE)


class Margins(unittest.TestCase):
    def content(self, product_price=100.0, mat_price=10.0):
        r = judged_run()
        r.bps = {7: {"mat": [(500, 4)], "prod": (900, 3)}}
        r.prices = {500: (mat_price, "base"), 900: (product_price, "recipe"), 950: (8.0, "pi")}
        return r

    def test_margin_ref_by_job(self):
        r = self.content()  # 2 runs: out 600, mats 80 plus job cost 10
        r.feed_world_lines([jobrow(1, 50), jobrow(2, 51)])
        v, d = got(r, bands=B)["margin-ref"]
        self.assertEqual(v, ber.PASS)
        self.assertIn("2 jobs", d)
        self.assertIn("+566.7%", d)
        low = self.content(product_price=30.0)  # out 180 over cost 90
        low.feed_world_lines([jobrow(1, 50)])
        self.assertEqual(got(low, bands=B)["margin-ref"][0], ber.PASS)
        loss = self.content(product_price=10.0)  # out 60 under cost 90
        loss.feed_world_lines([jobrow(1, 50)])
        self.assertEqual(got(loss, bands=B)["margin-ref"][0], ber.FAIL)
        thin = self.content(product_price=15.0)  # out 90 = cost 90: a margin of 0, under the invented median
        thin.feed_world_lines([jobrow(1, 50)])
        self.assertEqual(got(thin, bands=B)["margin-ref"][0], ber.INCONCLUSIVE)

    def test_margin_ref_needs_the_content_and_ignores_other_activities(self):
        r = judged_run()
        r.feed_world_lines([jobrow(1, 50)])
        v, d = got(r, bands=B)["margin-ref"]
        self.assertEqual(v, ber.NO_SIGNAL)
        self.assertIn("--content", d)
        c = self.content()
        c.feed_world_lines([jobrow(1, 50, activity=3)])
        self.assertEqual(got(c, bands=B)["margin-ref"][0], ber.NO_SIGNAL)

    def real(self, sell_price, scale=1.0):
        r = self.content()
        r.feed_world_lines([jobrow(1, 50), jobrow(2, 51)])
        # materials bought at 10 by each builder: unit cost = (4*2*10 + 10) / 6 = 15
        rows = [trade(1, 500, 8, 10, 50, 1), trade(2, 500, 8, 10, 51, 1)]
        rows += [trade(10 + i, 900, 1, sell_price, 60, 50 + i % 2) for i in range(4)]
        r.feed_ledger_lines(rows)
        return got(r, bands=B, scale=scale)["margin-real"]

    def test_margin_real_pass_and_fail_only_at_scale_one(self):
        self.assertEqual(self.real(20)[0], ber.PASS)
        self.assertEqual(self.real(10)[0], ber.FAIL)  # -50 % against the invented -20 % line
        v, d = self.real(10, scale=6)
        self.assertEqual(v, ber.INCONCLUSIVE)
        self.assertIn("x6", d)

    def test_margin_real_no_signal_on_few_fills_or_one_builder(self):
        r = self.content()
        r.feed_world_lines([jobrow(1, 50)])
        r.feed_ledger_lines([trade(1, 500, 8, 10, 50, 1)] + [trade(10 + i, 900, 1, 20, 60, 50) for i in range(5)])
        self.assertEqual(got(r, bands=B)["margin-real"][0], ber.NO_SIGNAL)


class Founders(unittest.TestCase):
    F, P = 800, 801

    def pledge(self, i, secs, amount, bal):
        return [jrow(i, secs, 10, self.P, self.F, -amount, 5, "{DESC: \"co-founding pledge\"}"),
                jrow(i + 1, secs, 10, self.P, self.F, amount, bal, "{DESC: \"co-founding pledge\"}")]

    def test_reserve_passes_and_fails_on_a_bot_side_debit(self):
        base = self.pledge(1, 0, 600, 700)
        ok = judged_run()
        ok.feed_ledger_lines(base + [jrow(3, 5, 2, self.F, 77, -50, 650)])
        self.assertEqual(got(ok, bands=B)["reserve"][0], ber.PASS)
        bad = judged_run()
        bad.feed_ledger_lines(base + [jrow(3, 5, 2, self.F, 77, -150, 550)])
        v, d = got(bad, bands=B)["reserve"]
        self.assertEqual(v, ber.FAIL)
        self.assertIn("ref 2", d)

    def test_offline_capable_debits_are_reported_not_judged(self):
        r = judged_run()
        r.feed_ledger_lines(self.pledge(1, 0, 600, 700) + [jrow(3, 5, 56, self.F, 77, -150, 550)])
        v, d = got(r, bands=B)["reserve"]
        self.assertEqual(v, ber.PASS)
        self.assertIn("offline-capable", d)

    def test_the_registration_fee_ends_the_window_and_no_pledges_is_no_signal(self):
        r = judged_run()
        r.feed_ledger_lines(self.pledge(1, 0, 600, 700) + [jrow(3, 5, 39, self.F, 1, -100, 600),
                                                           jrow(4, 6, 2, self.F, 77, -500, 100)])
        self.assertEqual(got(r, bands=B)["reserve"][0], ber.PASS)
        n = judged_run()
        n.feed_ledger_lines([jrow(1, 0, 2, self.F, 77, -5, 100)])
        self.assertEqual(got(n, bands=B)["reserve"][0], ber.NO_SIGNAL)

    def test_stall_needs_the_balance_over_the_line_for_the_time_while_acting(self):
        rows = self.pledge(1, 0, 1200, 1300) + [jrow(3, 30, 2, self.F, 77, -1, 1299), jrow(4, 100, 46, self.F, 77, -1, 1298)]
        r = judged_run()
        r.feed_ledger_lines(rows)
        v, d = got(r, bands=B)["stall"]
        self.assertEqual(v, ber.FAIL)
        self.assertIn("800", d)
        quick = judged_run()  # the same rows, the act inside the invented 60 s
        quick.feed_ledger_lines(self.pledge(1, 0, 1200, 1300) + [jrow(3, 30, 2, self.F, 77, -1, 1299)])
        self.assertNotEqual(got(quick, bands=B)["stall"][0], ber.FAIL)
        under = judged_run()
        under.feed_ledger_lines(self.pledge(1, 0, 600, 700) + [jrow(3, 100, 2, self.F, 77, -1, 699)])
        v, d = got(under, bands=B)["stall"]
        self.assertEqual(v, ber.INCONCLUSIVE)
        self.assertIn("peak founder wallet 700", d)

    def test_stall_passes_on_a_founded_corp_and_fails_on_a_refusal(self):
        r = judged_run(["[soc] F1 corp found Invented [INV] 9: ok", "[soc] bots 9 online 9 mix a=9 corps 1 alliances 0 blue 0 red 0"])
        self.assertEqual(got(r, bands=B)["stall"][0], ber.PASS)
        none = judged_run(["[soc] F1 corp found Invented [INV] 9: ok", "[soc] bots 9 online 9 mix a=9 corps 0 alliances 0 blue 0 red 0"])
        self.assertEqual(got(none, bands=B)["stall"][0], ber.INCONCLUSIVE)
        ref = judged_run(["[soc] F1 corp found Invented [INV]: refused CorpNameTaken"])
        self.assertEqual(got(ref, bands=B)["stall"][0], ber.FAIL)
        self.assertEqual(got(judged_run(["x"]), bands=B)["stall"][0], ber.NO_SIGNAL)


class Items(unittest.TestCase):
    def test_geode(self):
        v = lambda server: got(judged_run(server=server), bands=B)["geode"]  # noqa: E731
        drop = "[econ-mod] geode: tier 1 for %d (crit false, system 5)"
        opened = "[econ-mod] geode: tier 1 opened by %d, %d prizes%s"
        self.assertEqual(v([])[0], ber.NO_SUPPLY)
        self.assertEqual(v([drop % 1, opened % (1, 2, ": plex,skin")])[0], ber.PASS)
        self.assertEqual(v([drop % 1, opened % (1, 0, "")])[0], ber.FAIL)
        self.assertEqual(v([drop % 1, drop % 2, drop % 3])[0], ber.FAIL)
        self.assertEqual(v([drop % 1, drop % 2, drop % 3, drop % 4, opened % (1, 1, ": skin")])[0], ber.INCONCLUSIVE)
        self.assertEqual(v([drop % 1, drop % 2])[0], ber.INCONCLUSIVE)

    def test_no_supply_is_not_a_pass_and_strict_exits_one(self):
        r = judged_run([act("M1", "econ", "econ turn: econ: nothing to do")])
        d = got(r, bands=B)
        for k in ("plex", "skin", "injector", "haul"):
            self.assertEqual(d[k][0], ber.NO_SUPPLY, k)
        self.assertEqual(ber.status(r, bands=B, strict=True), 1)
        only = [v for k, _, v, _ in ber.verdicts(r, bands=B) if k in ("plex", "skin", "injector", "haul")]
        self.assertNotIn(ber.FAIL, only)

    def test_supply_and_a_use_pass_and_the_plex_idle_rule_fails(self):
        used = judged_run([act("M1", "econ", "econ turn: bought 2 PLEX under the anchor at 5")],
                          ["[econ-mod] geode: tier 1 opened by 5, 1 prizes: plex"])
        self.assertEqual(got(used, bands=B)["plex"][0], ber.PASS)
        idle_lines = ["[soc] M1 progress class F race G weapon H career miner isk_h - over 1.0h model - wallet 900 ledgers -"] + \
            [act("M1", "econ", "econ turn: econ: nothing to do")] * 2
        self.assertEqual(got(judged_run(idle_lines), bands=B)["plex"][0], ber.FAIL)
        short = idle_lines[:2]
        self.assertEqual(got(judged_run(short), bands=B)["plex"][0], ber.INCONCLUSIVE)
        prize = judged_run([], ["[econ-mod] geode: tier 1 opened by 5, 1 prizes: skin!"])
        self.assertEqual(got(prize, bands=B)["skin"][0], ber.NO_SUPPLY)  # a failed prize (`!`) is not supply

    def test_injector_drops_are_supply(self):
        r = judged_run(server=["[econ-mod] injector: large for 5 (bounty 1000)"])
        self.assertEqual(got(r, bands=B)["injector"][0], ber.INCONCLUSIVE)

    def test_hypercore_drops_are_summed(self):
        r = judged_run(server=["[econ-mod] hypercores: hypercores=3 for 5 (bounty 1200)",
                               "[econ-mod] hypercores: hypercores=2 for 6 (bounty 90)",
                               "[econ-mod] hypercores: none for 7 (bounty 90)",
                               "hypercores: hypercores=9 for 5 (bounty 1)"])
        self.assertEqual((r.hypercores, r.hypercore_drops), (5, 2))
        self.assertIn("5 hypercores in 2 drops", got(r, bands=B)["injector"][1])
        self.assertEqual(judged_run(server=[]).hypercores, 0)

    def test_haul(self):
        self.assertEqual(got(judged_run([act("H1", "haul", "x")]), bands=B)["haul"][0], ber.PASS)
        self.assertEqual(got(judged_run(server=[settle("H1", "freighter_hauler")]), bands=B)["haul"][0], ber.INCONCLUSIVE)


class Pi(unittest.TestCase):
    def pi_act(self, colonies, sold=0, kept=0):
        return act("P1", "pi", f"pi: tend colonies: {colonies} colonies, 0 restarted; sold {sold} into bids, listed 0, kept {kept}")

    def test_no_colonies_is_no_signal_even_on_ok_acts(self):
        r = judged_run([self.pi_act(0)] * 3)
        self.assertEqual(got(r, bands=B)["pi"][0], ber.NO_SIGNAL)
        self.assertEqual(got(judged_run(), bands=B)["pi"][0], ber.NO_SIGNAL)

    def test_sales_against_the_reference(self):
        def build(price):
            r = judged_run([self.pi_act(2, sold=3)])
            r.prices = {950: (100.0, "pi"), 500: (10.0, "base")}
            r.feed_ledger_lines([trade(1, 950, 3, price, 60, 8000), trade(2, 500, 1, 1, 60, 8000)])
            return got(r, bands=B)["pi"]
        self.assertEqual(build(80)[0], ber.PASS)  # over the invented 50 % line
        self.assertEqual(build(30)[0], ber.FAIL)

    def test_kept_only_fails_and_no_sale_is_inconclusive(self):
        r = judged_run([self.pi_act(1, sold=0, kept=4)])
        r.prices = {950: (100.0, "pi")}
        r.feed_ledger_lines([])
        self.assertEqual(got(r, bands=B)["pi"][0], ber.FAIL)
        q = judged_run([self.pi_act(1)])
        q.prices = {950: (100.0, "pi")}
        q.feed_ledger_lines([])
        self.assertEqual(got(q, bands=B)["pi"][0], ber.INCONCLUSIVE)
        self.assertEqual(got(judged_run([self.pi_act(1)]), bands=B)["pi"][0], ber.NO_SIGNAL)


class Rare(unittest.TestCase):
    def stay(self, bot, w=100, g=200):
        return act(bot, "rare", f"rare ore turn: stays at 5: 0 rare units; stays: wallet {w} below {g}")

    def test_all_wallet_stays_is_no_signal_with_the_max_wallet_and_gate(self):
        r = judged_run([self.stay("M1", 100), self.stay("M2", 150)])
        v, d = got(r, bands=B)["deploy"]
        self.assertEqual(v, ber.NO_SIGNAL)
        self.assertIn("max wallet 150", d)
        self.assertIn("gate 200", d)
        self.assertEqual(got(r, bands=B)["carry"][0], ber.NO_SIGNAL)

    def test_a_deploy_and_a_carry_pass(self):
        r = judged_run([act("M1", "rare", "rare ore turn: deployed to 5 (system 3, risk 1, rare [9])"),
                        act("M1", "rare", "rare ore turn: carried 40 units home to 6")])
        d = got(r, bands=B)
        self.assertEqual((d["deploy"][0], d["carry"][0]), (ber.PASS, ber.PASS))

    def test_a_failed_deploy_fails(self):
        r = judged_run([act("M1", "rare", "rare ore turn: deployed to 5 (system 3)", tail="refused: nope")])
        self.assertEqual(got(r, bands=B)["deploy"][0], ber.FAIL)

    def test_other_stays_are_inconclusive(self):
        r = judged_run([act("M1", "rare", "rare ore turn: stays at 5: no belt beats home by 20%")])
        self.assertEqual(got(r, bands=B)["deploy"][0], ber.INCONCLUSIVE)

    def test_a_deploy_without_a_carry_fails_only_after_the_invented_deadline(self):
        def timed(t, bot, what):
            m, sec = divmod(t, 60)
            return f"{bot} +{m:02d}:{sec:04.1f} rare        {what} -> ok"

        dep = timed(10, "M1", "rare ore turn: deployed to 5 (system 3)")
        late = judged_run([dep, timed(300, "M2", "rare ore turn: stays at 5: no route under MAX_RISK")])
        self.assertEqual(got(late, bands=B, scale=1.0)["carry"][0], ber.FAIL)  # 290 s > 1 invented day of 100 s
        soon = judged_run([dep, timed(50, "M2", "rare ore turn: stays at 5: no route under MAX_RISK")])
        self.assertEqual(got(soon, bands=B)["carry"][0], ber.INCONCLUSIVE)
        carried = judged_run([dep, timed(60, "M1", "rare ore turn: carried 3 units home to 6")])
        self.assertEqual(got(carried, bands=B)["carry"][0], ber.PASS)
        scaled = judged_run([dep, timed(300, "M2", "rare ore turn: stays at 5: no route under MAX_RISK")])
        self.assertEqual(got(scaled, bands=B, scale=1000.0)["carry"][0], ber.FAIL)  # the day shrinks with the scale


class Plumbing(unittest.TestCase):
    def test_the_world_ledger_and_content_files_load_from_a_run_dir(self):
        with tempfile.TemporaryDirectory() as d, tempfile.TemporaryDirectory() as c:
            with open(os.path.join(d, "bots.log"), "w") as f:
                f.write(act("H1", "haul", "x") + "\n")
            with open(os.path.join(d, "server.log"), "w") as f:
                f.write("[econ] day 1 faucet=1 sink=0 net=1 held=5 tx=9\n")
            os.makedirs(os.path.join(d, "world.dat.ledger"))
            with open(os.path.join(d, "world.dat.ledger", "a.tsv"), "w") as f:
                f.write(jrow(1, 0, 2, 1, 2, -5, 10) + "\n")
                f.write(trade(2, 900, 1, 5, 1, 2) + "\n")
            with open(os.path.join(d, "world.dat"), "w") as f:
                f.write("balance\t9\t12.5\n" + jobrow(4, 50) + "\n")
            with open(os.path.join(d, "summary.tsv"), "w") as f:
                f.write("usererror\tmarketProxy.PlaceMultiSellOrder\t7\n")
            os.makedirs(os.path.join(c, "industry"))
            os.makedirs(os.path.join(c, "econ"))
            with open(os.path.join(c, "industry", "blueprints.tsv"), "w") as f:
                f.write("#h\n7\t1\tmaterial\t500\t4\t\t1\t1\n7\t1\tproduct\t900\t3\t\t1\t1\n7\t3\tmaterial\t501\t9\t\t1\t1\n")
            with open(os.path.join(c, "econ", "prices.tsv"), "w") as f:
                f.write("#h\n500\t10.0\tbase\n900\t100.0\trecipe\n")
            r = ber.load(d, c)
            self.assertTrue(r.judges)
            self.assertEqual((len(r.journal), len(r.trades), r.balances, len(r.jobs), r.summary_errors),
                             (1, 1, {9: 12.5}, 1, 7))
            self.assertEqual(r.bps, {7: {"mat": [(500, 4)], "prod": (900, 3)}})
            self.assertEqual(r.prices[900], (100.0, "recipe"))

    def test_main_takes_the_new_flags_and_strict_counts_the_new_verdicts(self):
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "bots.log"), "w") as f:
                f.write(act("H1", "haul", "x") + "\n")
            out = io.StringIO()
            with redirect_stdout(out):
                ber.main([d, "--judges", "--scale", "6"])
            self.assertIn("builders pick no losing job", out.getvalue())
        r = judged_run()
        self.assertEqual(ber.status(r, bands=B, strict=True), 1)




class Round2(unittest.TestCase):
    def test_an_open_with_a_failed_prize_role_is_a_fail_not_a_pass(self):
        r = judged_run(server=["[econ-mod] geode: tier 1 for 5 (crit false, system 5)",
                               "[econ-mod] geode: tier 1 opened by 5, 1 prizes: skin!"])
        v, d = got(r, bands=B)["geode"]
        self.assertEqual(v, ber.FAIL)
        self.assertIn("role!", d)

    def test_zero_colonies_with_a_p1_sale_is_not_a_pass(self):
        r = judged_run([act("P1", "pi", "pi: tend colonies: 0 colonies, 0 restarted; sold 3 into bids, listed 0, kept 0")])
        r.prices = {950: (100.0, "pi")}
        r.feed_ledger_lines([trade(1, 950, 3, 100, 60, 8000)])
        self.assertEqual(got(r, bands=B)["pi"][0], ber.NO_SIGNAL)

    def test_the_carry_deadline_runs_on_the_scaled_society_day(self):
        sp = ber.Bands(carry_days=2, day_secs=86400.0)

        def timed(t, bot, what):
            m, sec = divmod(t, 60)
            return f"{bot} +{m:02d}:{sec:04.1f} rare        {what} -> ok"

        end = timed(57600, "M2", "rare ore turn: stays at 5: no route under MAX_RISK")
        early = judged_run([timed(1000, "M1", "rare ore turn: deployed to 5 (system 3)"), end])
        # two society days at x6 are 28,800 s, so a deploy at 1,000 s with 56,600 s left has missed it
        self.assertEqual(got(early, bands=sp, scale=6.0)["carry"][0], ber.FAIL)
        self.assertEqual(got(early, bands=sp, scale=1.0)["carry"][0], ber.INCONCLUSIVE)
        late = judged_run([timed(40000, "M1", "rare ore turn: deployed to 5 (system 3)"), end])
        self.assertEqual(got(late, bands=sp, scale=6.0)["carry"][0], ber.INCONCLUSIVE)

    def test_with_the_judges_on_the_old_lenient_readings_do_not_run(self):
        lines = [act("P1", "pi", "pi: tend colonies: 0 colonies, 0 restarted; sold 0 into bids, listed 0, kept 0"),
                 act("M1", "econ", "econ turn: econ: opened 1 geode(s) of type 5")]
        r = judged_run(lines, ["[econ-mod] geode: tier 1 opened by 5, 0 prizes"])
        v = ber.verdicts(r, bands=B)
        keys = [k for k, *_ in v]
        self.assertEqual(len(keys), len(set(keys)))
        d = {k: x for k, _, x, _ in v}
        self.assertEqual(d["geode"], ber.FAIL)  # the old reading passed on the econ note alone
        self.assertNotEqual(d["pi"], ber.PASS)  # the old reading passed on an ok pi act

    def test_soak_sh_passes_the_judge_flags(self):
        here = os.path.dirname(HERE)
        with open(os.path.join(here, "tools", "soak.sh")) as f:
            text = f.read()
        line = next(l for l in text.splitlines() if "tools/bot-econ-report.py" in l and "python3" in l)
        self.assertIn("bot_args", line)
        self.assertIn("--judges", text)
        self.assertIn('--scale "${TIME_SCALE:-1}"', text)
        self.assertIn("--content", text)
        self.assertIn("econ-report.py\" --identity", text)



FF_CLEAN = "[ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0"
FF_STALLS = "[ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=3"
FF_DOG = "[ff] watchdog exit real_s=60.0 virt_s=3601.25"


class FastForward(unittest.TestCase):
    def dir_with(self, server_lines):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        with open(os.path.join(d.name, "bots.log"), "w") as f:
            f.write(act("H1", "haul", "x") + "\n")
        with open(os.path.join(d.name, "server.log"), "w") as f:
            f.write("\n".join(server_lines) + "\n")
        return d.name

    def main(self, d, *extra):
        out = io.StringIO()
        with redirect_stdout(out):
            code = ber.main([d, *extra])
        return code, out.getvalue()

    def test_a_run_without_ff_reports_exactly_as_before(self):
        d = self.dir_with([settle("A", "miner", 1)])
        run_ = ber.load(d)
        self.assertIsNone(run_.ff)
        code, text = self.main(d)
        self.assertEqual(text, "\n".join(ber.report(run_)) + "\n")
        self.assertEqual(code, 1)
        self.assertNotIn("ff:", text)

    def test_a_clean_ff_line_keeps_the_old_verdicts(self):
        base_code, base = self.main(self.dir_with([settle("A", "miner", 1)]))
        code, text = self.main(self.dir_with([settle("A", "miner", 1), FF_CLEAN]))
        self.assertEqual(code, base_code)
        self.assertEqual(text.splitlines()[1:], base.splitlines())
        self.assertEqual(text.splitlines()[0], "ff: mode=lockstep ratio=405.8 stalls=0")

    def test_a_watchdog_exit_makes_every_row_n_a_and_exits_0(self):
        d = self.dir_with([settle("A", "miner", 1), FF_CLEAN, FF_DOG])
        code, text = self.main(d, "--strict")
        self.assertEqual(code, 0)
        rows = text.splitlines()[1:]
        self.assertTrue(rows)
        for line in rows:
            self.assertTrue(line.startswith("n/a"), line)

    def test_stalls_force_n_a(self):
        run_ = ber.load(self.dir_with([FF_STALLS]))
        self.assertTrue(all(v == ber.NA for _, _, v, _ in ber.verdicts(run_)))


if __name__ == "__main__":
    unittest.main()
