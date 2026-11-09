import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("bench_ramp", os.path.join(HERE, "bench_ramp.py"))
br = importlib.util.module_from_spec(spec)
spec.loader.exec_module(br)

LINE = ("[world] tick health: 40 ticks, late mean {lm} ms max {lx} ms, over budget {ob}, "
        "world lock wait mean 0.1 ms max 0.5 ms, world lock held mean {hm} ms max {h} ms, "
        "work max {w} ms, skipped {sk}{tail}")
TAIL = ", work p95 {p} ms, out {of} frames {ob} bytes, destiny {df} frames {db} bytes"


def tick(h=3.0, w=4.0, ob=0, sk=0, late="0.1", tail=True, p=3.5, of=800, nb=409600, df=400, db=204800):
    t = TAIL.format(p=p, of=of, ob=nb, df=df, db=db) if tail else ""
    return LINE.format(lm="0.1", lx=late, ob=ob, hm="2.0", h=h, w=w, sk=sk, tail=t)


def step(idx, bots, lines, pilots=None, **kw):
    live = f"[live] day 1 next-rollover 1h | systems hot 1 warm 0 | pilots {pilots or bots} (in space {bots}) | npcs 0"
    return br.summarize_step(idx, bots, lines + [live], [], {"cpu_pct": 5, "rss_mb": 300, "threads": 100},
                             {"cpu_pct": 2, "rss_mb": 600, "threads": 60}, bots)


class Parse(unittest.TestCase):
    def test_new_and_old_tick_lines(self):
        w = br.parse_ticks([tick(), tick(tail=False)])
        self.assertEqual(len(w), 2)
        self.assertEqual(w[0]["dest_bytes"], 204800.0)
        self.assertIsNone(w[1]["dest_bytes"])

    def test_slice_max_is_optional_and_parsed(self):
        w = br.parse_ticks([tick() + ", slice_max 12.5 ms", tick()])
        self.assertEqual(w[0]["slice_max"], 12.5)
        self.assertIsNone(w[1]["slice_max"])
        r = step(0, 50, [tick() + ", slice_max 12.5 ms", tick() + ", slice_max 3.0 ms"])
        self.assertEqual(r["slice_max_ms"], 12.5)
        self.assertEqual(step(0, 50, [tick()])["slice_max_ms"], 0.0)

    def test_step_rates(self):
        r = step(0, 50, [tick()] * 4)   # 4 windows = 40 s
        self.assertAlmostEqual(r["out_kBps"], 400 / 40.0 * 4 * 1.0, delta=0.01 * 400)  # 400 KB/window / 10 s
        self.assertAlmostEqual(r["destiny_frames_per_tick"], 10.0)
        self.assertAlmostEqual(r["destiny_bytes_per_bot_s"], 20480 / 50)
        self.assertEqual(r["pilots"], 50)


class Scenario(unittest.TestCase):
    def test_split_and_roster(self):
        self.assertEqual(sum(br.split_counts(7, {"miners": 60, "ratters": 25, "fighters": 15}).values()), 7)
        text, names = br.roster(300, br.parse_mix("miners=60,ratters=25,fighters=15"), "same")
        self.assertEqual(len(names), 300)
        self.assertEqual(len(set(names)), 300)
        self.assertEqual(len([l for l in text.splitlines() if not l.startswith("#")]), 3)

    def test_spread_miners_in_fives_and_fleet_cap(self):
        text, names = br.roster(100, {"miners": 1}, "spread")
        self.assertEqual(len(text.splitlines()) - 1, 20)
        text, _ = br.roster(600, {"miners": 1}, "same")
        self.assertEqual(len(text.splitlines()) - 1, 3)   # 250 + 250 + 100

    def test_steps(self):
        self.assertEqual(br.steps_of(50, 50, 300), [50, 100, 150, 200, 250, 300])
        self.assertEqual(br.steps_of(10, 15, 30), [10, 25, 30])

    def test_bad_mix(self):
        with self.assertRaises(ValueError):
            br.parse_mix("pirates=3")


STATS = ("[stats] bots=400 cpu={cpu}% rss={rss}MB (2600kB/bot) threads=412 in={kb}kB/s | decode {d}ms/s n=900 210us | "
         "observe 3.0ms/s n=900 3us | destiny 40.5ms/s n=900 45us | refresh 5.0ms/s n=30 160us | "
         "decide 12.0ms/s n=400 30us | absorb 2.5ms/s n=900 2us | log 1.0ms/s n=100 10us")


