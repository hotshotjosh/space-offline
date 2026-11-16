#!/usr/bin/env python3
"""Tests for tools/society-report.py (SOC-REPORT): invented log lines only.

    tools/analysis.sh python3 tools/test_society_report.py
"""
import contextlib
import importlib.util
import io
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("society_report", os.path.join(HERE, "society-report.py"))
sr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sr)


def log_of(lines):
    log = sr.Log()
    log.feed(lines)
    return log


def summary(corps=3, al=2, blue=1, red=1, day=None):
    d = f" day {day}" if day is not None else ""
    return f"[soc] bots 9 online 9 mix a=9 switches_per_bot_day 0.10 flap_max 1.00 corps {corps} alliances {al} blue {blue} red {red}{d}"


def stance(o, t, a, b):
    return f"[soc] Bot One corp stance {o} -> {t}: {a} -> {b} (R 5.0)"


def rows_of(res):
    return {n: s for n, s, _ in res[2]}


GC_OK = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1),
         "[soc] relation loss victim=1 killer=2 before=3.0 after=1.5", summary(day=2)]


class Gc(unittest.TestCase):
    def test_pass(self):
        res = sr.gate_gc(log_of(GC_OK))
        self.assertEqual(res[0], sr.PASS, res)

    def test_off_is_na(self):
        self.assertEqual(sr.gate_gc(log_of(["[doc] roll x"]))[0], sr.NA)

    def test_corps_alliances_blue_red_fail(self):
        for kw, name in [({"corps": 2}, "corps"), ({"al": 1}, "alliances"), ({"blue": 0}, "blue pair"), ({"red": 0}, "red pair")]:
            res = sr.gate_gc(log_of(GC_OK[:-1] + [summary(day=2, **kw)] if name in ("corps", "alliances") else
                                    [summary(day=0, **kw), summary(day=1, **kw)] + GC_OK[3:4]))
            self.assertEqual(res[0], sr.FAIL, name)
            self.assertEqual(rows_of(res)[name], sr.FAIL, name)

    def test_peak_blue_counts_even_if_the_last_summary_lost_it(self):
        res = sr.gate_gc(log_of([summary(blue=2), summary(blue=0)]))
        self.assertEqual(rows_of(res)["blue pair"], sr.PASS)

    def test_loss_pass_fail_na(self):
        self.assertEqual(rows_of(sr.gate_gc(log_of(GC_OK)))["loss moves R"], sr.PASS)
        flat = [summary(day=0), "[soc] relation loss victim=1 killer=2 before=3.0 after=3.0"]
        self.assertEqual(rows_of(sr.gate_gc(log_of(flat)))["loss moves R"], sr.FAIL)
        none = rows_of(sr.gate_gc(log_of([summary(day=0)])))
        self.assertEqual(none["loss moves R"], sr.NA)

    def test_na_criterion_means_the_gate_is_not_pass(self):
        res = sr.gate_gc(log_of([summary(day=0), summary(day=1)]))
        self.assertEqual(res[0], sr.NA)

    def test_flap_fail_pass_na_with_a_day_field(self):
        fast = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1), stance(1, 2, "blue", "red"),
                stance(1, 2, "red", "blue"), summary(day=1)]
        res = sr.gate_gc(log_of(fast))
        self.assertEqual(rows_of(res)["no flapping"], sr.FAIL)
        slow = [summary(day=0), stance(1, 2, "neutral", "blue"), summary(day=1), stance(1, 2, "blue", "red"),
                summary(day=3), stance(1, 2, "red", "blue"), summary(day=4)]
        self.assertEqual(rows_of(sr.gate_gc(log_of(slow)))["no flapping"], sr.PASS)

    def test_a_neutral_gap_still_counts_as_a_flip(self):
        lines = [summary(day=1), stance(1, 2, "neutral", "blue"), stance(1, 2, "blue", "neutral"),
                 stance(1, 2, "neutral", "red"), stance(1, 2, "red", "blue"), summary(day=1)]
        self.assertEqual(rows_of(sr.gate_gc(log_of(lines)))["no flapping"], sr.FAIL)

    def test_flap_clock_from_ticks_and_without_any_clock(self):
        lines = [summary(), stance(1, 2, "neutral", "blue"), summary(), stance(1, 2, "blue", "red"),
                 summary(), stance(1, 2, "red", "blue"), summary()]
        log = log_of(lines)
        self.assertEqual(rows_of(sr.gate_gc(log))["no flapping"], sr.NA)
        # 60s ticks, a 600s game day: two flips three ticks apart are inside one day.
        self.assertEqual(rows_of(sr.gate_gc(log, 600.0, 60.0))["no flapping"], sr.FAIL)
        self.assertEqual(rows_of(sr.gate_gc(log, 60.0, 60.0))["no flapping"], sr.PASS)

    def test_no_stance_lines_means_flaps_na_and_gc_not_pass(self):
        lines = [summary(day=0), "[soc] relation loss victim=1 killer=2 before=3.0 after=1.5", summary(day=1)]
        res = sr.gate_gc(log_of(lines))
        self.assertEqual(rows_of(res)["no flapping"], sr.NA)
        self.assertEqual(res[0], sr.NA)

    def test_a_prefixed_line_is_read(self):
        log = log_of(["12:00:01 " + summary(corps=4)])
        self.assertEqual(log.summaries[0][0], 4)


