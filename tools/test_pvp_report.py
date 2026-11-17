"""Unit tests for tools/pvp-report.py over invented log lines (no real ids)."""
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("pvp_report", os.path.join(HERE, "pvp-report.py"))
pr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pr)


def act(bot, what, tail="ok"):
    return f"{bot} +01:02.3 kit         {what} -> {tail}"


def run(lines):
    log = pr.Log()
    log.feed(lines)
    return {k: (v, d) for k, _, v, d in pr.verdicts(log)}, log


class Probes(unittest.TestCase):
    def test_pass(self):
        r, _ = run([act("Hunter1", "launch probe from 9001"), act("Hunter2", "launch probe from 9002")])
        self.assertEqual(r["probes"][0], pr.PASS)
        self.assertIn("2 launches by 2 bots", r["probes"][1])

    def test_fail_when_all_refused_or_none(self):
        r, _ = run([act("Hunter1", "launch probe from 9001: probe launch refused: nope")])
        self.assertEqual(r["probes"][0], pr.FAIL)
        self.assertIn("1 refused", r["probes"][1])
        r, _ = run([])
        self.assertEqual(r["probes"][0], pr.FAIL)


class Field(unittest.TestCase):
    def test_pass_and_fail(self):
        r, _ = run([act("Hic1", "field on 9003"), act("Hic1", "field off 9003")])
        self.assertEqual(r["field"][0], pr.PASS)
        self.assertIn("1 field-off", r["field"][1])
        r, _ = run([act("Hic1", "field on 9003", "refused: x")])
        self.assertEqual(r["field"][0], pr.FAIL)

    def test_other_act_kinds_do_not_count(self):
        r, _ = run(["Hic1 +00:01.0 lock        field on 9003 -> ok"])
        self.assertEqual(r["field"][0], pr.FAIL)


class Bubble(unittest.TestCase):
    def test_pass(self):
        r, _ = run(["[warp_bubble] Hunter1: held", "[warp_bubble] Hunter1: free after 42 s"])
        self.assertEqual(r["bubble"][0], pr.PASS)

    def test_fail_over_budget_never_freed_or_none(self):
        lines = ["[warp_bubble] Hunter1: held", "[warp_bubble] Hunter1: held over the budget",
                 "[warp_bubble] Hunter1: free after 400 s"]
        self.assertEqual(run(lines)[0]["bubble"][0], pr.FAIL)
        self.assertEqual(run(["[warp_bubble] Hunter1: held"])[0]["bubble"][0], pr.FAIL)
        self.assertEqual(run([])[0]["bubble"][0], pr.FAIL)


class Signals(unittest.TestCase):
    def test_no_signal_today(self):
        r, _ = run([act("Hunter1", "launch probe from 9001")])
        for k in ("tackler", "dictor", "bomb", "mjd", "gate"):
            self.assertEqual(r[k][0], pr.NO_SIGNAL, k)
            self.assertIn("needs to log", r[k][1])

    def test_the_lines_the_bots_print(self):
        # The shapes counter::signal_line prints (#136), invented ids.
        lines = ["[counter] Hunter1: tackler-first target=11 ok=1",
                 "[counter] Hunter1: dictor-held target=12 ok=1",
                 "[counter] Hunter1: bomb-keepoff target=13 range=7000 ok=1",
                 "[counter] Hunter1: mjd-scram target=14 ok=0",
                 "[camp] Dictor1: holding gate=15 system=16 ok=1",
                 "[camp] Dictor2: holding gate=none system=16 ok=0"]
        r, _ = run(lines)
        self.assertEqual(r["tackler"][0], pr.PASS)
        self.assertEqual(r["dictor"][0], pr.PASS)
        self.assertEqual(r["bomb"][0], pr.PASS)
        self.assertEqual(r["mjd"][0], pr.FAIL)
        self.assertNotEqual(r["gate"][0], pr.NO_SIGNAL)

    def test_pass_and_fail(self):
        lines = ["[counter] Hunter1: tackler-first target=1 ok=1", "[counter] Hunter2: tackler-first target=2 ok=1",
                 "[counter] Hunter1: dictor-held target=3 ok=0",
                 "[counter] Hunter1: bomb-keepoff target=4 range=9 ok=1",
                 "[counter] Hunter1: mjd-scram target=5 ok=0", "[counter] Hunter1: mjd-scram target=5 ok=0",
                 "[camp] Gang1: holding gate=6 system=7 ok=1"]
        r, _ = run(lines)
        self.assertEqual(r["tackler"][0], pr.PASS)
        self.assertEqual(r["dictor"][0], pr.FAIL)
        self.assertEqual(r["bomb"][0], pr.PASS)
        self.assertEqual(r["mjd"][0], pr.FAIL)
        self.assertEqual(r["gate"][0], pr.PASS)


SIEGE = "capital: siege on, fuel 900 -> 880"


class CapitalSourced(unittest.TestCase):
    def test_sourced_passes(self):
        r, _ = run(["[cap] Dread1 hull=9907001 from=hangar", f"Dread1 +00:09.0 capital     {SIEGE} -> ok"])
        self.assertEqual(r["capital"][0], pr.PASS)
        self.assertIn("1 siege lines, 1 sourced", r["capital"][1])

    def test_unsourced_fails_naming_the_bot(self):
        r, _ = run(["[cap] Dread1 hull=9907001 from=market", f"Dread1 +00:09.0 capital     {SIEGE} -> ok",
                    f"Dread2 +00:10.0 capital     {SIEGE} -> ok"])
        self.assertEqual(r["capital"][0], pr.FAIL)
        self.assertIn("Dread2", r["capital"][1])
        self.assertNotIn("Dread1,", r["capital"][1])

    def test_cap_line_after_the_siege_is_not_earlier(self):
        r, _ = run([f"Dread1 +00:09.0 capital     {SIEGE} -> ok", "[cap] Dread1 hull=9907001 from=hangar"])
        self.assertEqual(r["capital"][0], pr.FAIL)

    def test_aboard_only_is_na_not_fail(self):
        r, _ = run(["[cap] Dread1 hull=9907001 from=aboard", f"Dread1 +00:09.0 capital     {SIEGE} -> ok"])
        self.assertEqual(r["capital"][0], pr.NA)
        self.assertIn("aboard at login", r["capital"][1])

    def test_real_nested_note_shape(self):
        note = ("X +01:00.0 capital     capital pair: stock, fleet, light or jump: "
                "capital: siege on, fuel 1500 -> 1480 -> ok")
        self.assertEqual(run([note])[0]["capital"][0], pr.FAIL)
        self.assertEqual(run(["[cap] X hull=9907001 from=hangar", note])[0]["capital"][0], pr.PASS)

    def test_no_siege_is_na(self):
        self.assertEqual(run([])[0]["capital"][0], pr.NA)


class Output(unittest.TestCase):
    def test_one_line_each_and_status(self):
        log = pr.Log()
        log.feed([act("H", "launch probe from 1"), act("H", "field on 2"),
                  "[warp_bubble] H: held", "[warp_bubble] H: free after 1 s"])
        self.assertEqual(len(pr.report(log)), 9)
        self.assertEqual(pr.status(log), 0)
        self.assertEqual(pr.status(log, strict=True), 1)
        self.assertEqual(pr.status(pr.Log()), 1)


if __name__ == "__main__":
    unittest.main()
