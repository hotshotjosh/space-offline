#!/usr/bin/env python3
"""tools/battle_summary.py: invented log lines only (issue #100, lane battle-100t)."""
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import battle_summary as bs  # noqa: E402


def tick(late_max=3.0, skipped=0, held_max=4.0, over=0):
    return ("[world] tick health: 40 ticks, late mean 0.5 ms max %.1f ms, over budget %d, "
            "world lock wait mean 0.1 ms max 2.0 ms, world lock held mean 1.0 ms max %.1f ms, "
            "work max 5.0 ms, skipped %d, work p95 3.0 ms, out 100 frames 20000 bytes, "
            "destiny 400 frames 80000 bytes parts 500 merged 100, slice_max 6.0 ms"
            % (late_max, over, held_max, skipped))


def lock(slice_wait=1.5, capped=0, site="invented.site"):
    return ("[world] tick lock: secs=10.0, clock held 40.0 ms occupancy 0.4%% payload held 10.0 ms "
            "occupancy 0.1%% turns 12 hold max 7.50 ms, payload wait mean 0.10 ms max 1.00 ms, "
            "total occupancy 0.5%%, read turns 3 held 1.0 ms (shared, not in occupancy) hold max 0.50 ms "
            "wait mean 0.01 ms max 0.10 ms, clock slice wait max %.1f ms (slice 3), waiters max 9, "
            "other held max 8.00 ms (%s) wait max 1.00 ms (x.y), gate raised 2 held back 1 turns max 3.0 ms "
            "capped %d, cancel total scans 0 scanned 0 0.00 ms" % (slice_wait, site, capped))


STAGES = "[world] tick stages: a mean 1.00 ms max 2.00 ms; clock cpu max 3.50 ms runq max 4.50 ms"
TURNS = "[turns] secs=10.0 turns=12 hold_mean=0.8ms hold_max=7.5ms wait_mean=0.1ms wait_max=3.0ms slowest=a.b"
SLOW = "[turn-slow] worst label=invented.Call hold=7.5ms wait=0.1ms"


def live(pilots, in_space):
    return "[live] day 1, tiers x, pilots %d (in space %d), NPCs 0" % (pilots, in_space)


def block(**kw):
    live_n = kw.pop("pilots", 100)
    return [tick(**{k: v for k, v in kw.items() if k in ("late_max", "skipped", "held_max", "over")}),
            lock(**{k: v for k, v in kw.items() if k in ("slice_wait", "capped", "site")}),
            STAGES, TURNS, SLOW, live(live_n, live_n)]


