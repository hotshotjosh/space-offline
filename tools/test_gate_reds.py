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
