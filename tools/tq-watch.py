#!/usr/bin/env python3
"""Has Tranquility moved? Read the online install's start.ini, optionally the public
status endpoint and the patch-notes feed, and say what is new.

  python3 -I tools/tq-watch.py [--online-ini PATH] [--esi] [--notes] [--save-outline DIR]
                               [--state FILE] [--registry FILE] [--json]

Inputs:
  --online-ini PATH  ONE file: the online install's start.ini, opened read-only. That is
                     the whole of the access: nothing else under that prefix is listed,
                     stat-ed or opened. No default (the public tool knows no install).
  --esi              one HTTPS GET of the public status endpoint (stdlib urllib, a generic
                     User-Agent, no query data, no token). server_version is the build.
  --notes            one GET of the public RSS feed; only entries tagged patch notes are
                     kept (title, date, link).
  --save-outline DIR with --notes: store each new entry's headings and list items as text,
                     DIR/<version>.txt (html.parser on the entry's own description; no
                     second fetch). The text is CCP's: a local outline, never committed.
  --registry FILE    the build registry (default: tools/builds.tsv beside this script).
  --state FILE       default: out/tq-watch/state.tsv under the repo root (not the working directory).
                     A state file or outline dir that resolves (realpath) under the install's
                     directory, or whose path holds `compatdata`, is refused (exit 2).

Output: appends one row per new fact to --state (default out/tq-watch/state.tsv; columns
utc, source, build_or_title, line, note) and prints one line per new fact. Every write goes
under that state file's directory (and --save-outline's); nothing else is written.

Exit:
  0  nothing new
  3  a source shows a build, or a patch-notes entry, newer than anything registered / seen
  4  the online install's start.ini holds a build the registry does not have (import due)
  5  the notes announce a version above the served line's (a new release line: a T2 warning,
     usually ahead of the client)
  1  a source could not be read (never a pass)
  2  usage, or a write target under the install (see --state)
A source that fails is ALWAYS reported: a line `tq-watch: SOURCE FAILED: <what>` on stderr (and
`failed` in --json), also when a finding makes the exit 3, 4 or 5. The exit code carries the
finding; exit 1 means the failure was the only news. Read stderr, not only the code.
The standing conditions (3 by the status endpoint, 4, 5) repeat on every run until the
import or the line change is done; findings still print and are recorded once. When several
hold, 5 wins over 4 over 3.

[A] The status endpoint URL and the feed URL below are the public ones as the plan recorded
them; falsifier: a GET returns 404 or not the expected shape (then the run exits 1, not 0).
[A] The notes version is read from "Version X" / "VX" in the entry title; falsifier: a feed
whose titles carry no version (the entry is then listed, with no line, and exits 3 only).
"""
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import datetime
import html.parser
import errno
import json
import os
import re
import stat
import sys
import urllib.request
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
ESI_URL = 'https://esi.evetech.net/latest/status/'
NOTES_URL = 'https://www.eveonline.com/rss/patch-notes'
USER_AGENT = 'eve-offline-tq-watch'
TIMEOUT = 20
STATE_HEAD = ('utc', 'source', 'build_or_title', 'line', 'note')
VERSION = re.compile(r'\b(?:version|v)\s*(\d+)\.(\d+)\b', re.I)


class Failed(Exception):
    """A source could not be read."""


class Usage(Exception):
    """An argument or write target that is refused."""


