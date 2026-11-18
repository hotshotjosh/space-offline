"""Tests for `tools/host-check.sh` with fake programs and invented fixtures.

Nothing here touches a real path outside its temp dirs.

Run::

    python3 -I tools/test_host_check.py
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last: tools/dis.py must not shadow the stdlib
import testenv  # noqa: E402

SCRIPT = os.path.join(HERE, "host-check.sh")


class HostCheck(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="test-host-check-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.bin = os.path.join(self.tmp, "bin")
        self.repo = os.path.join(self.tmp, "repo")
        os.makedirs(os.path.join(self.repo, "tools"))
        shutil.copy(SCRIPT, os.path.join(self.repo, "tools", "host-check.sh"))

    def fake(self, name, body):
        os.makedirs(self.bin, exist_ok=True)
        p = os.path.join(self.bin, name)
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body)
        os.chmod(p, 0o755)

    def run_check(self, **extra):
        env = testenv.hermetic_git_env(testenv.clean_env(PATH=self.bin + os.pathsep + testenv.system_path(),
                                                         HOME=self.tmp, TMPDIR=self.tmp), **extra)
        return subprocess.run(["bash", os.path.join(self.repo, "tools", "host-check.sh")],
                              env=env, capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)

    def test_a_clean_host_passes(self):
        r = self.run_check()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("== host ok", r.stdout)

    def test_an_old_python_is_named(self):
        self.fake("python3", 'case "$*" in *print*) echo 3.10;; esac\nexit 1\n')
        r = self.run_check()
        self.assertEqual(r.returncode, 1)
        self.assertIn("python3 is 3.10", r.stderr)
        self.assertIn("3.11", r.stderr)

    def test_a_network_filesystem_is_named(self):
        self.fake("findmnt", "echo 9p\n")
        r = self.run_check()
        self.assertEqual(r.returncode, 1)
        self.assertIn("9p filesystem", r.stderr)
        self.assertIn("inside the Linux filesystem", r.stderr)

    def test_crlf_files_are_counted(self):
        g = lambda *a: subprocess.run(["git", "-C", self.repo, *a], check=True, capture_output=True,
                                      env=testenv.hermetic_git_env())
        g("init", "-q", "-b", "work")
        with open(os.path.join(self.repo, "a.txt"), "wb") as f:
            f.write(b"one\r\ntwo\r\n")
        g("add", "-A")
        r = self.run_check()
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("1 files are checked out with CRLF", r.stderr)

    def test_a_noexec_temp_directory_is_named(self):
        self.fake("mktemp", 'd=%s/noexec; mkdir -p "$d"; echo "$d"\n' % self.tmp)
        self.fake("chmod", "exit 0\n")
        r = self.run_check()
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("noexec", r.stderr)

    def test_too_little_ram_is_a_warning_only(self):
        mi = os.path.join(self.tmp, "meminfo")
        with open(mi, "w") as f:
            f.write("MemTotal: 4194304 kB\nMemAvailable: 3000000 kB\n")
        r = self.run_check(EVE_MEMINFO=mi)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("warning: 4 GB of RAM", r.stdout)


if __name__ == "__main__":
    unittest.main()
