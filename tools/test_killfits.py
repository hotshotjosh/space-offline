"""Tests for tools/killfits.py --check's summary, on invented rows.

Run::

    python3 -I tools/test_killfits.py
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("killfits", os.path.join(HERE, "killfits.py"))
kf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(kf)

# hull count class reasons builder hull_name, all invented.
CHECK = (
    "#hull\tcount\tclass\treasons\tbuilder\thull_name\n"
    "9001\t30\tFrigate\t-\tok\tAlpha\n"
    "9001\t10\tFrigate\thigh_slots,turret_hardpoints\tfails_cpu,_power_or_capacitor\tAlpha\n"
    "9002\t50\tCruiser\tcalibration\tok\tBeta\n"
    "9002\t5\tCruiser\tcalibration,rig_size,offline_cpu\tno_weapon_or_combat_drone\tBeta\n"
    "9003\t7\tgroup99\thull_restriction\tok\tGamma\n"
    "9004\t20\tFrigate\toffline_cpu,offline_power\tok\tDelta\n"
    "9004\t3\tFrigate\tno_engine\tok\tDelta\n"
)


class Parse(unittest.TestCase):
    def test_rows(self):
        rows = kf.parse_check(CHECK)
        self.assertEqual(len(rows), 7)
        self.assertEqual(rows[0]["reasons"], [])
        self.assertEqual(rows[1]["reasons"], ["high_slots", "turret_hardpoints"])
        self.assertEqual(kf.refusals(rows[3]), ["calibration", "rig_size"])
        self.assertEqual(kf.refusals(rows[5]), [])
        self.assertEqual(rows[2]["cls"], "Cruiser")

    def test_loader_noise_before_the_header_is_skipped(self):
        rows = kf.parse_check("[authored] x: compiled-in (6 rows)\n" + CHECK)
        self.assertEqual(len(rows), 7)

    def test_a_bad_line_is_named(self):
        with self.assertRaises(ValueError):
            kf.parse_check("9001\t3\tFrigate\n")


class Summary(unittest.TestCase):
    def setUp(self):
        self.text = kf.summarize_check(kf.parse_check(CHECK), top=2)

    def test_totals(self):
        self.assertIn("fits checked 7  killmails 125", self.text)
        self.assertIn("REFUSED by the server's fitting rules: 4 fits, 72 killmails", self.text)

    def test_by_reason_counts_fits_and_killmails(self):
        self.assertIn("calibration\t2\t55", self.text)
        self.assertIn("high_slots\t1\t10", self.text)
        self.assertIn("rig_size\t1\t5", self.text)

    def test_the_budget_is_not_a_refusal(self):
        part = self.text.split("refusals by reason")[1].split("refused for exactly")[0]
        self.assertNotIn("offline", part)
        self.assertNotIn("no_engine", part)
        tail = self.text.split("NOT refusals")[1].split("the doctrine builder")[0]
        self.assertIn("offline_cpu\t2\t25", tail)
        self.assertIn("offline_power\t1\t20", tail)
        self.assertIn("no_engine\t1\t3", tail)

    def test_exactly_one_reason(self):
        part = self.text.split("refused for exactly one reason:")[1].split("refusals by hull class:")[0]
        self.assertIn("calibration\t1", part)
        self.assertIn("hull_restriction\t1", part)
        self.assertNotIn("high_slots", part)

    def test_by_class(self):
        self.assertIn("Cruiser\t2\t2\t55", self.text)
        self.assertIn("Frigate\t4\t1\t10", self.text)
        self.assertIn("group99\t1\t1\t7", self.text)

    def test_top_is_cut_and_ordered_by_killmails(self):
        part = self.text.split("top 2 refused fits")[1].split("top 2 hulls")[0]
        lines = [l for l in part.splitlines() if l[:1].isdigit()]
        self.assertEqual(len(lines), 2)
        self.assertTrue(lines[0].startswith("1\tBeta\tCruiser\t50\t"))
        self.assertTrue(lines[1].startswith("2\tAlpha\tFrigate\t10\t"))

    def test_top_hulls(self):
        part = self.text.split("top 2 hulls")[1]
        self.assertIn("1\tBeta\t2\t55", part)

    def test_builder_verdicts(self):
        self.assertIn("ok\t5", self.text)

    def test_builder_verdicts_carry_their_killmails(self):
        t = kf.summarize_check(kf.parse_check(
            "9001\t3\tFrigate\t-\tok\tAlpha\n"
            "9002\t4\tFrigate\t-\tok\tBeta\n"
            "9003\t6\tFrigate\t-\tthe_server_refuses_the_fit\tGamma\n"))
        self.assertIn("verdict\tfits\tkillmails", t)
        self.assertIn("ok\t2\t7\n", t)
        self.assertIn("the_server_refuses_the_fit\t1\t6\n", t)

    def test_the_level_is_printed_with_its_note(self):
        text = "#level 4\n" + CHECK
        self.assertEqual(kf.parse_level(text), 4)
        self.assertIsNone(kf.parse_level(CHECK))
        s = kf.summarize_check(kf.parse_check(text), level=kf.parse_level(text))
        self.assertIn("skill level of the budget check: 4", s)
        self.assertIn("overstates", s)
        self.assertNotIn("skill level of", self.text)

    def test_nothing_refused(self):
        t = kf.summarize_check(kf.parse_check("9001\t3\tFrigate\t-\tok\tAlpha\n"))
        self.assertIn("REFUSED by the server's fitting rules: 0 fits", t)


class Paths(unittest.TestCase):
    def test_inside_the_repo_is_relative_to_it(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(kf.repo_path("out/bots/fits.tsv", d), os.path.join("out", "bots", "fits.tsv"))
            self.assertEqual(kf.repo_path(os.path.join(d, "out", "x"), d), os.path.join("out", "x"))

    def test_outside_the_repo_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            for p in ("../elsewhere/fits.tsv", "/nonexistent/probe/fits.tsv", "out/../../x"):
                with self.assertRaises(ValueError):
                    kf.repo_path(p, d)


class FromFile(unittest.TestCase):
    def test_from_writes_the_check_table_and_prints(self):
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "in.tsv")
            with open(src, "w", encoding="utf-8") as f:
                f.write(CHECK)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = kf.main(["--check", "--from", src, "--out", os.path.join(d, "o")])
            self.assertEqual(rc, 0)
            self.assertIn("REFUSED by the server's fitting rules: 4 fits", buf.getvalue())
            with open(os.path.join(d, "o", "fits-check.tsv"), encoding="utf-8") as f:
                self.assertEqual(f.read(), CHECK)


if __name__ == "__main__":
    unittest.main()
