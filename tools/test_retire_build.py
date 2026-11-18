#!/usr/bin/env python3
"""Tests for tools/retire-build.sh on an invented prefix and an invented checkout.

A fake checkout holds the real retire-build.sh and prefix-lib.sh; every path the script could
delete is inside a temporary tree. The online-prefix probe is a made-up path under that tree
(`.../steamapps/compatdata/8500`): nothing real is named, so nothing real could be deleted even
if the refusal were broken. Nothing here needs a client or Docker.

    python3 -I tools/test_retire_build.py
"""
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
OLD, PINNED, MID = "1000001", "1000003", "1000002"


def put(path, text=""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


class Retire(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-retire-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = os.path.join(self.tmp, "repo")
        self.prefix = os.path.join(self.tmp, "prefix")
        self.eve = os.path.join(self.prefix, "pfx/drive_c/CCP/EVE")
        os.makedirs(os.path.join(self.repo, "tools"))
        for f in ("retire-build.sh", "prefix-lib.sh"):
            shutil.copy(os.path.join(HERE, f), os.path.join(self.repo, "tools", f))
        put(self.repo + "/tools/builds.tsv",
            "# registry\n%s\tL1\tunverified\t-\told\n%s\tL1\tunverified\t-\tmiddle\n%s\tL2\tpinned\t-\tpinned\n" % (OLD, MID, PINNED))
        # tq-offline is the pinned build; the old build has all five copies; the others one each.
        put(self.eve + "/tq-offline/start.ini", "[main]\nbuild = %s\n" % PINNED)
        for b in (OLD, MID):
            put("%s/tq-%s/start.ini" % (self.eve, b), "[main]\nbuild = %s\n" % b)
            put("%s/tq-%s-control/start.ini" % (self.eve, b), "[main]\nbuild = %s\n" % b)
            put("%s/out/pyc-%s/a.pyc" % (self.repo, b))
            put("%s/out/content-%s/MANIFEST.tsv" % (self.repo, b))
            put("%s/out/new-build/%s/x" % (self.repo, b))
        put(self.repo + "/out/pyc/keep.pyc")
        put(self.repo + "/out/content/keep")

    def run_retire(self, *args, prefix=True, env=None):
        a = ["bash", os.path.join(self.repo, "tools/retire-build.sh"), *args]
        if prefix:
            a += ["--prefix", self.prefix]
        e = dict(os.environ, EVE_RETIRE_REPO=self.repo, HOME=self.tmp)
        e.update(env or {})
        return subprocess.run(a, capture_output=True, text=True, timeout=60, env=e)

    def copies(self, b):
        return [self.eve + "/tq-" + b, self.eve + "/tq-%s-control" % b, "%s/out/pyc-%s" % (self.repo, b),
                "%s/out/content-%s" % (self.repo, b), "%s/out/new-build/%s" % (self.repo, b)]

    def refused(self, r, *words):
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        for w in words:
            self.assertIn(w, r.stderr)
        # a refusal changes nothing
        for b in (OLD, MID):
            for p in self.copies(b):
                self.assertTrue(os.path.exists(p), p)

    def test_dry_run_prints_the_steps_and_deletes_nothing(self):
        r = self.run_retire(OLD, "--dry-run")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for p in self.copies(OLD):
            self.assertIn(p, r.stdout)
            self.assertTrue(os.path.exists(p), p)
        self.assertIn("tools/builds.tsv", r.stdout)
        self.assertIn("tools/patches/%s.json" % OLD, r.stdout)
        self.assertIn("keep findings/build-delta-*-%s.tsv" % OLD, r.stdout)
        self.assertNotIn("git rm findings", r.stdout)
        self.assertIn("dry run", r.stdout)

    def test_it_deletes_exactly_the_five_copies(self):
        r = self.run_retire(OLD)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for p in self.copies(OLD):
            self.assertFalse(os.path.exists(p), p)
        for p in self.copies(MID) + [self.eve + "/tq-offline", self.repo + "/out/pyc/keep.pyc",
                                     self.repo + "/out/content/keep", self.repo + "/tools/builds.tsv"]:
            self.assertTrue(os.path.exists(p), p)

    def test_a_copy_already_gone_is_skipped(self):
        shutil.rmtree(self.eve + "/tq-%s-control" % OLD)
        r = self.run_retire(OLD)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("not there, skipped", r.stdout)

    def test_it_edits_no_tracked_file(self):
        before = open(self.repo + "/tools/builds.tsv").read()
        self.run_retire(OLD)
        self.assertEqual(open(self.repo + "/tools/builds.tsv").read(), before)

    def test_the_pinned_build_is_refused(self):
        os.makedirs("%s/out/pyc-%s" % (self.repo, PINNED))
        self.refused(self.run_retire(PINNED), "pinned")
        self.assertTrue(os.path.isdir("%s/out/pyc-%s" % (self.repo, PINNED)))

    def test_the_pinned_build_is_refused_on_a_dry_run_too(self):
        self.refused(self.run_retire(PINNED, "--dry-run"), "pinned")

    def test_the_rollback_window_build_is_refused_on_a_real_and_a_dry_run(self):
        # MID is the previous build: no registered build lies between it and the pinned one.
        self.refused(self.run_retire(MID), "rollback window")
        self.refused(self.run_retire(MID, "--dry-run"), "rollback window")

    def test_a_build_with_one_between_is_not_the_window(self):
        self.assertEqual(self.run_retire(OLD, "--dry-run").returncode, 0)

    def test_pinned_older_than_the_build_is_refused_as_the_window(self):
        # The pinned build is the old one's own number: nothing newer is pinned.
        put(self.repo + "/tools/builds.tsv", "%s\tL1\tunverified\t-\told\n1000000\tL1\tpinned\t-\tp\n" % OLD)
        self.refused(self.run_retire(OLD), "rollback window")

    def test_the_live_install_is_refused(self):
        put(self.eve + "/tq-offline/start.ini", "[main]\nbuild = %s\n" % OLD)
        self.refused(self.run_retire(OLD), "live install")

    def test_an_unknown_or_malformed_build_is_refused(self):
        self.refused(self.run_retire("1999999"), "not a row")
        self.refused(self.run_retire("../x"), "not a build number")
        self.refused(self.run_retire("tq-offline"), "not a build number")

    def test_usage(self):
        self.assertEqual(self.run_retire(prefix=False).returncode, 2)
        self.assertEqual(self.run_retire(OLD, MID).returncode, 2)
        self.assertEqual(self.run_retire("--bogus").returncode, 2)

    def test_the_online_prefix_is_refused(self):
        # A made-up path in the tree: the check is on the name, so no real prefix is named.
        fake = os.path.join(self.tmp, "steamapps/compatdata/8500")
        put(fake + "/pfx/drive_c/CCP/EVE/tq-%s/start.ini" % OLD)
        r = self.run_retire(OLD, "--prefix", fake, prefix=False)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("online prefix", r.stderr)
        self.assertTrue(os.path.exists(fake + "/pfx/drive_c/CCP/EVE/tq-%s/start.ini" % OLD))

    def test_a_copy_that_is_a_link_into_the_online_prefix_is_refused(self):
        fake = os.path.join(self.tmp, "steamapps/compatdata/8500/inner")
        put(fake + "/keep")
        shutil.rmtree(self.eve + "/tq-%s-control" % OLD)
        os.symlink(fake, self.eve + "/tq-%s-control" % OLD)
        r = self.run_retire(OLD)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("online prefix", r.stderr)
        self.assertTrue(os.path.exists(fake + "/keep"))

    def test_a_link_is_removed_as_a_link_and_its_target_stays(self):
        target = os.path.join(self.tmp, "elsewhere")
        put(target + "/keep")
        shutil.rmtree(self.repo + "/out/pyc-" + OLD)
        os.symlink(target, self.repo + "/out/pyc-" + OLD)
        r = self.run_retire(OLD)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertFalse(os.path.lexists(self.repo + "/out/pyc-" + OLD))
        self.assertTrue(os.path.exists(target + "/keep"))

    def test_a_failed_delete_is_exit_one(self):
        d = self.repo + "/out/content-" + OLD
        os.chmod(self.repo + "/out", 0o555)  # rm cannot remove entries of a read-only directory
        try:
            if os.access(self.repo + "/out", os.W_OK):
                self.skipTest("permission bits are not enforced here (running as root)")
            r = self.run_retire(OLD)
        finally:
            os.chmod(self.repo + "/out", 0o755)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertTrue(os.path.exists(d))


if __name__ == "__main__":
    unittest.main()