SOV_OK = [
    "[sov] hub 10 launched in 5 by 77 at (0,0,0)",
    "[sov] hub 11 launched in 6 by 78 at (0,0,0)",
    "[sov] hub 10 in 5 captured by alliance 100 (corp 1)",
    "[sov] campaign in 5 begins on hub 10",
    "[sov] command node 3 appears in 5 for the campaign on hub 10",
    "[sov] a node of hub 10 was taken by the Attackers: the bar is X",
    "[sov] hub 12 in 5 captured by alliance 200 (corp 2)",
]


class Gd(unittest.TestCase):
    def test_pass(self):
        res = sr.gate_gd(log_of(SOV_OK))
        self.assertEqual(res[0], sr.PASS, res)

    def test_off_is_na(self):
        self.assertEqual(sr.gate_gd(log_of([summary()]))[0], sr.NA)

    def test_claims_fail(self):
        res = sr.gate_gd(log_of(SOV_OK[:1] + SOV_OK[3:6]))
        self.assertEqual(rows_of(res)["claims"], sr.FAIL)
        self.assertEqual(res[0], sr.FAIL)

    def test_contested_fail_when_no_node_was_taken(self):
        res = sr.gate_gd(log_of([l for l in SOV_OK if "was taken" not in l]))
        self.assertEqual(rows_of(res)["contested campaign"], sr.FAIL)

    def test_a_node_on_another_hub_is_not_a_fight_for_this_campaign(self):
        lines = [l.replace("taken by", "taken by").replace("node of hub 10", "node of hub 99") for l in SOV_OK]
        self.assertEqual(rows_of(sr.gate_gd(log_of(lines)))["contested campaign"], sr.FAIL)

    def test_owner_change_fail_and_na(self):
        same = [l.replace("alliance 200", "alliance 100") for l in SOV_OK]
        self.assertEqual(rows_of(sr.gate_gd(log_of(same)))["change of owner"], sr.FAIL)
        nocap = [l for l in SOV_OK if "captured" not in l]
        res = sr.gate_gd(log_of(nocap))
        self.assertEqual(rows_of(res)["change of owner"], sr.NA)
        self.assertEqual(res[0], sr.NA)

    def test_owner_change_is_per_system(self):
        lines = ["[sov] hub 1 in 5 captured by alliance 100 (corp 1)", "[sov] hub 2 in 6 captured by alliance 200 (corp 2)"]
        self.assertEqual(sr.owner_changes([("1", "5", "100"), ("2", "6", "200")]), 0)
        self.assertEqual(sr.owner_changes([("1", "5", "100"), ("2", "5", "200")]), 1)
        self.assertEqual(len(log_of(lines).captured), 2)


ADS = [
    "[soc] Invented One corp advert posted corp=11",
    "[soc] Invented Two corp advert posted corp=12",
    "[soc] Invented One corp advert removed corp=11",
    "[soc] Invented Three corp applied corp=12 via=finder",
    "[soc] Invented Four corp apply 12: ok",
    "[soc] Invented Five applied corp=11 via=finder",
]


