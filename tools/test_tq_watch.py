#!/usr/bin/env python3
"""Tests for tools/tq-watch.py. Invented start.ini, RSS and ESI documents and invented build
numbers; the network function is replaced, so nothing here touches the network.

    python3 -I tools/test_tq_watch.py
"""
import builtins
import contextlib
import importlib.util
import io
import json
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location('tq_watch', os.path.join(HERE, 'tq-watch.py'))
W = importlib.util.module_from_spec(_spec)
sys.modules['tq_watch'] = W
_spec.loader.exec_module(W)

REG = '# invented registry\n#hdr\n1000\tV10.01\tpinned\t-\tx\n1100\tV10.01\tunverified\t-\ty\n'
INI = '[main]\nbuild = %d\ncodename = V10.01\nregion = x\n'
ESI = '{"players": 1, "server_version": "%s", "start_time": "t"}'
RSS = '''<?xml version="1.0"?><rss version="2.0"><channel>
<item><title>Version 10.02 Patch Notes</title><pubDate>d1</pubDate><link>https://example.invalid/a</link>
<category>Patch Notes</category>
<description>&lt;h2&gt;Ships&lt;/h2&gt;&lt;ul&gt;&lt;li&gt;The Widget gets &lt;b&gt;faster&lt;/b&gt;&lt;/li&gt;&lt;/ul&gt;&lt;p&gt;prose&lt;/p&gt;</description></item>
<item><title>Version 10.01 Patch Notes</title><pubDate>d0</pubDate><link>https://example.invalid/b</link><category>Patch Notes</category></item>
<item><title>A dev blog</title><link>https://example.invalid/c</link><category>Dev Blogs</category></item>
</channel></rss>'''
RSS_SAME_LINE = RSS.replace('Version 10.02', 'Version 10.01 hotfix')


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix='tqwatch-')
        self.addCleanup(shutil.rmtree, self.td)
        self.reg = os.path.join(self.td, 'builds.tsv')
        self.state = os.path.join(self.td, 'out', 'state.tsv')
        self.install = os.path.join(self.td, 'online')
        os.makedirs(self.install)
        self.ini = os.path.join(self.install, 'start.ini')
        with open(self.reg, 'w') as f:
            f.write(REG)
        self.net = {}
        self.fetched = []
        self._orig = W.fetch
        W.fetch = self.fake_fetch
        self.addCleanup(setattr, W, 'fetch', self._orig)

    def fake_fetch(self, url):
        self.fetched.append(url)
        v = self.net.get(url)
        if v is None:
            raise W.Failed('no route')
        return v.encode()

    def ini_build(self, b):
        with open(self.ini, 'w') as f:
            f.write(INI % b)

    def run_tool(self, *extra):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = W.main(['--registry', self.reg, '--state', self.state, *extra])
        return rc, out.getvalue(), err.getvalue()


