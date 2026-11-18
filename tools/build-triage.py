#!/usr/bin/env python3
"""Sort a new client build into a tier (T0-T3) from the tools' own output.

  python3 -I tools/build-triage.py BUILD [--pinned P] [--delta FILE] [--surface DIR]
                                   [--recheck DIR] [--shapes DIR] [--bundle-diff FILE]
                                   [--locate-exit N] [--out DIR] [--registry FILE]

BUILD is the new build number; P defaults to the pinned row of tools/builds.tsv.
Inputs (only the delta is required; a missing one is reported as `skipped` and is never a
pass):
  --delta        findings/build-delta-<P>-<BUILD>.tsv (build-delta.py, format 2: it has the
                 `recompiled` module class)
  --surface      out/new-build/<BUILD>/surface    (surface-diff.tsv, from port-surface)
  --recheck      out/new-build/<BUILD>/recheck    (recheck.tsv, from port-recheck)
  --shapes       out/new-build/<BUILD>/shapes, else .../surface/shapes (shapes.tsv; port-surface G7 writes it there)
  --bundle-diff  out/new-build/<BUILD>/bundle-diff.txt (bundle-diff.py --uses)
  --locate-exit  the exit code of `eve_patch_author.py locate` (4: a rule refused; any other non-zero is skipped, not a pass)
Output: DIR/triage.md and DIR/triage.tsv (tier, rule, evidence, next), DIR defaulting to
out/new-build/<BUILD>. One row per deciding fact; the highest tier wins, and a higher tier's
sequence includes the lower tiers' items (docs/guides/tracking-tq.md, section 3).

  T3 native protocol change   blue.dll, _destiny.dll, exefile.exe or python27.dll
                              not identical or signature-only (code-differs, rebuilt,
                              data-differs, added, removed); the macho version module, the handshake secret or
                              padding changed; a wire module changed, added or gone; the gateway
                              transport strings changed; a patch rule refuses
  T2 new release line         start.ini codename, version or region differs
  T1 code change, same line   a module changed in meaning; a served surface row; a shapes
                              SERVED row; a recheck changed or gone row; a bundle REMOVED or
                              CLASS flag on an id we use; a module added or removed
  T0 data only, same line     none of the above

Exit: 20 for T0, 21 T1, 22 T2, 23 T3 (distinct from the repo's 3 and 10). 1: an input is
malformed. 2: usage.

[I] "Wire module" is a path under carbon/common/script/net, carbon/common/lib,
eve/common/script/net, or a name holding destiny, ballpark or michelle; falsifier: a build
where such a module changes and the wire does not (the tier is then too high, never too low).
[I] The handshake constants are found as a changed eveMachoNetVersion module or a recheck row
naming them; falsifier: a build that moves them in a module neither covers.
"""
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
QUIET_NATIVE = ('identical', 'signature-only')
NATIVES = ('blue.dll', '_destiny.dll', 'exefile.exe', 'python27.dll')
WIRE_DIRS = ('carbon/common/script/net/', 'carbon/common/lib/', 'eve/common/script/net/')
WIRE_WORDS = ('destiny', 'ballpark', 'michelle')
HANDSHAKE = ('eveMachoNetVersion', '__HANDSHAKE_SECRET_KEY', 'handShakePaddingLength')
LINE_KEYS = ('main.codename', 'main.version', 'main.region')
SHOW = 5
TARGET = {
    'T0': 'provisional in 2 h, served on its own bundle the same day; intake issues in 24 h',
    'T1': 'login-capable within 24 h, must-serve within 72 h; intake issues in 24 h',
    'T2': 'login within 24 h of the import, must-serve within 72 h; intake issues in 48 h',
    'T3': 'best effort; a status every 24 h; intake issues in 48 h',
}
NAMES = {'T0': 'data only, same line', 'T1': 'code change, same line',
         'T2': 'new release line', 'T3': 'native protocol change'}
#: eve_patch_author.EXIT_REFUSED (not 2, which is usage)
LOCATE_REFUSED = 4
DELTA_COLS = ('layer', 'item', 'class')


