"""eve_login_prompt with no terminal: the advice must not be circular for
`tools/account.sh login` itself. Runs with stdin from /dev/null, HOME and the
login file under a temp dir inside the build tree; no network, no real state."""
import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LIB = os.path.join(HERE, "login-lib.sh")
ACCOUNT = os.path.join(HERE, "account.sh")
ADVICE = "run tools/account.sh login first"


class NoTerminal(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(dir=os.path.dirname(HERE))
        self.home = self.tmp.name
        self.file = os.path.join(self.home, "login")
        self.env = {"PATH": os.environ["PATH"], "HOME": self.home,
                    "EVE_LOGIN_FILE": self.file}

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_bash(self, argv, extra=None):
        env = dict(self.env, **(extra or {}))
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, env=env, cwd=self.home)

    def test_other_callers_keep_the_old_advice(self) -> None:
        r = self.run_bash(["bash", "-c", f'source "{LIB}"; eve_login_prompt "$EVE_LOGIN_FILE"'])
        self.assertEqual(r.returncode, 1)
        self.assertIn(ADVICE, r.stderr)
        self.assertFalse(os.path.exists(self.file))

    def test_account_login_is_not_told_to_run_itself(self) -> None:
        r = self.run_bash(["bash", ACCOUNT, "login"])
        self.assertEqual(r.returncode, 1)
        self.assertIn("no terminal to ask on", r.stderr)
        self.assertNotIn(ADVICE, r.stderr)
        self.assertIn("from a terminal", r.stderr)
        self.assertFalse(os.path.exists(self.file))


if __name__ == "__main__":
    unittest.main()
