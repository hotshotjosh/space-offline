"""Tests for tools/red-on-main.py, on a scratch git repo with invented tests.

Run::

    python3 -I tools/test_red_on_main.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SCRIPT = os.path.join(HERE, "red-on-main.py")

ENV = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@test.invalid",
           GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@test.invalid")


UNITTEST_MAIN = """import unittest
class T(unittest.TestCase):
    def test_red_everywhere(self):
        self.fail("invented")
if __name__ == "__main__":
    unittest.main()
"""
UNITTEST_BRANCH_EXTRA = """
class U(unittest.TestCase):
    def test_red_only_here(self):
        self.fail("invented")
"""


class RedOnMain(unittest.TestCase):
    def setUp(self) -> None:
        # on disk under var/ (gitignored), never /tmp
        var = os.path.join(ROOT, "var")
        os.makedirs(var, exist_ok=True)
        self.repo = tempfile.mkdtemp(prefix="red-on-main-", dir=var)
        self.addCleanup(shutil.rmtree, self.repo, True)
        self.git("init", "-q", "-b", "main")
        self.write("tools/test_always_red.py", "raise SystemExit(1)\n")
        self.write("tools/test_branch_red.py", "raise SystemExit(0)\n")
        self.write("tools/test_ids.py", UNITTEST_MAIN)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "main")
        self.git("checkout", "-q", "-b", "work")
        self.write("tools/test_branch_red.py", "raise SystemExit(1)\n")
        self.write("tools/test_ids.py", UNITTEST_MAIN.replace("if __name__", UNITTEST_BRANCH_EXTRA + "if __name__"))
        self.git("commit", "-q", "-am", "branch")

    def git(self, *a: str) -> str:
        return subprocess.run(["git", *a], cwd=self.repo, env=ENV, check=True,
                              capture_output=True, text=True).stdout

    def write(self, rel: str, text: str) -> None:
        p = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def run_tool(self, log_lines: list[str], *extra: str) -> subprocess.CompletedProcess:
        self.write("gate.log", "\n".join(log_lines) + "\n")
        return subprocess.run(
            [sys.executable, "-I", SCRIPT, "--repo", self.repo, "--base", "main",
             "--log", os.path.join(self.repo, "gate.log"), *extra],
            capture_output=True, text=True, timeout=120)

    def worktree_gone(self) -> None:
        self.assertFalse(os.path.exists(os.path.join(self.repo, "out", "red-on-main-wt")))
        self.assertNotIn("red-on-main-wt", self.git("worktree", "list"))

    def test_a_red_that_main_has_too_is_reported_as_such(self) -> None:
        r = self.run_tool(["== tools tests", "gate: tools/test_always_red.py failed"])
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(r.stdout.splitlines(),
                         ["Gate: red-on-main-too", "red: tools/test_always_red.py: branch exit 1, main exit 1"])
        self.worktree_gone()

    def test_a_red_only_the_branch_has_is_this_changes_fault(self) -> None:
        r = self.run_tool(["gate: tools/test_always_red.py failed", "gate: tools/test_branch_red.py failed"])
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stdout.splitlines(), [
            "Gate: red-caused-by-this-change",
            "red: tools/test_always_red.py: branch exit 1, main exit 1",
            "red: tools/test_branch_red.py: branch exit 1, main exit 0",
        ])
        self.worktree_gone()

    def test_a_file_red_on_main_with_a_new_failing_test_is_this_changes_fault(self) -> None:
        r = self.run_tool(["gate: tools/test_ids.py failed"])
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stdout.splitlines()[0], "Gate: red-caused-by-this-change")
        self.assertIn("fails only on the branch: test_red_only_here", r.stdout)
        self.assertNotIn("test_red_everywhere", r.stdout)

    def test_a_name_that_is_not_one_of_our_test_files_is_listed_and_never_run(self) -> None:
        r = self.run_tool(["gate: /nonexistent/probe/x.py failed", "gate: ../nonexistent/probe/x.py failed",
                           "gate: tools/test_always_red.py failed"])
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stdout.splitlines()[0], "Gate: red-unproven")
        self.assertIn("red: /nonexistent/probe/x.py (not rerun)", r.stdout)
        self.assertIn("red: ../nonexistent/probe/x.py (not rerun)", r.stdout)

    def test_a_cargo_red_is_listed_but_not_proven(self) -> None:
        r = self.run_tool(["test invented::case ... FAILED", "gate: tools/test_always_red.py failed"])
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(r.stdout.splitlines()[0], "Gate: red-unproven")
        self.assertIn("red: tools/test_always_red.py: branch exit 1, main exit 1", r.stdout)
        self.assertIn("red: invented::case (cargo; rerun by hand on main)", r.stdout)
        self.worktree_gone()

    def test_no_log_or_a_missing_base_exits_2(self) -> None:
        r = subprocess.run([sys.executable, "-I", SCRIPT, "--repo", self.repo, "--base", "main"],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 2, r.stdout)
        r = self.run_tool(["gate: tools/test_always_red.py failed"], "--base", "nonexistent-probe")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.worktree_gone()


if __name__ == "__main__":
    unittest.main()
