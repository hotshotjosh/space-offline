#!/usr/bin/env python3
"""Tests for tools/pr_overlap.py, on a fake tracker and scratch git repos. Invented paths."""

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.parse
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pr_overlap as po  # noqa: E402
import testenv  # noqa: E402
import tracker  # noqa: E402

REPO = "/repos/acme/game"
TITLE_SENTINEL = "TITLE-SENTINEL-91ab"
BODY_SENTINEL = "BODY-SENTINEL-77cd"


def patch(*hunks):
    return "\n".join(f"@@ -{h} +1 @@\n-x\n+y" for h in hunks)


def pr(n, login, ref="x", label=None, title="t", body="b"):
    return {"number": n, "title": title, "body": body, "user": {"login": login},
            "head": {"ref": ref, "label": label or f"{login}:{ref}"}}


class Fake:
    has_token = False

    def __init__(self, pulls, files, branches=(), compare=None):
        self.pulls, self.files = pulls, files
        self.branches, self.compare = list(branches), compare or {}
        self.requests = []

    def request(self, method, path, body=None):
        assert method == "GET", method
        u = urllib.parse.urlparse(path)
        p = u.path
        self.requests.append(p)
        if urllib.parse.parse_qs(u.query).get("page", ["1"])[0] != "1":
            return []
        if p == f"{REPO}/pulls":
            return self.pulls
        if p.startswith(f"{REPO}/pulls/") and p.endswith("/files"):
            return self.files.get(int(p.split("/")[-2]), [])
        if p == REPO:
            return {"default_branch": "main"}
        if p == f"{REPO}/branches":
            return [{"name": b} for b in self.branches]
        if p.startswith(f"{REPO}/compare/"):
            return self.compare[urllib.parse.unquote(p.split("...", 1)[1])]
        raise tracker.TrackerError(404, "GET", p)


def mk(fake):
    return tracker.Tracker({"kind": "github", "api": "https://t.invalid", "repo": "acme/game"},
                           fake)


def run(argv, fake, ours=None, hot=(), branch="me/1-mine", owner="zed"):
    """Run main with `ours` ({path: ranges}) standing in for git; hot paths given."""
    out, err = io.StringIO(), io.StringIO()

    def git(*args):
        if args[:2] == ("rev-parse", "--abbrev-ref"):
            return branch + "\n"
        raise AssertionError(args)

    with mock.patch.object(po, "ours_ranges", lambda base, g: ours or {}), \
            mock.patch.object(tracker, "origin_owner", lambda: owner):
        code = po.main(argv, tr=mk(fake), out=out, err=err, git=git, hot=list(hot))
    return code, out.getvalue(), err.getvalue()


def rows(out):
    return [ln.split("\t") for ln in out.split("\n")[1:] if ln.count("\t") == 5]


class Hunks(unittest.TestCase):
    def test_forms(self):
        self.assertEqual(po.parse_hunks("@@ -5 +5 @@"), [(5, 5)])
        self.assertEqual(po.parse_hunks("@@ -5,0 +6,2 @@"), [(5, 5)])
        self.assertEqual(po.parse_hunks("@@ -5,3 +5,1 @@"), [(5, 7)])
        self.assertEqual(po.parse_hunks("x\n@@ -1,2 +1 @@ fn a()\n y\n@@ -40,5 +39,0 @@\n"),
                         [(1, 2), (40, 44)])
        self.assertEqual(po.parse_hunks(""), [])
        self.assertEqual(po.parse_hunks(None), [])

    def test_context_distance(self):
        near = po.compare_files({"a.rs": [(10, 12)]}, {"a.rs": [(15, 20)]})
        self.assertEqual(near[0][1], "line")            # 15 is 3 after 12
        far = po.compare_files({"a.rs": [(10, 12)]}, {"a.rs": [(16, 20)]})
        self.assertEqual(far[0][1], "file")
        below = po.compare_files({"a.rs": [(10, 12)]}, {"a.rs": [(1, 7)]})
        self.assertEqual(below[0][1], "line")
        below = po.compare_files({"a.rs": [(10, 12)]}, {"a.rs": [(1, 6)]})
        self.assertEqual(below[0][1], "file")

    def test_whole_file_is_file_level(self):
        self.assertEqual(po.compare_files({"a.rs": None}, {"a.rs": [(1, 2)]})[0][1], "file")
        self.assertEqual(po.compare_files({"a.rs": [(1, 2)]}, {"a.rs": None})[0][1], "file")

    def test_other_paths_and_exempt_files_are_skipped(self):
        self.assertEqual(po.compare_files({"a.rs": [(1, 1)]}, {"b.rs": [(1, 1)]}), [])
        self.assertEqual(po.compare_files({"docs/status/progress.tsv": [(1, 1)]},
                                          {"docs/status/progress.tsv": [(1, 1)]}), [])

    def test_file_ranges_rename_and_no_patch(self):
        got = po.file_ranges([
            {"filename": "new.rs", "previous_filename": "old.rs", "patch": patch("3,2")},
            {"filename": "bin.dat"},
        ])
        self.assertEqual(got, {"new.rs": [(3, 4)], "old.rs": [(3, 4)], "bin.dat": None})


