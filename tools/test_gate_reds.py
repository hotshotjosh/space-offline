"""Tests for tools/gate_reds.py: the gate's red log. Fixture logs, invented names only.

Run::

    python3 -I tools/test_gate_reds.py
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "gate_reds.py")
SHA = "a" * 40
TREE_A = "1" * 40
TREE_B = "2" * 40
TREE_C = "3" * 40
TREE_D = "4" * 40

SERIAL_RED = """== test
   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)
running 3 tests
test one::ok ... ok
test one::bad_a ... FAILED
test one::bad_b ... FAILED
failures:
---- one::bad_a stdout ----
thread 'one::bad_a' panicked at src/lib.rs:1:1:
test result: FAILED. 1 passed; 2 failed
gate: tests failed (exit 101)
"""

TWO_BINARIES = """== test
   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)
test lib::bad ... FAILED
   Running tests/bots/main.rs (target/debug/deps/bots-99aa77ff)
test sock::bad ... FAILED
"""


class Base(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="eve-test-reds-")
        self.tmp = self._tmp.name
        self.file = os.path.join(self.tmp, "reds.tsv")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def log(self, text: str, name: str = "gate.log") -> str:
        p = os.path.join(self.tmp, name)
        with open(p, "w") as f:
            f.write(text)
        return p

    def run_tool(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run([sys.executable, "-I", TOOL, *args], capture_output=True,
                              text=True, timeout=60)

    def record(self, text: str, result: str = "red", tree: str = TREE_A, code: int = 101,
               name: str = "gate.log") -> subprocess.CompletedProcess:
        return self.run_tool("record", "--log", self.log(text, name), "--sha", SHA, "--tree", tree,
                             "--exit", str(code), "--result", result, "--file", self.file)

    def rows(self) -> list[list[str]]:
        with open(self.file) as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[0], "#gate-reds 1")
        self.assertTrue(lines[1].startswith("date\tsha"))
        return [ln.split("\t") for ln in lines[2:]]


class Recorder(Base):
    def test_a_serial_red_gives_one_row_per_failed_test(self) -> None:
        r = self.record(SERIAL_RED)
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = self.rows()
        self.assertEqual(len(rows), 2)
        for row in rows:
            self.assertEqual(len(row), 11)
            self.assertEqual((row[1], row[2], row[3], row[4]), (SHA, TREE_A, "red", "full"))
            self.assertEqual((row[5], row[6], row[7]), ("test", "101", "test"))
            self.assertEqual(row[8], "eve_invented:src/lib.rs")
        self.assertEqual([row[9] for row in rows], ["one::bad_a", "one::bad_b"])

    def test_each_failure_belongs_to_its_own_running_header(self) -> None:
        self.record(TWO_BINARIES)
        got = {row[9]: row[8] for row in self.rows()}
        self.assertEqual(got, {"lib::bad": "eve_invented:src/lib.rs", "sock::bad": "bots:tests/bots/main.rs"})

    def test_a_thread_that_is_not_a_failing_test_is_a_panic_row(self) -> None:
        self.record(SERIAL_RED + "thread 'world-ticker' (4242) panicked at src/x.rs:2:2:\n")
        kinds = sorted(row[7] for row in self.rows())
        self.assertEqual(kinds, ["panic", "test", "test"])
        panic = [row for row in self.rows() if row[7] == "panic"][0]
        self.assertEqual(panic[9], "world-ticker")

    def test_the_old_panic_format_without_a_thread_id_still_parses(self) -> None:
        self.record(SERIAL_RED + "thread 'old-ticker' panicked at src/x.rs:2:2:\n")
        self.assertEqual(sorted(row[9] for row in self.rows() if row[7] == "panic"), ["old-ticker"])

    def test_a_failing_test_with_a_thread_id_is_not_a_panic_row(self) -> None:
        self.record(SERIAL_RED.replace("thread 'one::bad_a' panicked", "thread 'one::bad_a' (77) panicked"))
        self.assertEqual(sorted(row[7] for row in self.rows()), ["test", "test"])

    def test_python_format_and_compile_reds(self) -> None:
        self.record("== python unit tests\ngate: tools/test_invented.py failed\n")
        self.record("== format\nDiff in /repo/server/crates/x/src/a.rs:1:\n")
        self.record("== lint\nerror: could not compile `invented` (lib) due to 1 previous error\n")
        rows = self.rows()
        self.assertEqual([(r[5], r[7], r[8]) for r in rows], [
            ("python", "python", "tools/test_invented.py"),
            ("format", "format", "server/crates/x/src/a.rs"),
            ("lint", "compile", "-"),
        ])

    def test_a_status_red_without_a_gate_line_is_a_status_row(self) -> None:
        self.record("== tree hygiene\n   took 0s\n== feature status\nsomething printed\n")
        row = self.rows()[0]
        self.assertEqual((row[5], row[7]), ("status", "status"))

    def test_the_tier_comes_from_the_header_line(self) -> None:
        self.record("== tools only (2 file(s)): x\n== python unit tests\ngate: tools/test_a.py failed\n")
        self.assertEqual(self.rows()[0][4], "tools")

    def test_a_tab_in_a_test_name_becomes_a_space_and_rows_keep_11_fields(self) -> None:
        self.record("   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\n"
                    "test tab\there::x ... FAILED\n")
        rows = self.rows()
        self.assertEqual(len(rows[0]), 11)
        self.assertEqual(rows[0][9], "tab here::x")

    def test_twenty_parallel_records_keep_twenty_intact_rows(self) -> None:
        text = "== test\n   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\ntest p::x ... FAILED\n"
        with ThreadPoolExecutor(max_workers=20) as ex:
            res = list(ex.map(lambda i: self.record(text, name=f"g{i}.log"), range(20)))
        self.assertTrue(all(r.returncode == 0 for r in res))
        rows = self.rows()
        self.assertEqual(len(rows), 20)
        self.assertTrue(all(len(r) == 11 for r in rows))

    def test_a_foreign_header_is_refused_and_the_file_unchanged(self) -> None:
        with open(self.file, "w") as f:
            f.write("#something-else 9\nfoo\n")
        r = self.record(SERIAL_RED)
        self.assertEqual(r.returncode, 1)
        with open(self.file) as f:
            self.assertEqual(f.read(), "#something-else 9\nfoo\n")


class Report(Base):
    FAIL = ("== test\n   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\n"
            "test flaky::case ... FAILED\n")

    def report(self, *args: str) -> str:
        r = self.run_tool("report", "--file", self.file, *args)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout

    def line(self, out: str) -> str:
        return [ln for ln in out.splitlines() if "flaky::case" in ln][0]

    def test_a_red_followed_by_a_pass_on_the_same_tree_is_flaky(self) -> None:
        self.record(self.FAIL, tree=TREE_A)
        self.record("== gate passed\n", result="pass", tree=TREE_A, code=0)
        self.record(self.FAIL, tree=TREE_B)
        out = self.report()
        parts = self.line(out).split()
        self.assertEqual(parts[:3], ["2", "1", "2"], out)
        brief = self.report("--tree", TREE_A, "--brief")
        self.assertIn("land: this tree was red before: flaky::case", brief)
        self.assertIn("marked flaky", brief)
        self.assertEqual(self.report("--tree", TREE_B, "--brief"), "")

    def test_red_on_a_tree_with_no_pass_is_not_flaky_but_three_trees_recur(self) -> None:
        self.record(self.FAIL, tree=TREE_B)
        self.assertNotIn("FLAKY", self.line(self.report()))
        self.record(self.FAIL, tree=TREE_C)
        self.record(self.FAIL, tree=TREE_D)
        out = self.line(self.report())
        self.assertEqual(out.split()[1:3], ["0", "3"])
        self.assertIn("FLAKY", out)

    def test_a_panic_is_flagged(self) -> None:
        self.record("== test\nthread 'world-ticker' panicked at x:1:1:\n")
        self.assertIn("PANIC", self.report())

    def test_a_missing_file_reports_nothing_and_exits_0(self) -> None:
        r = self.run_tool("report", "--file", os.path.join(self.tmp, "none.tsv"))
        self.assertEqual(r.returncode, 0)


FLAKY_HEAD = "#flaky 1\n# a comment\ntarget\ttest\tissue\tadded\n"
LIB = "eve_invented:src/lib.rs"
RERUN_LINE = "error: test failed, to rerun pass `-p eve_invented --lib`\n"


def red(*names: str, extra: str = "", tail: str = RERUN_LINE) -> str:
    return ("== test\n   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\n"
            + "".join(f"test {n} ... FAILED\n" for n in names) + extra
            + f"test result: FAILED. 1 passed; {len(names)} failed\n" + tail)


class RerunPlan(Base):
    def plan(self, text: str, *listed: str, head: str = FLAKY_HEAD, env: dict | None = None):
        flaky = os.path.join(self.tmp, "flaky.tsv")
        with open(flaky, "w") as f:
            f.write(head + "".join(f"{LIB}\t{n}\t#999\t2026-11-09\n" for n in listed))
        e = {k: v for k, v in os.environ.items() if k != "EVE_GATE_NO_RERUN"}
        e.update(env or {})
        return subprocess.run([sys.executable, "-I", TOOL, "rerun-plan", "--log", self.log(text), "--flaky", flaky],
                              capture_output=True, text=True, timeout=60, env=e)

    def test_a_listed_failure_is_planned_with_cargos_own_arguments(self) -> None:
        r = self.plan(red("a::b"), "a::b")
        self.assertEqual((r.returncode, r.stdout), (0, "rerun -p eve_invented --lib -- --exact a::b\n"), r.stderr)

    def test_each_failure_goes_to_the_binary_that_printed_it(self) -> None:
        text = ("== test\n   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\ntest lib::x ... FAILED\n"
                "test result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p eve_invented --lib`\n"
                "   Running tests/bots/main.rs (target/debug/deps/bots-99aa77ff)\ntest sock::y ... FAILED\n"
                "test result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p eve_bot --test bots`\n")
        flaky = os.path.join(self.tmp, "f2.tsv")
        with open(flaky, "w") as f:
            f.write(FLAKY_HEAD + f"{LIB}\tlib::x\t#1\t2026-11-09\nbots:tests/bots/main.rs\tsock::y\t#2\t2026-11-09\n")
        r = subprocess.run([sys.executable, "-I", TOOL, "rerun-plan", "--log", self.log(text), "--flaky", flaky],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.stdout.splitlines(), ["rerun -p eve_invented --lib -- --exact lib::x",
                                                 "rerun -p eve_bot --test bots -- --exact sock::y"], r.stdout)

    def test_the_summary_block_supplies_the_arguments_when_there_is_no_inline_line(self) -> None:
        text = red("a::b", tail="error: 1 target failed:\n    `-p eve_invented --lib`\n")
        r = self.plan(text, "a::b")
        self.assertEqual(r.stdout, "rerun -p eve_invented --lib -- --exact a::b\n")

    def no_rerun(self, r, why: str) -> None:
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertEqual(r.stdout, f"no-rerun: {why}\n")

    def test_an_unlisted_test_is_a_new_red_and_never_reruns(self) -> None:
        self.no_rerun(self.plan(red("a::b", "c::d"), "a::b"), f"c::d on {LIB} is not on the flake list")

    def test_the_target_must_match_too(self) -> None:
        text = red("a::b").replace("eve_invented-0123abcd", "eve_other-0123abcd")
        self.no_rerun(self.plan(text, "a::b"), "a::b on eve_other:src/lib.rs is not on the flake list")

    def test_a_panic_outside_the_test_threads_never_reruns(self) -> None:
        text = red("a::b", extra="thread 'world-ticker' panicked at src/lib.rs:1:1:\n")
        self.no_rerun(self.plan(text, "a::b"), "a panic outside the test threads (world-ticker)")

    def test_four_failures_never_rerun_and_three_do(self) -> None:
        names = ["t::a", "t::b", "t::c", "t::d"]
        self.no_rerun(self.plan(red(*names), *names), "4 failures (at most 3 are rerun)")
        self.assertEqual(self.plan(red(*names[:3]), *names[:3]).returncode, 0)

    def test_a_compile_error_never_reruns(self) -> None:
        text = "== test\nerror[E0308]: mismatched types\nerror: could not compile `eve_invented`\n"
        self.no_rerun(self.plan(text, "a::b"), "a compile error")

    def test_a_run_without_test_result_lines_never_reruns(self) -> None:
        text = red("a::b").replace("test result: FAILED. 1 passed; 1 failed\n", "")
        self.no_rerun(self.plan(text, "a::b"), "the run printed no 'test result:' lines")

    def test_a_failure_that_is_not_a_test_never_reruns(self) -> None:
        text = "== python unit tests\ngate: tools/test_x.py failed\ntest result: ok. 1 passed\n"
        r = self.plan(text, "a::b")
        self.assertEqual(r.returncode, 1)
        self.assertTrue(r.stdout.startswith("no-rerun: a red that is not a failing test"), r.stdout)

    def test_no_rerun_arguments_in_the_log_means_no_rerun(self) -> None:
        self.no_rerun(self.plan(red("a::b", tail=""), "a::b"), f"no 'to rerun pass' arguments for {LIB}")

    def test_the_switch_and_an_empty_list(self) -> None:
        self.no_rerun(self.plan(red("a::b"), "a::b", env={"EVE_GATE_NO_RERUN": "1"}), "EVE_GATE_NO_RERUN is set")
        self.no_rerun(self.plan(red("a::b")), "the flake list has no rows")

    def test_a_bad_flake_list_is_refused(self) -> None:
        for head, why in (
            ("target\ttest\tissue\tadded\n", "does not start with '#flaky 1'"),
            ("#flaky 1\ntarget\ttest\n", "the column header must be"),
        ):
            r = self.plan(red("a::b"), "a::b", head=head)
            self.assertEqual(r.returncode, 1)
            self.assertIn(why, r.stdout)
        flaky = os.path.join(self.tmp, "bad.tsv")
        for row, why in ((f"{LIB}\ta::b\t999\t2026-11-09\n", "must cite an issue as #N"),
                         (f"{LIB}\ta::b\t#9\tyesterday\n", "must be a date"),
                         (f"{LIB}\ta::b\t#9\n", "4 tab-separated fields")):
            with open(flaky, "w") as f:
                f.write(FLAKY_HEAD + row)
            r = subprocess.run([sys.executable, "-I", TOOL, "rerun-plan", "--log", self.log(red("a::b")),
                                "--flaky", flaky], capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 1)
            self.assertIn(why, r.stdout)
        r = self.run_tool("rerun-plan", "--log", self.log(red("a::b")), "--flaky", os.path.join(self.tmp, "none"))
        self.assertEqual(r.returncode, 1)
        self.assertIn("no-rerun: cannot read", r.stdout)

    # --- fail closed on anything the plan cannot account for (review F1, F3) ----------------

    BOTS = ("   Running tests/bots/main.rs (target/debug/deps/bots-99aa77ff)\n")

    def test_a_second_binary_that_dies_with_no_failure_or_result_line_blocks_the_rerun(self) -> None:
        text = red("a::b") + self.BOTS + "error: test failed, to rerun pass `-p eve_bot --test bots`\n"
        r = self.plan(text, "a::b")
        self.no_rerun(r, "bots:tests/bots/main.rs failed with no failing test named (it crashed or stopped)")

    def test_a_segfault_after_the_flake_failed_in_the_same_binary_blocks_the_rerun(self) -> None:
        text = red("a::b", extra="error: test failed, to rerun pass `-p eve_invented --lib`\n"
                   "Caused by:\n  process didn't exit successfully: `deps/x` (signal: 11, SIGSEGV)\n")
        r = self.plan(text, "a::b")
        self.assertEqual(r.returncode, 1)
        self.assertIn("a crash marker", r.stdout)
        for marker in ("fatal runtime error: stack overflow", "thread 'x' has overflowed its stack"):
            self.assertIn("crash marker", self.plan(red("a::b", extra=marker + "\n"), "a::b").stdout)

    def job_log(self, *jobs: tuple[str, str]) -> str:
        out = ""
        for job, body in jobs:
            out += f"   -- cargo test --locked --no-fail-fast {job}\n{body}"
        for job, body in jobs:
            if "FAILED" in body or "error:" in body:
                out += f"test-parallel: cargo test --locked --no-fail-fast {job} FAILED\n"
        return out

    def test_a_second_job_that_fails_without_a_test_blocks_the_rerun(self) -> None:
        body = red("a::b").replace("== test\n", "")
        text = self.job_log(("-p eve_invented --lib", body),
                            ("-p eve_nowhere --doc", "error: no test target named `x` in default-run packages\n"))
        self.assertEqual(self.plan(text, "a::b").returncode, 1)
        text = self.job_log(("-p eve_invented --lib", body), ("-p eve_other --lib", "test result: ok. 1 passed\n"))
        self.assertEqual(self.plan(text, "a::b").returncode, 0)

    def test_a_failed_job_with_no_planned_binary_is_named(self) -> None:
        body = red("a::b").replace("== test\n", "")
        text = self.job_log(("-p eve_invented --lib", body), ("-p eve_other --lib", "test result: ok. 1 passed\n"))
        text += "test-parallel: cargo test --locked --no-fail-fast -p eve_other --lib FAILED\n"
        r = self.plan(text, "a::b")
        self.no_rerun(r, "the job '-p eve_other --lib' failed with no failing test of its own")

    def test_a_failure_closed_by_another_binarys_rerun_line_blocks_the_rerun(self) -> None:
        text = ("== test\n   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\n"
                "test a::b ... FAILED\n" + self.BOTS
                + "error: test failed, to rerun pass `-p eve_bot --test bots`\n"
                "test result: FAILED. 0 passed; 1 failed\n")
        r = self.plan(text, "a::b")
        self.assertEqual(r.returncode, 1)
        self.assertIn("no-rerun: ", r.stdout)

    def test_a_result_line_that_counts_other_failures_blocks_the_rerun(self) -> None:
        text = red("a::b").replace("1 passed; 1 failed", "1 passed; 2 failed")
        self.no_rerun(self.plan(text, "a::b"), f"{LIB}: 2 failed but 1 named")

    def test_the_summary_count_must_match_the_failing_binaries(self) -> None:
        text = red("a::b", tail="error: 2 targets failed:\n    `-p eve_invented --lib`\n    `-p eve_x --lib`\n")
        self.no_rerun(self.plan(text, "a::b"), "cargo counts 2 failed target(s), the log holds 1")

    def test_rerun_arguments_must_be_of_the_plain_form(self) -> None:
        for bad in ("-p eve_invented --lib --config build.rustflags=['x']", "--config x=1 -p a --lib",
                    "-p eve_invented --lib; rm -rf /nonexistent/probe", "--workspace", "-p a --test"):
            r = self.plan(red("a::b", tail=f"error: test failed, to rerun pass `{bad}`\n"), "a::b")
            self.assertEqual(r.returncode, 1, bad)
            self.assertIn("not of the plain form", r.stdout)
        for ok in ("-p eve-bot --test bots", "-p eve_invented --bin tool", "-p eve_invented --lib"):
            r = self.plan(red("a::b", tail=f"error: test failed, to rerun pass `{ok}`\n"), "a::b")
            self.assertEqual(r.returncode, 0, ok)

    def test_the_shipped_list_is_valid(self) -> None:
        r = self.run_tool("rerun-plan", "--log", self.log(red("a::b")), "--flaky", os.path.join(HERE, "flaky.tsv"))
        self.no_rerun(r, "the flake list has no rows")


class Rescued(Base):
    PASS_LOG = red("t::a", "t::b") + "== flaky rerun: 2 test(s) on the flake list (once)\n== flaky rerun passed\n== gate passed\n"

    def test_a_rescued_pass_records_the_first_failures_and_a_pass_row(self) -> None:
        r = self.record(self.PASS_LOG, result="pass", code=0)
        self.assertEqual(r.returncode, 0, r.stderr)
        got = [(row[3], row[7], row[9]) for row in self.rows()]
        self.assertEqual(got, [("rescued", "test", "t::a"), ("rescued", "test", "t::b"), ("pass", "-", "-")])

    def test_a_plain_pass_has_no_rescued_row(self) -> None:
        self.record("== gate passed\n", result="pass", code=0)
        self.assertEqual([row[3] for row in self.rows()], ["pass"])

    def test_rescues_count_as_flaky_in_the_report(self) -> None:
        self.record(self.PASS_LOG, result="pass", code=0)
        out = self.run_tool("report", "--file", self.file, "--flaky", os.path.join(self.tmp, "none")).stdout
        line = [ln for ln in out.splitlines() if "t::a" in ln][0]
        self.assertEqual(line.split()[:3], ["1", "1", "1"], out)

    def test_a_row_with_five_rescues_or_older_than_two_weeks_is_fix_or_drop(self) -> None:
        for i in range(5):
            self.record(self.PASS_LOG.replace("t::b", "t::z"), result="pass", code=0, tree=str(i) * 40)
        flaky = os.path.join(self.tmp, "flaky.tsv")
        today = __import__("datetime").datetime.now(__import__("datetime").timezone.utc).date().isoformat()
        with open(flaky, "w") as f:
            f.write(FLAKY_HEAD + f"{LIB}\tt::a\t#1\t{today}\n{LIB}\tt::z\t#2\t2020-01-01\n{LIB}\tt::q\t#3\t{today}\n")
        out = self.run_tool("report", "--file", self.file, "--flaky", flaky).stdout
        self.assertIn(f"FIX-OR-DROP {LIB} t::a #1 (5 rescues)", out)
        self.assertIn("t::z #2 (5 rescues, listed ", out)
        self.assertNotIn("t::q", out)


class KeepLog(Base):
    def test_the_newest_fifty_logs_stay(self) -> None:
        d = os.path.join(self.tmp, "logs")
        os.makedirs(d)
        old = [f"20200101T0000{i:02d}Z-deadbeef.log" for i in range(55)]
        for n in old:
            open(os.path.join(d, n), "w").close()
        r = self.run_tool("keep-log", "--log", self.log("x\n"), "--sha", SHA, "--dir", d)
        self.assertEqual(r.returncode, 0, r.stderr)
        new = r.stdout.strip()
        self.assertTrue(os.path.exists(new))
        left = sorted(os.listdir(d))
        self.assertEqual(len(left), 50)
        self.assertIn(os.path.basename(new), left)
        for n in old[:6]:
            self.assertNotIn(n, left)
        self.assertIn(old[-1], left)


if __name__ == "__main__":
    unittest.main()
