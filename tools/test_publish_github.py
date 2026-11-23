#!/usr/bin/env python3
"""Tests for tools/publish-github.py, on invented repositories (a bare one stands in for GitHub)."""
import sys as _sys
_sys.path.append(_sys.path.pop(0))

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PUB = os.path.join(HERE, "publish-github.py")
ALIAS = "thewhitekidney <0x81e70020@outlook.com>"
PREFIXES_NONE = ("# Docs the public export omits on purpose, as sha256 of each path (written by "
                 "tools/public-export.py --write-prefixes).\n")
_LEAK = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
         "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR")


def env():
    e = {k: v for k, v in os.environ.items() if k not in _LEAK}
    e.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_COUNT": "2",
              "GIT_CONFIG_KEY_0": "commit.gpgsign", "GIT_CONFIG_VALUE_0": "false",
              "GIT_CONFIG_KEY_1": "core.hooksPath", "GIT_CONFIG_VALUE_1": os.devnull,
              "GIT_TERMINAL_PROMPT": "0"})
    return e


def git(cwd, *args, who=("tester", "tester@example.invalid"), ok=True):
    p = subprocess.run(["git", "-c", f"user.name={who[0]}", "-c", f"user.email={who[1]}", "-C", cwd, *args],
                       env=env(), capture_output=True, text=True)
    if ok and p.returncode != 0:
        raise AssertionError(f"git {args}: {p.stderr}")
    return p.stdout.strip()