class Adverts(unittest.TestCase):
    def test_counts_are_read_with_or_without_the_corp_word(self):
        c = sr.info(log_of(ADS))
        self.assertEqual(c, {"adverts posted": 2, "adverts removed": 1, "corps advertising": 2, "applied via finder": 2})

    def test_no_such_lines_count_zero(self):
        c = sr.info(log_of(GC_OK))
        self.assertEqual(sum(c.values()), 0)

    def test_the_counts_never_change_a_verdict(self):
        res = sr.judge(log_of(GC_OK + ADS))
        self.assertEqual(res["GC"][0], sr.PASS)
        self.assertEqual({g: v[0] for g, v in sr.judge(log_of(GC_OK)).items()}, {g: v[0] for g, v in res.items()})

    def test_the_text_report_ends_with_the_info_line(self):
        code, text = Report().run_main(GC_OK + ADS, SOV_OK)
        self.assertEqual(code, 0, text)
        self.assertIn("info (not a gate): adverts posted 2, adverts removed 1, corps advertising 2, applied via finder 2", text)


class Report(unittest.TestCase):
    def run_main(self, bots, server, *extra):
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for name, lines in (("bots.log", bots), ("server.log", server)):
                p = os.path.join(d, name)
                with open(p, "w") as f:
                    f.write("\n".join(lines) + "\n")
                paths.append(p)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = sr.main(paths + list(extra))
            return code, out.getvalue()

    def test_exit_by_fail_and_both_gates_listed(self):
        code, text = self.run_main(GC_OK, SOV_OK)
        self.assertEqual(code, 0, text)
        self.assertIn("GC  PASS", text)
        self.assertIn("GD  PASS", text)
        code, text = self.run_main(GC_OK, SOV_OK[:1])
        self.assertEqual(code, 1)
        self.assertIn("GD  FAIL", text)

    def test_na_only_exits_zero(self):
        code, text = self.run_main(["nothing"], ["nothing"])
        self.assertEqual(code, 0)
        self.assertIn("GC  n/a", text)
        self.assertIn("0 PASS, 0 FAIL, 2 n/a", text)

    def test_json(self):
        code, text = self.run_main(GC_OK, SOV_OK, "--json")
        self.assertEqual(code, 0)
        self.assertIn('"GC"', text)


FOUND_OK = "[soc] Founder1 corp found Invented Name [INV] 98000001: ok"
FOUND_REF = "[soc] Founder1 corp found Invented Name [INV]: refused CorpNameTaken"
PLEDGE = "[soc] Pledger1 corp pledge 1200 to 90000001: ok"


class Found(unittest.TestCase):
    run_main = Report.run_main

    def test_the_lines_are_read(self):
        log = log_of([FOUND_OK, FOUND_REF, PLEDGE, "12:00 " + PLEDGE])
        self.assertEqual(log.founded, [("Founder1", "Invented Name [INV] 98000001")])
        self.assertEqual(log.found_refused, [("Founder1", "Invented Name [INV]", "CorpNameTaken")])
        self.assertEqual(log.pledges, [("Pledger1", 1200.0, "90000001"), ("Pledger1", 1200.0, "90000001")])

    def test_pass_needs_a_found_and_a_corp_in_the_summary(self):
        self.assertEqual(sr.gate_found(log_of([PLEDGE, FOUND_OK, summary(corps=1)]))[0], sr.PASS)
        self.assertEqual(sr.gate_found(log_of([PLEDGE, FOUND_OK, summary(corps=0)]))[0], sr.NA)
        self.assertEqual(sr.gate_found(log_of([FOUND_OK]))[0], sr.NA)

    def test_a_refusal_fails_unless_a_corp_was_founded(self):
        st, detail, _ = sr.gate_found(log_of([FOUND_REF, summary(corps=0)]))
        self.assertEqual(st, sr.FAIL)
        self.assertIn("CorpNameTaken", detail)
        self.assertEqual(sr.gate_found(log_of([FOUND_REF, FOUND_OK, summary(corps=1)]))[0], sr.PASS)

    def test_pledges_alone_are_na_never_a_pass(self):
        self.assertEqual(sr.gate_found(log_of([PLEDGE, summary(corps=0)]))[0], sr.NA)

    def test_found_sits_between_gc_and_gd_and_fails_the_exit(self):
        res = sr.judge(log_of(GC_OK + SOV_OK + [PLEDGE, FOUND_REF]))
        self.assertEqual(list(res), ["GC", "FOUND", "GD"])
        code, text = self.run_main(GC_OK + [PLEDGE, FOUND_REF], SOV_OK)
        self.assertEqual(code, 1)
        self.assertIn("FOUND  FAIL", text)

    def test_a_soak_without_founding_lines_reports_as_before(self):
        res = sr.judge(log_of(GC_OK + SOV_OK))
        self.assertEqual(list(res), ["GC", "GD"])
        code, text = self.run_main(GC_OK, SOV_OK)
        self.assertNotIn("FOUND", text)
        self.assertIn("gates: 2 PASS, 0 FAIL, 0 n/a", text)
        self.assertEqual(code, 0)