def stats(cpu=700.0, rss=900, kb=1500, d=120.5):
    return STATS.format(cpu=cpu, rss=rss, kb=kb, d=d)


class Stats(unittest.TestCase):
    def test_a_stats_line_parses_into_phases(self):
        (x,) = br.parse_stats(["noise", stats()])
        self.assertEqual((x["cpu"], x["rss"], x["threads"], x["kbps"]), (700.0, 900, 412, 1500))
        self.assertEqual((x["decode"], x["destiny"], x["decide"], x["log"]), (120.5, 40.5, 12.0, 1.0))

    def test_step_columns_are_means_over_the_steps_stats_lines(self):
        live = "[live] day 1 next-rollover 1h | systems hot 7 warm 23 | pilots 400 (in space 380) | npcs 55 rocks 90 | deltas none"
        r = br.summarize_step(3, 400, [tick(), live], [stats(cpu=600.0, d=100.0), stats(cpu=800.0, d=140.0), "x MISSING y"],
                              {}, {}, 400)
        self.assertEqual(r["stats_n"], 2)
        self.assertAlmostEqual(r["stats_cpu_pct"], 700.0)
        self.assertAlmostEqual(r["decode_ms_s"], 120.0)
        self.assertEqual((r["hot_systems"], r["warm_systems"], r["npcs"], r["rocks"]), (7, 23, 55, 90))
        self.assertEqual(r["missing_lines"], 1)
        row = "\t".join(br.fmt(r[c]) for c in br.COLUMNS).split("\t")
        self.assertEqual(len(row), len(br.COLUMNS))

    def test_no_stats_lines_read_zero(self):
        r = step(0, 50, [tick()])
        self.assertEqual((r["stats_n"], r["decode_ms_s"], r["hot_systems"], r["rocks"]), (0, 0.0, 1, 0))

    def test_half_shift_skips_the_batch_burst_wherever_it_lands(self):
        # A burst in the first three windows is skipped (any burst within 20 s of the step start).
        for pos in (0, 1, 2):
            v = [2.0] * 9
            v[pos] = 9.0
            self.assertEqual(br.half_shift_pct(v), 0.0, pos)
        # Past the skipped windows a burst is real.
        v = [2.0] * 9
        v[3] = 9.0
        self.assertGreater(br.half_shift_pct(v), 30.0)
        self.assertEqual(br.half_shift_pct([5.0] * 8), 0.0)
        # Each half's p95 (its worst here), not its mean: one spike in the second half counts.
        self.assertAlmostEqual(br.half_shift_pct([0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 3.0, 2.0, 2.0, 3.0]), 50.0)
        self.assertEqual(br.half_shift_pct([1.0, 9.0, 9.0, 9.0, 9.0, 9.0]), 0.0)   # too few windows to say

    def test_floor_uses_the_halves_not_the_burst(self):
        # Step p95 is high only from the burst in window 2; the halves are quiet: no warning,
        # even though the halves' p95s differ by over 30% (2 vs 3 ms, far below the floor).
        v = [tick(p=2.0), tick(p=200.0), tick(p=2.0)] + [tick(p=2.0)] * 3 + [tick(p=3.0)] * 3
        r = step(0, 100, v)
        self.assertGreater(r["work_p95_ms"], 25.0)
        self.assertGreater(r["p95_half_shift_pct"], 30.0)
        self.assertLess(r["half_p95_max_ms"], 25.0)
        self.assertNotIn("between its halves", "\n".join(br.final_summary([r], "")))

    def test_half_shift_warning_needs_a_burst_free_shift_above_the_floor(self):
        # Burst in the first window of a quiet step: no warning (the old code warned).
        quiet = [step(0, 100, [tick(w=9.0, p=9.0)] + [tick(w=2.0, p=2.0)] * 9)]
        self.assertNotIn("between its halves", "\n".join(br.final_summary(quiet, "")))
        # A real doubling, but far below 10% of the budget (25 ms): no warning.
        low = [step(0, 100, [tick()] * 3 + [tick(p=2.0)] * 4 + [tick(p=6.0)] * 4)]
        self.assertGreater(low[0]["p95_half_shift_pct"], 30.0)
        self.assertNotIn("between its halves", "\n".join(br.final_summary(low, "")))
        # The same shift above the floor: warning.
        high = [step(0, 100, [tick()] * 3 + [tick(p=20.0)] * 4 + [tick(p=60.0)] * 4)]
        self.assertIn("between its halves", "\n".join(br.final_summary(high, "")))
        short = "\n".join(br.final_summary([step(0, 100, [tick()] * 3)], ""))
        self.assertIn("hold is too short", short)
        self.assertNotIn("hold is too short", "\n".join(br.final_summary([step(0, 100, [tick()] * 3)], "", prep=True)))
        ok = "\n".join(br.final_summary([step(0, 100, [tick()] * 6)], ""))
        self.assertNotIn("WARNING", ok)

    def test_bot_words(self):
        class A:
            brawl, stats, quiet = True, 10, False
        self.assertEqual(br.bot_extra_args(A, ""), ["--brawl", "--stats", "10"])
        self.assertEqual(br.bot_extra_args(A, "--stats 20 --quiet"), ["--brawl", "--stats", "20", "--quiet"])
        A.brawl, A.stats, A.quiet = False, 0, True
        self.assertEqual(br.bot_extra_args(A, ""), ["--quiet"])


