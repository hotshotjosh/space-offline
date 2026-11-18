"""Tests for tools/status-guard.py, on invented paths and a throwaway git repo.

Run::

    python3 -I tools/test_status_guard.py
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

# Hermetic git: no user or system config, no signing, no global hooks, and none of the
# variables that point git at another repository. Built per call, never set globally.
_GIT_LEAK = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
             "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR")


def hermetic_env(**extra):
    e = {k: v for k, v in os.environ.items() if k not in _GIT_LEAK}
    e.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
              "GIT_CONFIG_COUNT": "2",
              "GIT_CONFIG_KEY_0": "commit.gpgsign", "GIT_CONFIG_VALUE_0": "false",
              "GIT_CONFIG_KEY_1": "core.hooksPath", "GIT_CONFIG_VALUE_1": os.devnull,
              "GIT_TERMINAL_PROMPT": "0"})
    e.update(extra)
    return e


HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "status-guard.py")

_spec = importlib.util.spec_from_file_location("status_guard", SCRIPT)
sg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sg)


class DefaultBase(unittest.TestCase):
    def test_prefers_origin_head_then_main_then_master(self):
        with tempfile.TemporaryDirectory() as d:
            env = hermetic_env(GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@test.invalid",
                               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@test.invalid")
            patcher = mock.patch.dict(os.environ, env, clear=True)  # in-process calls too
            patcher.start()
            self.addCleanup(patcher.stop)
            g = lambda *a: subprocess.run(["git", *a], cwd=d, env=env, check=True,
                                          capture_output=True, text=True).stdout.strip()
            g("init", "-q", "-b", "work")
            g("commit", "-q", "--allow-empty", "-m", "x")
            sha = g("rev-parse", "HEAD")
            old = os.getcwd()
            os.chdir(d)
            try:
                self.assertIsNone(sg.default_base())
                self.assertEqual(sg.main([]), 2)  # no base: fails closed
                g("update-ref", "refs/remotes/origin/master", sha)
                self.assertEqual(sg.default_base(), "refs/remotes/origin/master")
                g("update-ref", "refs/remotes/origin/main", sha)
                self.assertEqual(sg.default_base(), "refs/remotes/origin/main")
                g("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main")
                self.assertEqual(sg.default_base(), "refs/remotes/origin/HEAD")
                # a fork: upstream wins over a stale origin
                g("update-ref", "refs/remotes/upstream/master", sha)
                self.assertEqual(sg.default_base(), "refs/remotes/upstream/master")
                g("update-ref", "refs/remotes/upstream/main", sha)
                self.assertEqual(sg.default_base(), "refs/remotes/upstream/main")
                g("symbolic-ref", "refs/remotes/upstream/HEAD", "refs/remotes/upstream/main")
                self.assertEqual(sg.default_base(), "refs/remotes/upstream/HEAD")
            finally:
                os.chdir(old)


class Decide(unittest.TestCase):
    def test_docs_only_passes(self):
        self.assertTrue(sg.decide(["docs" + "/guides/x.md"], [""])[0])

    def test_server_without_progress_fails(self):
        ok, msg = sg.decide(["server/crates/a/src/b.rs"], ["Fix b"])
        self.assertFalse(ok)
        self.assertIn("server/crates/a/src/b.rs", msg)

    def test_content_and_scenarios_are_watched(self):
        self.assertFalse(sg.decide(["content/x.tsv"], [""])[0])
        self.assertFalse(sg.decide(["tools/client-ci/scenarios/fci_x.py"], [""])[0])

    def test_other_tools_are_not_watched(self):
        self.assertTrue(sg.decide(["tools/client-ci/run.sh", "tools/x.py"], [""])[0])

    def test_server_with_progress_passes(self):
        self.assertTrue(sg.decide(["server/a.rs", "docs/status/progress.tsv"], [""])[0])

    def test_waiver_in_commit_message_passes(self):
        self.assertTrue(sg.decide(["server/a.rs"], ["Refactor\n\nStatus: none (rename only)\n"])[0])

    def test_waiver_in_pr_body_passes(self):
        self.assertTrue(sg.decide(["server/a.rs"], ["Refactor", "body\r\nStatus: none (tests only)\r\n"])[0])

    def test_empty_reason_fails(self):
        self.assertFalse(sg.decide(["server/a.rs"], ["Status: none ( )"])[0])

    def test_a_placeholder_reason_fails(self):
        for reason in ("<why>", "<why no feature state changed>", "...", "\u2026"):
            self.assertFalse(sg.decide(["server/a.rs"], [f"Status: none ({reason})"])[0], reason)

    def test_waiver_must_start_a_line(self):
        self.assertFalse(sg.decide(["server/a.rs"], ["see Status: none (x)"])[0])


class Cli(unittest.TestCase):
    def run_git(self, d, *args):
        subprocess.run(["git", "-C", d, *args], check=True, capture_output=True,
                       env=hermetic_env())

    def write(self, d, rel, text):
        os.makedirs(os.path.dirname(os.path.join(d, rel)), exist_ok=True)
        with open(os.path.join(d, rel), "w", encoding="utf-8") as f:
            f.write(text)

    def guard(self, d, *args, body=""):
        env = hermetic_env(PR_BODY=body)
        return subprocess.run([sys.executable, "-I", SCRIPT, *args], cwd=d, env=env,
                              capture_output=True, text=True).returncode

    def test_end_to_end(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_git(d, "init", "-q")
            self.write(d, "README.md", "x\n")
            self.run_git(d, "add", "-A")
            self.run_git(d, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base")
            self.run_git(d, "tag", "base")
            self.write(d, "server/a.rs", "fn a() {}\n")
            self.run_git(d, "add", "-A")
            self.run_git(d, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "code")
            self.assertEqual(self.guard(d, "--base", "base"), 1)
            self.assertEqual(self.guard(d, "--base", "base", body="Status: none (no feature)"), 0)
            self.write(d, "docs/status/progress.tsv", "row\n")
            self.assertEqual(self.guard(d, "--base", "base"), 1)
            self.assertEqual(self.guard(d, "--base", "base", "--worktree"), 0)
            self.assertEqual(self.guard(d, "--base", "no-such-ref"), 2)

    def commit(self, d, msg):
        self.run_git(d, "add", "-A")
        self.run_git(d, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", msg)

    def test_a_waiver_in_an_earlier_commit_of_the_range_counts(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_git(d, "init", "-q")
            self.write(d, "README.md", "x\n")
            self.commit(d, "base")
            self.run_git(d, "tag", "base")
            self.write(d, "server/a.rs", "fn a() {}\n")
            self.commit(d, "code\n\nStatus: none (invented reason)")
            self.write(d, "README.md", "y\n")
            self.commit(d, "fix-up")
            self.assertEqual(self.guard(d, "--base", "base"), 0)

    def test_a_waiver_before_the_base_does_not_count(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_git(d, "init", "-q")
            self.write(d, "README.md", "x\n")
            self.commit(d, "base\n\nStatus: none (old waiver)")
            self.run_git(d, "tag", "base")
            self.write(d, "server/a.rs", "fn a() {}\n")
            self.commit(d, "code")
            self.assertEqual(self.guard(d, "--base", "base"), 1)


if __name__ == "__main__":
    unittest.main()
