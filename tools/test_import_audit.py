"""Tests for `tools/import_audit.py` (#292 class). Invented module names only.

Run::

    python3 -I tools/test_import_audit.py
"""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last: tools/dis.py must not shadow the stdlib

import import_audit  # noqa: E402


def tree(files):
    d = tempfile.TemporaryDirectory()
    for name, text in files.items():
        with open(os.path.join(d.name, name), "w", encoding="utf-8") as f:
            f.write(text)
    return d


class Violations(unittest.TestCase):
    def check(self, files):
        with tree(files) as d:
            return import_audit.violations(d)

    def test_a_reached_module_with_a_top_level_foreign_import_is_one_violation(self):
        v = self.check({"test_a.py": "import mod_b\n", "mod_b.py": "import zz_not_a_module\n"})
        self.assertEqual(v, ["mod_b.py:1 imports zz_not_a_module at module level, reached from test_a.py"])

    def test_the_chain_is_followed(self):
        v = self.check({"test_a.py": "from mod_b import x\n", "mod_b.py": "import mod_c\n",
                        "mod_c.py": "from zz_not_a_module.sub import y\n"})
        self.assertEqual(len(v), 1)
        self.assertIn("mod_c.py:1", v[0])

    def test_try_except_importerror_is_allowed(self):
        v = self.check({"test_a.py": "import mod_b\n",
                        "mod_b.py": "try:\n    import zz_not_a_module\nexcept ImportError:\n    zz_not_a_module = None\n"})
        self.assertEqual(v, [])

    def test_a_try_that_catches_something_else_is_not(self):
        v = self.check({"test_a.py": "import mod_b\n",
                        "mod_b.py": "try:\n    import zz_not_a_module\nexcept KeyError:\n    pass\n"})
        self.assertEqual(len(v), 1)

    def test_an_import_inside_a_function_is_allowed(self):
        v = self.check({"test_a.py": "import mod_b\n",
                        "mod_b.py": "def f():\n    import zz_not_a_module\n"})
        self.assertEqual(v, [])

    def test_stdlib_future_and_local_imports_are_allowed(self):
        v = self.check({"test_a.py": "import mod_b\n",
                        "mod_b.py": "from __future__ import annotations\nimport os, json\nimport mod_c\n",
                        "mod_c.py": "x = 1\n"})
        self.assertEqual(v, [])

    def test_optional_load_makes_a_module_unreachable(self):
        v = self.check({"test_a.py": "import testenv\nM = testenv.optional_load('mod_b', 'zz_not_a_module')\n",
                        "mod_b.py": "import zz_not_a_module\n", "testenv.py": "x = 1\n"})
        self.assertEqual(v, [])

    def test_a_script_named_by_a_literal_is_reached(self):
        v = self.check({"test_a.py": "SCRIPT = 'mod_b.py'\n", "mod_b.py": "import zz_not_a_module\n"})
        self.assertEqual(len(v), 1)
        v = self.check({"test_a.py": "SCRIPT = 'mod-b.py'\n", "mod_b.py": "import zz_not_a_module\n"})
        self.assertEqual(len(v), 1)

    def test_a_tests_own_top_level_import_counts(self):
        v = self.check({"test_a.py": "import zz_third\n"})
        self.assertEqual(v, ["test_a.py:1 imports zz_third at module level, reached from test_a.py"])

    def test_class_bodies_and_except_star_are_walked(self):
        v = self.check({"test_a.py": "import mod_b\n",
                        "mod_b.py": "class C:\n    import zz_one\n"})
        self.assertEqual(len(v), 1)
        if sys.version_info >= (3, 11):
            v = self.check({"test_a.py": "import mod_b\n",
                            "mod_b.py": "try:\n    import zz_two\nexcept* KeyError:\n    pass\n"})
            self.assertEqual(len(v), 1)

    def test_optional_load_does_not_hide_a_plain_import(self):
        v = self.check({"test_a.py": "import testenv\nimport mod_b\nM = testenv.optional_load('mod_b', 'zz_x')\n",
                        "mod_b.py": "import zz_not_a_module\n", "testenv.py": "x = 1\n"})
        self.assertEqual(len(v), 1)

    def test_a_module_no_test_reaches_is_ignored(self):
        self.assertEqual(self.check({"test_a.py": "x = 1\n", "mod_b.py": "import zz_not_a_module\n"}), [])


class Live(unittest.TestCase):
    def test_no_test_reaches_a_third_party_import_at_module_level(self):
        self.assertEqual(import_audit.violations(HERE), [])


if __name__ == "__main__":
    unittest.main()
