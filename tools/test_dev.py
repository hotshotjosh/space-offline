#!/usr/bin/env python3
"""Tests for tools/dev.sh's CPU cap, with a fake `docker` and `nproc` on PATH.

Nothing here starts a container: the fake docker prints its arguments.
"""

import os
import subprocess
import tempfile
import sys
import unittest

sys.path.append(os.path.dirname(os.path.abspath(__file__)))  # last: tools/dis.py must not shadow the stdlib
import testenv  # noqa: E402

DEV = os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev.sh")


def run_dev(max_jobs, host_cpus):
    with tempfile.TemporaryDirectory() as d:
        for name, body in (("docker", 'echo "$@"\n'), ("nproc", f"echo {host_cpus}\n")):
            p = os.path.join(d, name)
            with open(p, "w", encoding="utf-8") as f:
                f.write("#!/bin/sh\n" + body)
            os.chmod(p, 0o755)
        env = testenv.clean_env(PATH=d + os.pathsep + os.environ["PATH"],
                   EVE_MAX_JOBS=str(max_jobs), EVE_MIN_FREE_GB="0",
                   EVE_TARGET_DIR=os.path.join(d, "target"))
        p = subprocess.run(["bash", DEV, "true"], env=env, capture_output=True, text=True,
                           timeout=60, start_new_session=True)
    return p.returncode, p.stdout, p.stderr


class CpuCap(unittest.TestCase):
    def test_capped_at_the_machine(self):
        # The CI case: 4 asked for, 2 present.
        code, out, err = run_dev(4, 2)
        self.assertEqual(code, 0, err)
        self.assertIn("--cpus 2 ", out)
        self.assertIn("CARGO_BUILD_JOBS=2 ", out)
        self.assertIn("RUST_TEST_THREADS=2 ", out)
        self.assertIn("using 2", err)

    def test_below_the_machine_unchanged(self):
        code, out, err = run_dev(4, 16)
        self.assertEqual(code, 0, err)
        self.assertIn("--cpus 4 ", out)
        self.assertNotIn("using", err)


def run_dev_meminfo(total_kb, avail_kb, min_free_gb="12", timeout=30):
    with tempfile.TemporaryDirectory() as d:
        for name, body in (("docker", 'echo "$@"\n'), ("nproc", "echo 4\n")):
            p = os.path.join(d, name)
            with open(p, "w", encoding="utf-8") as f:
                f.write("#!/bin/sh\n" + body)
            os.chmod(p, 0o755)
        mi = os.path.join(d, "meminfo")
        with open(mi, "w", encoding="utf-8") as f:
            f.write("MemTotal: %d kB\nMemAvailable: %d kB\n" % (total_kb, avail_kb))
        env = testenv.clean_env(PATH=d + os.pathsep + os.environ["PATH"], EVE_MEMINFO=mi,
                                EVE_MIN_FREE_GB=min_free_gb, EVE_TARGET_DIR=os.path.join(d, "target"))
        p = subprocess.run(["bash", DEV, "true"], env=env, capture_output=True, text=True,
                           timeout=timeout, stdin=subprocess.DEVNULL, start_new_session=True)
    return p.returncode, p.stdout, p.stderr


class MemoryWait(unittest.TestCase):
    def test_a_small_machine_waits_for_half_not_forever(self):
        code, out, err = run_dev_meminfo(8388608, 6291456)
        self.assertEqual(code, 0, err)
        self.assertIn("can never be free", err)
        self.assertIn("--cpus", out)

    def test_a_big_machine_is_unchanged(self):
        code, out, err = run_dev_meminfo(67108864, 62914560)
        self.assertEqual(code, 0, err)
        self.assertNotIn("can never be free", err)


    def test_a_big_machine_with_little_free_waits(self):
        with self.assertRaises(subprocess.TimeoutExpired) as cm:
            run_dev_meminfo(67108864, 4194304, timeout=3)
        dec = lambda x: x.decode() if isinstance(x, bytes) else (x or "")
        self.assertIn("waiting for memory", dec(cm.exception.stderr))
        self.assertNotIn("--cpus", dec(cm.exception.stdout))


class FreshClone(unittest.TestCase):
    def test_the_mount_point_is_made_when_the_target_is_elsewhere(self):
        # A copy of dev.sh in a tree with no server/target, EVE_TARGET_DIR outside it.
        with tempfile.TemporaryDirectory() as repo, tempfile.TemporaryDirectory() as bin_:
            os.makedirs(os.path.join(repo, "tools"))
            with open(DEV, encoding="utf-8") as src, \
                    open(os.path.join(repo, "tools", "dev.sh"), "w", encoding="utf-8") as dst:
                dst.write(src.read())
            for name, body in (("docker", 'echo "$@"\n'), ("nproc", "echo 4\n")):
                p = os.path.join(bin_, name)
                with open(p, "w", encoding="utf-8") as f:
                    f.write("#!/bin/sh\n" + body)
                os.chmod(p, 0o755)
            env = testenv.clean_env(PATH=bin_ + os.pathsep + os.environ["PATH"],
                       EVE_MAX_JOBS="2", EVE_MIN_FREE_GB="0",
                       EVE_TARGET_DIR=os.path.join(bin_, "target"))
            p = subprocess.run(["bash", os.path.join(repo, "tools", "dev.sh"), "true"],
                               env=env, capture_output=True, text=True, timeout=60,
                               start_new_session=True)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertTrue(os.path.isdir(os.path.join(repo, "server", "target")))


if __name__ == "__main__":
    unittest.main()