class Diff(unittest.TestCase):
    def test_parse_diff(self):
        text = (
            "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n"
            "@@ -3,2 +3,3 @@\n-x\n@@ -20 +21,0 @@\n"
            "diff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\n"
            "rename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1 +1 @@\n"
            "diff --git a/n.rs b/n.rs\nnew file mode 100644\n--- /dev/null\n+++ b/n.rs\n"
            "@@ -0,0 +1,4 @@\n"
            "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"
            "diff --git a/mv.rs b/mv2.rs\nsimilarity index 100%\nrename from mv.rs\n"
            "rename to mv2.rs\n")
        got = po.parse_diff(text)
        self.assertEqual(got["a.rs"], [(3, 4), (20, 20)])
        self.assertEqual(got["old.rs"], [(1, 1)])
        self.assertEqual(got["new.rs"], [(1, 1)])
        self.assertEqual(got["n.rs"], [(0, 0)])
        self.assertIsNone(got["img.png"])
        self.assertIsNone(got["mv.rs"])
        self.assertIsNone(got["mv2.rs"])


class Scratch(unittest.TestCase):
    """ours_ranges against a scratch git repo (no network, invented files)."""

    def git(self, *a):
        subprocess.run(["git", "-C", self.d.name, *a], check=True, capture_output=True,
                       env=testenv.hermetic_git_env())

    def test_committed_and_working_tree_changes(self):
        self.d = tempfile.TemporaryDirectory()
        self.addCleanup(self.d.cleanup)
        self.git("init", "-q", "-b", "main")
        p = lambda n: os.path.join(self.d.name, n)
        with open(p("a.txt"), "w") as f:
            f.write("".join(f"line {i}\n" for i in range(1, 31)))
        with open(p("b.txt"), "w") as f:
            f.write("one\n")
        self.git("add", ".")
        self.git("commit", "-q", "-m", "base")
        self.git("checkout", "-q", "-b", "work")
        with open(p("a.txt"), "w") as f:
            f.write("".join(f"line {i}\n" if i != 10 else "changed\n" for i in range(1, 31)))
        self.git("commit", "-q", "-am", "c1")
        with open(p("b.txt"), "a") as f:
            f.write("two\n")                      # uncommitted
        git = lambda *a: subprocess.run(["git", "-C", self.d.name, *a], capture_output=True,
                                        text=True, check=True).stdout
        got = po.ours_ranges("main", git)
        self.assertEqual(got["a.txt"], [(10, 10)])
        self.assertEqual(got["b.txt"], [(1, 1)])   # an insertion after line 1 counts as line 1
        with self.assertRaises(po.GitError):
            po.ours_ranges("--output=x", git)
        # a user's diff.mnemonicPrefix must not change the keys
        mnem = lambda *a: git("-c", "diff.mnemonicPrefix=true", *a)
        got = po.ours_ranges("main", mnem)
        self.assertEqual(sorted(got), ["a.txt", "b.txt"])