class Systems(unittest.TestCase):
    # An invented graph: 1 - 2 - 3 - 4 and 1 - 5; 3 has no station, 2 is highsec.
    # 6 is 0.47: high-sec by the server's line (low is below 0.45), so it is skipped
    # (a 0.5 line would take it as low-sec); 7 is 0.44, low-sec, and is taken.
    SEC = {1: 0.3, 2: 0.9, 3: -0.2, 4: -0.5, 5: 0.1, 6: 0.47, 7: 0.44}
    NAMES = {"Alpha": 1}
    GATES = {1: [2, 5, 6, 7], 2: [1, 3], 3: [2, 4], 4: [3], 5: [1], 6: [1], 7: [1]}
    STATIONS = {1, 2, 4, 5, 6, 7}
    U = (SEC, NAMES, GATES, STATIONS)

    def test_nearest_low_null_with_a_station_home_first(self):
        self.assertEqual(br.pick_systems(self.U, "Alpha", 1), [1])
        self.assertEqual(br.pick_systems(self.U, "Alpha", 3), [1, 5, 7])   # 2, 6 high; the 0.44 one is low
        self.assertEqual(br.pick_systems(self.U, "Alpha", 4), [1, 5, 7, 4])   # 3 has no station
        self.assertEqual(br.pick_systems(self.U, "1", 2), [1, 5])

    def test_too_many_or_unknown(self):
        with self.assertRaises(SystemExit):
            br.pick_systems(self.U, "Alpha", 5)
        with self.assertRaises(SystemExit):
            br.pick_systems(self.U, "Nowhere", 2)

    def test_read_universe_from_files(self):
        import tempfile
        from pathlib import Path
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "universe").mkdir()
            (Path(d) / "stations").mkdir()
            cols = ["10", "20", "0", "0", "0.25", "C", "1", "1", "0", "0", "0", "0", "0", "0", "0", "9"]
            (Path(d) / "universe/systems.tsv").write_text(
                "#hdr\n" + "\t".join(["7"] + cols[1:5] + ["-0.4"] + cols[5:] + ["Zed"]) + "\n")
            (Path(d) / "universe/celestials.tsv").write_text(
                "#kind\nplanet\t1\t2\t3\t4\t5\t6\t7\t8\t9\ngate\t1\t2\t3\t4\t5\t6\t7\t8\t9\n")
            (Path(d) / "stations/stations.tsv").write_text("#hdr\n99\t7\t1\n")
            sec, names, gates, stations = br.read_universe(d)
        self.assertEqual((sec[7], names["Zed"], gates, stations), (-0.4, 7, {7: [9]}, {7}))

    def test_spread_roster_seats_fleets_round_robin(self):
        text, names = br.roster(60, {"miners": 1}, "spread", [11, 22, 33])
        rows = [l.split("\t") for l in text.splitlines() if not l.startswith("#")]
        self.assertEqual(len(rows), 12)
        self.assertEqual([r[6] for r in rows[:4]], ["11", "22", "33", "11"])
        self.assertEqual(len(names), 60)
        # One system, or no list, or the same grid: no column, as before.
        for sy, grid in ((None, "spread"), ([11], "spread"), ([11, 22], "same")):
            t, _ = br.roster(60, {"miners": 1}, grid, sy)
            self.assertTrue(all(len(l.split("\t")) == 6 for l in t.splitlines() if not l.startswith("#")))

    def test_the_other_side_of_a_fight(self):
        text, names = br.roster(100, br.parse_mix("fighters=1,blues=1"), "same")
        rows = [l.split("\t") for l in text.splitlines() if not l.startswith("#")]
        self.assertEqual(sorted((r[1], r[3]) for r in rows), [("blu", "50"), ("red", "50")])
        self.assertEqual(len(set(names)), 100)


    def test_prep_seats_both_corporations_on_one_side(self):
        mix = br.parse_mix("fighters=1,blues=1")
        text, names = br.roster(100, mix, "same", one_side=True)
        rows = [l.split("\t") for l in text.splitlines() if not l.startswith("#")]
        self.assertEqual([(r[1], r[3]) for r in rows], [("red", "50"), ("red", "50")])
        _, plain = br.roster(100, mix, "same")
        self.assertEqual(names, plain)   # the accounts are the same

    def test_final_hold_must_be_a_real_hold(self):
        # Harmless even if the guard is missing: check_args is pure, and the
        # paths of any run that did start point at nothing real.
        probe = ["--bin", "/nonexistent/probe/bin", "--out", "/nonexistent/probe/o",
                 "--world", "/nonexistent/probe/w", "--tokens", "/nonexistent/probe/t"]
        for bad in ("5", "19"):
            a = br.build_parser().parse_args(probe + ["--final-hold", bad])
            self.assertIn("--final-hold", br.check_args(a))
        for ok in ("0", "20"):
            self.assertIsNone(br.check_args(br.build_parser().parse_args(probe + ["--final-hold", ok])))
        self.assertIn("required", br.check_args(br.build_parser().parse_args(["--final-hold", "30"])))


