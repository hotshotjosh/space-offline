#!/usr/bin/env python3
"""Tests for tools/intake-issues.py on an invented intake.tsv. Nothing is posted, and the tool
has no network code to test; the checks are the drafts and the index.

    python3 -I tools/test_intake_issues.py
"""
import contextlib
import importlib.util
import io
import os
import re
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location('intake_issues', os.path.join(HERE, 'intake-issues.py'))
I = importlib.util.module_from_spec(_spec)
sys.modules['intake_issues'] = I
_spec.loader.exec_module(I)

HEAD = 'area\tkind\twhat\tcount\tsample_ids\tserver_readers\tin_notes\tverdict\tsuggested_title\n'
ROWS = [
    ('upwell', 'dogma', 'new modifier func', '3', '1001,1002,1003,1004,1005,1006,1007', 'server/a.rs:10,/home/alice/secret.rs:3,../../etc/x',
     'yes', 'needs-work', 'the Widget effect applies its modifier'),
    ('upwell', 'dogma', 'effect with no modifiers', '2', '2001', '-', 'no', 'review', 'effects with no modifiers do nothing visible'),
    ('market', 'types', 'new hulls', '40', '3001,3002', '-', '-', 'data-driven', 'nothing to do'),
    ('pve', 'dungeons', 'new archetype', '1', '4001', '-', '-', 'notes-only', 'the new archetype spawns'),
]
TRIAGE = ('tier\trule\tevidence\tnext\nT2\trelease-line\tstart.ini main.codename V10.01 -> V10.02\tT2.1\n'
          'T1\tmodules-in-meaning\t1 module\tT1.1\n')


def row(*c):
    return '\t'.join(c) + '\n'


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix='intake-')
        self.addCleanup(shutil.rmtree, self.td)
        self.dir = os.path.join(self.td, 'out', 'new-build', '9999', 'content')
        os.makedirs(self.dir)
        self.intake = os.path.join(self.dir, 'intake.tsv')
        self.triage = os.path.join(self.td, 'triage.tsv')
        self.out = os.path.join(self.td, 'issues')
        self.put(self.intake, HEAD + ''.join(row(*r) for r in ROWS))
        self.put(self.triage, TRIAGE)

    def put(self, p, text):
        with open(p, 'w') as f:
            f.write(text)

    def run_tool(self, *extra):
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = I.main([self.intake, '--out', self.out, *extra])
        return rc, err.getvalue()

    def read(self, name):
        with open(os.path.join(self.out, name)) as f:
            return f.read()


class Drafts(Base):
    def test_one_draft_per_area_and_kind_and_none_for_data_driven(self):
        rc, _ = self.run_tool('--surface', self.triage)
        self.assertEqual(rc, 0)
        self.assertEqual(sorted(os.listdir(self.out)),
                         ['01-pve-dungeons.md', '02-upwell-dogma.md', 'index.tsv'])

    def test_index_has_file_title_labels(self):
        self.run_tool('--surface', self.triage)
        lines = self.read('index.tsv').splitlines()
        self.assertEqual(lines[0], 'file\ttitle\tlabels')
        f, title, labels = lines[2].split('\t')
        self.assertEqual(f, '02-upwell-dogma.md')
        self.assertTrue(title.startswith('V10.02: '), title)
        self.assertEqual(labels, 'tq-intake,area/upwell')
        self.assertNotIn('status/ready', self.read('index.tsv'))

    def test_the_draft_follows_the_template(self):
        self.run_tool('--surface', self.triage, '--parent', '77')
        d = self.read('02-upwell-dogma.md')
        for field in ('**Build:** 9999 (line V10.02), tier T2', '**Area:** upwell', '**Verdict:** must',
                      '**What changed in the client:**', '**What the server does today:**', '**Spec:**',
                      '**Proof:**', '**Status row:**', '**Evidence:** [V]', '**[A] values:**', 'Part of #77'):
            self.assertIn(field, d)
        self.assertTrue(d.startswith('Title: V10.02: '))

    def test_without_parent_triage_or_line_the_draft_has_placeholders(self):
        self.run_tool()
        d = self.read('02-upwell-dogma.md')
        self.assertIn('Part of #<patch-day issue>', d)
        self.assertIn('**Build:** 9999 (line ?), tier ?', d)
        self.assertTrue(d.startswith('Title: Build 9999: '))

    def test_no_id_list_over_five_and_no_path_outside_the_repo(self):
        self.run_tool('--surface', self.triage)
        for n in os.listdir(self.out):
            self.assertNotRegex(self.read(n), r'/home/|\.\./|(?<![\w.<])~/')
        d = self.read('02-upwell-dogma.md')
        self.assertIn('1001, 1002, 1003, 1004, 1005', d)
        self.assertNotIn('1006', d)
        self.assertIn('server/a.rs:10', d)
        self.assertIn('<path removed>', d)
        for n in os.listdir(self.out):
            for line in self.read(n).splitlines():
                self.assertLessEqual(len(re.findall(r'\b\d{4,}\b', line.replace('9999', ''))), 5, line)

    def test_nothing_is_written_outside_out(self):
        before = set(os.listdir(self.td))
        self.run_tool('--surface', self.triage)
        self.assertEqual(set(os.listdir(self.td)) - before, {'issues'})


