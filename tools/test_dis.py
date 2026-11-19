#!/usr/bin/env python3
"""tools/dis.py must run under `python3 -I`, which drops the script directory
from sys.path, on an invented code object (no client data).

    python3 -I tools/test_dis.py
"""
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)
import test_scan_calls as t  # noqa: E402


class DisUnderIsolatedMode(unittest.TestCase):
    def test_lists_and_disassembles_an_invented_function(self):
        mod = t.module(t.fn('inventedFn', [('LOAD_CONST', 0), ('RETURN_VALUE', None)], (None,), ()))
        with tempfile.TemporaryDirectory() as d:
            pyc = os.path.join(d, 'invented.pyc')
            t.write_pyc(pyc, mod)
            # The tools directory reached through a symlink: CPython puts its
            # realpath on sys.path, so the stdlib `dis` must still win.
            link = os.path.join(d, 'tools-link')
            os.symlink(HERE, link)
            for script in (os.path.join(HERE, 'dis.py'), os.path.join(link, 'dis.py')):
                for flags in (['-I'], []):
                    for args in (['--names'], ['inventedFn']):
                        r = subprocess.run(
                            [sys.executable] + flags + [script, pyc] + args,
                            capture_output=True, text=True, cwd=d)
                        self.assertEqual(r.returncode, 0, (script, flags, r.stderr))
                        self.assertIn('inventedFn', r.stdout)


if __name__ == '__main__':
    unittest.main()
