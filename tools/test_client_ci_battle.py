#!/usr/bin/env python3
"""client-ci.sh --release and --bots-args (lane battle-100t, issue #100).

Nothing here starts a client, a server or a bot: `--check` stops after the
preflight, in a throwaway repo with stub tools (the pattern of
test_client_ci_parallel.ClientCiSlotPreflight).
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
CI = os.path.join(HERE, "client-ci")
SLOW = 120


def write(path, text, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if mode:
        os.chmod(path, mode)


class BattleFlagsPreflight(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-ci-battle-")
        self.home = os.path.join(self.tmp, "home")
        repo = self.repo = os.path.join(self.tmp, "repo")
        tools = os.path.join(repo, "tools")
        os.makedirs(os.path.join(tools, "client-ci"))
        for name in ("client-ci.sh", "prefix-lib.sh", "content-lib.sh"):
            shutil.copy(os.path.join(HERE, name), tools)
        shutil.copy(os.path.join(CI, "slot-lib.sh"), os.path.join(tools, "client-ci"))
        for name, body in (("install-jail-helper.sh", "exit 4"), ("jail-rootless.sh", "exit 0")):
            write(os.path.join(tools, name), "#!/bin/sh\n%s\n" % body, 0o755)
        write(os.path.join(repo, "out", "content", "MANIFEST.tsv"), "x\n")
        self.scen = os.path.join(self.tmp, "scenarios")
        write(os.path.join(self.scen, "plain_x.py"), "# invented\n")
        write(os.path.join(self.scen, "hdr_x.py"), "# BOTS-ARGS: --brawl --stats 20\n# invented\n")
        write(os.path.join(self.scen, "bad_x.py"), "# BOTS-ARGS: --evil\n# invented\n")
        self.bin = os.path.join(self.tmp, "bin")
        write(os.path.join(self.bin, "sudo"), "#!/bin/sh\nexit 1\n", 0o755)
        write(os.path.join(self.bin, "pgrep"), "#!/bin/sh\nexit 1\n", 0o755)
        d = os.path.join(self.home, ".local/share/eve-offline/prefix/pfx/drive_c")
        write(os.path.join(d, "CCP/EVE/tq-offline/bin64/exefile.exe"), "x\n")
        os.makedirs(os.path.join(d, "eve-logs"))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def check(self, scenario, *extra):
        e = {"PATH": self.bin + ":/usr/bin:/bin", "HOME": self.home, "DISPLAY": ":987654",
             "EVE_CI_SCENARIOS": self.scen, "EVE_PREFIX": os.path.join(self.home, ".local/share/eve-offline/prefix")}
        return subprocess.run(["bash", os.path.join(self.repo, "tools", "client-ci.sh"), "run", scenario,
                               "--check", "--inherit-display", "--no-scenario-flag", *extra],
                              capture_output=True, text=True, env=e, timeout=SLOW)

    def test_release_needs_rootless(self):
        r = self.check("plain_x", "--release")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--release needs --rootless", r.stderr)

    def test_release_rootless_prints_the_profile(self):
        r = self.check("plain_x", "--release", "--rootless")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertRegex(r.stdout, r"profile:\s+release")
        r = self.check("plain_x", "--rootless")
        self.assertRegex(r.stdout, r"profile:\s+debug")

    def test_release_no_build_needs_the_binaries(self):
        r = self.check("plain_x", "--release", "--rootless", "--no-build")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--release --no-build: no", r.stderr)
        rel = os.path.join(self.repo, "server", "target", "release")
        write(os.path.join(rel, "eve-offline-server"), "#!/bin/sh\n", 0o755)
        r = self.check("plain_x", "--release", "--rootless", "--no-build")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        r = self.check("plain_x", "--release", "--rootless", "--no-build", "--with-bots", "--", "--dev")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("eve-bots", r.stderr)

    def test_a_word_off_the_allow_list_is_refused(self):
        r = self.check("plain_x", "--rootless", "--with-bots", "--bots-args", "--evil", "--", "--dev")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--evil", r.stderr)
        self.assertIn("not allowed", r.stderr)

    def test_stats_takes_a_bounded_whole_number(self):
        for bad in ("--stats", "--stats 0", "--stats 3601", "--stats x", "--stats 5;ls"):
            r = self.check("plain_x", "--rootless", "--with-bots", "--bots-args", bad, "--", "--dev")
            self.assertEqual(r.returncode, 3, bad + r.stdout + r.stderr)
        r = self.check("plain_x", "--rootless", "--with-bots", "--bots-args", "--quiet --stats 3600", "--", "--dev")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_bots_args_without_with_bots_is_refused(self):
        r = self.check("plain_x", "--rootless", "--bots-args", "--brawl")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("--bots-args needs --with-bots", r.stderr)

    def test_the_header_applies_when_none_is_given(self):
        r = self.check("hdr_x", "--rootless", "--with-bots", "--", "--dev")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("bots args: --brawl --stats 20", r.stdout)

    def test_the_command_line_beats_the_header(self):
        r = self.check("hdr_x", "--rootless", "--with-bots", "--bots-args", "--quiet", "--", "--dev")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("bots args: --quiet", r.stdout)
        self.assertNotIn("--brawl", r.stdout)

    def test_a_bad_header_is_refused_too(self):
        r = self.check("bad_x", "--rootless", "--with-bots", "--", "--dev")
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertIn("not allowed", r.stderr)


if __name__ == "__main__":
    unittest.main()
