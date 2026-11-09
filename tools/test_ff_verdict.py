#!/usr/bin/env python3
"""Tests for tools/ff_verdict.py: invented `[ff]` lines in the shape of specs/ff-now.md section 2.

    python3 -I tools/test_ff_verdict.py
"""
import importlib.util
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("ff_verdict", os.path.join(HERE, "ff_verdict.py"))
fv = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fv)

AFFINE = "[ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=0"
LOCKSTEP = "[ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0"
STALL = "[ff] stall real_s=10.0 virt_s=3601.25 running=bot-a-3,bot-b-1 links=127.0.0.1:40122(c2s 812/790)"
WATCHDOG = "[ff] watchdog exit real_s=60.0 virt_s=3601.25"


class Parse(unittest.TestCase):
    def test_affine_line(self):
        r = fv.parse([AFFINE])
        self.assertEqual(r.summary["mode"], "affine")
        self.assertEqual(r.summary["k"], "20")
        self.assertEqual(r.summary["ratio"], "19.7")
        self.assertEqual(fv.gate(r)[0], fv.OK)
        self.assertEqual(fv.describe(r), "ff: mode=affine ratio=19.7 stalls=0")

    def test_lockstep_line(self):
        r = fv.parse([LOCKSTEP])
        self.assertEqual(r.summary["mode"], "lockstep")
        self.assertEqual(fv.describe(r), "ff: mode=lockstep ratio=405.8 stalls=0")

    def test_stall_event_line_alone_is_counted_not_a_summary(self):
        r = fv.parse([STALL])
        self.assertEqual(r.stall_events, 1)
        self.assertIsNone(r.summary)
        self.assertEqual(fv.gate(r)[0], fv.OK)

    def test_watchdog_exit_is_na(self):
        r = fv.parse([AFFINE, WATCHDOG])
        self.assertTrue(r.watchdog)
        self.assertEqual(fv.gate(r)[0], fv.NA)

    def test_stalls_on_the_last_summary_is_na(self):
        r = fv.parse([AFFINE, AFFINE.replace("stalls=0", "stalls=2")])
        self.assertEqual(fv.gate(r)[0], fv.NA)

    def test_only_the_last_summary_counts(self):
        r = fv.parse([AFFINE.replace("stalls=0", "stalls=2"), STALL, AFFINE])
        self.assertEqual(fv.gate(r)[0], fv.OK)

    def test_unknown_key_is_ignored(self):
        r = fv.parse([AFFINE + " future_key=7 another=x"])
        self.assertEqual(r.summary["future_key"], "7")
        self.assertEqual(fv.gate(r)[0], fv.OK)

    def test_prefix_before_the_marker_is_fine(self):
        r = fv.parse(["12:00:01 INFO " + LOCKSTEP])
        self.assertEqual(r.summary["mode"], "lockstep")

    def test_no_ff_line_is_none(self):
        self.assertIsNone(fv.parse(["[world] tick health: 40 ticks", "[soc] bots 9", ""]))
        self.assertIsNone(fv.describe(None))
        self.assertIsNone(fv.banner(None))
        self.assertEqual(fv.gate(None)[0], fv.OK)

    def test_banner_names_the_reason_when_na(self):
        b = fv.banner(fv.parse([LOCKSTEP, WATCHDOG]))
        self.assertIn("ff: mode=lockstep", b)
        self.assertIn("verdict n/a", b)

    def test_junk_stalls_value_does_not_crash(self):
        r = fv.parse([AFFINE.replace("stalls=0", "stalls=x")])
        self.assertEqual(fv.gate(r)[0], fv.OK)

    def test_parse_paths_skips_missing_and_none(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "server.log")
            with open(p, "w") as f:
                f.write("noise\n" + LOCKSTEP + "\n")
            r = fv.parse_paths([None, os.path.join(d, "absent.log"), p])
            self.assertEqual(r.summary["mode"], "lockstep")
            self.assertIsNone(fv.parse_paths([None, os.path.join(d, "absent.log")]))


if __name__ == "__main__":
    unittest.main()