class Exits(Base):
    def test_nothing_new_is_zero(self):
        self.ini_build(1100)
        rc, out, _ = self.run_tool('--online-ini', self.ini)
        self.assertEqual(rc, 0)
        rc, out, _ = self.run_tool('--online-ini', self.ini)
        self.assertEqual((rc, out), (0, ''), 'a fact is printed once')

    def test_an_unregistered_install_build_is_four(self):
        self.ini_build(1200)
        rc, out, _ = self.run_tool('--online-ini', self.ini)
        self.assertEqual(rc, 4)
        self.assertIn('start.ini: 1200 (V10.01)', out)
        rc, out, _ = self.run_tool('--online-ini', self.ini)
        self.assertEqual((rc, out), (4, ''), 'still due, not printed again')

    def test_a_status_build_newer_than_the_registry_is_three(self):
        self.net[W.ESI_URL] = ESI % 1300
        rc, out, _ = self.run_tool('--esi')
        self.assertEqual(rc, 3)
        self.assertIn('esi: 1300', out)

    def test_a_status_build_that_is_registered_is_zero(self):
        self.net[W.ESI_URL] = ESI % 1100
        self.assertEqual(self.run_tool('--esi')[0], 0)

    def test_a_new_line_in_the_notes_is_five(self):
        self.net[W.NOTES_URL] = RSS
        rc, out, _ = self.run_tool('--notes')
        self.assertEqual(rc, 5)
        self.assertIn('Version 10.02 Patch Notes (V10.02)', out)
        self.assertNotIn('A dev blog', out)
        rc, out, _ = self.run_tool('--notes')
        self.assertEqual((rc, out), (5, ''), 'the warning stands until the line changes')

    def test_a_new_notes_entry_on_the_served_line_is_three(self):
        self.net[W.NOTES_URL] = RSS_SAME_LINE
        self.assertEqual(self.run_tool('--notes')[0], 3)
        self.assertEqual(self.run_tool('--notes')[0], 0)

    def test_five_wins_over_four_over_three(self):
        self.ini_build(1200)
        self.net[W.ESI_URL] = ESI % 1300
        self.net[W.NOTES_URL] = RSS
        self.assertEqual(self.run_tool('--online-ini', self.ini, '--esi', '--notes')[0], 5)
        self.net[W.NOTES_URL] = RSS_SAME_LINE
        self.assertEqual(self.run_tool('--online-ini', self.ini, '--esi', '--notes')[0], 4)

    def test_a_failed_fetch_is_one_never_zero(self):
        rc, _, err = self.run_tool('--esi')
        self.assertEqual(rc, 1)
        self.assertIn('SOURCE FAILED', err)
        self.net[W.ESI_URL] = 'not json'
        self.assertEqual(self.run_tool('--esi')[0], 1)
        self.assertEqual(self.run_tool('--online-ini', os.path.join(self.td, 'elsewhere', 'missing.ini'))[0], 1)

    def test_usage_is_two(self):
        self.assertEqual(self.run_tool()[0], 2)
        self.assertEqual(self.run_tool('--frobnicate')[0], 2)
        self.assertEqual(self.run_tool('--esi', '--save-outline', self.td)[0], 2)

    def test_nothing_is_fetched_unless_asked(self):
        self.ini_build(1100)
        self.run_tool('--online-ini', self.ini)
        self.assertEqual(self.fetched, [])
        self.net[W.ESI_URL] = ESI % 1100
        self.run_tool('--esi')
        self.assertEqual(self.fetched, [W.ESI_URL])
        self.assertTrue(W.ESI_URL.startswith('https://'))


class State(Base):
    def test_state_has_the_columns_and_one_row_per_new_fact(self):
        self.ini_build(1200)
        self.run_tool('--online-ini', self.ini)
        self.run_tool('--online-ini', self.ini)
        with open(self.state) as f:
            rows = [l.rstrip('\n').split('\t') for l in f]
        self.assertEqual(rows[0], list(W.STATE_HEAD))
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[1][1:4], ['start.ini', '1200', 'V10.01'])

    def test_json_output(self):
        self.ini_build(1200)
        rc, out, _ = self.run_tool('--online-ini', self.ini, '--json')
        d = json.loads(out)
        self.assertEqual((rc, d['exit']), (4, 4))
        self.assertEqual(d['facts'][0]['build_or_title'], '1200')


class Outline(Base):
    def test_save_outline_keeps_headings_and_list_items_only(self):
        self.net[W.NOTES_URL] = RSS
        d = os.path.join(self.td, 'notes')
        rc, out, _ = self.run_tool('--notes', '--save-outline', d)
        self.assertEqual(rc, 5)
        with open(os.path.join(d, 'V10.02.txt')) as f:
            text = f.read()
        self.assertIn('## Ships', text)
        self.assertIn('- The Widget gets faster', text)
        self.assertNotIn('prose', text)
        self.assertEqual(os.listdir(d), ['V10.02.txt'], 'an entry with no body writes nothing')


class OneFile(Base):
    def test_exactly_one_file_is_opened_under_the_install(self):
        self.ini_build(1100)
        # make anything else a trap: a sibling that must never be touched
        os.makedirs(os.path.join(self.td, 'online-compat'))
        opened, real_open, real_os_open = [], builtins.open, os.open

        def spy(path, *a, **k):
            opened.append(os.fspath(path) if not isinstance(path, int) else path)
            return real_open(path, *a, **k)

        def spy_os(path, flags, *a, **k):
            opened.append(os.fspath(path))
            self.assertTrue(flags & os.O_NOFOLLOW and flags & os.O_NONBLOCK, 'the ini is opened no-follow, non-blocking')
            return real_os_open(path, flags, *a, **k)

        calls = []
        real = {n: getattr(os, n) for n in ('listdir', 'scandir', 'walk', 'stat')}
        for n in ('listdir', 'scandir', 'walk'):
            setattr(os, n, lambda p, *a, _n=n, **k: (calls.append((_n, p)), real[_n](p, *a, **k))[1])
        builtins.open = spy
        os.open = spy_os
        try:
            rc, _, _ = self.run_tool('--online-ini', self.ini)
        finally:
            builtins.open = real_open
            os.open = real_os_open
            for n in ('listdir', 'scandir', 'walk'):
                setattr(os, n, real[n])
        self.assertEqual(rc, 0)
        under = [p for p in opened if isinstance(p, str) and p.startswith(self.install)]
        self.assertEqual(under, [self.ini])
        self.assertEqual([c for c in calls if str(c[1]).startswith(self.install)], [])
        self.assertTrue(all(not p.startswith(os.path.join(self.td, 'online-compat')) for p in opened if isinstance(p, str)))