class Malformed(Exception):
    pass


def read_rows(path, cols, what):
    """(header, rows as dicts) of a TSV; `#` lines are comments. Raises Malformed."""
    try:
        with open(path, encoding='utf8') as fh:
            lines = [ln.rstrip('\n') for ln in fh]
    except OSError as e:
        raise Malformed('%s: %s' % (what, e.strerror or e))
    body = [ln for ln in lines if ln and not ln.startswith('#')]
    if not body:
        raise Malformed('%s: empty' % what)
    head = body[0].split('\t')
    for c in cols:
        if c not in head:
            raise Malformed('%s: no %s column in the header' % (what, c))
    rows = []
    for ln in body[1:]:
        c = ln.split('\t')
        c += [''] * (len(head) - len(c))
        rows.append(dict(zip(head, c)))
    return lines, rows


def shown(items):
    items = sorted(items)
    s = ', '.join(items[:SHOW])
    return s + (' (+%d more)' % (len(items) - SHOW) if len(items) > SHOW else '')


def is_wire(item):
    low = item.lower()
    return item.startswith(WIRE_DIRS) or any(w in low for w in WIRE_WORDS)


def registry_pinned(path):
    try:
        with open(path, encoding='utf8') as fh:
            for ln in fh:
                c = ln.rstrip('\n').split('\t')
                if not ln.startswith('#') and len(c) > 2 and c[2] == 'pinned':
                    return c[0]
    except OSError:
        pass
    return None


class Triage:
    def __init__(self):
        self.rows = []      # (tier, rule, evidence, next)
        self.skipped = []   # (input, why)
        self.counts = {}
        self.line_change = ''

    def fact(self, tier, rule, evidence, nxt):
        self.rows.append((tier, rule, evidence, nxt))

    @property
    def tier(self):
        return max((r[0] for r in self.rows), default='T0')


def from_delta(t, path):
    lines, rows = read_rows(path, DELTA_COLS, 'delta')
    if not any(l.startswith('# build-delta format 2') for l in lines):
        raise Malformed('delta: not format 2 (no `recompiled` class); regenerate it with the current tools/build-delta.py')
    if not any(r['layer'] == 'verdict' for r in rows):
        raise Malformed('delta: no verdict row (truncated?)')
    by = lambda layer: [r for r in rows if r['layer'] == layer]
    meaning = [r['item'] for r in by('module') if r['class'] == 'changed']
    gone = [r['item'] for r in by('module') if r['class'] == 'removed']
    recompiled = sum(1 for r in by('module') if r['class'] == 'recompiled')
    t.counts['modules in meaning'] = len(meaning)
    t.counts['modules recompiled'] = recompiled
    # A native is quiet only when identical or signature-only. rebuilt, data-differs, code-differs,
    # added or removed (a class we have not met included) all put the wire in doubt.
    nat = ['%s (%s)' % (r['item'], r['class']) for r in by('file')
           if r['class'] not in QUIET_NATIVE and r['item'].rsplit('/', 1)[-1] in NATIVES]
    if nat:
        t.fact('T3', 'native-differs', 'native not identical or signature-only: ' + shown(nat), 'T3.2')
    hs = [m for m in meaning if m.rsplit('/', 1)[-1] == 'eveMachoNetVersion']
    if hs:
        t.fact('T3', 'handshake-module', 'changed in meaning: ' + shown(hs), 'T3.2')
    added = [r['item'] for r in by('module') if r['class'] == 'added']
    wire = [m for m in meaning + gone + added if is_wire(m)]
    if wire:
        t.fact('T3', 'wire-module', '%d module(s) changed or gone: %s' % (len(wire), shown(wire)), 'T3.2')
    line = [(k, r['old'], r['new']) for r in by('ini') for k in [r['item']]
            if k in LINE_KEYS and r['class'] == 'differs']
    if line:
        t.line_change = ', '.join('%s %s -> %s' % x for x in line)
        t.fact('T2', 'release-line', 'start.ini ' + t.line_change, 'T2.1')
    t.counts['modules added'] = len(added)
    t.counts['modules removed'] = len(gone)
    if meaning:
        t.fact('T1', 'modules-in-meaning', '%d module(s) changed in meaning (%d recompiled only): %s'
               % (len(meaning), recompiled, shown(meaning)), 'T1.1')
    if added or gone:
        t.fact('T1', 'modules-added-removed', '%d added, %d removed: %s' % (
            len(added), len(gone), shown(['+' + m for m in added] + ['-' + m for m in gone])), 'T1.1')


