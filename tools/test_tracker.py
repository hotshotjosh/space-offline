#!/usr/bin/env python3
"""Tests for tools/tracker.py, on a fake tracker. Invented logins and issues only."""

import io
import json
import os
import sys
import unicodedata
import unittest
import urllib.error
import urllib.parse
import urllib.request
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tracker  # noqa: E402

REPO = "/repos/acme/game"
SENTINEL = "tok-SENTINEL-7f3a9c"
BODY_SENTINEL = "BODY-SENTINEL-4c1d"


def lb(*names):
    return [{"name": n, "id": 100 + i} for i, n in enumerate(names)]


def issue(n, title, labels=(), body="", milestone=None, state="open"):
    return {"number": n, "title": title, "state": state, "comments": 0,
            "labels": lb(*labels), "assignees": [], "body": body, "milestone": milestone}


def fake_tree():
    """An invented tracker: a few issues, two open PRs."""
    return {
        "user": {"login": "alice"},
        "issues": {
            1: issue(1, "Ready server task", ["status/ready", "prio/2", "area/combat"],
                     "Area: other\nOwns: server/x.rs\n" + BODY_SENTINEL),
            2: issue(2, "Needs a client", ["status/ready", "prio/1", "needs-client"]),
            3: issue(3, "Claimed by bob", ["status/ready", "prio/1"]),
            6: issue(6, "Top prio, free", ["status/ready", "prio/1"]),
        },
        "pulls": [
            {"number": 50, "body": "Closes #3", "user": {"login": "bob"},
             "head": {"ref": "bob/3-thing"}, "updated_at": "2020-01-02T00:00:00Z"},
            {"number": 51, "body": "unlinked", "user": {"login": "dave"},
             "head": {"ref": "dave/x"}, "updated_at": "2020-01-02T00:00:00Z"},
        ],
    }


class Fake:
    """A transport over the invented tree. Records every request."""

    has_token = True

    def __init__(self, tree):
        self.tree = tree
        self.calls = []

    def request(self, method, path, body=None):
        u = urllib.parse.urlparse(path)
        q = urllib.parse.parse_qs(u.query)
        p = u.path
        self.calls.append((method, p))
        if q.get("page", ["1"])[0] != "1":
            return []
        if p == "/user":
            return self.tree["user"]
        if p == f"{REPO}/issues":
            return json.loads(json.dumps([i for i in self.tree["issues"].values()
                                          if i["state"] == "open"]))
        if p == f"{REPO}/pulls":
            return self.tree["pulls"]
        raise tracker.TrackerError(404, method, p)


def mk(tree=None, kind="github"):
    cfg = {"kind": kind, "api": "https://tracker.invalid", "repo": "acme/game"}
    fake = Fake(tree or fake_tree())
    return tracker.Tracker(cfg, fake), fake


def run(argv, tr):
    out, err = io.StringIO(), io.StringIO()
    code = tracker.main(argv, tracker=tr, out=out, err=err)
    return code, out.getvalue(), err.getvalue()


class Token(unittest.TestCase):
    def test_sentinel_never_leaks(self):
        env = {"EVE_TRACKER": "gitea", "EVE_TRACKER_API": "https://tracker.invalid/api/v1",
               "EVE_TRACKER_REPO": "acme/game", "EVE_TRACKER_TOKEN": SENTINEL}
        seen = []

        def fake_urlopen(req, timeout=None):
            seen.append((req.full_url, dict(req.header_items())))
            raise urllib.error.HTTPError(req.full_url, 401, "Unauthorized", {}, None)

        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, env, clear=False), \
                mock.patch.object(urllib.request, "urlopen", fake_urlopen), \
                mock.patch.object(tracker, "CACHE", os.devnull):
            cfg = tracker.config()
            tr = tracker.Tracker(cfg, tracker.Http(cfg, tracker.read_token(), cache_path=os.devnull))
            code = tracker.main(["whoami"], tracker=tr, out=out, err=err)
            try:
                tr.get("/user")
            except tracker.TrackerError as e:
                exc_text = repr(e) + str(e) + repr(e.args)
        self.assertEqual(code, 2)
        self.assertEqual(seen[0][1].get("Authorization"), "token " + SENTINEL)
        self.assertNotIn(SENTINEL, seen[0][0])
        for text in (out.getvalue(), err.getvalue(), exc_text):
            self.assertNotIn(SENTINEL, text)
        self.assertIn("401", err.getvalue())

    def test_token_file(self):
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "test_tracker.py")
        self.assertTrue(tracker.read_token({"EVE_TRACKER_TOKEN_FILE": path}))
        self.assertIsNone(tracker.read_token({}))