class Knee(unittest.TestCase):
    def rows(self, helds):
        return [step(i, 50 * (i + 1), [tick(h=h, w=h + 1)] * 5) for i, h in enumerate(helds)]

    def test_no_knee_below_half_budget(self):
        k, _ = br.knee(self.rows([3, 20, 60, 120]))
        self.assertIsNone(k)

    def test_knee_on_lock_held_p95(self):
        k, why = br.knee(self.rows([3, 20, 130, 300]))
        self.assertEqual(k["target_bots"], 150)
        self.assertIn("lock held p95", why)

    def test_knee_on_skipped_or_over_budget(self):
        rows = self.rows([3, 3])
        rows.append(step(2, 150, [tick(h=3, ob=1)] * 5))
        k, why = br.knee(rows)
        self.assertEqual(k["target_bots"], 150)
        self.assertIn("over budget", why)
        k, why = br.knee([step(0, 50, [tick(sk=2)] * 3)])
        self.assertIn("skipped", why)

    def test_collapse(self):
        ok = step(0, 50, [tick()] * 5)
        self.assertEqual(br.collapse([ok]), "")
        bad = step(1, 100, [tick(ob=3)] * 5)
        self.assertIn("consecutive", br.collapse([ok, bad]))
        gone = step(1, 100, [tick()] * 5, pilots=40)
        self.assertIn("disconnecting", br.collapse([ok, gone]))
        some = step(1, 100, [tick(ob=3), tick(), tick(ob=3), tick()])
        self.assertEqual(br.collapse([ok, some]), "")

    def test_summary_text(self):
        rows = self.rows([3, 130])
        text = "\n".join(br.final_summary(rows, "stopped"))
        self.assertIn("KNEE at 100 bots", text)
        self.assertIn("STOPPED EARLY", text)


