"""Tests for `tools/testenv.py`, and the rule that keeps tests hermetic (#292).

Run::

    python3 -I tools/test_testenv.py
"""
import os
import re
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.append(HERE)  # last: tools/dis.py must not shadow the stdlib

import testenv  # noqa: E402

# Test files exempt from the hermetic-git check below (none: every committing
# test sets an empty global config).
PENDING_HERMETIC: set[str] = set()


class Helpers(unittest.TestCase):
    def test_hermetic_git_env_drops_repo_pointers_and_config(self):
        env = testenv.hermetic_git_env({"GIT_DIR": "/nonexistent/probe", "X": "1"})
        self.assertNotIn("GIT_DIR", env)
        self.assertEqual(env["X"], "1")
        self.assertEqual(env["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(env["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(env["GIT_AUTHOR_EMAIL"], "t@test.invalid")
        self.assertEqual(testenv.hermetic_git_env({}, Y="2")["Y"], "2")

    def test_injected_config_in_the_environment_is_dropped(self):
        import tempfile
        base = {"PATH": os.environ.get("PATH", ""), "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "commit.gpgsign",
                "GIT_CONFIG_VALUE_0": "true", "GIT_CONFIG_PARAMETERS": "'commit.gpgsign=true'"}
        env = testenv.hermetic_git_env(base)
        for k in ("GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0", "GIT_CONFIG_VALUE_0", "GIT_CONFIG_PARAMETERS"):
            self.assertNotIn(k, env)
        with tempfile.TemporaryDirectory() as d:
            r = subprocess.run(["git", "config", "--get", "commit.gpgsign"], cwd=d, env=env,
                               capture_output=True, text=True)
            self.assertEqual(r.stdout.strip(), "")

    def test_a_hostile_global_config_does_not_reach_git(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            cfg = os.path.join(d, "gitconfig")
            with open(cfg, "w") as f:
                f.write("[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /nonexistent/gpg-probe\n")
            env = testenv.hermetic_git_env({"PATH": os.environ.get("PATH", ""), "GIT_CONFIG_GLOBAL": cfg})
            r = subprocess.run(["git", "config", "--get", "commit.gpgsign"], cwd=d,
                               env=env, capture_output=True, text=True)
            self.assertEqual(r.stdout.strip(), "")

    def test_system_path_keeps_the_running_python(self):
        p = testenv.system_path("/nonexistent/a").split(os.pathsep)
        self.assertEqual(p[0], "/nonexistent/a")
        self.assertEqual(p.count(os.path.dirname(sys.executable)), 1)
        self.assertEqual(p[-2:], ["/usr/bin", "/bin"])
        self.assertEqual(len(p), len(set(p)))

    def test_clean_env_drops_eve_variables(self):
        key = "EVE_" + "Z"
        env = testenv.clean_env({key + "Q": "x", "HOME": "/h"}, **{key: "1"})
        self.assertNotIn(key + "Q", env)
        self.assertEqual(env["HOME"], "/h")
        self.assertEqual(env[key], "1")

    def test_module_and_binary_probes(self):
        self.assertTrue(testenv.has_module("os.path"))
        self.assertFalse(testenv.has_module("zz_not_a_module.sub"))
        self.assertIsNone(testenv.optional_load("testenv", "zz_not_a_module"))
        self.assertIsNotNone(testenv.optional_load("testenv", "os"))

        @testenv.needs_module("zz_not_a_module", "invented")
        def f():
            pass
        self.assertTrue(f.__unittest_skip__)
        self.assertIn("zz_not_a_module", f.__unittest_skip_why__)

        @testenv.needs_binary("zz-no-such-binary", "invented")
        def g():
            pass
        self.assertTrue(g.__unittest_skip__)


class Rules(unittest.TestCase):
    def test_every_test_that_commits_is_hermetic(self):
        pat = re.compile(r"""["']commit["']|git commit""")
        bad = []
        for n in sorted(os.listdir(HERE)):
            if not (n.startswith("test_") and n.endswith(".py")) or n in PENDING_HERMETIC:
                continue
            with open(os.path.join(HERE, n), encoding="utf-8") as f:
                text = f.read()
            if n == "test_testenv.py":
                continue
            if pat.search(text) and "GIT_CONFIG_GLOBAL" not in text and "hermetic_git_env" not in text:
                bad.append(n)
        self.assertEqual(bad, [], "these tests commit under the caller's git config; use testenv.hermetic_git_env()")

    def test_gitattributes_forces_lf(self):
        p = os.path.join(ROOT, ".gitattributes")
        self.assertTrue(os.path.exists(p))
        with open(p, encoding="utf-8") as f:
            self.assertIn("* text=auto eol=lf", f.read().splitlines())
        if not os.path.exists(os.path.join(ROOT, ".git")):
            self.skipTest("no .git here: the checkout is an export")
        r = subprocess.run(["git", "ls-files", "--eol"], cwd=ROOT, capture_output=True, text=True,
                           env=testenv.hermetic_git_env())
        self.assertEqual(r.returncode, 0, r.stderr)
        crlf = [l for l in r.stdout.splitlines() if re.match(r"i/(crlf|mixed)\s", l)]
        self.assertEqual(crlf, [])


if __name__ == "__main__":
    unittest.main()