class Resolve(unittest.TestCase):
    """Tracker resolution from the remotes, the fork lookup and its cache. No network."""

    def setUp(self):
        import tempfile
        self._d = tempfile.TemporaryDirectory()
        self.addCleanup(self._d.cleanup)
        self.gitdir = self._d.name
        self.remotes = {"origin": "git@github.com:zed/game.git"}
        self.calls = []
        self.git_calls = []
        self.err = io.StringIO()

    def run_git(self, *args):
        if args[:2] == ("remote", "get-url"):
            return self.remotes.get(args[2], "")
        self.git_calls.append(args)
        if args == ("rev-parse", "--git-common-dir"):
            return self.gitdir
        return ""

    def fetcher(self, info):
        def fetch(url, token):
            self.calls.append((url, token))
            if isinstance(info, Exception):
                raise info
            return info
        return fetch

    def cfg(self, info=None, env=None):
        info = {"fork": False} if info is None else info
        return tracker.config(env or {}, run=self.run_git, fetch=self.fetcher(info), err=self.err)

    def test_fork_resolves_to_parent(self):
        c = self.cfg({"fork": True, "parent": {"full_name": "acme/game"}})
        self.assertEqual((c["kind"], c["repo"], c["api"]), ("github", "acme/game",
                                                            "https://api.github.com"))
        self.assertEqual(self.calls[0][0], "https://api.github.com/repos/zed/game")

    def test_non_fork_is_itself(self):
        self.assertEqual(self.cfg()["repo"], "zed/game")

    def test_upstream_wins_over_origin(self):
        self.remotes["upstream"] = "https://github.com/acme/game.git"
        c = self.cfg()
        self.assertEqual(c["repo"], "acme/game")
        self.assertEqual(self.calls[0][0], "https://api.github.com/repos/acme/game")

    def test_cache_used_then_refreshed_on_new_url(self):
        info = {"fork": True, "parent": {"full_name": "acme/game"}}
        self.assertEqual(self.cfg(info)["repo"], "acme/game")
        self.assertEqual(self.cfg(info)["repo"], "acme/game")
        self.assertEqual(len(self.calls), 1)
        self.assertTrue(os.path.exists(os.path.join(self.gitdir, "eve-tracker")))
        self.remotes["origin"] = "git@github.com:yan/game.git"
        self.cfg({"fork": False})
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(self.cfg({"fork": False})["repo"], "yan/game")
        self.assertEqual(len(self.calls), 2)

    def test_api_failure_falls_back_with_note_and_is_not_cached(self):
        c = self.cfg(urllib.error.URLError("offline"))
        self.assertEqual(c["repo"], "zed/game")
        self.assertEqual(len(self.err.getvalue().strip().splitlines()), 1)
        self.assertIn("note:", self.err.getvalue())
        self.assertFalse(os.path.exists(os.path.join(self.gitdir, "eve-tracker")))
        c = self.cfg({"fork": True, "parent": {"full_name": "acme/game"}})
        self.assertEqual(c["repo"], "acme/game")

    def test_bad_cache_is_ignored(self):
        with open(os.path.join(self.gitdir, "eve-tracker"), "w", encoding="utf-8") as f:
            f.write("not json")
        self.assertEqual(self.cfg()["repo"], "zed/game")

    def test_full_override_wins_and_asks_nothing(self):
        env = {"EVE_TRACKER": "gitea", "EVE_TRACKER_API": "https://t.invalid/api/v1/",
               "EVE_TRACKER_REPO": "acme/other"}
        self.assertEqual(self.cfg(env=env), {"kind": "gitea", "api": "https://t.invalid/api/v1",
                                              "repo": "acme/other", "from": "override"})
        self.assertEqual(self.calls, [])

    def test_other_host_is_gitea_and_asks_nothing(self):
        self.remotes["origin"] = "https://git.invalid/acme/game.git"
        c = self.cfg()
        self.assertEqual((c["kind"], c["api"], c["repo"]),
                         ("gitea", "https://git.invalid/api/v1", "acme/game"))
        self.assertEqual(self.calls, [])

    def test_no_remote_and_no_override_is_a_config_error(self):
        self.remotes = {}
        with self.assertRaises(tracker.TrackerError):
            self.cfg()

    def test_lookup_sends_the_token_to_github_only(self):
        self.cfg(env={"GH_TOKEN": SENTINEL})
        self.assertEqual(self.calls[0][1], SENTINEL)
        self.assertNotIn(SENTINEL, self.err.getvalue())

    def test_token_order(self):
        env = {"EVE_TRACKER_TOKEN": "a", "GH_TOKEN": "c", "GITHUB_TOKEN": "d"}
        self.assertEqual(tracker.read_token(env), "a")
        path = os.path.join(self.gitdir, "tok")
        with open(path, "w", encoding="utf-8") as f:
            f.write("b\n")
        env = {"EVE_TRACKER_TOKEN_FILE": path, "GH_TOKEN": "c", "GITHUB_TOKEN": "d"}
        self.assertEqual(tracker.read_token(env), "b")
        self.assertEqual(tracker.read_token({"GH_TOKEN": "c", "GITHUB_TOKEN": "d"}), "c")
        self.assertEqual(tracker.read_token({"GITHUB_TOKEN": "d"}), "d")
        self.assertIsNone(tracker.read_token({}))
        # GitHub's names go only to api.github.com, whatever the kind
        self.assertIsNone(tracker.read_token({"GH_TOKEN": "c"}, "https://other.invalid/api/v1"))
        self.assertIsNone(tracker.read_token({"GITHUB_TOKEN": "d"}, "https://other.invalid"))
        self.assertEqual(tracker.read_token({"EVE_TRACKER_TOKEN": "a"}, "https://other.invalid"), "a")
        self.assertEqual(tracker.read_token({"GH_TOKEN": "c"}, "https://api.github.com/"), "c")

    def test_fork_of_a_fork_resolves_to_the_source(self):
        c = self.cfg({"fork": True, "parent": {"full_name": "mid/game"},
                      "source": {"full_name": "acme/game"}})
        self.assertEqual(c["repo"], "acme/game")
        self.remotes["origin"] = "git@github.com:yan/game.git"
        c = self.cfg({"fork": True, "parent": {"full_name": "mid/game"}})
        self.assertEqual(c["repo"], "mid/game")

    def test_cache_key_has_no_credentials(self):
        self.remotes["origin"] = "https://zed:%s@github.com/zed/game.git" % SENTINEL
        self.cfg()
        with open(os.path.join(self.gitdir, "eve-tracker"), encoding="utf-8") as f:
            text = f.read()
        self.assertNotIn(SENTINEL, text)
        self.assertEqual(json.loads(text), {"key": "github.com/zed/game", "repo": "zed/game"})
        self.assertEqual(self.cfg()["repo"], "zed/game")
        self.assertEqual(len(self.calls), 1)

    def test_cache_is_in_the_common_git_dir(self):
        self.cfg()
        self.assertIn(("rev-parse", "--git-common-dir"), self.git_calls)
        self.assertNotIn(("rev-parse", "--git-dir"), self.git_calls)

    def test_github_host_forms(self):
        for url in ("git@GitHub.com:zed/game.git", "https://www.github.com/zed/game",
                    "ssh://git@ssh.github.com:443/zed/game.git"):
            self.assertEqual(tracker.parse_remote(url), ("github.com", "zed/game"), url)
            self.remotes["origin"] = url
            self.calls.clear()
            c = self.cfg()
            self.assertEqual((c["kind"], c["api"]), ("github", "https://api.github.com"), url)
            self.assertEqual(len(self.calls), 1, url)
            os.remove(os.path.join(self.gitdir, "eve-tracker"))
        # an SSH host alias is not guessed: it is read as another host
        self.assertEqual(tracker.parse_remote("git@github-work:zed/game.git"),
                         ("github-work", "zed/game"))

    def test_partial_override_with_other_api_sends_no_github_token(self):
        env = {"EVE_TRACKER": "github", "EVE_TRACKER_API": "https://other.invalid",
               "GH_TOKEN": SENTINEL}
        cfg = self.cfg(env=env)
        self.assertEqual(self.calls, [])
        self.assertIsNone(tracker.read_token(env, cfg["api"]))

    def test_repo_subcommand(self):
        self.remotes["upstream"] = "https://github.com/acme/game.git"
        out = io.StringIO()
        with mock.patch.object(tracker, "git", self.run_git), \
                mock.patch.object(tracker, "github_get", self.fetcher({"fork": False})), \
                mock.patch.dict(os.environ, {}, clear=True):
            code = tracker.main(["repo"], out=out, err=io.StringIO())
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().splitlines(),
                         ["board=acme/game", "kind=github", "origin=zed/game", "from=remote"])

    def repo_out(self, info, env=None):
        out = io.StringIO()
        with mock.patch.object(tracker, "git", self.run_git), \
                mock.patch.object(tracker, "github_get", self.fetcher(info)), \
                mock.patch.dict(os.environ, env or {}, clear=True):
            self.assertEqual(tracker.main(["repo"], out=out, err=io.StringIO()), 0)
        return dict(ln.split("=", 1) for ln in out.getvalue().splitlines())

    def test_from_fork_override_and_remote(self):
        fork = {"fork": True, "parent": {"full_name": "acme/game"}}
        self.assertEqual(self.repo_out(fork)["from"], "fork")
        os.remove(os.path.join(self.gitdir, "eve-tracker"))
        self.assertEqual(self.repo_out({"fork": False})["from"], "remote")
        # the maintainer shape: board set by override to a GitHub repo, origin elsewhere
        self.remotes["origin"] = "ssh://git@forge.invalid:2222/team/game.git"
        env = {"EVE_TRACKER": "github", "EVE_TRACKER_API": "https://api.github.com",
               "EVE_TRACKER_REPO": "acme/game"}
        r = self.repo_out(fork, env)
        self.assertEqual((r["board"], r["origin"], r["from"]), ("acme/game", "team/game", "override"))

    def test_no_token_message_names_gh_token(self):
        tr, fake = mk()
        fake.has_token = False
        self.assertIn("GH_TOKEN", run(["whoami"], tr)[2])