def write(base, rel, text):
    p = os.path.join(base, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(text)


class Publish(unittest.TestCase):
    def setUp(self):
        os.makedirs(os.path.join(ROOT, "var"), exist_ok=True)
        self.tmp = tempfile.mkdtemp(prefix="test-publish-", dir=os.path.join(ROOT, "var"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.gh = os.path.join(self.tmp, "gh.git")
        git(self.tmp, "init", "-q", "--bare", "-b", "main", self.gh)
        # GitHub's history: one public commit
        seed = os.path.join(self.tmp, "seed")
        git(self.tmp, "init", "-q", "-b", "main", seed)
        write(seed, "README.md", "hello\n")
        write(seed, "tools/doc-private-prefixes.txt", PREFIXES_NONE)
        git(seed, "add", "-A")
        git(seed, "commit", "-qm", "Initial public export", who=("thewhitekidney", "0x81e70020@outlook.com"))
        git(seed, "push", "-q", self.gh, "main")
        # the private repo: GitHub's history plus the private paths
        self.priv = os.path.join(self.tmp, "priv")
        git(self.tmp, "clone", "-q", "-o", "github", self.gh, self.priv)
        write(self.priv, "private/export-scan.txt", "invented-secret-name\n")
        write(self.priv, "private/export-private-paths.txt", "findings/\n")
        write(self.priv, "findings/table.tsv", "row\n")
        git(self.priv, "add", "-A")
        git(self.priv, "commit", "-qm", "Maintainer lines: the private overlay")

    def pub(self, *args):
        return subprocess.run([sys.executable, "-I", PUB, *args, "--root", self.priv],
                              env=env(), capture_output=True, text=True)

    def gh_main(self):
        return git(self.gh, "rev-parse", "main")

    def land(self, files, msg):
        for rel, text in files.items():
            write(self.priv, rel, text)
        git(self.priv, "add", "-A")
        git(self.priv, "commit", "-qm", msg)

    def test_public_commits_are_published_private_ones_are_not(self):
        before = self.gh_main()
        self.land({"lib/a.txt": "a\n"}, "A public fix from the private history (#1)")
        self.land({"private/notes.md": "n\n"}, "Maintainer lines: a private note")
        self.land({"lib/b.txt": "b\n", "findings/table.tsv": "row2\n"},
                  "A mixed fix (#2)\n\nIt also moves findings/table.tsv.\nA public line stays.")
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        tip = self.gh_main()
        log = git(self.gh, "log", "--format=%an <%ae>|%s", f"{before}..{tip}").split("\n")
        # the word "private" is not a private path: the subject stays
        self.assertEqual(log, [f"{ALIAS}|A mixed fix (#2)", f"{ALIAS}|A public fix from the private history (#1)"])
        self.assertEqual(set(git(self.gh, "log", "--format=%ai%n%ci", f"{before}..{tip}").split("\n")) and
                         {d[-5:] for d in git(self.gh, "log", "--format=%ai%n%ci", f"{before}..{tip}").split("\n")},
                         {"+0000"})
        body = git(self.gh, "log", "-1", "--format=%b", tip)
        self.assertIn("A public line stays.", body)
        self.assertNotIn("findings/", body)
        files = git(self.gh, "ls-tree", "-r", "--name-only", tip).split("\n")
        self.assertIn("lib/b.txt", files)
        self.assertFalse([f for f in files if f.startswith(("private/", "findings/"))])
        # the same history gives the same public commits: nothing new to push
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.gh_main(), tip)
        self.assertIn("0 new commit(s)", r.stdout)

    def test_a_merged_pr_is_pulled_and_kept_byte_for_byte(self):
        self.land({"lib/a.txt": "a\n"}, "A public fix (#1)")
        self.assertEqual(self.pub("push").returncode, 0)
        # a contributor's PR merges on GitHub
        other = os.path.join(self.tmp, "other")
        git(self.tmp, "clone", "-q", self.gh, other)
        write(other, "lib/pr.txt", "from a contributor\n")
        git(other, "add", "-A")
        git(other, "commit", "-qm", "Contributor fix", who=("Pat", "1234+pat@users.noreply.github.com"))
        git(other, "push", "-q", "origin", "HEAD:main")
        pr = self.gh_main()
        # our next landing, before pulling: push refuses
        self.land({"lib/c.txt": "c\n"}, "Another fix (#3)")
        r = self.pub("push")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("run publish-github.py pull", r.stderr)
        self.assertEqual(self.gh_main(), pr)
        r = self.pub("pull", "--check")
        self.assertEqual(r.returncode, 1)
        self.assertIn("has commits not merged here", r.stderr)
        r = self.pub("pull")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.pub("pull", "--check").returncode, 0)
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        tip = self.gh_main()
        self.assertTrue(git(self.gh, "merge-base", "--is-ancestor", pr, tip, ok=False) == "")
        self.assertEqual(git(self.gh, "log", "-1", "--format=%an <%ae>", pr),
                         "Pat <1234+pat@users.noreply.github.com>")
        self.assertEqual(git(self.gh, "show", f"{tip}:lib/pr.txt"), "from a contributor")
        self.assertEqual(git(self.gh, "show", f"{tip}:lib/c.txt"), "c")
        # and life goes on: the next landing is a plain fast-forward on top
        self.assertIn("already merged", self.pub("pull").stdout)
        self.land({"lib/d.txt": "d\n"}, "A later fix (#5)")
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(git(self.gh, "rev-parse", "main~1"), tip)

    def test_a_github_commit_on_a_private_path_is_refused(self):
        other = os.path.join(self.tmp, "other")
        git(self.tmp, "clone", "-q", self.gh, other)
        write(other, "findings/planted.tsv", "x\n")
        git(other, "add", "-A")
        git(other, "commit", "-qm", "Planted")
        git(other, "push", "-q", "origin", "HEAD:main")
        r = self.pub("pull")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("findings/planted.tsv", r.stderr)

    def test_old_github_history_may_hold_paths_that_are_private_now(self):
        other = os.path.join(self.tmp, "other")
        git(self.tmp, "clone", "-q", self.gh, other)
        write(other, "findings/old.tsv", "public once\n")
        git(other, "add", "-A")
        git(other, "commit", "-qm", "Once public")
        git(other, "rm", "-q", "findings/old.tsv")
        git(other, "commit", "-qm", "Made private")
        git(other, "push", "-q", "origin", "HEAD:main")
        # merging history whose tip no longer holds it is fine...
        r = self.pub("pull")
        self.assertNotEqual(r.returncode, 0)   # ...but pull checks each new commit: refused
        self.assertIn("findings/old.tsv", r.stderr)
        # a private tree that already carries that history publishes on top of it
        git(self.priv, "fetch", "-q", "github")
        git(self.priv, "merge", "-q", "--no-edit", "github/main")
        self.land({"lib/a.txt": "a\n"}, "A public fix (#6)")
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_a_later_change_to_the_private_list_does_not_remap_published_commits(self):
        self.land({"lib/a.txt": "a\n", "tools/old-mirror.sh": "x\n",
                   "private/export-private-paths.txt": "findings/\ntools/old-mirror.sh\n"},
                  "A fix with a private tool (#7)")
        self.assertEqual(self.pub("push").returncode, 0)
        pushed = self.gh_main()
        # the tool is retired: gone from the tree and from the private list
        os.remove(os.path.join(self.priv, "tools/old-mirror.sh"))
        self.land({"private/export-private-paths.txt": "findings/\n", "lib/b.txt": "b\n"},
                  "The old tool is gone (#8)")
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(git(self.gh, "rev-parse", "main~1"), pushed)

    def test_a_leaking_subject_is_withheld(self):
        self.land({"lib/a.txt": "a\n"}, "Fix for invented-secret-name (#4)\n\nfine line")
        r = self.pub("push")
        self.assertEqual(r.returncode, 0, r.stderr)
        msg = git(self.gh, "log", "-1", "--format=%B", "main")
        self.assertNotIn("invented-secret-name", msg)
        self.assertTrue(msg.startswith("Update lib"))
        self.assertIn("fine line", msg)


if __name__ == "__main__":
    unittest.main()
