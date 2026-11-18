#!/usr/bin/env python3
"""Tests for tools/build-triage.py: invented delta, surface, recheck, shapes and bundle-diff
files, one test per rule and one per tier boundary. No client data, no [A] values.

    python3 -I tools/test_build_triage.py
"""
import contextlib
import importlib.util
import io
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location('build_triage', os.path.join(HERE, 'build-triage.py'))
B = importlib.util.module_from_spec(_spec)
sys.modules['build_triage'] = B
_spec.loader.exec_module(B)

HEAD = ('# build-delta: invented\n# build-delta format 2: module class `recompiled`\n'
        'layer\titem\tclass\told\tnew\tdetail\n')
VERDICT = 'verdict\t1->2\tdiffers\t-\t-\twhatever\n'
SURF_HEAD = 'section\tchange\tkey\tdetail\tserved\timpact\n'
RECHECK_HEAD = 'section\tstatus\twhere\twhat\tdetail\n'
SHAPES_HEAD = 'pass\tstatus\tkey\tserver_site\told_readers\tnew_readers\tdetail\n'


def row(*c):
    return '\t'.join(c) + '\n'


def delta(*rows, verdict=True):
    return HEAD + (VERDICT if verdict else '') + ''.join(rows)


QUIET = [row('module', 'alpha/one', 'recompiled', 'a', 'b', 'file name, lines, future flags only'),
         row('file', 'bin64/blue.dll', 'signature-only', 'a', 'b', '-'),
         row('res', 'resfileindex.txt:res:/x/*', 'other', '-', '-', '+1 -0 ~2'),
         row('schema', 'thingLoader', 'identical', '-', '-', '-'),
         row('ini', 'main.build', 'differs', '1', '2', '-'),
         row('ini', 'main.codename', 'same', 'V1.0', 'V1.0', '-')]


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix='triage-')
        self.addCleanup(shutil.rmtree, self.td)
        self.out = os.path.join(self.td, 'out')
        self.delta = os.path.join(self.td, 'delta.tsv')

    def w(self, rel, text):
        p = os.path.join(self.td, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, 'w') as f:
            f.write(text)
        return p

    def run_tool(self, delta_rows=(), *extra, **kw):
        self.w('delta.tsv', delta(*delta_rows, **kw) if not isinstance(delta_rows, str) else delta_rows)
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = B.main(['2', '--pinned', '1', '--delta', self.delta, '--out', self.out,
                         '--registry', os.path.join(self.td, 'none.tsv'), *extra])
        return rc, err.getvalue()

    def tsv(self):
        with open(os.path.join(self.out, 'triage.tsv')) as f:
            return [l.rstrip('\n').split('\t') for l in f][1:]

    def md(self):
        with open(os.path.join(self.out, 'triage.md')) as f:
            return f.read()


