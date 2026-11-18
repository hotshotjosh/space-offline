#!/usr/bin/env python3
"""Write issue DRAFTS for a new build's intake rows. It never posts anything.

  python3 -I tools/intake-issues.py out/new-build/<N>/content/intake.tsv
        [--surface out/new-build/<N>/triage.tsv] [--build N] [--line V24.02]
        [--parent ISSUE] --out out/new-build/<N>/issues/

intake.tsv is tools/content-report.py's table (area, kind, what, count, sample_ids,
server_readers, in_notes, verdict, suggested_title). One draft per (area, kind) group, from
the standard template of docs/guides/tracking-tq.md section 5.4, as NN-<area>-<slug>.md in
--out, plus index.tsv (file, title, labels). Filing is the maintainer's; this tool opens no
connection and writes only under --out.

Rows with verdict `data-driven` need no work and get no draft. The build comes from the path
(.../new-build/<N>/...) or --build; the tier from the first row of --surface (triage.tsv);
the line from --line or the triage release-line row. A draft never carries more than 5 ids
and never a path outside the repo (an absolute path, `..` or `~` is replaced).

Labels: `tq-intake`, `area/<slug>`. `status/ready` is NOT added: the spec part of a draft is
filled in by a human first.

Exit: 0 drafts written, 1 nothing to file, 2 usage.

[A] Verdict mapping needs-work -> must, review -> should, notes-only -> should (a group takes
its highest); falsifier: content-report.py settling on other verdict names.
"""
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import re
import sys

MAX_IDS = 5
COLS = ('area', 'kind', 'what', 'count', 'sample_ids', 'server_readers', 'in_notes', 'verdict', 'suggested_title')
RANK = {'must': 2, 'should': 1}
VERDICT = {'needs-work': 'must', 'review': 'should', 'notes-only': 'should'}
PATHISH = re.compile(r'(?<![\w.\-/])(?:~|\.\.?|/)[^\s,;]*')
URL = re.compile(r'\b[A-Za-z][A-Za-z0-9+.-]*://[^\s,;)]*')
WINPATH = re.compile(r'[A-Za-z]:[\\/][^\s,;]*|\\\\[^\s,;]+')
IPV4 = re.compile(r'\b\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?\b')
HOST = re.compile(r'\b(?:[A-Za-z0-9-]+\.)+(?:com|net|org|io|dev|info|gg|eu|co|uk|de|ru|cn|local|lan|internal|invalid|test|example)\b(?::\d+)?', re.I)


def slug(s):
    return re.sub(r'[^a-z0-9]+', '-', s.lower()).strip('-') or 'x'


def clean(text):
    """Nothing that names a place outside the repo: a URL, a Windows or UNC path, an absolute,
    home-relative or parent-relative path, a host name or an IPv4 address goes."""
    text = URL.sub('<host removed>', text)
    text = WINPATH.sub('<path removed>', text)
    text = IPV4.sub('<host removed>', text)
    text = HOST.sub('<host removed>', text)
    return PATHISH.sub(lambda m: '<path removed>' if ('/' in m.group(0) or m.group(0) in ('~', '..')) else m.group(0), text)


def listing(cell, limit=MAX_IDS):
    items = [x.strip() for x in re.split(r'[,;]', cell) if x.strip() and x.strip() != '-']
    items = [clean(x) for x in items]
    return items[:limit], max(0, len(items) - limit)


def read_tsv(path):
    try:
        with open(path, encoding='utf8') as fh:
            lines = [l.rstrip('\n') for l in fh if l.strip() and not l.startswith('#')]
    except OSError as e:
        raise SystemExit('intake-issues: %s: %s' % (path, e.strerror or e))
    if not lines:
        return [], []
    head = lines[0].split('\t')
    return head, [dict(zip(head, (l.split('\t') + [''] * len(head)))) for l in lines[1:]]