def fetch(url):
    """One HTTPS GET, bytes. Tests replace this; nothing else here touches the network."""
    if not url.startswith('https://'):
        raise Failed('refusing a non-https url')
    req = urllib.request.Request(url, headers={'User-Agent': USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            final = r.geturl()
            if not final.startswith('https://'):
                raise Failed('%s: redirected to a non-https url' % url)
            return r.read()
    except Failed:
        raise
    except Exception as e:
        raise Failed('%s: %s' % (url, type(e).__name__))


def read_registry(path):
    """(set of builds, served line) from builds.tsv; `#` lines are comments."""
    builds, served = set(), None
    try:
        with open(path, encoding='utf8') as fh:
            for ln in fh:
                if ln.startswith('#') or not ln.strip():
                    continue
                c = ln.rstrip('\n').split('\t')
                if c[0].isdigit():
                    builds.add(int(c[0]))
                    if len(c) > 2 and c[2] == 'pinned':
                        served = c[1]
    except OSError as e:
        raise Failed('registry: %s' % e)
    return builds, served


def line_version(line):
    """'V24.02' -> (24, 2); None when it does not read as one."""
    m = VERSION.search(line or '')
    return (int(m.group(1)), int(m.group(2))) if m else None


def read_ini(path):
    """(build, codename) from the one file."""
    # One open, no symlink followed, no blocking on a FIFO, and only a regular file is read.
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as e:
        raise Failed('start.ini: %s' % errno.errorcode.get(e.errno, 'error'))
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise Failed('start.ini is not a regular file')
        with os.fdopen(fd, encoding='utf8', errors='replace') as fh:
            fd = -1
            text = fh.read()
    except OSError as e:
        raise Failed('start.ini: %s' % type(e).__name__)
    finally:
        if fd >= 0:
            os.close(fd)
    build = re.search(r'^\s*build\s*=\s*(\d+)', text, re.M)
    code = re.search(r'^\s*codename\s*=\s*(\S+)', text, re.M)
    if not build:
        raise Failed('start.ini has no numeric build')
    return int(build.group(1)), (code.group(1) if code else '')


def read_esi(data):
    try:
        v = json.loads(data.decode('utf8'))['server_version']
        return int(v)
    except Exception:
        raise Failed('status endpoint: not the expected JSON')


class Outline(html.parser.HTMLParser):
    """Headings and list items of an HTML fragment, as lines of text."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.lines, self._tag, self._buf = [], None, []

    def handle_starttag(self, tag, attrs):
        if tag in ('h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'li'):
            self._flush()
            self._tag, self._buf = tag, []

    def handle_endtag(self, tag):
        if tag == self._tag:
            self._flush()

    def handle_data(self, data):
        if self._tag:
            self._buf.append(data)

    def _flush(self):
        if self._tag:
            t = ' '.join(''.join(self._buf).split())
            if t:
                self.lines.append(('#' * int(self._tag[1]) + ' ' if self._tag[0] == 'h' else '- ') + t)
        self._tag, self._buf = None, []


def outline_of(fragment):
    p = Outline()
    p.feed(fragment or '')
    p.close()
    p._flush()
    return p.lines


def read_notes(data):
    """Patch-notes entries: dicts of title, date, link, line (a 'V24.02' or ''), outline."""
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        raise Failed('notes feed: not XML')
    out = []
    for item in root.iter('item'):
        def tx(name):
            e = item.find(name)
            return (e.text or '').strip() if e is not None else ''
        title = tx('title')
        cats = [(c.text or '').lower() for c in item.findall('category')]
        if not (any('patch notes' in c for c in cats) or 'patch notes' in title.lower()):
            continue
        body = ''
        for e in item.iter():
            if e.tag.endswith('encoded') or e.tag == 'description':
                body = e.text or body
        m = VERSION.search(title)
        out.append({'title': title, 'date': tx('pubDate'), 'link': tx('link'),
                    'line': 'V%s.%s' % (m.group(1), m.group(2)) if m else '', 'outline': outline_of(body)})
    return out


def read_state(path):
    seen = set()
    try:
        with open(path, encoding='utf8') as fh:
            for ln in fh:
                c = ln.rstrip('\n').split('\t')
                if len(c) >= 3 and c[0] != 'utc':
                    seen.add((c[1], c[2]))
    except OSError:
        pass
    return seen


def append_state(path, rows):
    if not rows:
        return
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    new = not os.path.exists(path)
    with open(path, 'a', encoding='utf8', newline='\n') as fh:
        if new:
            fh.write('\t'.join(STATE_HEAD) + '\n')
        for r in rows:
            fh.write('\t'.join(str(x).replace('\t', ' ').replace('\n', ' ') for x in r) + '\n')


def save_outline(dirpath, entry):
    if not entry['outline']:
        return None
    name = re.sub(r'[^A-Za-z0-9._-]+', '-', entry['line'] or entry['title']).strip('-') or 'notes'
    os.makedirs(dirpath, exist_ok=True)
    p = os.path.join(dirpath, name + '.txt')
    with open(p, 'w', encoding='utf8', newline='\n') as fh:
        fh.write('# %s (%s)\n# %s\n' % (entry['title'], entry['date'], entry['link']))
        fh.write('\n'.join(entry['outline']) + '\n')
    return p


def refuse_target(path, ini):
    """Why `path` may not be written, or None. A write must never land in the online install
    (under the ini's directory, by realpath) or anywhere holding `compatdata`."""
    real = os.path.realpath(path)
    if 'compatdata' in path or 'compatdata' in real:
        return '%s is under compatdata' % path
    if ini:
        for base in {os.path.abspath(os.path.dirname(ini)), os.path.realpath(os.path.dirname(ini))}:
            if real == base or real.startswith(base + os.sep):
                return '%s is under the install directory' % path
    return None


def watch(args, now=None):
    """(exit code, findings). A finding is (source, build_or_title, line, note, new?, code)."""
    for target in (args.state, args.save_outline):
        if target:
            why = refuse_target(target, args.online_ini)
            if why:
                raise Usage('refusing to write: ' + why)
    now = now or datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    builds, served = read_registry(args.registry)
    top = max(builds) if builds else 0
    seen = read_state(args.state)
    found, failures = [], []
    served_v = line_version(served)

    def add(source, key, line, note, code):
        found.append((source, str(key), line, note, (source, str(key)) not in seen, code))

    if args.online_ini:
        try:
            b, code = read_ini(args.online_ini)
            if b not in builds:
                add('start.ini', b, code, 'not in the registry: the import is due', 4)
            else:
                add('start.ini', b, code, 'registered', 0)
        except Failed as e:
            failures.append(str(e))
    if args.esi:
        try:
            b = read_esi(fetch(ESI_URL))
            add('esi', b, '', 'newer than every registered build' if b > top else 'registered or older',
                3 if b > top else 0)
        except Failed as e:
            failures.append(str(e))
    saved = []
    if args.notes:
        try:
            for e in read_notes(fetch(NOTES_URL)):
                v = line_version(e['line'])
                new_line = v is not None and served_v is not None and v > served_v
                note = e['date'] + ' ' + e['link']
                if new_line:
                    add('notes', e['title'], e['line'], note + ' announces a line above the served one (%s)' % served, 5)
                else:
                    add('notes', e['title'], e['line'], note, 0)
                if (('notes', e['title']) not in seen) and args.save_outline:
                    p = save_outline(args.save_outline, e)
                    if p:
                        saved.append(p)
        except Failed as e:
            failures.append(str(e))
    # a notes entry we have not seen is news (3) even when it is no new line
    codes = []
    for source, key, line, note, is_new, code in found:
        if code:
            codes.append(code)
        elif source == 'notes' and is_new:
            codes.append(3)
    append_state(args.state, [(now, s, k, l, n) for s, k, l, n, new, c in found if new])
    rc = 0
    for want in (5, 4, 3):
        if want in codes:
            rc = want
            break
    if rc == 0 and failures:
        rc = 1
    return rc, found, failures, saved


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0], add_help=True)
    ap.add_argument('--online-ini')
    ap.add_argument('--esi', action='store_true')
    ap.add_argument('--notes', action='store_true')
    ap.add_argument('--save-outline')
    ap.add_argument('--state', default=os.path.join(REPO, 'out', 'tq-watch', 'state.tsv'))
    ap.add_argument('--registry', default=os.path.join(HERE, 'builds.tsv'))
    ap.add_argument('--json', action='store_true')
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    if a.save_outline and not a.notes:
        print('tq-watch: --save-outline needs --notes', file=sys.stderr)
        return 2
    if not (a.online_ini or a.esi or a.notes):
        print('tq-watch: nothing to read: give --online-ini, --esi or --notes', file=sys.stderr)
        return 2
    try:
        rc, found, failures, saved = watch(a)
    except Usage as e:
        print('tq-watch: %s' % e, file=sys.stderr)
        return 2
    except Failed as e:
        print('tq-watch: %s' % e, file=sys.stderr)
        return 1
    if a.json:
        print(json.dumps({'exit': rc, 'facts': [dict(zip(('source', 'build_or_title', 'line', 'note', 'new', 'code'), f))
                                                for f in found], 'failed': failures, 'outlines': saved}, indent=1))
    else:
        for source, key, line, note, is_new, code in found:
            if is_new:
                print('%s: %s%s: %s' % (source, key, (' (%s)' % line) if line else '', note))
        for p in saved:
            print('outline saved: %s' % p)
    for f in failures:
        print('tq-watch: SOURCE FAILED: %s' % f, file=sys.stderr)
    return rc


if __name__ == '__main__':
    sys.exit(main())