class Tiers(Base):
    def test_t0_when_only_data_recompiled_and_signature_moved(self):
        rc, _ = self.run_tool(QUIET)
        self.assertEqual(rc, 20)
        self.assertEqual(self.tsv()[0][:2], ['T0', 'data-only'])
        self.assertIn('T0, data only, same line', self.md())

    def test_t1_a_module_changed_in_meaning(self):
        rc, _ = self.run_tool(QUIET + [row('module', 'alpha/two', 'changed', 'a', 'b', 'x')])
        self.assertEqual(rc, 21)
        self.assertEqual(self.tsv()[0][:2], ['T1', 'modules-in-meaning'])
        self.assertIn('modules in meaning: 1', self.md())

    def test_t1_a_served_surface_row(self):
        self.w('s/surface-diff.tsv', SURF_HEAD + row('G1', 'arity', 'svc.Call', 'd', 'yes', 'yes')
               + row('G3', 'added', 'OnThing', 'd', 'no', 'no'))
        rc, _ = self.run_tool(QUIET, '--surface', os.path.join(self.td, 's'))
        self.assertEqual(rc, 21)
        self.assertEqual(self.tsv()[0][1], 'surface-served')
        self.assertIn('served-and-changed surface rows: 1', self.md())

    def test_an_unserved_surface_row_stays_t0(self):
        self.w('s/surface-diff.tsv', SURF_HEAD + row('G3', 'added', 'OnThing', 'd', 'no', 'no'))
        rc, _ = self.run_tool(QUIET, '--surface', os.path.join(self.td, 's'))
        self.assertEqual(rc, 20)

    def test_t1_a_shapes_served_row(self):
        self.w('sh/shapes.tsv', SHAPES_HEAD + row('B', 'SERVED gone', 'inventedKey', 'x.rs:1', '1', '0', 'd'))
        rc, _ = self.run_tool(QUIET, '--shapes', os.path.join(self.td, 'sh'))
        self.assertEqual(rc, 21)
        self.assertEqual(self.tsv()[0][1], 'shapes-served')

    def test_t1_a_recheck_changed_or_gone_row(self):
        for status, want in (('changed', 21), ('gone', 21), ('unresolved', 20), ('skipped', 20)):
            self.w('r/recheck.tsv', RECHECK_HEAD + row('table', status, 'a/b', 'sym', 'd'))
            rc, _ = self.run_tool(QUIET, '--recheck', os.path.join(self.td, 'r'))
            self.assertEqual(rc, want, status)

    def test_t1_a_bundle_flag_on_an_id_we_use(self):
        text = ('files: 1\nFLAGS (1)\n  REMOVED    types/typedata.tsv: 1234, 5678\n\n'
                'USES of changed ids:\n  1234: server/x.rs x1\n')
        rc, _ = self.run_tool(QUIET, '--bundle-diff', self.w('b.txt', text))
        self.assertEqual(rc, 21)
        self.assertEqual(self.tsv()[0][1], 'bundle-flag-used')
        self.assertIn('1 id(s) we use: 1234', self.tsv()[0][2])

    def test_a_bundle_flag_on_an_id_we_do_not_use_stays_t0(self):
        for text in ('FLAGS (1)\n  REMOVED    a: 1234\n\nUSES of changed ids:\n  9999: server/x.rs x1\n',
                     'FLAGS (1)\n  REMOVED    a: 1234\n\nUSES of changed ids:\n  none\n',
                     'FLAGS: none\n\nUSES:\n  1234: x x1\n',
                     'FLAGS (1)\n  HEADER     a.tsv\n\nUSES of changed ids:\n  1234: x x1\n'):
            rc, _ = self.run_tool(QUIET, '--bundle-diff', self.w('b.txt', text))
            self.assertEqual(rc, 20, text)

    def test_t2_a_new_codename_version_or_region(self):
        for item in ('main.codename', 'main.version', 'main.region'):
            rc, _ = self.run_tool(QUIET + [row('ini', item, 'differs', 'A', 'B', '-')])
            self.assertEqual(rc, 22, item)
        self.assertEqual(self.tsv()[0][:2], ['T2', 'release-line'])
        self.assertIn('Line change: main.region A -> B', self.md())

    def test_t2_beats_t1_and_keeps_the_t1_rows(self):
        rc, _ = self.run_tool(QUIET + [row('ini', 'main.codename', 'differs', 'A', 'B', '-'),
                                       row('module', 'alpha/two', 'changed', 'a', 'b', 'x')])
        self.assertEqual(rc, 22)
        self.assertEqual([r[0] for r in self.tsv()], ['T2', 'T1'])

    def test_t3_a_native_that_is_not_identical_or_signature_only(self):
        for f in ('bin64/blue.dll', 'bin64/_destiny.dll', 'bin64/exefile.exe', 'bin64/python27.dll'):
            for cls in ('code-differs', 'rebuilt', 'data-differs', 'added', 'removed'):
                rc, _ = self.run_tool(QUIET + [row('file', f, cls, 'a', 'b', '-')])
                self.assertEqual(rc, 23, '%s %s' % (f, cls))
            for cls in ('identical', 'signature-only'):
                rc, _ = self.run_tool(QUIET + [row('file', f, cls, 'a', 'b', '-')])
                self.assertEqual(rc, 20, '%s %s' % (f, cls))
        rc, _ = self.run_tool(QUIET + [row('file', 'bin64/other.dll', 'code-differs', 'a', 'b', '-')])
        self.assertEqual(rc, 20, 'only the four natives decide')

    def test_t1_a_module_added_or_removed(self):
        for cls in ('added', 'removed'):
            rc, _ = self.run_tool(QUIET + [row('module', 'alpha/new', cls, 'a', 'b', '-')])
            self.assertEqual(rc, 21, cls)
            self.assertEqual(self.tsv()[0][1], 'modules-added-removed')
        self.assertIn('modules removed: 1', self.md())

    def test_t3_a_wire_module_changed_or_gone(self):
        for item, cls, want in (('carbon/common/script/net/machoNet', 'changed', 23),
                                ('carbon/common/lib/blue', 'removed', 23),
                                ('carbon/common/script/net/newThing', 'added', 23),
                                ('eve/common/script/net/eveMachoNet', 'changed', 23),
                                ('eve/client/script/ui/destinyui', 'changed', 23),
                                ('eve/client/script/environment/ballparkSvc', 'changed', 23),
                                ('eve/client/script/michelle', 'changed', 23),
                                ('carbon/common/script/net/machoNet', 'recompiled', 20),
                                ('eve/client/script/ui/other', 'changed', 21)):
            rc, _ = self.run_tool(QUIET + [row('module', item, cls, 'a', 'b', '-')])
            self.assertEqual(rc, want, item)

    def test_t3_the_handshake_module_or_a_recheck_row_naming_a_constant(self):
        rc, _ = self.run_tool(QUIET + [row('module', 'eve/common/lib/eveMachoNetVersion', 'changed', 'a', 'b', '-')])
        self.assertEqual(rc, 23)
        self.w('r/recheck.tsv', RECHECK_HEAD + row('table', 'changed', 'a/b', '__HANDSHAKE_SECRET_KEY', 'd'))
        rc, _ = self.run_tool(QUIET, '--recheck', os.path.join(self.td, 'r'))
        self.assertEqual(rc, 23)

    def test_t3_gateway_strings(self):
        self.w('s/surface-diff.tsv', SURF_HEAD + row('G4', 'changed', 'eve_public.gateway.Thing', 'd', 'no', 'no'))
        rc, _ = self.run_tool(QUIET, '--surface', os.path.join(self.td, 's'))
        self.assertEqual(rc, 23)
        self.w('s/surface-diff.tsv', SURF_HEAD + row('G4', 'added', 'eve_public.gateway.Thing', 'd', 'no', 'no'))
        rc, _ = self.run_tool(QUIET, '--surface', os.path.join(self.td, 's'))
        self.assertEqual(rc, 20)

    def test_t3_a_patch_rule_that_refuses(self):
        self.assertEqual(self.run_tool(QUIET, '--locate-exit', '4')[0], 23)
        self.assertEqual(self.run_tool(QUIET, '--locate-exit', '0')[0], 20)

    def test_another_locate_exit_is_skipped_not_a_refusal_and_not_a_pass(self):
        for code in ('1', '2'):
            rc, _ = self.run_tool(QUIET, '--locate-exit', code)
            self.assertEqual(rc, 20)
            self.assertIn('SKIPPED locate: exit %s is not a refusal' % code, self.md())