FF_CLEAN = "[ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0"
FF_STALLS = "[ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=3"
FF_DOG = "[ff] watchdog exit real_s=60.0 virt_s=3601.25"


class FastForward(unittest.TestCase):
    run_main = Report.run_main

    def test_a_run_without_ff_reports_exactly_as_before(self):
        code, text = self.run_main(GC_OK, SOV_OK)
        want = sr.render(sr.judge(log_of(GC_OK + SOV_OK))) + "\n" + sr.render_info(sr.info(log_of(GC_OK + SOV_OK))) + "\n"
        self.assertEqual(text, want)
        self.assertEqual(code, 0)
        self.assertNotIn("ff:", text)

    def test_a_clean_ff_line_keeps_the_old_verdict_and_prints_the_line(self):
        code, text = self.run_main(GC_OK, SOV_OK + [FF_CLEAN])
        self.assertEqual(code, 0, text)
        self.assertIn("ff: mode=lockstep ratio=405.8 stalls=0", text)
        self.assertIn("GC  PASS", text)
        self.assertIn("GD  PASS", text)

    def test_a_watchdog_exit_forces_n_a_even_over_a_fail(self):
        code, text = self.run_main(GC_OK, SOV_OK[:1] + [FF_CLEAN, FF_DOG])
        self.assertEqual(code, 0, text)
        self.assertIn("GC  n/a", text)
        self.assertIn("GD  n/a", text)
        self.assertIn("0 PASS, 0 FAIL, 2 n/a", text)

    def test_stalls_on_the_last_summary_force_n_a(self):
        code, text = self.run_main(GC_OK, SOV_OK + [FF_STALLS])
        self.assertEqual(code, 0)
        self.assertIn("2 n/a", text)

    def test_json_carries_the_ff_line_only_for_an_ff_run(self):
        _, plain = self.run_main(GC_OK, SOV_OK, "--json")
        self.assertNotIn('"ff"', plain)
        _, ff = self.run_main(GC_OK, SOV_OK + [FF_DOG], "--json")
        self.assertIn('"ff"', ff)
        self.assertIn('"status": "n/a"', ff)


def pair(v, t, r, at, touched="none", half=10.0, stance="neutral"):
    return (
        f"[soc] relation pair viewer={v} target={t} r={r} stance={stance} day={int(at)} "
        f"t={at:.3f} half={half:.3f} touched={touched}"
    )


def engage(stance):
    return f"[soc] engage shooter=11 target=12 stance={stance}"


def betrayal(stance, after="6.00"):
    return f"[soc] betrayal victim=11 killer=12 before=6.00 after={after} stance={stance}"


