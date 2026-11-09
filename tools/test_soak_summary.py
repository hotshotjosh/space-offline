import importlib.util
import os
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("soak_summary", os.path.join(HERE, "soak_summary.py"))
ss = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ss)

TICK = ("[world] tick health: 40 ticks, late mean {lm} ms max {lx} ms, over budget {ob}, "
        "world lock wait mean 0.0 ms max 0.0 ms, world lock held mean 0.2 ms max {h} ms, "
        "work max {w} ms, skipped {sk}")
SERVER = "\n".join(
    [TICK.format(lm="0.1", lx="0.5", ob=0, h=1.0 + i, w=2.0 + i, sk=0) for i in range(20)]
    + [TICK.format(lm="0.3", lx="9.0", ob=2, h="1.0", w="30.0", sk=1)]
    + ["[svc] marketProxy.PlaceMultiSellOrder/5 as 90000001 raises UserError (by design)"] * 3
    + ["[svc] skillMgr77::skillHandler.SaveNewQueue/2 as 90000002 raises UserError (by design)"]
    + ["[svc] UNANSWERED foo.Bar -- answering None"]
)
BOTS = "\n".join([
    "hulka1 +03:53.1 dock        CmdDock 60002590: why: homing Recovery -> ok",
    "hulka2 +04:05.8 dock        CmdDock 60002590: why: homing Recovery -> ok",
    "tradea1 +15:08.0 lost        the connection -> FAILED: protocol: closed",
    "x1 +16:00.0 warp-member CmdWarpToStuff char 1 -> refused: no WarpTo",
    "x1 +16:01.0 observe     docked -> ok",
])


def run(server, bots, offline):
    d = Path(tempfile.mkdtemp())
    (d / "server.log").write_text(server)
    (d / "bots.log").write_text(bots)
    if offline is not None:
        (d / "offline.txt").write_text(offline)
    return d, *ss.summarize(d)


