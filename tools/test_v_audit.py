#!/usr/bin/env python3
"""Tests for tools/v-audit.py (V2). Invented files in a throwaway git repo; nothing is a client claim.

    python3 -I tools/test_v_audit.py
"""
import contextlib
import importlib.util
import io
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last: tools/dis.py must not shadow the stdlib
import testenv  # noqa: E402
_spec = importlib.util.spec_from_file_location('v_audit', os.path.join(HERE, 'v-audit.py'))
va = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(va)


def uncited(text):
    return [m[1] for m in va.marks_of(text.split('\n'))]


class Marks(unittest.TestCase):
    def test_a_bare_mark_is_uncited(self):
        self.assertEqual(uncited('// the widget is blue [V]'), ['[V]'])

    def test_file_line_pyc_row_and_section_cite(self):
        for t in ('x [V `widget.py:12`]', 'x [V] (`gadget.rs:160-165`)', 'x [V inventedmod.pyc]',
                  'x [V] see findings/invented.tsv', 'x [V lane C §C2b]',
                  'x [V] in section 4', 'x [V recheck.md]', 'x [V] docs/guides/getting-started.md', 'x [V: tools/inv.py]'):
            self.assertEqual(uncited(t), [], t)

    def test_evidence_on_the_following_lines_of_the_paragraph_counts(self):
        self.assertEqual(uncited('[V] the flag is set\nin `widget.py:9`'), [])
        self.assertEqual(uncited('[V] the flag is set\n\nwidget.py:9'), ['[V]'], 'a blank line ends it')
        self.assertEqual(uncited('[V] a\nb\nc\nd\nwidget.py:9'), ['[V]'], 'at most three lines on')

    def test_not_marks(self):
        self.assertEqual(uncited('the `[V]` convention, a Vec as [Vec<u8>], [V1], [I] and [A]'), [])
        self.assertEqual(uncited('let a = [V, 3]; let b = [V; 4]; say "[V]" or \'[V]\' or `x [V]`'), [])

    def test_a_directory_or_a_prose_row_is_not_evidence(self):
        self.assertEqual(uncited('x [V] see docs/ and tools/ and row alphaKey'), ['[V]'])
        self.assertEqual(uncited('x [V] a bare .pyc'), ['[V]'])

    def test_a_citation_on_an_earlier_line_of_the_paragraph_counts(self):
        self.assertEqual(uncited('see `widget.py:9`\nthe flag is set [V]'), [])
        self.assertEqual(uncited('see `widget.py:9`\n\nthe flag is set [V]'), ['[V]'])

    def test_other_marks_on_one_line_are_judged_each_but_share_the_evidence(self):
        self.assertEqual(uncited('a [V] b [V] c'), ['[V]', '[V]'])


class Cli(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix='test-v-audit-')
        self.g = lambda *a: subprocess.run(['git', '-C', self.tmp] + list(a),
                                           check=True, capture_output=True, env=testenv.hermetic_git_env())
        self.g('init', '-q')
        self.w('old.md', 'an old claim [V]\n')
        self.w('other.md', 'quiet\n')
        self.g('add', '.')
        self.g('commit', '-qm', 'base')

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def w(self, name, text):
        with open(os.path.join(self.tmp, name), 'w') as f:
            f.write(text)

    def run_cli(self, *args):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = va.main(list(args) + ['--repo', self.tmp])
        return rc, out.getvalue()

    def test_changed_files_only_and_added_lines_only(self):
        self.w('old.md', 'an old claim [V]\na new claim [V]\n')
        self.w('new.md', 'fresh [V]\n\nciteed [V `a.py:1`]\n')
        rc, out = self.run_cli('--base', 'HEAD')
        self.assertEqual(rc, 3)
        self.assertIn('old.md\t1\t', out)
        self.assertIn('old.md\t2\t', out)
        self.assertIn('new.md\t1\t', out)
        self.assertNotIn('other.md', out)
        self.assertNotIn('new.md\t2\t', out)
        rc, out = self.run_cli('--base', 'HEAD', '--added-only')
        self.assertNotIn('old.md\t1\t', out, 'the old line is not part of the change')
        self.assertIn('old.md\t2\t', out)

    def test_odd_names_option_like_bases_and_a_subdirectory(self):
        os.makedirs(os.path.join(self.tmp, 'sub'))
        self.w('sub/caf\u00e9 \"q\".md', 'a claim [V]\n')
        rc, out = self.run_cli('--base', 'HEAD', '--added-only')
        self.assertIn('sub/caf\u00e9 "q".md\t1\t', out)
        out2 = io.StringIO()
        with contextlib.redirect_stdout(out2):
            rc = va.main(['--base', 'HEAD', '--repo', os.path.join(self.tmp, 'sub')])
        self.assertIn('sub/caf', out2.getvalue(), 'paths are relative to the repo top')
        self.assertEqual(self.run_cli('--base=--output=x')[0], 2)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, 'x')))

    def test_clean_tree_exits_zero_and_usage_errors_exit_two(self):
        self.w('old.md', 'an old claim [V `x.py:3`]\n')
        self.assertEqual(self.run_cli('--base', 'HEAD')[0], 0)
        self.assertEqual(self.run_cli()[0], 2)
        self.assertEqual(self.run_cli('--added-only', '--paths', 'old.md')[0], 2)
        self.assertEqual(self.run_cli('--base', 'no-such-ref')[0], 2)

    def test_paths_and_tsv(self):
        tsv = os.path.join(self.tmp, 'rows.tsv')
        rc, out = self.run_cli('--paths', 'old.md', '--tsv', tsv)
        self.assertEqual(rc, 3)
        with open(tsv) as f:
            self.assertEqual(f.read().splitlines()[0], 'path\tline\tmark\ttext')


if __name__ == '__main__':
    unittest.main()