class Rel(unittest.TestCase):
    """#95 SOAK-REL: invented lines; the half-life and stamps come from the lines."""

    def rel(self, lines):
        return sr.gate_rel(log_of(lines))

    def test_no_lines_is_na_and_the_gate_is_left_out_of_judge(self):
        self.assertEqual(self.rel(["[doc] roll x"])[0], sr.NA)
        self.assertNotIn("REL", sr.judge(log_of(GC_OK)))

    def test_friendly_engage(self):
        self.assertEqual(rows_of(self.rel([engage("hostile"), engage("wary")]))["no friendly engage"], sr.PASS)
        for st in ("friendly", "allied"):
            res = self.rel([engage("hostile"), engage(st)])
            self.assertEqual(rows_of(res)["no friendly engage"], sr.FAIL)
            self.assertEqual(res[0], sr.FAIL)
        self.assertEqual(rows_of(self.rel([betrayal("hostile")]))["no friendly engage"], sr.NA)

    def test_betrayal_sours(self):
        # The stance is judged; R may stay where it was (after 6.00 here) and still pass.
        self.assertEqual(rows_of(self.rel([betrayal("hostile")]))["betrayal sours"], sr.PASS)
        res = self.rel([betrayal("hostile"), betrayal("wary", "-2.0")])
        self.assertEqual(rows_of(res)["betrayal sours"], sr.FAIL)
        self.assertEqual(rows_of(self.rel([engage("hostile")]))["betrayal sours"], sr.NA)
        nostance = "[soc] betrayal victim=1 killer=2 before=1 after=-9"
        self.assertEqual(rows_of(self.rel([nostance]))["betrayal sours"], sr.NA)

    def test_memory_fades(self):
        # Ten days at half 10 halves it: 8 -> 4 passes, 8 -> 4.4 passes on the 10% slack, 8 -> 5 fails.
        # The later line's touched stamp is before t0 (day 1), so only the decay moved it.
        ok = [pair(1, 2, 8, 1.0, 0.5), pair(1, 2, 4.4, 11.0, 0.5)]
        self.assertEqual(rows_of(self.rel(ok))["memory fades"], sr.PASS)
        self.assertEqual(rows_of(self.rel([pair(1, 2, 8, 1.0, 0.5), pair(1, 2, 5, 11.0, 0.5)]))["memory fades"], sr.FAIL)
        neg = [pair(1, 2, -8, 1.0, 0.5), pair(1, 2, -4, 11.0, 0.5)]
        self.assertEqual(rows_of(self.rel(neg))["memory fades"], sr.PASS)

    def test_the_half_life_is_the_lines_own_and_k_is_fractional(self):
        # Half 4 days, 2.5 days apart: 0.5^(2.5/4) = 0.648 (x1.1 slack: 5.70); 8 -> 5.6 passes, 8 -> 5.8 fails.
        a = pair(1, 2, 8, 1.0, 0.5, half=4.0)
        self.assertEqual(rows_of(self.rel([a, pair(1, 2, 5.6, 3.5, 0.5, half=4.0)]))["memory fades"], sr.PASS)
        self.assertEqual(rows_of(self.rel([a, pair(1, 2, 5.8, 3.5, 0.5, half=4.0)]))["memory fades"], sr.FAIL)

    def test_a_touched_pair_is_skipped(self):
        # touched at or after t0 (something was added in between), or none: nothing to judge.
        for touched in (1.0, 5.0, "none"):
            lines = [pair(1, 2, 1, 1.0, 0.5), pair(1, 2, 3, 11.0, touched)]
            self.assertEqual(rows_of(self.rel(lines))["memory fades"], sr.NA, touched)
        # Touched strictly before t0 is judged (and 1 -> 3 grows: FAIL).
        lines = [pair(1, 2, 1, 1.0, 0.5), pair(1, 2, 3, 11.0, 0.9)]
        self.assertEqual(rows_of(self.rel(lines))["memory fades"], sr.FAIL)

    def test_memory_needs_two_lines_a_later_time(self):
        self.assertEqual(rows_of(self.rel([pair(1, 2, 8, 1.0, 0.5)]))["memory fades"], sr.NA)
        same = [pair(1, 2, 8, 3.0, 0.5), pair(1, 2, 4, 3.0, 0.5)]
        self.assertEqual(rows_of(self.rel(same))["memory fades"], sr.NA)

    def test_rel_sits_between_gc_and_gd_and_fails_the_exit(self):
        res = sr.judge(log_of(GC_OK + [engage("allied")]))
        self.assertEqual(list(res), ["GC", "REL", "GD"])
        self.assertIn("      FAIL  no friendly engage", sr.render(res))


if __name__ == "__main__":
    unittest.main()
