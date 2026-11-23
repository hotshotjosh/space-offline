"""Tests for tools/export-parity.sh. A fake `docker` on PATH records its arguments; no
container starts. Invented data only. Run::

    python3 -I tools/test_export_parity.py
"""

import os
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

FAKE_DOCKER = r"""#!/bin/bash
echo "$*" >>"$FAKE_LOG"
if [[ $1 == image && ${FAKE_NOIMAGE:-} == 1 ]]; then exit 1; fi
if [[ $1 == info && ${FAKE_NODAEMON:-} == 1 ]]; then exit 1; fi
if [[ $1 == run ]]; then
    prev=
    for a in "$@"; do
        if [[ $prev == -v && $a == *:/repo ]]; then src=${a%%:*}; echo "MOUNT $src" >>"$FAKE_LOG"; (cd "$src" && find . -not -path './.git/*' | sort >>"$FAKE_LOG"; git -C "$src" symbolic-ref refs/remotes/origin/HEAD >>"$FAKE_LOG" 2>&1); fi
        prev=$a
    done
    if [[ ${FAKE_RO:-} == 1 ]]; then mkdir -p "$src/../ro/sub" && touch "$src/../ro/sub/f" && chmod a-w "$src/../ro/sub"; fi
    exit "${FAKE_RC:-0}"
fi
exit 0
"""

ENV = dict(
    os.environ,
    GIT_CONFIG_GLOBAL="/dev/null",
    GIT_CONFIG_NOSYSTEM="1",
    GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
    GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid",
)


def sh(args, cwd, **kw):
    env = dict(ENV, **kw.pop("env", {}))
    return subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, **kw)


class Parity(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_export_parity-")
        self.bin = os.path.join(self.root, "bin")
        os.makedirs(self.bin)
        self.log = os.path.join(self.root, "docker.log")
        open(self.log, "w").close()
        d = os.path.join(self.bin, "docker")
        with open(d, "w") as f:
            f.write(FAKE_DOCKER)
        os.chmod(d, os.stat(d).st_mode | stat.S_IXUSR)
        self.repo = os.path.join(self.root, "repo")
        for rel, text in {
            "private/export-private-paths.txt": "secret-dir/\n",
            "secret-dir/x.txt": "x\n",
            "tools/test_ok.py": "print(1)\n",
            # the private tree holds its exported form (public-export.py identity())
            "tools/doc-private-prefixes.txt": "# Docs the public export omits on purpose, as sha256 of each path (written by tools/public-export.py --write-prefixes).\n",
            "LICENSE.md": "l\n", "LICENSE-additions.md": "l\n", "CONTRIBUTING-terms.md": "t\n", "NOTICE": "n\n",
        }.items():
            self.put(rel, text)
        for n in ("public-export.py", "export-parity.sh"):
            shutil.copy(os.path.join(HERE, n), os.path.join(self.repo, "tools", n))
        sh(["git", "init", "-q", "-b", "work", self.repo], self.root)
        sh(["git", "add", "-A"], self.repo)
        sh(["git", "commit", "-qm", "fixture"], self.repo)

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, text):
        p = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def run_parity(self, **env):
        env = dict(env, PATH=self.bin + ":" + os.environ["PATH"], FAKE_LOG=self.log)
        return sh(["bash", "tools/export-parity.sh"], self.repo, env=env)

    def logged(self):
        with open(self.log) as f:
            return f.read()

    def test_the_docker_call_is_isolated(self):
        r = self.run_parity()
        self.assertEqual(r.returncode, 0, r.stderr)
        log = self.logged()
        run = [l for l in log.split("\n") if l.startswith("run ")][0]
        self.assertIn("--network none", run)
        self.assertIn("-u %d:%d" % (os.getuid(), os.getgid()), run)
        self.assertIn("/tmp:rw,exec", run)
        self.assertIn("HOME=/home/contrib", run)
        for flag in ("--memory 6g", "--pids-limit 4096", "--cap-drop ALL", "--security-opt no-new-privileges",
                     "/etc/contrib-gitconfig:ro"):
            self.assertIn(flag, run)
        mount = [l for l in log.split("\n") if l.startswith("MOUNT ")][0][6:]
        self.assertTrue(mount.startswith(os.path.join(self.repo, "out", "parity") + "/"), mount)

    def test_the_mounted_tree_is_the_export(self):
        self.run_parity()
        log = self.logged()
        self.assertIn("./tools/test_ok.py", log)
        self.assertNotIn("secret-dir", log)
        self.assertNotIn("./private", log)
        self.assertIn("refs/remotes/origin/main", log)

    def test_a_red_container_is_a_red_stage(self):
        self.assertEqual(self.run_parity(FAKE_RC="1").returncode, 1)

    def test_the_work_dir_is_removed(self):
        for rc in ("0", "1"):
            self.run_parity(FAKE_RC=rc)
            left = [n for n in os.listdir(os.path.join(self.repo, "out", "parity")) if n.startswith("run-")]
            self.assertEqual(left, [])

    def test_a_tree_without_the_private_list_skips(self):
        os.remove(os.path.join(self.repo, "private", "export-private-paths.txt"))
        r = self.run_parity()
        self.assertEqual(r.returncode, 0)
        self.assertIn("skipped", r.stdout)
        self.assertEqual(self.logged(), "")

    def test_a_missing_image_says_how_to_build_it(self):
        r = self.run_parity(FAKE_NOIMAGE="1")
        self.assertEqual(r.returncode, 2)
        self.assertIn("--image", r.stderr)

    def test_an_unreachable_daemon_is_named(self):
        r = self.run_parity(FAKE_NODAEMON="1")
        self.assertEqual(r.returncode, 2)
        self.assertIn("docker daemon unreachable", r.stderr)

    def test_the_work_dir_is_removed_even_when_a_run_left_it_read_only(self):
        self.run_parity(FAKE_RO="1")
        left = [n for n in os.listdir(os.path.join(self.repo, "out", "parity")) if n.startswith("run-")]
        self.assertEqual(left, [])

    def test_inside_refuses_outside_the_container(self):
        home = os.path.join(self.root, "home")  # a temp HOME: harmless even if the refusal is missing
        os.makedirs(home)
        r = sh(["bash", "tools/export-parity.sh", "--inside"], self.repo, env={"HOME": home})
        self.assertEqual(r.returncode, 2)
        self.assertFalse(os.path.exists(os.path.join(home, ".gitconfig")))

    def test_inside_runner_names_each_failing_test(self):
        d = os.path.join(self.root, "runner")
        os.makedirs(os.path.join(d, "tools"))
        with open(os.path.join(d, "tools", "test_a.py"), "w") as f:
            f.write("print('ok a')\n")
        with open(os.path.join(d, "tools", "test_b.py"), "w") as f:
            f.write("import sys\nprint('bad b')\nsys.exit(1)\n")
        r = sh(["bash", "tools/export-parity.sh", "--run-tests", d], self.repo)
        self.assertEqual(r.returncode, 1)
        self.assertIn("tools/test_b.py failed", r.stderr)
        self.assertIn("tools/test_a.py: ok a", r.stdout)


if __name__ == "__main__":
    unittest.main()