class Scrub(Base):
    HOSTILE = [
        'E:\\Users\\someone\\game\\x.rs', 'D:/work/secret.txt', '\\\\fileserver\\share\\x',
        '/home/alice/x', '~/x', '../up/x', 'file:/etc/passwd', 'ref:/root/x',
        'https://internal.example.com/a?b=1', 'http://10.0.0.5:8080/x', '10.20.30.40', '10.20.30.40:22',
        'build-box.example.com', 'gitlab.internal', 'host.example.com:9443',
    ]

    def test_every_interpolated_cell_is_scrubbed(self):
        for i, bad in enumerate(self.HOSTILE):
            cells = {'area': 'a %s' % bad, 'kind': 'k', 'what': 'w %s w' % bad, 'count': '1',
                     'sample_ids': '1001,%s' % bad, 'server_readers': 'x.rs:1,%s' % bad,
                     'in_notes': bad, 'verdict': 'needs-work', 'suggested_title': 'do %s' % bad}
            self.put(self.intake, HEAD + row(*[cells[c] for c in I.COLS]))
            out = os.path.join(self.td, 'iss%d' % i)
            err = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                rc = I.main([self.intake, '--out', out, '--surface', self.triage, '--parent', '5',
                             '--line', 'V1.0 ' + bad, '--build', '77'])
            self.assertEqual(rc, 0, bad)
            for n in os.listdir(out):
                with open(os.path.join(out, n)) as f:
                    text = f.read()
                for frag in ('someone', 'secret', 'fileserver', '/etc/passwd', '/root', 'example.com', '10.0.0.5',
                             '192.168', 'gitlab.internal', 'C:\\', 'D:/', '../up', '~/x', 'build-box'):
                    self.assertNotIn(frag, text, '%s leaked %r in %s' % (bad, frag, n))

    def test_repo_relative_paths_and_rs_names_survive(self):
        self.assertEqual(I.clean('server/a.rs:10 and tools/x.py'), 'server/a.rs:10 and tools/x.py')
        self.assertEqual(I.clean('a.rs'), 'a.rs')


class Exits(Base):
    def test_nothing_to_file_is_one(self):
        self.put(self.intake, HEAD + row(*ROWS[2]))
        self.assertEqual(self.run_tool()[0], 1)
        self.assertFalse(os.path.exists(os.path.join(self.out, 'index.tsv')))

    def test_usage_is_two(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(I.main([]), 2)
            self.assertEqual(I.main([os.path.join(self.td, 'none.tsv'), '--out', self.out]), 2)
            self.assertEqual(I.main([self.intake, '--out', self.out, '--parent', 'abc']), 2)
        self.put(self.intake, 'area\tkind\n')
        self.assertEqual(self.run_tool()[0], 2)


if __name__ == '__main__':
    unittest.main()