class Report(unittest.TestCase):
    def test_line_overlap_exit_3_and_never_prints_title_or_body(self):
        fake = Fake([pr(7, "bob", title=TITLE_SENTINEL, body=BODY_SENTINEL)],
                    {7: [{"filename": "a.rs", "patch": patch("11,1")}]})
        code, out, err = run([], fake, ours={"a.rs": [(10, 12)]})
        self.assertEqual(code, 3, err)
        self.assertEqual(rows(out), [["line", "#7", "bob", "a.rs", "10-12", "11-11"]])
        self.assertIn("overlap=line", out)
        self.assertIn('"Depends on: #N"', out)
        self.assertNotIn(TITLE_SENTINEL, out)
        self.assertNotIn(BODY_SENTINEL, out)

    def test_file_only_exit_1_and_none_exit_0(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "a.rs", "patch": patch("90,1")}]})
        code, out, _ = run([], fake, ours={"a.rs": [(1, 2)]})
        self.assertEqual(code, 1)
        self.assertIn("overlap=file", out)
        self.assertNotIn("Depends on", out)
        code, out, _ = run([], fake, ours={"z.rs": [(1, 2)]})
        self.assertEqual(code, 0)
        self.assertIn("overlap=none", out)

    def test_patchless_file_counts_as_the_whole_file(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "a.rs"}]})
        code, out, _ = run([], fake, ours={"a.rs": [(1, 2)]})
        self.assertEqual((code, rows(out)[0][0], rows(out)[0][5]), (1, "file", "all"))

    def test_own_pr_is_excluded_by_branch_and_owner(self):
        files = {7: [{"filename": "a.rs", "patch": patch("1,1")}]}
        fake = Fake([pr(7, "zed", ref="me/1-mine")], files)
        self.assertEqual(run([], fake, ours={"a.rs": [(1, 1)]}, owner="zed")[0], 0)
        # the same branch name opened by someone else is not mine
        fake = Fake([pr(7, "bob", ref="me/1-mine")], files)
        self.assertEqual(run([], fake, ours={"a.rs": [(1, 1)]}, owner="zed")[0], 3)

    def test_pr_flag_excludes(self):
        fake = Fake([pr(9, "bob", ref="other")], {9: [{"filename": "a.rs", "patch": patch("1,1")}]})
        self.assertEqual(run(["--pr", "9"], fake, ours={"a.rs": [(1, 1)]})[0], 0)

    def test_rename_matches_under_the_old_name(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "moved.rs", "previous_filename": "a.rs",
                                          "patch": patch("5,1")}]})
        code, out, _ = run([], fake, ours={"a.rs": [(5, 5)]})
        self.assertEqual(code, 3)
        self.assertEqual(rows(out)[0][3], "a.rs")

    def test_exempt_file_is_skipped(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "docs/status/progress.tsv",
                                          "patch": patch("5,1")}]})
        self.assertEqual(run([], fake, ours={"docs/status/progress.tsv": [(5, 5)]})[0], 0)

    def test_branches_compare_path(self):
        fake = Fake([pr(7, "bob", ref="bob/x")], {7: []},
                    branches=["main", "bob/x", "me/1-mine", "side\tbranch", "idle"],
                    compare={"side\tbranch": {"files": [{"filename": "a.rs", "patch": patch("4,1")}],
                                              "commits": [{"author": {"login": "carl"}}]},
                             "idle": {"files": [], "commits": []}})
        code, out, _ = run(["--branches"], fake, ours={"a.rs": [(4, 4)]})
        self.assertEqual(code, 3)
        self.assertEqual(rows(out), [["line", "branch:side_branch", "carl", "a.rs", "4-4", "4-4"]])
        compared = [r for r in fake.requests if "/compare/" in r]
        self.assertEqual(len(compared), 2)            # not main, not the open PR's, not ours

    def test_no_branch_requests_without_the_flag(self):
        fake = Fake([], {})
        run([], fake, ours={"a.rs": [(1, 1)]})
        self.assertFalse(any("branches" in r or "compare" in r for r in fake.requests))

    def test_bad_login_and_unprintable_path(self):
        fake = Fake([pr(7, "bob\tevil"), pr(8, "ok-user")],
                    {7: [{"filename": "a‮b.rs", "patch": patch("1,1")}],
                     8: [{"filename": "c.rs", "patch": patch("1,1")}]})
        _, out, _ = run([], fake, ours={"a‮b.rs": [(1, 1)], "c.rs": [(1, 1)]})
        got = rows(out)
        self.assertEqual(got[0][2:4], ["?", "<unprintable path>"])
        self.assertEqual(got[1][2], "ok-user")
        self.assertNotIn("‮", out)
        self.assertEqual(po.printable_path("a"+chr(0x2028)+"b"), "<unprintable path>")
        self.assertEqual(po.printable_path("a"+chr(0x2029)+"b"), "<unprintable path>")

    def test_json(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "a.rs", "patch": patch("1,1")}]})
        code, out, _ = run(["--json"], fake, ours={"a.rs": [(1, 1)]})
        data = json.loads(out)
        self.assertEqual((code, data["overlap"]), (3, "line"))
        self.assertEqual(set(data["rows"][0]), {"kind", "pr", "author", "path", "ours", "theirs"})

    def test_tracker_error_exit_2(self):
        class Down:
            has_token = False

            def request(self, m, p, body=None):
                raise tracker.TrackerError(503, m, p)
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(po, "ours_ranges", lambda b, g: {"a.rs": [(1, 1)]}):
            code = po.main([], tr=mk(Down()), out=out, err=err, git=lambda *a: "main\n", hot=[])
        self.assertEqual(code, 2)
        self.assertIn("503", err.getvalue())

    def test_git_error_exit_2(self):
        def bad(*a):
            raise po.GitError("no base ref")
        err = io.StringIO()
        code = po.main([], tr=mk(Fake([], {})), out=io.StringIO(), err=err, git=bad, hot=[])
        self.assertEqual(code, 2)

    def test_paths_flag_fakes_whole_files(self):
        fake = Fake([pr(7, "bob")], {7: [{"filename": "a.rs", "patch": patch("1,1")}]})
        out, err = io.StringIO(), io.StringIO()
        code = po.main(["--paths", "a.rs, b.rs"], tr=mk(fake), out=out, err=err,
                       git=lambda *a: (_ for _ in ()).throw(AssertionError("no git")), hot=[])
        self.assertEqual(code, 1)
        self.assertEqual(rows(out.getvalue())[0][:4] + [rows(out.getvalue())[0][4]],
                         ["file", "#7", "bob", "a.rs", "all"])