class ForkPull(unittest.TestCase):
    def pulls(self):
        tree = fake_tree()
        tree["pulls"] = [
            {"number": 60, "body": "", "user": {"login": "yan"},
             "head": {"ref": "zed/7-fix", "label": "yan:zed/7-fix"}},
            {"number": 61, "body": "", "user": {"login": "zed"},
             "head": {"ref": "zed/7-fix", "label": "zed:zed/7-fix"}},
        ]
        return mk(tree)[0]

    def find(self, tr, origin):
        out = io.StringIO()
        with mock.patch.object(tracker, "git", lambda *a: origin if a[:2] == ("remote", "get-url") else ""):
            code = tracker.main(["pr-for-branch", "--branch", "zed/7-fix"], tracker=tr, out=out,
                               err=io.StringIO())
        return code, out.getvalue().strip()

    def test_fork_pr_on_the_parent_is_found_by_its_owner(self):
        tr = self.pulls()
        self.assertEqual(self.find(tr, "git@github.com:zed/game.git"), (0, "61"))
        self.assertEqual(self.find(tr, "git@github.com:yan/game.git"), (0, "60"))

    def test_other_persons_branch_of_the_same_name_is_not_mine(self):
        self.assertEqual(self.find(self.pulls(), "git@github.com:xia/game.git"), (1, "none"))

    def test_pr_without_label_matches_by_branch(self):
        tr, _ = mk()
        self.assertEqual(self.find(tr, "git@github.com:zed/game.git"), (1, "none"))
        out = io.StringIO()
        with mock.patch.object(tracker, "git", lambda *a: ""):
            code = tracker.main(["pr-for-branch", "--branch", "bob/3-thing"], tracker=tr, out=out,
                               err=io.StringIO())
        self.assertEqual((code, out.getvalue().strip()), (0, "50"))