class Summary(unittest.TestCase):
    def test_refusals_are_counted_by_key_too(self):
        bots = "\n".join([
            "b1 +01:00.0 trade       sell -> refused: PlaceMultiSellOrder answered None (NoCashX)",
            "b1 +01:01.0 trade       sell -> refused: PlaceMultiSellOrder answered None (NoCashX)",
            "b2 +01:02.0 trade       sell -> refused: PlaceMultiSellOrder answered None (OtherY)",
            "b2 +01:03.0 trade       search -> refused: SearchContracts: KeyZ",
            "b2 +01:04.0 trade       odd -> refused: no UserError key",
        ])
        _, lines, rows, _ = run(SERVER, bots, "pilots 0")
        r = {(m, k): v for m, k, v in rows}
        self.assertEqual(r[("act_refused", "trade")], "5")  # totals as before
        self.assertEqual(r[("act_refused_key", "trade NoCashX")], "2")
        self.assertEqual(r[("act_refused_key", "trade OtherY")], "1")
        self.assertEqual(r[("act_refused_key", "trade KeyZ")], "1")
        self.assertEqual(r[("act_refused_key", "trade -")], "1")
        self.assertTrue(any("2 trade NoCashX" in l for l in lines))
        self.assertFalse([k for m, k, _ in rows if m == "usererror_key"])

    def test_refusal_key_forms_and_non_keys(self):
        k = ss.refusal_key
        self.assertEqual(k("refused: UserError MarketExceededOrderCount"), "MarketExceededOrderCount")
        self.assertEqual(k("refused: InstallJob: SomeKey"), "SomeKey")
        self.assertEqual(k("refused: X answered Some(Bool(false)) (Key9)"), "Key9")
        for bad in ("refused: nope", "refused: x", "refused: ConeScan answered Some(None)",
                    "refused: foo(x)", "refused: no UserError key", "refused: AB"):
            self.assertEqual(k(bad), "-", bad)

    def test_p95_nearest_rank(self):
        self.assertEqual(ss.p95([]), 0.0)
        self.assertEqual(ss.p95(list(range(1, 101))), 95)
        self.assertEqual(ss.p95([7]), 7)

    def test_fixture(self):
        _, lines, rows, fails = run(SERVER, BOTS, "[live] day 1 | pilots 0 (in space 0) | npcs 1\n")
        r = {(m, k): v for m, k, v in rows}
        self.assertEqual(r[("tick", "tick_work_max_ms")], "30.00")
        self.assertEqual(r[("tick", "tick_work_p95_ms")], "21.00")  # 21 windows, rank 20
        self.assertEqual(r[("tick", "late_over_budget")], "2")
        self.assertEqual(r[("tick", "ticks_skipped")], "1")
        self.assertEqual(r[("usererror", "marketProxy.PlaceMultiSellOrder")], "3")
        self.assertEqual(r[("usererror", "skillMgrN::skillHandler.SaveNewQueue")], "1")
        self.assertEqual(r[("bots", "homing_recovery_docks")], "2")
        self.assertEqual(r[("act_failed", "lost")], "1")
        self.assertEqual(r[("act_refused", "warp-member")], "1")
        self.assertEqual(fails, ["deny: 1 UNANSWERED in server.log"])
        self.assertTrue(any("every bot offline after stop" in l for l in lines))

    def test_allow_and_clean(self):
        d, _, _, _ = run(SERVER, BOTS, "pilots 0\n")
        _, _, fails = ss.summarize(d, {"UNANSWERED": 1})
        self.assertEqual(fails, [])

    def test_bot_left_online(self):
        _, _, _, fails = run("", "", "[live] pilots 3 (in space 1)\n")
        self.assertEqual(fails, ["cleanup: 3 online after stop"])

    def test_unknown_offline_is_a_note(self):
        _, lines, _, fails = run("", "", None)
        self.assertEqual(fails, [])
        self.assertTrue(any("not judged" in l for l in lines))

    def test_a_bots_chat_reset_is_not_an_error(self):
        line = "[chat] 127.0.0.1:1 (7) ended: read error: Connection reset by peer (os error 104)"
        self.assertEqual(run(line, "", "pilots 0")[3], [])

    def test_panic_and_error(self):
        _, _, _, fails = run("thread 'x' panicked at y\nERROR boom", "something error: z", "pilots 0")
        self.assertEqual(sorted(fails), ["deny: 1 error in bots.log", "deny: 1 error in server.log",
                                         "deny: 1 panic in server.log"])


PROF = [
    "[world] tick sections: secs=10.0, take_due n 3 mean 0.100 ms max 1.000 ms, filter n 3 mean 2.000 ms max 9.000 ms, "
    "audience n 40 mean 3.000 ms max 8.000 ms, session_changes n 40 mean 0.010 ms max 0.100 ms, save n 2 mean 1.000 ms max 40.000 ms",
    "[world] tick lock: secs=10.0, clock held 900.0 ms occupancy 9.0%, payload held 400.0 ms occupancy 4.0% turns 260 "
    "hold max 12.00 ms, payload wait mean 0.50 ms max 30.00 ms, total occupancy 13.0%",
    "[world] tick methods: secs=10.0, turns 260 methods 8 hold 400.0 ms, ro-guess turns 35.0% hold 20.0%, "
    "top_hold a.Move:100:200.0ms:5.0ms:rw,b.GetX:50:10.0ms:1.0ms:ro, top_count a.Move:100:200.0ms:5.0ms:rw",
    "[world] tick writers: secs=10.0, frames 5000 (500/s) bytes 900000 (90.0 kB/s), passes 40 pass mean 2.00 ms max 9.00 ms, "
    "deflate n 3 total 4.0 ms max 2.00 ms, writes 5000 write total 50.0 ms longest write 7.00 ms",
]


