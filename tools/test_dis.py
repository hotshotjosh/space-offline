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
sys.path.insert(0, HERE)
import test_scan_calls as t  # noqa: E402


class DisUnderIsolatedMode(unittest.TestCase):
    def test_lists_and_disassembles_an_invented_function(self):
        mod = t.module(t.fn('inventedFn', [('LOAD_CONST', 0), ('RETURN_VALUE', None)], (None,), ()))
        with tempfile.TemporaryDirectory(dir=HERE) as d:
            pyc = os.path.join(d, 'invented.pyc')
            t.write_pyc(pyc, mod)
            for args in (['--names'], ['inventedFn']):
                r = subprocess.run(
                    [sys.executable, '-I', os.path.join(HERE, 'dis.py'), pyc] + args,
                    capture_output=True, text=True, cwd=d)
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertIn('inventedFn', r.stdout)


if __name__ == '__main__':
    unittest.main()