class Issues(unittest.TestCase):
    def rows(self, tr, *argv):
        code, out, err = run(["issues", *argv], tr)
        lines = out.strip().split("\n")
        return code, lines[0], [ln.split("\t") for ln in lines[1:]], out

    def test_ordering_ready_prio_due_number(self):
        tree = fake_tree()
        tree["issues"][7] = issue(7, "Not ready", ["prio/1"])
        tree["issues"][8] = issue(8, "Due early", ["status/ready", "prio/1"],
                                  milestone={"due_on": "2026-01-01T00:00:00Z"})
        tree["issues"][9] = issue(9, "Due late", ["status/ready", "prio/1"],
                                  milestone={"due_on": "2026-06-01T00:00:00Z"})
        code, head, rows, _ = self.rows(mk(tree)[0])
        self.assertEqual(code, 0)
        self.assertEqual(head, "issue\tready\tprio\tarea\tclient\tprs\ttitle")
        self.assertEqual([r[0] for r in rows], ["8", "9", "2", "3", "6", "1", "7"])

    def test_held_issues_are_last_and_marked(self):
        tree = fake_tree()
        tree["issues"][7] = issue(7, "Held back", ["status/ready", "prio/1", "needs-owner"])
        tree["issues"][8] = issue(8, "Parked", ["parked"])
        _, _, rows, _ = self.rows(mk(tree)[0])
        self.assertEqual([(r[0], r[1]) for r in rows[-2:]], [("7", "held"), ("8", "held")])
        self.assertNotIn("held", [r[1] for r in rows[:-2]])

    def test_title_is_cleaned(self):
        tree = fake_tree()
        tree["issues"][7] = issue(7, "a\tb\nc\u202ed\u200be\x07f\ue000g\u2028h" + "x" * 200,
                                  ["status/ready"])
        _, _, rows, out = self.rows(mk(tree)[0])
        row = next(r for r in rows if r[0] == "7")
        self.assertEqual(len(row), 7)
        self.assertTrue(row[6].startswith("a b cdefg hxxx"))
        self.assertEqual(len(row[6]), 100)
        for ch in out:
            self.assertNotIn(unicodedata.category(ch), ("Cf", "Co", "Cs"))
        self.assertNotIn("\u202e", out)

    def test_prs_column_comes_from_closes_only(self):
        tree = fake_tree()
        tree["pulls"].append({"number": 52, "body": "mentions #6 but closes nothing",
                              "user": {"login": "eve"}, "head": {"ref": "e"}})
        tree["pulls"].append({"number": 53, "body": "Fixes #3 and closes #6",
                              "user": {"login": "eve"}, "head": {"ref": "f"}})
        _, _, rows, _ = self.rows(mk(tree)[0])
        prs = {r[0]: r[5] for r in rows}
        self.assertEqual(prs["3"], "50,53")
        self.assertEqual(prs["6"], "53")
        self.assertEqual(prs["1"], "-")

    def test_no_body_field_is_ever_printed(self):
        _, _, rows, out = self.rows(mk()[0])
        self.assertNotIn(BODY_SENTINEL, out)
        self.assertNotIn("server/x.rs", out)
        # area comes from the label, not from the body line `Area: other`
        self.assertEqual(next(r for r in rows if r[0] == "1")[3], "combat")

    def test_filters_and_limit(self):
        _, _, rows, _ = self.rows(mk()[0], "--no-client")
        self.assertNotIn("2", [r[0] for r in rows])
        _, _, rows, _ = self.rows(mk()[0], "--area", "combat")
        self.assertEqual([r[0] for r in rows], ["1"])
        _, _, rows, _ = self.rows(mk()[0], "--limit", "2")
        self.assertEqual(len(rows), 2)

    def test_none(self):
        tree = fake_tree()
        tree["issues"] = {}
        code, out, _ = run(["issues"], mk(tree)[0])
        self.assertEqual(code, 1)
        self.assertIn("no issues", out)

    def test_pull_requests_are_dropped(self):
        tree = fake_tree()
        tree["issues"][9] = dict(issue(9, "A PR"), pull_request={"url": "x"})
        _, _, rows, _ = self.rows(mk(tree)[0])
        self.assertNotIn("9", [r[0] for r in rows])