def from_surface(t, d):
    p = os.path.join(d, 'surface-diff.tsv')
    if not os.path.isfile(p):
        t.skipped.append(('surface', p + ' not found'))
        return
    _, rows = read_rows(p, ('section', 'change', 'key', 'impact'), 'surface')
    served = [r for r in rows if r['impact'] == 'yes']
    t.counts['served-and-changed surface rows'] = len(served)
    gw = [r for r in rows if r['section'] == 'G4' and 'eve_public.gateway.' in r['key']
          and r['change'] in ('changed', 'removed', 'msg-changed', 'msg-removed')]
    if gw:
        t.fact('T3', 'gateway-strings', '%d gateway row(s): %s' % (len(gw), shown(r['key'] for r in gw)), 'T3.2')
    if served:
        t.fact('T1', 'surface-served', '%d row(s) touch what we serve: %s' % (
            len(served), shown('%s %s %s' % (r['section'], r['change'], r['key']) for r in served)), 'T1.1')


def from_recheck(t, d):
    p = os.path.join(d, 'recheck.tsv')
    if not os.path.isfile(p):
        t.skipped.append(('recheck', p + ' not found'))
        return
    _, rows = read_rows(p, ('section', 'status', 'where', 'what'), 'recheck')
    hit = [r for r in rows if r['status'] in ('changed', 'gone')]
    t.counts['recheck changed or gone rows'] = len(hit)
    hs = [r for r in hit if any(h in r['what'] or h in r['where'] for h in HANDSHAKE)]
    if hs:
        t.fact('T3', 'handshake-constant', shown('%s %s' % (r['where'], r['what']) for r in hs), 'T3.2')
    if hit:
        t.fact('T1', 'recheck', '%d row(s): %s' % (len(hit), shown('%s %s %s' % (r['status'], r['where'], r['what']) for r in hit)), 'T1.4')


def from_shapes(t, d):
    p = os.path.join(d, 'shapes.tsv')
    if not os.path.isfile(p):
        t.skipped.append(('shapes', p + ' not found'))
        return
    _, rows = read_rows(p, ('pass', 'status', 'key'), 'shapes')
    served = [r for r in rows if r['status'].startswith('SERVED')]
    t.counts['shapes SERVED rows'] = len(served)
    if served:
        t.fact('T1', 'shapes-served', '%d key(s) we emit are gone from the client: %s' % (
            len(served), shown(r['key'] for r in served)), 'T1.2')


def from_bundle(t, p):
    if not os.path.isfile(p):
        t.skipped.append(('bundle-diff', p + ' not found'))
        return
    try:
        with open(p, encoding='utf8') as fh:
            text = fh.read().splitlines()
    except OSError as e:
        raise Malformed('bundle-diff: %s' % e)
    if not any(l.startswith('FLAGS') for l in text):
        raise Malformed('bundle-diff: no FLAGS line')
    flag_lines = [l for l in text if re.match(r'^  (REMOVED|CLASS|HEADER)\b', l)]
    t.counts['bundle flags'] = len(flag_lines)
    uses, on = set(), False
    for l in text:
        if l.startswith('USES'):
            on = True
        elif on and re.match(r'^\s+\d+:', l):
            uses.add(l.split(':')[0].strip())
    t.counts['bundle ids in use'] = len(uses)
    # a REMOVED or CLASS flag only counts for an id we use (plan T0.6): the ids a flag names
    # that the --uses section also lists
    hit = sorted({i for l in flag_lines if not l.startswith('  HEADER') for i in re.findall(r'\b\d{4,}\b', l)} & uses)
    if hit:
        t.fact('T1', 'bundle-flag-used', 'REMOVED/CLASS flag on %d id(s) we use: %s' % (len(hit), shown(hit)), 'T0.6')