class Safety(Base):
    def test_default_state_is_under_the_repo_root_not_the_working_directory(self):
        self.assertEqual(W.REPO, os.path.dirname(HERE))
        ap_default = os.path.join(W.REPO, 'out', 'tq-watch', 'state.tsv')
        old = os.getcwd()
        os.chdir(self.td)
        self.addCleanup(os.chdir, old)
        self.ini_build(1100)
        seen = {}
        real = W.append_state
        W.append_state = lambda path, rows: seen.setdefault('p', path)
        self.addCleanup(setattr, W, 'append_state', real)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            W.main(['--registry', self.reg, '--online-ini', self.ini])
        self.assertEqual(seen['p'], ap_default)

    def test_a_write_target_under_the_install_or_compatdata_is_refused(self):
        self.ini_build(1100)
        link = os.path.join(self.td, 'sneaky')
        os.symlink(self.install, link)
        for target in (os.path.join(self.install, 'state.tsv'), os.path.join(link, 'state.tsv'),
                       os.path.join(self.td, 'compatdata', '8500', 'state.tsv')):
            rc, _, err = self.run_tool('--online-ini', self.ini, '--state', target)
            self.assertEqual(rc, 2, target)
            self.assertIn('refusing to write', err)
            self.assertFalse(os.path.exists(target))
        self.net[W.NOTES_URL] = RSS
        rc, _, err = self.run_tool('--online-ini', self.ini, '--notes', '--save-outline', os.path.join(link, 'o'))
        self.assertEqual(rc, 2)
        self.assertEqual(self.fetched, [], 'nothing is fetched once a target is refused')
        self.assertEqual(os.listdir(self.install), ['start.ini'])

    def test_a_symlinked_or_non_regular_ini_is_a_failure(self):
        self.ini_build(1100)
        os.makedirs(os.path.join(self.td, 'inis'))
        link = os.path.join(self.td, 'inis', 'link.ini')
        os.symlink(self.ini, link)
        rc, _, err = self.run_tool('--online-ini', link)
        self.assertEqual(rc, 1)
        self.assertIn('SOURCE FAILED', err)
        fifo = os.path.join(self.td, 'inis', 'fifo.ini')
        os.mkfifo(fifo)
        rc, _, err = self.run_tool('--online-ini', fifo)  # must not block
        self.assertEqual(rc, 1)
        self.assertIn('not a regular file', err)
        os.makedirs(os.path.join(self.td, 'inis', 'adir'))
        rc, _, err = self.run_tool('--online-ini', os.path.join(self.td, 'inis', 'adir'))  # a directory
        self.assertEqual(rc, 1)

    def test_a_redirect_to_http_is_refused(self):
        class R:
            def __init__(s, url): s.url = url
            def geturl(s): return s.url
            def read(s): return b'x'
            def __enter__(s): return s
            def __exit__(s, *a): return False
        W.fetch = self._orig
        for final, ok in (('http://evil.invalid/x', False), ('https://example.invalid/x', True)):
            with mock_urlopen(R(final)):
                if ok:
                    self.assertEqual(W.fetch('https://example.invalid/'), b'x')
                else:
                    with self.assertRaises(W.Failed):
                        W.fetch('https://example.invalid/')
        with self.assertRaises(W.Failed):
            W.fetch('http://example.invalid/')

    def test_a_failure_is_reported_even_when_a_finding_decides_the_exit(self):
        self.ini_build(1200)
        rc, out, err = self.run_tool('--online-ini', self.ini, '--esi')  # esi has no route
        self.assertEqual(rc, 4)
        self.assertIn('SOURCE FAILED', err)
        rc, out, _ = self.run_tool('--online-ini', self.ini, '--esi', '--json')
        self.assertEqual(json.loads(out)['failed'], ['no route'])
        self.assertIn('SOURCE FAILED', W.__doc__)


@contextlib.contextmanager
def mock_urlopen(resp):
    real = W.urllib.request.urlopen
    W.urllib.request.urlopen = lambda *a, **k: resp
    try:
        yield
    finally:
        W.urllib.request.urlopen = real


if __name__ == '__main__':
    unittest.main()
