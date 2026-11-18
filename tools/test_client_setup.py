"""tools/client-setup.sh follows the served build: `check` and `refresh` (K1).

Runs the script from a scratch copy of the kit's tools/ with invented builds
and stub helpers (offline-prefix.sh, pin-client.sh, eve_patch.py, jail-rootless.sh
write their arguments to a log), on an invented install: nothing real is touched.
The older single-build paths are in tools/test_offline_prefix.py.

Run: python3 -I tools/test_client_setup.py -q
"""

import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
EVE = os.path.join("pfx", "drive_c", "CCP", "EVE")

# Invented: 100 pinned and 101 provisional on line vA are served; 99 is not;
# 300 is another line.
BUILDS = ("# invented rows\n"
          "99\tvA\tunverified\t-\told\n"
          "100\tvA\tpinned\t-\tx\n"
          "101\tvA\tprovisional\t-\tx\n"
          "300\tvB\tprovisional\t-\tanother line\n")

STUBS = {
    "pin-client.sh": '#!/bin/bash\necho "pin-client $*" >> "$STUB_LOG"\necho "pin-client: ok"\n',
    "jail-rootless.sh": '#!/bin/bash\necho "ok    probe"\n',
    "offline-prefix.sh": r'''#!/bin/bash
echo "offline-prefix $*" >> "$STUB_LOG"
[[ $1 == --check ]] && exit 0
OFF=${EVE_OFFLINE_PREFIX:-$HOME/.local/share/eve-offline/prefix}
ON=$EVE_ONLINE_PREFIX
rm -rf "$OFF/pfx/drive_c/CCP/EVE/tq-offline"
mkdir -p "$OFF/pfx/drive_c/CCP/EVE"
cp -a "$ON/pfx/drive_c/CCP/EVE/tq-offline" "$OFF/pfx/drive_c/CCP/EVE/tq-offline"
echo stub > "$OFF/eve-offline-prefix.txt"
''',
    "eve_patch.py": '#!/usr/bin/env python3\nimport os, sys\nopen(os.environ["STUB_LOG"], "a").write("eve_patch " + " ".join(sys.argv[1:]) + "\\n")\n',
}


