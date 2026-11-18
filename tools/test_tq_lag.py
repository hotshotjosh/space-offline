#!/usr/bin/env python3
"""Tests for tools/tq-lag.py, on invented builds and times.

    python3 -I tools/test_tq_lag.py
"""

from __future__ import annotations

import importlib.util
import io
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("tq_lag", os.path.join(HERE, "tq-lag.py"))
tq = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(tq)

HEADER = "\t".join(tq.COLUMNS)


def line(build, ln="LX", tier="T1", seen="-", by="-", imp="-", prov="-", served="-", pinned="-", ret="-", issue="-", note=""):
    return "\t".join([build, ln, tier, seen, by, imp, prov, served, pinned, ret, issue, note]) + "\n"


class Lag(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.tmp.name, "tq-builds.tsv")

    def tearDown(self):
        self.tmp.cleanup()

    def put(self, *rows, comment="# invented\n"):
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(comment + HEADER + "\n" + "".join(rows))

    def run_main(self, *args):
        out, err = io.StringIO(), io.StringIO()
        code = tq.main(["--file", self.path, *args], out=out, err=err)
        return code, out.getvalue(), err.getvalue()

    def test_lags_are_hours_from_seen(self):
        r = tq.read(self.path_with(line("9000001", seen="2000-01-01T00:00:00Z", prov="2000-01-01T02:00:00Z", served="2000-01-01T10:30:00Z")))[1][0]
        self.assertEqual(tq.login_lag(r), 2.0)
        self.assertEqual(tq.build_lag(r), 10.5)
        r["provisional_utc"] = "-"
        self.assertEqual(tq.login_lag(r), 10.5)
        r["seen_utc"] = "-"
        self.assertIsNone(tq.build_lag(r))

    def path_with(self, *rows):
        self.put(*rows)
        return self.path

    def test_check_is_green_on_time_and_red_over_target(self):
        self.put(line("9000001", tier="T1", seen="2000-01-01T00:00:00Z"))
        self.assertEqual(self.run_main("--check", "--now", "2000-01-01T23:00:00Z")[0], 0)
        code, out, _ = self.run_main("--check", "--now", "2000-01-02T01:00:00Z")
        self.assertEqual(code, 3)
        self.assertIn("OVER", out)

    def test_t0_has_a_two_hour_provisional_target(self):
        self.put(line("9000001", tier="T0", seen="2000-01-01T00:00:00Z"))
        self.assertEqual(self.run_main("--check", "--now", "2000-01-01T01:59:00Z")[0], 0)
        code, out, _ = self.run_main("--check", "--now", "2000-01-01T03:00:00Z")
        self.assertEqual(code, 3)
        self.assertIn("provisional target 2 h", out)
        self.put(line("9000001", tier="T0", seen="2000-01-01T00:00:00Z", prov="2000-01-01T01:00:00Z"))
        self.assertEqual(self.run_main("--check", "--now", "2000-01-01T03:00:00Z")[0], 0)

    def test_a_served_build_and_a_build_without_a_target_never_fail(self):
        self.put(line("9000001", tier="T1", seen="2000-01-01T00:00:00Z", served="2000-01-05T00:00:00Z"),
                 line("9000002", tier="T3", seen="2000-01-01T00:00:00Z"),
                 line("9000003", tier="-", seen="2000-01-01T00:00:00Z"),
                 line("9000004", tier="T2", seen="-"))
        code, out, _ = self.run_main("--check", "--now", "2001-01-01T00:00:00Z")
        self.assertEqual(code, 0, out)
        self.assertIn("no target", out)

    def test_record_sets_a_cell_keeps_the_comments_and_the_order(self):
        self.put(line("9000002", seen="2000-01-02T00:00:00Z"), line("9000001", seen="2000-01-01T00:00:00Z"))
        code, out, _ = self.run_main("--record", "served", "9000001", "--at", "2000-01-03T00:00:00Z")
        self.assertEqual(code, 0, out)
        with open(self.path, encoding="utf-8") as f:
            text = f.read()
        self.assertTrue(text.startswith("# invented\n" + HEADER))
        rows = tq.read(self.path)[1]
        self.assertEqual([r["build"] for r in rows], ["9000001", "9000002"])
        self.assertEqual(rows[0]["served_utc"], "2000-01-03T00:00:00Z")

    def test_record_never_moves_a_cell_backwards(self):
        self.put(line("9000001", seen="2000-01-02T00:00:00Z"))
        code, _, err = self.run_main("--record", "seen", "9000001", "--at", "2000-01-01T00:00:00Z")
        self.assertEqual(code, 2)
        self.assertIn("backwards", err)
        self.assertEqual(tq.read(self.path)[1][0]["seen_utc"], "2000-01-02T00:00:00Z")
        self.assertEqual(self.run_main("--record", "seen", "9000001", "--at", "2000-01-03T00:00:00Z")[0], 0)

    def test_record_starts_a_row_only_with_seen(self):
        self.put()
        self.assertEqual(self.run_main("--record", "served", "9000009", "--at", "2000-01-01T00:00:00Z")[0], 2)
        code, _, _ = self.run_main("--record", "seen", "9000009", "--at", "2000-01-01T00:00:00Z", "--line", "LX", "--tier", "T2", "--issue", "#1")
        self.assertEqual(code, 0)
        r = tq.read(self.path)[1][0]
        self.assertEqual((r["build"], r["line"], r["tier"], r["issue"], r["served_utc"]), ("9000009", "LX", "T2", "#1", "-"))

    def test_record_refuses_bad_input(self):
        self.put(line("9000001"))
        self.assertEqual(self.run_main("--record", "nope", "9000001")[0], 2)
        self.assertEqual(self.run_main("--record", "seen", "abc")[0], 2)
        self.assertEqual(self.run_main("--record", "seen", "9000001", "--at", "yesterday")[0], 2)
        self.assertEqual(self.run_main("--record", "seen", "9000001", "--at", "2000-13-45T99:00:00Z")[0], 2)
        self.assertEqual(self.run_main("--record", "seen", "9000001", "--tier", "T9")[0], 2)

    def test_a_malformed_file_exits_1(self):
        for bad in ("nonsense\n", HEADER + "\n9000001\tLX\n", HEADER + "\n" + line("x1"),
                    HEADER + "\n" + line("9000001", tier="T7"), HEADER + "\n" + line("9000001", seen="2000-01-01"),
                    HEADER + "\n" + line("9000001", seen="2000-13-45T99:00:00Z"),
                    HEADER + "\n" + line("9000001") + line("9000001")):
            with open(self.path, "w", encoding="utf-8") as f:
                f.write(bad)
            self.assertEqual(self.run_main("--check")[0], 1, bad)
        self.assertEqual(tq.main(["--file", os.path.join(self.tmp.name, "absent"), "--check"], out=io.StringIO(), err=io.StringIO()), 1)

    def test_usage_errors_exit_2(self):
        self.put()
        self.assertEqual(self.run_main()[0], 2)
        self.assertEqual(self.run_main("--check", "--md")[0], 2)
        self.assertEqual(self.run_main("--bogus")[0], 2)
        self.assertEqual(self.run_main("--check", "--now", "soon")[0], 2)

    def test_the_block_names_the_builds_the_lags_and_the_medians(self):
        self.put(line("9000001", ln="LX", tier="T1", seen="2000-01-01T00:00:00Z", served="2000-01-01T10:00:00Z", pinned="2000-01-01T11:00:00Z"),
                 line("9000002", ln="LX", tier="T1", seen="2000-01-02T00:00:00Z", served="2000-01-02T20:00:00Z"),
                 line("9000003", ln="LY", tier="T2", seen="2000-01-03T00:00:00Z", note="[A] a guess"))
        code, out, _ = self.run_main("--md")
        self.assertEqual(code, 0)
        self.assertIn("## Tracking TQ", out)
        self.assertIn("TQ build **9000003 (LY)** · served **9000002 (LX)** · pinned **9000001 (LX)**", out)
        self.assertIn("| 9000002 | T1 | 20.0 | 20.0 |", out)
        self.assertIn("| 9000003 | T2 | - | - |", out)
        self.assertIn("T1 15.0 h (2)", out)
        self.assertIn("1 build served since the last re-pin", out)
        self.assertIn("tracker not asked", out)
        self.assertNotIn("tracker not asked: 0", out)
        self.assertIn("assumptions", out)

    def test_the_block_counts_intake_per_area_from_a_file_and_never_prints_a_zero(self):
        self.put(line("9000001", seen="2000-01-01T00:00:00Z"))
        tsv = os.path.join(self.tmp.name, "issues.tsv")
        with open(tsv, "w", encoding="utf-8") as f:
            f.write("issue\tready\tprio\tarea\tclient\tprs\ttitle\n#1\tyes\t1\tcontent\tno\t-\tx\n#2\tno\t2\tcontent\tno\t-\ty\n#3\tno\t2\tserver\tno\t-\tz\n")
        out = self.run_main("--md", "--intake-tsv", tsv)[1]
        self.assertIn("content 2, server 1", out)
        code, out, err = self.run_main("--md", "--intake-tsv", os.path.join(self.tmp.name, "none.tsv"))
        self.assertIn("tracker not asked", out)
        self.assertIn("cannot read --intake-tsv", err)

    def test_an_empty_table_still_renders(self):
        self.put()
        out = self.run_main("--md")[1]
        self.assertIn("none", out)
        self.assertIn("no served build yet", out)


class Seed(unittest.TestCase):
    def test_the_committed_table_parses_and_marks_its_guesses(self):
        _, rows = tq.read(os.path.join(os.path.dirname(HERE), "docs", "status", "tq-builds.tsv"))
        self.assertTrue(rows)
        for r in rows:
            if r["seen_utc"] != "-":
                self.assertTrue("[A]" in r["note"] or r["seen_by"] != "-", r["build"])


if __name__ == "__main__":
    unittest.main()