class ReadOnly(unittest.TestCase):
    def test_non_get_raises_and_sends_nothing(self):
        cfg = {"kind": "github", "api": "https://tracker.invalid", "repo": "acme/game"}
        http = tracker.Http(cfg, SENTINEL, cache_path=os.devnull)
        with mock.patch.object(urllib.request, "urlopen",
                               side_effect=AssertionError("no network")):
            for m in ("POST", "PATCH", "PUT", "DELETE"):
                with self.assertRaises(tracker.TrackerError) as cm:
                    http.request(m, "/repos/acme/game/issues/1/comments", {"body": "x"})
                self.assertEqual(cm.exception.status, "read-only")
                self.assertNotIn(SENTINEL, str(cm.exception))
                with self.assertRaises(tracker.TrackerError) as cm:
                    http._send(m, "/repos/acme/game/issues/1/comments", {"body": "x"})
                self.assertEqual(cm.exception.status, "read-only")
        self.assertFalse(hasattr(http, "send"))

    def test_tracker_has_no_write_methods(self):
        for name in ("comment", "assign", "add_label", "remove_label", "label_id"):
            self.assertFalse(hasattr(tracker.Tracker, name), name)


class ForkName(unittest.TestCase):
    def test_a_bad_fork_name_falls_back_with_a_note(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            def run_git(*args):
                if args[:2] == ("remote", "get-url"):
                    return "git@github.com:zed/game.git" if args[2] == "origin" else ""
                return d if args == ("rev-parse", "--git-common-dir") else ""
            err = io.StringIO()
            info = {"fork": True, "parent": {"full_name": "evil/x\nrm -rf"}}
            c = tracker.config({}, run=run_git, fetch=lambda u, t: info, err=err)
            self.assertEqual(c["repo"], "zed/game")
            self.assertIn("note:", err.getvalue())
            self.assertFalse(os.path.exists(os.path.join(d, "eve-tracker")))
            info = {"fork": True, "parent": {"full_name": "acme/game.v2"}}
            c = tracker.config({}, run=run_git, fetch=lambda u, t: info, err=io.StringIO())
            self.assertEqual(c["repo"], "acme/game.v2")


class Scan(unittest.TestCase):
    def with_root(self, files):
        import tempfile
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        for rel, text in files.items():
            os.makedirs(os.path.join(d.name, os.path.dirname(rel)), exist_ok=True)
            with open(os.path.join(d.name, rel), "w", encoding="utf-8") as f:
                f.write(text)
        return mock.patch.object(tracker, "ROOT", d.name)

    def test_a_malformed_scan_pattern_refuses_everything(self):
        with self.with_root({"private/export-scan.txt": "# c\nfine\n(unclosed\n"}):
            with self.assertRaises(tracker.LeakRefused) as cm:
                tracker.export_patterns()
        self.assertIn("scan pattern export-scan.txt#3 is malformed", str(cm.exception))
        self.assertNotIn("(unclosed", str(cm.exception))

    def test_a_private_pattern_is_named_by_file_and_line_never_by_text(self):
        with self.with_root({"private/export-scan.txt": "# c\n\nzzsecretword\n"}):
            hits = tracker.leak_lines("see zzsecretword", tracker.export_patterns())
        self.assertEqual(hits, ["line 1: matches private pattern export-scan.txt#3"])

    def test_the_export_scan_alone_does_not_carry_the_post_words(self):
        with self.with_root({"private/post-scan.txt": "zzword\n"}):
            self.assertEqual(tracker.leak_lines("zzword", tracker.export_patterns()), [])
            self.assertTrue(tracker.leak_lines("zzword", tracker.export_patterns(post=True)))

    def test_the_built_in_patterns_are_loaded(self):
        pats = tracker.export_patterns()
        self.assertTrue(tracker.leak_lines("path /" + "home/zed/x", pats))
        self.assertEqual(tracker.leak_lines("server/x.rs", pats), [])


if __name__ == "__main__":
    unittest.main()