@unittest.skipIf(os.geteuid() == 0, "client-setup.sh refuses root, by design")
class ServedBuild(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-client-setup-k1-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        tools = os.path.join(self.tmp, "repo", "tools")
        os.makedirs(tools)
        for f in ("client-setup.sh", "prefix-lib.sh"):
            shutil.copy2(os.path.join(HERE, f), tools)
        for name, body in STUBS.items():
            with open(os.path.join(tools, name), "w") as fh:
                fh.write(body)
            os.chmod(os.path.join(tools, name), 0o755)
        with open(os.path.join(tools, "builds.tsv"), "w") as fh:
            fh.write(BUILDS)
        self.script = os.path.join(tools, "client-setup.sh")
        self.home = os.path.join(self.tmp, "home")
        self.pfx = os.path.join(self.home, ".local", "share", "Steam", "steamapps", "compatdata", "8500")
        self.on = os.path.join(self.pfx, EVE)
        self.off = os.path.join(self.home, ".local", "share", "eve-offline", "prefix", EVE)
        self.log = os.path.join(self.tmp, "stub.log")
        self.proc = os.path.join(self.tmp, "proc")
        os.makedirs(self.proc)

    def put(self, base, rel, text):
        p = os.path.join(base, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def install(self, tq, offline=None, own=None):
        """tq: the online install's build; offline: tq-offline's, in the online
        prefix; own: tq-offline's in your own prefix."""
        self.put(self.on, "tq/start.ini", "[main]\nbuild = %s\n" % tq)
        self.put(self.on, "tq/bin64/exefile.exe", "MZ")
        row = "res:/a,ab/ab12_cafe,x,8,4\n"
        self.put(self.on, "tq/resfileindex.txt", row)
        self.put(self.on, "tq/resfileindex_Windows.txt", row)
        self.put(self.on, "ResFiles/ab/ab12_cafe", "12345678")
        for base, b in ((self.on, offline), (self.off, own)):
            if b:
                self.put(base, "tq-offline/start.ini", "[main]\nbuild = %s\n" % b)
                self.put(base, "tq-offline/bin64/exefile.exe", "MZ")
        if own:
            self.put(os.path.join(self.home, ".local", "share", "eve-offline", "prefix"),
                     "eve-offline-prefix.txt", "stub\n")

    def run_step(self, *args):
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.home,
               "EVE_PROC_DIR": self.proc,
               "EVE_ONLINE_PREFIX": self.pfx,
               "EVE_OFFLINE_PREFIX": os.path.join(self.home, ".local", "share", "eve-offline", "prefix"),
               "PROTON": "/bin/true", "STUB_LOG": self.log}
        return subprocess.run(["bash", self.script, *args], capture_output=True, text=True,
                              env=env, timeout=120)

    def logged(self):
        try:
            with open(self.log) as f:
                return f.read()
        except OSError:
            return ""

    def build_of(self, base, name):
        with open(os.path.join(base, name, "start.ini")) as f:
            return f.read().split("=")[1].strip()

    # --- check
    def test_check_names_the_newest_served_build_and_says_up_to_date(self):
        self.install("101", "101", "101")
        r = self.run_step("check")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("newest served build 101", r.stdout)
        self.assertIn("up to date: build 101", r.stdout)

    def test_check_says_refresh_when_the_launcher_moved_to_a_served_build(self):
        self.install("101", "100", "100")
        r = self.run_step("check")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("refresh: your tq-offline copy is build 100", r.stdout)
        self.assertIn("tools/client-setup.sh refresh", r.stdout)
        self.assertNotIn("rm -rf", r.stdout)

    def test_check_says_wait_when_the_launcher_is_ahead_of_the_server(self):
        self.install("150")
        r = self.run_step("check")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("the server has not caught up with build 150 yet: wait", r.stdout)
        # With a served tq-offline the player can still play, and is told to wait.
        self.install("150", "100", "100")
        r = self.run_step("check")
        self.assertIn("play on that one", r.stdout)
        self.assertIn("the server has not caught up with build 150 yet: wait", r.stdout)

    def test_a_build_of_another_line_is_not_served(self):
        self.install("300")
        r = self.run_step("check")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("You cannot play on this copy", r.stdout)

    # --- refresh
    def test_refresh_keeps_the_old_copy_and_copies_and_patches_again(self):
        self.install("101", "100", "100")
        r = self.run_step("refresh")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.build_of(self.on, "tq-100"), "100")       # kept, not removed
        self.assertEqual(self.build_of(self.on, "tq-offline"), "101")   # re-copied from tq
        self.assertEqual(self.build_of(self.off, "tq-offline"), "101")  # own prefix made again
        log = self.logged()
        self.assertIn("offline-prefix --force", log)
        self.assertIn("eve_patch apply", log)
        self.assertIn("eve_patch verify", log)

    def test_refresh_with_nothing_old_just_copies(self):
        self.install("101")
        r = self.run_step("refresh")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.build_of(self.on, "tq-offline"), "101")
        self.assertFalse(os.path.exists(os.path.join(self.on, "tq-unknown")))

    def test_refresh_when_up_to_date_changes_nothing(self):
        self.install("101", "101", "101")
        r = self.run_step("refresh")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("up to date", r.stdout)
        self.assertEqual(self.logged(), "")

    def test_refresh_waits_when_the_server_has_not_caught_up(self):
        self.install("150", "100", "100")
        r = self.run_step("refresh")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("has not caught up with build 150", r.stdout)
        self.assertEqual(self.build_of(self.on, "tq-offline"), "100")   # untouched
        self.assertFalse(os.path.exists(os.path.join(self.on, "tq-100")))
        self.assertEqual(self.logged(), "")

    def test_refresh_refuses_to_overwrite_an_existing_kept_copy(self):
        self.install("101", "100", "100")
        self.put(self.on, "tq-100/start.ini", "[main]\nbuild = 100\n")
        r = self.run_step("refresh")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("exists already", r.stderr)
        self.assertEqual(self.build_of(self.on, "tq-offline"), "100")   # not moved

    def test_refresh_refuses_while_wine_runs_in_the_online_prefix(self):
        self.install("101", "100", "100")
        d = os.path.join(self.proc, "77")
        os.makedirs(d)
        with open(os.path.join(d, "environ"), "wb") as f:
            f.write(b"WINEPREFIX=" + os.path.join(self.pfx, "pfx").encode() + b"\0")
        with open(os.path.join(d, "comm"), "w") as f:
            f.write("wineserver\n")
        r = self.run_step("refresh")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("Wine is running", r.stderr)
        self.assertEqual(self.build_of(self.on, "tq-offline"), "100")


class PerBuildPins(unittest.TestCase):
    """tools/pin-client.sh verifies against tools/pins/<build>.sha256 when it exists."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-pins-")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = os.path.join(self.tmp, "repo")
        os.makedirs(os.path.join(self.repo, "tools", "pins"))
        shutil.copy2(os.path.join(HERE, "pin-client.sh"), os.path.join(self.repo, "tools"))
        self.client = os.path.join(self.tmp, "client")
        os.makedirs(self.client)
        for name, text in (("start.ini", "[main]\nbuild = 101\n"), ("a.txt", "invented\n")):
            with open(os.path.join(self.client, name), "w") as f:
                f.write(text)

    def pin(self, *args):
        return subprocess.run(["bash", os.path.join(self.repo, "tools", "pin-client.sh"),
                               "--path", self.client, *args], capture_output=True, text=True,
                              env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": self.tmp})

    def test_the_manifest_is_chosen_by_the_installs_build(self):
        r = self.pin("--write", "--manifest", "tools/pins/101.sha256")
        self.assertEqual(r.returncode, 0, r.stderr)
        # The pinned build's own manifest is for another build and must not be used.
        with open(os.path.join(self.repo, "tools", "client-pin.sha256"), "w") as f:
            f.write("# build:   100\n")
        r = self.pin()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("ok, 2 files match build 101", r.stdout)

    def test_without_a_per_build_file_the_pinned_manifest_is_used(self):
        with open(os.path.join(self.repo, "tools", "client-pin.sha256"), "w") as f:
            f.write("# build:   100\n")
        r = self.pin()
        self.assertEqual(r.returncode, 1)
        self.assertIn("BUILD MOVED", r.stderr)

    def test_a_changed_file_fails_against_the_per_build_manifest(self):
        self.pin("--write", "--manifest", "tools/pins/101.sha256")
        with open(os.path.join(self.client, "a.txt"), "w") as f:
            f.write("changed\n")
        self.assertEqual(self.pin().returncode, 1)


if __name__ == "__main__":
    unittest.main()