class LoginGate(unittest.TestCase):
    """#252: the login curve of `--login-rate R`, from invented log lines."""

    seq = 0

    def login(self, n):
        """n login lines with fresh invented character ids."""
        LoginGate.seq += n
        return [f"[svc] session change: character {90000000 + LoginGate.seq - i} entering the game" for i in range(n)]

    def test_counts_per_second(self):
        events = [(0.4, self.login(1)), (0.9, self.login(1)), (1.2, ["[svc] handed out object N=1:1 for dogmaIM"]),
                  (2.5, self.login(2))]
        self.assertEqual(br.per_second(events), [2, 0, 2])
        self.assertEqual(br.count_logins(["[svc] session change: character 1 left"]), 0)

    def test_burst_above_window_limit_is_flagged(self):
        # rate 3: a 5 s window may hold 5*3+1 = 16
        self.assertEqual(br.burst_limit(3.0), 16)
        events = [(0.5, self.login(3)), (1.5, self.login(4)), (2.5, self.login(3)), (3.5, self.login(3)),
                  (4.5, self.login(3))]
        self.assertEqual(br.bursts(br.per_second(events), 3.0), [])  # 16: allowed
        events.append((5.5, self.login(40)))
        counts = br.per_second(events)
        self.assertEqual(br.bursts(counts, 3.0)[0], (1, 53))
        text = "\n".join(br.gate_summary(events, 3.0, 56))
        self.assertIn("WARNING login gate", text)
        self.assertIn("5 s window(s) above 16", text)
        self.assertIn("peak 40/s", text)

    def test_skew_across_a_second_edge_is_not_flagged(self):
        # a steady 3/s curve whose stamps slip: 6, 3, 3, 3, 1 is 16 in 5 s
        events = [(1.5, self.login(6)), (2.5, self.login(3)), (3.5, self.login(3)),
                  (4.5, self.login(3)), (5.5, self.login(1))]
        self.assertEqual(br.bursts(br.per_second(events), 3.0), [])

    def test_steady_curve_is_clean(self):
        events = [(i + 0.5, self.login(3)) for i in range(10)]
        self.assertEqual(br.bursts(br.per_second(events), 3.0), [])
        text = "\n".join(br.gate_summary(events, 3.0, 30))
        self.assertNotIn("WARNING", text)
        self.assertIn("30 of 30 logins", text)

    def test_distinct_characters_counted(self):
        line = "[svc] session change: character 77 entering the game"
        events = [(0.5, [line, line]), (1.5, [line.replace("77", "78")])]
        self.assertIn("2 of 2 logins", "\n".join(br.gate_summary(events, 3.0, 2)))

    def test_short_run_warns(self):
        text = "\n".join(br.gate_summary([(0.5, self.login(2))], 3.0, 100))
        self.assertIn("only 2 of 100 logins", text)

    def test_windows_carry_logins_and_tick_health(self):
        events = [(5.0, self.login(10)), (10.0, [tick(p=7.5, late="1.5")] + self.login(5)),
                  (20.0, [tick(p=3.0)])]
        rows = br.login_windows(events)
        self.assertEqual([r["logins"] for r in rows], [15, 0])
        self.assertEqual(rows[0]["work_p95_ms"], 7.5)
        self.assertEqual(rows[0]["late_max_ms"], 1.5)
        text = "\n".join(br.gate_summary(events, 3.0, 15))
        self.assertIn("work p95 worst 7.5 ms", text)

    def test_tail_reads_whole_lines_only(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "server.log")
            t = br.LogTail(path, 100.0)
            t.poll(101.0)  # no file yet
            self.assertEqual(t.events, [])
            with open(path, "w") as f:
                f.write(self.login(1)[0] + "\n[svc] session change: char")
            t.poll(102.0)
            with open(path, "a") as f:
                f.write("acter 5 entering the game\n")
            t.poll(103.0)
            self.assertEqual(br.per_second(t.events), [0, 0, 1, 1])
            with open(path, "a") as f:
                f.write("[svc] handed out object N=1:1 for dogmaIM\n")
            t.poll(104.0)
            self.assertEqual(len(t.events), 2)  # other lines are not kept
            br.write_logins(os.path.join(d, "logins.tsv"), t.events)
            with open(os.path.join(d, "logins.tsv")) as f:
                self.assertEqual(f.read().splitlines()[-1], "3\t1\t2")

    def test_args(self):
        p = br.build_parser()
        base = ["--bin", "b", "--out", "o", "--world", "w", "--tokens", "t", "--max", "1000"]
        self.assertIsNone(br.check_args(p.parse_args(base)))  # no gate: the ramp, unchanged
        self.assertIsNone(br.check_args(p.parse_args(base + ["--login-rate", "2", "--hold", "600"])))
        self.assertIn("--hold >= 540", br.check_args(p.parse_args(base + ["--login-rate", "2"])))
        self.assertIn("--prep", br.check_args(p.parse_args(base + ["--login-rate", "2", "--hold", "600", "--prep"])))
        self.assertIn("--login-rate", br.check_args(p.parse_args(base + ["--login-rate", "-1"])))
        # --final-hold must not shorten the gate run below what it needs
        self.assertIn("--final-hold", br.check_args(p.parse_args(base + ["--login-rate", "2", "--hold", "600", "--final-hold", "100"])))
        self.assertIsNone(br.check_args(p.parse_args(base + ["--login-rate", "2", "--hold", "600", "--final-hold", "600"])))


if __name__ == "__main__":
    unittest.main()