def draft(build, line, tier, area, kind, rows, parent):
    build, line, tier, area, kind = (clean(x) for x in (build, line, tier, area, kind))
    rows = [{k: clean(v) for k, v in r.items()} for r in rows]
    verdict = max((VERDICT[r['verdict']] for r in rows), key=lambda v: RANK[v])
    titles = []
    for r in rows:
        t = clean(r['suggested_title'].strip())
        if t and t not in titles:
            titles.append(t)
    prefix = '%s: ' % line if line else 'Build %s: ' % build
    if len(titles) == 1:
        what = titles[0]
    else:
        what = 'the server handles the new %s %s (%d items)' % (area, kind, len(rows))
    title = what if what.startswith(prefix) else prefix + what
    n = sum(int(re.sub(r'\D', '', r['count']) or 0) for r in rows)
    ids, more = [], 0
    readers = []
    for r in rows:
        i, m = listing(r['sample_ids'])
        ids += i
        more += m
        readers += listing(r['server_readers'], 1000)[0]
    extra = max(0, len(ids) - MAX_IDS) + more
    ids = ids[:MAX_IDS]
    readers = sorted(set(readers))[:MAX_IDS]
    notes = sorted({r['in_notes'] for r in rows} - {'-', ''})
    body = [
        'Title: %s' % title,
        '',
        '**Build:** %s (line %s), tier %s' % (build, line or '?', tier or '?'),
        '**Area:** %s            **Verdict:** %s' % (area, verdict),
        '**What changed in the client:** %d item(s) of kind `%s` in `%s`: %s. %d row(s) counted%s; sample ids: %s%s.%s' % (
            len(rows), kind, area, '; '.join(clean(r['what']) for r in rows[:MAX_IDS]), n,
            '' if not notes else ' (in the patch notes: %s)' % ', '.join(notes),
            ', '.join(ids) or 'none', ' (+%d more)' % extra if extra else '',
            ' Bulk ids stay in out/.'),
        '**What the server does today:** %s' % (
            'read by %s (fill in file:line and the effect)' % ', '.join(readers) if readers
            else '<fill in: the reader and the effect, with file:line>'),
        '**Spec:** <files to change, what changes> (or: "see the lane spec section X" when one exists)',
        '**Proof:** <unit test names (invented ids), fci scenario and step>',
        '**Status row:** <docs/status/progress.tsv system | feature> -> built-unproved',
        '**Evidence:** [V] tools/content-report.py intake row (%s, %s); regenerate with '
        'tools/content-report.py or tools/port-surface.sh against build %s' % (area, kind, build),
        '**[A] values:** <each with its falsifier, or none>',
        '',
        'Part of #%s' % (parent or '<patch-day issue>'),
        '',
    ]
    return clean(title), clean('\n'.join(body)), ['tq-intake', 'area/%s' % slug(area)]


def triage_facts(path):
    """(tier, line) from triage.tsv: the first row's tier, the release-line row's new codename."""
    tier, line = '', ''
    head, rows = read_tsv(path)
    if rows:
        tier = rows[0].get('tier', '')
    for r in rows:
        if r.get('rule') == 'release-line':
            m = re.search(r'main\.codename \S+ -> (\S+)', r.get('evidence', ''))
            if m:
                line = m.group(1)
    return tier, line


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('intake')
    ap.add_argument('--surface', help='triage.tsv, for the tier and the new line')
    ap.add_argument('--build')
    ap.add_argument('--line')
    ap.add_argument('--parent', help='the patch-day issue number')
    ap.add_argument('--out', required=True)
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    if not os.path.isfile(a.intake):
        print('intake-issues: no such file: %s' % a.intake, file=sys.stderr)
        return 2
    if a.parent and not a.parent.isdigit():
        print('intake-issues: --parent is an issue number', file=sys.stderr)
        return 2
    head, rows = read_tsv(a.intake)
    missing = [c for c in COLS if c not in head]
    if missing:
        print('intake-issues: intake.tsv has no %s column' % ', '.join(missing), file=sys.stderr)
        return 2
    build = a.build
    if not build:
        m = re.search(r'new-build/(\d+)/', os.path.abspath(a.intake).replace(os.sep, '/'))
        build = m.group(1) if m else '?'
    tier, line = triage_facts(a.surface) if a.surface and os.path.isfile(a.surface) else ('', '')
    line = a.line or line
    groups = {}
    for r in rows:
        if r['verdict'] in VERDICT:
            groups.setdefault((clean(r['area']), clean(r['kind'])), []).append(r)
    if not groups:
        print('intake-issues: nothing to file')
        return 1
    os.makedirs(a.out, exist_ok=True)
    index = ['file\ttitle\tlabels']
    for n, key in enumerate(sorted(groups), 1):
        title, body, labels = draft(build, line, tier, key[0], key[1], groups[key], a.parent)
        name = '%02d-%s-%s.md' % (n, slug(key[0]), slug(key[1]))
        with open(os.path.join(a.out, name), 'w', encoding='utf8', newline='\n') as fh:
            fh.write(body)
        index.append('%s\t%s\t%s' % (name, title.replace('\t', ' '), ','.join(labels)))
    with open(os.path.join(a.out, 'index.tsv'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write('\n'.join(index) + '\n')
    print('intake-issues: %d draft(s) in %s (nothing posted)' % (len(groups), a.out))
    return 0


if __name__ == '__main__':
    sys.exit(main())