class Prof(unittest.TestCase):
    def test_profile_lines_parse_and_summarise(self):
        prof = ss.parse_prof(PROF)
        self.assertEqual([len(prof[k]) for k in ("sections", "lock", "methods", "writers")], [1, 1, 1, 1])
        self.assertEqual(prof["methods"][0][5], "a.Move")
        text = "\n".join(ss.prof_summary(prof))
        self.assertIn("audience mean 3.00 max 8.00", text)
        self.assertIn("clock 9.0%", text)
        self.assertIn("longest write 7.0 ms", text)

    def test_read_turn_fields_are_optional_and_summarised(self):
        # The old lines parse unchanged and print no read-turn line.
        old = ss.prof_summary(ss.parse_prof(PROF))
        self.assertFalse(any("read turns" in l for l in old))
        new = list(PROF)
        new[1] += ", read turns 120 held 30.0 ms (shared, not in occupancy) hold max 1.00 ms wait mean 0.01 ms max 0.20 ms"
        new[2] += ", ro-served turns 40.0% hold 10.0%, fallbacks 7"
        prof = ss.parse_prof(new)
        self.assertEqual([len(prof[k]) for k in ("lock", "methods")], [1, 1])
        text = "\n".join(ss.prof_summary(prof))
        self.assertIn("120 read turns", text)
        self.assertIn("ro-served 40.0% of turns", text)
        self.assertIn("7 fallbacks", text)

    def test_old_log_has_no_profile(self):
        self.assertEqual(ss.prof_summary(ss.parse_prof([TICK.format(lm="0", lx="0", ob=0, h=1, w=1, sk=0)])), [])


FF_CLEAN = "[ff] mode=lockstep virt_s=86400.0 real_s=212.9 ratio=405.8 stalls=0"
FF_STALLS = "[ff] mode=affine k=20 virt_s=600.0 real_s=30.4 ratio=19.7 late_p95_ms=12.0 skipped=0 stalls=3"
FF_DOG = "[ff] watchdog exit real_s=60.0 virt_s=3601.25"


class FastForward(unittest.TestCase):
    def test_a_run_without_ff_summarises_as_before(self):
        _, lines, rows, fails = run(SERVER, BOTS, "pilots 0")
        self.assertFalse(any("ff:" in l or "n/a" in l for l in lines))
        d2, lines2, rows2, fails2 = run(SERVER + "\nnoise [ffx] not a line", BOTS, "pilots 0")
        self.assertEqual((lines2, rows2, fails2), (lines, rows, fails))

    def test_a_clean_ff_line_adds_the_line_and_keeps_the_fails(self):
        _, lines, rows, fails = run(SERVER, BOTS, "pilots 3")
        self.assertTrue(fails)
        _, lines2, rows2, fails2 = run(SERVER + "\n" + FF_CLEAN, BOTS, "pilots 3")
        self.assertEqual(fails2, fails)
        self.assertEqual(rows2, rows)
        self.assertEqual(lines2[:-1], lines)
        self.assertEqual(lines2[-1], "soak: ff: mode=lockstep ratio=405.8 stalls=0")

    def test_a_watchdog_exit_drops_the_fails_to_n_a(self):
        _, lines, rows, fails = run(SERVER, BOTS, "pilots 3")
        _, lines2, rows2, fails2 = run(SERVER + "\n" + FF_DOG, BOTS, "pilots 3")
        self.assertEqual(fails2, [])
        self.assertTrue(any(l.startswith("soak: verdict n/a") for l in lines2))
        self.assertEqual(sum(l.startswith("soak: note: would FAIL") for l in lines2), len(fails))

    def test_main_exits_0_on_stalls(self):
        d, *_ = run(SERVER + "\n" + FF_STALLS, BOTS, "pilots 3")
        import contextlib
        import io

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(ss.main(["soak_summary.py", str(d)]), 0)


if __name__ == "__main__":
    unittest.main()