class PathsFlag(unittest.TestCase):
    def test_non_file_paths_are_refused(self):
        for bad in ("src/*.rs", "a?.rs", "a[1].rs", "src/", "./a.rs", "ok.rs,dir/", ","):
            err = io.StringIO()
            code = po.main(["--paths", bad], tr=mk(Fake([], {})), out=io.StringIO(), err=err,
                           git=lambda *a: "", hot=[])
            self.assertEqual(code, 2, bad)
            self.assertIn("--paths takes file paths", err.getvalue())


HOT_DOC = ("## 10. Areas\n| area | x |\n|---|---|\n| a | b |\n\n### Hot files\n\n"
           "| file | why |\n|---|---|\n| `tools/gate.sh` | gate |\n| `src/conn.rs` | wire |\n"
           "\n## 11. End\n")


class Hot(unittest.TestCase):
    def doc(self, text):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        p = os.path.join(d.name, "c.md")
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)
        return p

    def test_table_parse(self):
        self.assertEqual(po.hot_files(self.doc(HOT_DOC)),
                         ["tools/gate.sh", "server/crates/eve-server/src/conn.rs"])

    def test_missing_table_gives_no_hot_rows_and_exit_0(self):
        p = self.doc("## 10. Areas\nnothing here\n## 11. End\n")
        self.assertIsNone(po.hot_table(p))
        self.assertEqual(po.hot_files(p), [])
        self.assertEqual(po.hot_files("/nonexistent/probe"), [])
        fake = Fake([], {})
        code, out, _ = run([], fake, ours={"tools/gate.sh": [(1, 1)]}, hot=po.hot_files(p))
        self.assertEqual(code, 0)
        self.assertEqual(rows(out), [])

    def test_hot_rows_are_information_only(self):
        hot = po.hot_files(self.doc(HOT_DOC))
        code, out, _ = run([], Fake([], {}), ours={"tools/gate.sh": [(1, 1)], "x.rs": [(1, 1)]},
                           hot=hot)
        self.assertEqual(code, 0)
        self.assertEqual(rows(out), [["hot", "-", "-", "tools/gate.sh", "1-1", "-"]])

    def test_hot_subcommand(self):
        out = io.StringIO()
        code = po.main(["hot"], stdin=io.StringIO("tools/gate.sh\nx.rs\n\n"
                                                  "server/crates/eve-server/src/conn.rs\n"),
                       out=out, hot=po.hot_files(self.doc(HOT_DOC)))
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().split(), ["tools/gate.sh",
                                                  "server/crates/eve-server/src/conn.rs"])

    def test_live_table_has_the_gate(self):
        self.assertIn("tools/land.sh", po.hot_files())


if __name__ == "__main__":
    unittest.main()