class Inputs(Base):
    def test_a_missing_input_is_skipped_and_named_never_a_pass(self):
        rc, _ = self.run_tool(QUIET)
        self.assertEqual(rc, 20)
        md = self.md()
        for what in ('surface', 'recheck', 'shapes', 'bundle-diff'):
            self.assertIn('SKIPPED %s' % what, md)
        self.assertIn('(not a pass)', md)

    def test_malformed_inputs_are_exit_one(self):
        self.assertEqual(self.run_tool('layer\titem\tclass\nverdict\tx\tdiffers\n')[0], 1, 'format 1')
        self.assertEqual(self.run_tool(QUIET, verdict=False)[0], 1, 'no verdict')
        self.assertEqual(self.run_tool('')[0], 1, 'empty')
        self.w('s/surface-diff.tsv', 'wrong\theader\n')
        self.assertEqual(self.run_tool(QUIET, '--surface', os.path.join(self.td, 's'))[0], 1)
        self.assertEqual(self.run_tool(QUIET, '--bundle-diff', self.w('b.txt', 'nothing useful\n'))[0], 1)

    def test_a_missing_delta_is_exit_one(self):
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = B.main(['2', '--pinned', '1', '--delta', os.path.join(self.td, 'none.tsv'), '--out', self.out])
        self.assertEqual(rc, 1)

    def test_usage_is_exit_two(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(B.main([]), 2)
            self.assertEqual(B.main(['notanumber']), 2)
            self.assertEqual(B.main(['2', '--frob']), 2)

    def test_shapes_default_to_the_surface_dir_port_surface_writes(self):
        self.w('out/surface/shapes/shapes.tsv', SHAPES_HEAD + row('B', 'SERVED gone', 'inventedKey', 'x.rs:1', '1', '0', 'd'))
        rc, _ = self.run_tool(QUIET)
        self.assertEqual(rc, 21)

    def test_pinned_and_delta_default_from_the_registry(self):
        reg = self.w('builds.tsv', '#hdr\n7\tL\tpinned\t-\tx\n8\tL\tunverified\t-\ty\n')
        old = os.getcwd()
        os.chdir(self.td)
        self.addCleanup(os.chdir, old)
        self.w('findings/build-delta-7-8.tsv', delta(*QUIET))
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            rc = B.main(['8', '--registry', reg])
        self.assertEqual(rc, 20)
        self.assertTrue(os.path.isfile(os.path.join(self.td, 'out', 'new-build', '8', 'triage.md')))


if __name__ == '__main__':
    unittest.main()
