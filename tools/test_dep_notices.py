"""Tests for `tools/dep-notices.py` on an invented registry tree.

Run::

    python3 -I tools/test_dep_notices.py
"""

from __future__ import annotations

import importlib.util
import os
import pathlib
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("dep_notices", os.path.join(HERE, "dep-notices.py"))
dep_notices = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dep_notices)


class DepNotices(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.reg = pathlib.Path(self._tmp.name)
        for name, files in {"beta-2.0.0": {"COPYING": "beta copying\n", "src.rs": "code\n"},
                            "alpha-1.0.0": {"LICENSE-MIT": "alpha mit\n", "NOTICE.md": "alpha notice\n",
                                            "README.md": "readme\n"}}.items():
            (self.reg / name).mkdir()
            for n, t in files.items():
                (self.reg / name / n).write_text(t)
        self.rows = [{"name": "beta", "version": "2.0.0", "licence": "ISC"},
                     {"name": "alpha", "version": "1.0.0", "licence": "MIT"}]

    def tearDown(self):
        self._tmp.cleanup()

    def test_only_licence_and_notice_files_are_carried_and_crates_are_sorted(self):
        text = dep_notices.render(self.rows, self.reg)
        self.assertLess(text.index("alpha 1.0.0"), text.index("beta 2.0.0"))
        for needle in ("Licence: MIT", "Licence: ISC", "alpha mit", "alpha notice", "beta copying"):
            self.assertIn(needle, text)
        self.assertNotIn("readme", text)
        self.assertNotIn("code", text.split("beta copying")[1])

    def test_the_output_is_deterministic(self):
        self.assertEqual(dep_notices.render(self.rows, self.reg), dep_notices.render(self.rows[::-1], self.reg))

    def test_a_crate_missing_from_the_registry_renders_its_header_only(self):
        text = dep_notices.render([{"name": "gamma", "version": "9.9.9", "licence": "MIT"}], self.reg)
        self.assertIn("gamma 9.9.9", text)


if __name__ == "__main__":
    unittest.main()