class Summarize(unittest.TestCase):
    def run_summary(self, lines, per_side=50, extra_server=(), bots=None, result=None, procs=None):
        d = tempfile.mkdtemp(prefix="test-battle-")
        sl = os.path.join(d, "server.log")
        Path(sl).write_text("\n".join(list(lines) + list(extra_server)) + "\n")
        argv = ["summarize", "--server-log", sl, "--per-side", str(per_side), "--out", os.path.join(d, "out")]
        if bots:
            Path(d, "bots.log").write_text(bots)
            argv += ["--bots-log", os.path.join(d, "bots.log")]
        if result:
            Path(d, "result.json").write_text(json.dumps(result))
            argv += ["--result", os.path.join(d, "result.json")]
        if procs:
            Path(d, "procs.tsv").write_text(procs)
            argv += ["--procs", os.path.join(d, "procs.tsv")]
        self.assertEqual(bs.main(argv), 0)
        return Path(d, "out")

    def test_a_clean_run_passes_and_writes_no_stall_files(self):
        lines = ["boot line"] + block() + block() + block()
        out = self.run_summary(lines)
        text = (out / "summary.txt").read_text()
        self.assertIn("verdict: PASS", text)
        self.assertEqual(list((out / "stalls").iterdir()), [])
        rows = (out / "battle.tsv").read_text().splitlines()
        self.assertEqual(len(rows), 4)
        self.assertEqual(rows[1].split("\t")[bs.COLUMNS.index("pilots")], "100")

    def test_a_late_window_is_a_stall_with_its_block_and_site(self):
        lines = block() + block(late_max=2700.0, site="bad.site") + block()
        out = self.run_summary(lines)
        text = (out / "summary.txt").read_text()
        self.assertIn("verdict: STALLS", text)
        self.assertIn("2700.0 ms in window 1", text)
        self.assertIn("bad.site", text)
        self.assertIn("THE CLOCK DID NOT KEEP TIME", text)
        self.assertEqual([p.name for p in (out / "stalls").iterdir()], ["w1.txt"])
        blob = (out / "stalls" / "w1.txt").read_text()
        self.assertIn("late mean 0.5 ms max 2700.0 ms", blob)
        self.assertIn("[turn-slow] worst", blob)

    def test_skipped_capped_and_slice_wait_each_fail_the_verdict(self):
        for kw, word in (({"skipped": 2}, "skipped ticks 2"), ({"capped": 1}, "capped 1"),
                         ({"slice_wait": 150.0}, "clock slice wait max")):
            text = (self.run_summary(block(**kw)) / "summary.txt").read_text()
            self.assertIn("STALLS", text, kw)
            self.assertIn(word, text, kw)

    def test_a_quiet_stretch_does_not_hide_the_stall_in_the_fight(self):
        # target 2 x 50 = 100; the first window has 10 pilots (no fight), the second 100
        lines = block(pilots=10, late_max=500.0) + block(pilots=100)
        text = (self.run_summary(lines) / "summary.txt").read_text()
        fight = text.split("== whole run")[0]
        self.assertIn("fight windows (1 windows)", fight)
        self.assertIn("verdict: PASS", fight)
        self.assertIn("verdict: STALLS", text.split("== whole run")[1])

    def test_alarms_bots_procs_and_the_client_result_are_reported(self):
        extra = ["[live] DEGRADED: world lock held max 90 ms", "[push] pilot7: outbox over cap at 5 frames",
                 "[push] cipilot: outbox over cap at 5 frames", "[push] write failed: broken", "[world] TAINTED x"]
        procs = "time\tproc\tcpu\trss\tthr\n00:00:01\teve-server\t12\t500\t90\n00:00:11\teve-server\t40\t600\t95\n"
        res = {"scenario": "fci_battle", "ok": True, "checks": [{"name": "n", "ok": False, "detail": "d"}], "error": None}
        out = self.run_summary(block(), extra_server=extra, bots="[stats] a\nMISSING x\n[stats] b\n",
                               result=res, procs=procs)
        text = (out / "summary.txt").read_text()
        for want in ("DEGRADED: 1", "outbox over cap: 1", "outbox over cap (cipilot): 1", "write failed: 1",
                     "TAINTED: 1", "[stats] lines: 2, MISSING lines: 1", "eve-server: cpu 40, rss 600, threads 95",
                     "[FAIL] n -- d"):
            self.assertIn(want, text)

    def test_an_older_tick_line_without_the_tail_parses_but_is_incomplete(self):
        old = ("[world] tick health: 40 ticks, late mean 0.5 ms max 3.0 ms, over budget 0, world lock wait mean 0.1 ms "
               "max 2.0 ms, world lock held mean 1.0 ms max 4.0 ms, work max 5.0 ms, skipped 0")
        text = (self.run_summary([old]) / "summary.txt").read_text()
        self.assertIn("verdict: INCOMPLETE", text)
        self.assertEqual(text.count("no tick lock data"), 1)   # the reason once, not twice in one line
        self.assertNotIn("verdict: PASS", text)

    def test_stalls_without_lock_lines_still_read_stalls(self):
        base = ("[world] tick health: 40 ticks, late mean 0.5 ms max %s ms, over budget 0, world lock wait mean 0.1 ms "
                "max 2.0 ms, world lock held mean 1.0 ms max 4.0 ms, work max 5.0 ms, skipped %d")
        for late, skipped in (("3.0", 3), ("2800.0", 0)):
            text = (self.run_summary([base % (late, skipped)]) / "summary.txt").read_text()
            self.assertIn("verdict: STALLS", text)
            self.assertIn("not measured", text)
            self.assertNotIn("INCOMPLETE", text)

    def test_one_window_without_a_lock_line_is_not_a_pass(self):
        lines = block() + [tick()]
        text = (self.run_summary(lines) / "summary.txt").read_text()
        self.assertIn("INCOMPLETE", text)
        self.assertNotIn("verdict: PASS", text)


class Sample(unittest.TestCase):
    def test_it_writes_rows_and_exits_when_the_first_pid_is_gone(self):
        d = tempfile.mkdtemp(prefix="test-battle-")
        child = subprocess.Popen([sys.executable, "-I", "-c", "import time; time.sleep(2)"])
        try:
            out = os.path.join(d, "procs.tsv")
            saved, bs.SAMPLE_S = bs.SAMPLE_S, 1
            try:
                self.assertEqual(bs.main(["sample", "--out", out, str(child.pid)]), 0)
            finally:
                bs.SAMPLE_S = saved
        finally:
            child.wait()
        rows = Path(out).read_text().splitlines()
        self.assertEqual(rows[0], "time\tproc\tcpu_pct_one_core\trss_mb\tthreads")
        self.assertGreaterEqual(len(rows), 2)
        self.assertEqual(len(rows[1].split("\t")), 5)

    def test_read_proc_of_a_pid_that_cannot_exist_is_none(self):
        self.assertIsNone(bs.read_proc(2 ** 30))


if __name__ == "__main__":
    unittest.main()
