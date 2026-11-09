"""Tests for the base and approval checks of tools/import-pr.sh.

Stdlib only; run with `python3 -I tools/test_import_pr.py`. Scratch git repos
with invented names under var/; a local bare repo stands in for GitHub through
the script's MIRROR_REMOTE variable, so no network call and no gh is involved.
"""

from __future__ import annotations

import os
import pty
import shutil
import subprocess
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMPORT = os.path.join(ROOT, "tools", "import-pr.sh")


def run(cmd, env=None, stdin=subprocess.DEVNULL):
    e = dict(os.environ)
    e.update({"GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
              "GIT_TERMINAL_PROMPT": "0"})
    e.update(env or {})
    return subprocess.run(cmd, env=e, stdin=stdin, capture_output=True, text=True)


def git(cwd, *args):
    p = run(["git", "-c", "user.name=tester", "-c", "user.email=tester@example.invalid",
             "-C", cwd, *args])
    assert p.returncode == 0, f"git {args} failed: {p.stdout}{p.stderr}"
    return p.stdout.strip()


def write(base, rel, text):
    path = os.path.join(base, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


class ImportPrTest(unittest.TestCase):
    def setUp(self):
        os.makedirs(os.path.join(ROOT, "var"), exist_ok=True)
        self.tmp = tempfile.mkdtemp(prefix="test-import-pr-", dir=os.path.join(ROOT, "var"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.bare = self.p("gh.git")
        assert run(["git", "init", "-q", "--bare", self.bare]).returncode == 0
        self.mirror = self.p("mirror")
        os.makedirs(self.mirror)
        git(self.mirror, "init", "-q", "-b", "main")
        write(self.mirror, "README.md", "hi\n")
        git(self.mirror, "add", "-A")
        git(self.mirror, "commit", "-qm", "root")
        git(self.mirror, "push", "-q", self.bare, "main")
        self.env = {"MIRROR_DIR": self.mirror, "MIRROR_REMOTE": self.bare,
                    "MIRROR_BRANCH": "main", "LANES": self.p("lanes")}

    def p(self, *a):
        return os.path.join(self.tmp, *a)

    def clone(self, name):
        w = self.p(name)
        assert run(["git", "clone", "-q", self.bare, w]).returncode == 0
        git(w, "checkout", "-q", "-B", "work", "origin/main")
        return w

    def pr(self, n):
        w = self.clone(f"w{n}")
        write(w, "src/x.rs", f"fn x{n}() {{}}\n")
        git(w, "add", "-A")
        git(w, "commit", "-q", "--author=Pat <1+pat@users.noreply.github.com>", "-m", f"PR {n}")
        git(w, "push", "-q", self.bare, f"work:refs/pull/{n}/head")
        return git(w, "rev-parse", "HEAD")

    def tool(self, *args, tty=False):
        if not tty:
            return run(["bash", IMPORT, *args], env=self.env)
        master, slave = pty.openpty()
        try:
            return run(["bash", IMPORT, *args], env=self.env, stdin=slave)
        finally:
            os.close(master)
            os.close(slave)

    def test_a_main_that_is_not_the_mirrors_last_push_is_refused(self):
        self.pr(1)
        w = self.clone("other")
        write(w, "extra.txt", "outside\n")
        git(w, "add", "-A")
        git(w, "commit", "-qm", "outside change")
        git(w, "push", "-q", self.bare, "work:main")
        r = self.tool("1")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not the mirror's last push", r.stderr)
        self.assertFalse(os.path.exists(self.p("lanes", "handback", "pr-1", "pr.diff")))

    def test_a_matching_main_is_imported(self):
        self.pr(2)
        r = self.tool("2")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_approve_is_refused_when_stdin_is_not_a_tty(self):
        self.pr(5)
        self.assertEqual(self.tool("5").returncode, 0)
        r = self.tool("approve", "5")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("never approves", r.stderr)
        self.assertFalse(os.path.exists(self.p("lanes", "handback", "pr-5", "approved")))

    def test_approve_works_in_a_terminal(self):
        self.pr(6)
        self.assertEqual(self.tool("6").returncode, 0)
        r = self.tool("approve", "6", tty=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue(os.path.exists(self.p("lanes", "handback", "pr-6", "approved")))


if __name__ == "__main__":
    unittest.main()
