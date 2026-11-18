"""Tests for tools/export_refs.py: exported live files naming a withheld path.

Invented names only; the fixture repo is a throwaway git repo. Run::

    python3 -I tools/test_export_refs.py
"""

import contextlib
import importlib.util
import io
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
spec = importlib.util.spec_from_file_location("export_refs", os.path.join(HERE, "export_refs.py"))
er = importlib.util.module_from_spec(spec)
spec.loader.exec_module(er)

GIT_ENV = dict(
    os.environ,
    GIT_CONFIG_GLOBAL="/dev/null",
    GIT_CONFIG_NOSYSTEM="1",
    GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
    GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid",
)


class Fixture(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_export_refs-")
        shutil.copy(os.path.join(HERE, "public-export.py"), self.put_dir("tools") + "/public-export.py")
        self.put("private/export-private-paths.txt", "docs/secret/\ntools/secret-tool.py\n")
        self.put("docs/secret/a.txt", "x\n")
        self.put("tools/secret-tool.py", "print(1)\n")
        self.allow = os.path.join(self.root, "allow.tsv")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put_dir(self, rel):
        os.makedirs(os.path.join(self.root, rel), exist_ok=True)
        return os.path.join(self.root, rel)

    def put(self, rel, text):
        self.put_dir(os.path.dirname(rel) or ".")
        with open(os.path.join(self.root, rel), "w") as f:
            f.write(text)

    def run_check(self):
        subprocess.run(["git", "init", "-q", "-b", "work", self.root], check=True, env=GIT_ENV)
        subprocess.run(["git", "-C", self.root, "add", "-A"], check=True, env=GIT_ENV)
        buf = io.StringIO()
        with contextlib.redirect_stderr(io.StringIO()):
            # the module reads git through subprocess with the caller's env; keep it hermetic
            old = os.environ.copy()
            os.environ.update({k: v for k, v in GIT_ENV.items() if k.startswith("GIT_")})
            try:
                rc = er.check(self.root, self.allow, out=buf)
            finally:
                os.environ.clear()
                os.environ.update(old)
        return rc, buf.getvalue()


class Cases(Fixture):
    def test_a_shell_command_naming_a_withheld_file_is_live(self):
        self.put("tools/run.sh", "python3 tools/secret-tool.py\n")
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("tools/run.sh:1: names tools/secret-tool.py", out)

    def test_a_shell_comment_is_ignored(self):
        self.put("tools/run.sh", "# see tools/secret-tool.py\n")
        self.assertEqual(self.run_check()[0], 0)

    def test_a_python_docstring_is_ignored_and_a_string_is_live(self):
        self.put("tools/x.py", '"""Names docs/secret/a.txt."""\n\ndef f():\n    """docs/secret/a.txt"""\n    # docs/secret/a.txt\n')
        self.assertEqual(self.run_check()[0], 0)

    def test_a_python_string_is_live(self):
        self.put("tools/x.py", 'open("docs/secret/a.txt")\n')
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("tools/x.py:1: names docs/secret/a.txt", out)

    def test_a_path_that_is_not_tracked_is_never_a_hit(self):
        self.put("tools/x.py", 'p = "docs/secret/never-tracked.tsv"\n')
        self.assertEqual(self.run_check()[0], 0)

    def test_a_path_built_with_os_path_join_is_live(self):
        self.put("tools/x.py", 'import os\nROOT = "."\np = os.path.join(ROOT, "tools", "secret-tool.py")\n')
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("names tools/secret-tool.py", out)

    def test_a_path_built_with_the_slash_operator_is_live(self):
        self.put("tools/x.py", 'ROOT = None\np = ROOT / "tools" / "secret-tool.py"\n')
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("names tools/secret-tool.py", out)

    def test_an_fstring_path_is_live(self):
        self.put("tools/x.py", 'ROOT = "."\np = f"{ROOT}/tools/secret-tool.py"\n')
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("names tools/secret-tool.py", out)

    def test_a_directory_that_also_holds_exported_files_is_not_private(self):
        self.put("docs/secret/open.txt", "x\n")  # not in the private list? the list names docs/secret/
        with open(os.path.join(self.root, "private/export-private-paths.txt"), "w") as f:
            f.write("docs/secret/a.txt\ntools/secret-tool.py\n")
        self.put("tools/run.sh", "ls docs/secret\n")  # the directory has an exported file
        self.assertEqual(self.run_check()[0], 0)

    def test_an_allow_row_is_exact_not_a_prefix(self):
        self.put("tools/run.sh", "ls docs/secret/a.txt\n")
        with open(self.allow, "w") as f:
            f.write("tools/run.sh\tdocs/secret\tinvented reason\n")
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("names docs/secret/a.txt", out)

    def test_a_directory_prefix_counts(self):
        self.put("tools/run.sh", "ls docs/secret/\n")
        self.assertEqual(self.run_check()[0], 1)

    def test_markdown_is_a_note_not_a_failure(self):
        self.put("docs/X.md", "See docs/secret/a.txt.\n")
        rc, out = self.run_check()
        self.assertEqual(rc, 0)
        self.assertIn("note: docs/X.md:1: names docs/secret/a.txt", out)

    def test_an_allow_row_silences_its_hit(self):
        self.put("tools/run.sh", "python3 tools/secret-tool.py\n")
        with open(self.allow, "w") as f:
            f.write("# c\ntools/run.sh\ttools/secret-tool.py\tinvented reason\n")
        rc, out = self.run_check()
        self.assertEqual(rc, 0, out)
        self.assertIn("0 live, 1 allowed", out)

    def test_an_allow_row_matching_nothing_is_stale(self):
        with open(self.allow, "w") as f:
            f.write("tools/run.sh\tdocs/secret/a.txt\tinvented reason\n")
        rc, out = self.run_check()
        self.assertEqual(rc, 1)
        self.assertIn("stale allow row", out)

    def test_an_allow_row_needs_a_reason(self):
        with open(self.allow, "w") as f:
            f.write("tools/run.sh\tdocs/secret/a.txt\n")
        with self.assertRaises(ValueError):
            er.load_allow(self.allow)

    def test_a_python2_scenario_that_does_not_tokenize_falls_back_to_the_shell_rule(self):
        self.put("tools/client-ci/scenarios/s.py", 'print "a"\nx = (\nopen("docs/secret/a.txt")\n')
        self.assertEqual(self.run_check()[0], 1)


class Live(unittest.TestCase):
    def test_the_tree_names_no_excluded_path(self):
        if not os.path.isfile(os.path.join(ROOT, "private", "export-private-paths.txt")):
            self.skipTest("this tree is the export: nothing is excluded here")
        out = io.StringIO()
        rc = er.check(ROOT, out=out)
        self.assertEqual(rc, 0, "\n".join(l for l in out.getvalue().split("\n") if not l.startswith("note:")))


if __name__ == "__main__":
    unittest.main()