def from_locate(t, code):
    if code is None:
        t.skipped.append(('locate', 'no --locate-exit given'))
    elif code == LOCATE_REFUSED:
        t.fact('T3', 'patch-rule-refused', 'eve_patch_author.py locate refused (exit %d)' % code, 'T3.3')
    elif code != 0:
        t.skipped.append(('locate', 'exit %d is not a refusal (usage or a failure): not a pass' % code))


def render(t, build, pinned):
    tier = t.tier
    md = ['# Build %s against %s: %s, %s' % (build, pinned, tier, NAMES[tier]), '']
    md.append('- Line change: %s' % (t.line_change or 'none'))
    md.append('- Target: %s' % TARGET[tier])
    for k, v in t.counts.items():
        md.append('- %s: %d' % (k, v))
    for what, why in t.skipped:
        md.append('- SKIPPED %s: %s (not a pass)' % (what, why))
    md += ['', '## Deciding facts', '']
    for tr, rule, ev, nxt in sorted(t.rows, key=lambda r: r[0], reverse=True):
        md.append('- **%s** `%s`: %s (next: %s)' % (tr, rule, ev, nxt))
    if not t.rows:
        md.append('- none: only res, schema, static data, recompiled modules and signature-only natives changed')
    md.append('')
    return '\n'.join(md)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('build')
    ap.add_argument('--pinned')
    ap.add_argument('--delta')
    ap.add_argument('--surface')
    ap.add_argument('--recheck')
    ap.add_argument('--shapes')
    ap.add_argument('--bundle-diff')
    ap.add_argument('--locate-exit', type=int)
    ap.add_argument('--out')
    ap.add_argument('--registry', default=os.path.join(HERE, 'builds.tsv'))
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    if not a.build.isdigit():
        print('build-triage: BUILD is a number', file=sys.stderr)
        return 2
    pinned = a.pinned or registry_pinned(a.registry)
    if not (pinned or a.delta):
        print('build-triage: no pinned build in %s; give --pinned or --delta' % a.registry, file=sys.stderr)
        return 2
    out = a.out or os.path.join('out', 'new-build', a.build)
    delta = a.delta or os.path.join('findings', 'build-delta-%s-%s.tsv' % (pinned, a.build))
    t = Triage()
    try:
        from_delta(t, delta)
        from_surface(t, a.surface or os.path.join(out, 'surface'))
        from_recheck(t, a.recheck or os.path.join(out, 'recheck'))
        shapes = a.shapes or os.path.join(out, 'shapes')
        if not a.shapes and not os.path.isdir(shapes):
            shapes = os.path.join(out, 'surface', 'shapes')  # where port-surface G7 writes it
        from_shapes(t, shapes)
        from_bundle(t, a.bundle_diff or os.path.join(out, 'bundle-diff.txt'))
        from_locate(t, a.locate_exit)
    except Malformed as e:
        print('build-triage: %s' % e, file=sys.stderr)
        return 1
    os.makedirs(out, exist_ok=True)
    rows = sorted(t.rows, key=lambda r: r[0], reverse=True) or [('T0', 'data-only', 'no module, surface, shapes or recheck row moved', 'T0.1')]
    with open(os.path.join(out, 'triage.tsv'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write('tier\trule\tevidence\tnext\n')
        for r in rows:
            fh.write('\t'.join(x.replace('\t', ' ') for x in r) + '\n')
    with open(os.path.join(out, 'triage.md'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write(render(t, a.build, pinned or '?'))
    tier = t.tier
    print('build-triage: %s (%s); %s' % (tier, NAMES[tier], os.path.join(out, 'triage.md')))
    for what, why in t.skipped:
        print('build-triage: skipped %s: %s' % (what, why))
    return 20 + int(tier[1])


if __name__ == '__main__':
    sys.exit(main())
