#!/usr/bin/env python3
"""Which keys do we put in our answers that the new client no longer reads?

  tools/analysis.sh python3 -I /work/tools/answer-shapes.py OLD_PYC NEW_PYC
                    [--server server/crates] [--served served.json] [--out DIR]

port-surface G1 sees calls, arity and the first consumer, but not the keys and
attributes the client reads from our answers. In 3707965 the asset-safety row keys were
renamed under an unchanged call and G1 said nothing. This tool reads the keys.

Pass A, the server key inventory: every string literal the server uses as an answer key,
from the Rust sources (tests and `#[cfg(test)]` items skipped): the tuple forms `("key", value)`,
`("key".to_string(), value)` (also to_owned, into) and `(KEY, value)` with a `const KEY: &str`,
and the column arrays (`&["col", ..]`, `["col"]`, `vec!["col"]`, `[&str; N]`). Identifier-like,
four characters or more.

Pass B, the client key census: per tree, how many modules hold the key as a whole
marshalled string (the 4-byte length followed by the bytes, which is what a subscript,
`.get`, `getattr` or `LOAD_ATTR` name is). A key read in OLD and nowhere in NEW is
`SERVED gone`; the new string constants in the code objects that held it are listed as
`candidate rename`.

Pass C (consumer shapes at served call sites) is not here; it lives in scan_calls.

Known limits (also written into shapes.md):
- keys built at run time (`'%sID' % x`) are not seen;
- answers passed through generic scroll-node dicts more than one hop away: a rename there
  is found by Pass B, a newly required key is not;
- a key that is also a common word the client reads for another reason hides a rename.

Output in DIR (default: <dir of NEW_PYC>/shapes): shapes.md, shapes.tsv
(pass, status, key, server_site, old_readers, new_readers, detail), server-keys.tsv.
Exit 0: nothing we emit moved. 3: a row needs a reader. 2: usage or a missing tree.
1: a tool failed. These are port-surface's codes. Reads bytes and the .pyc format only;
it runs no client code.
"""
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import importlib.util
import json
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

MIN_KEY = 4
IDENT = r'[A-Za-z_][A-Za-z0-9_]*'
#: a string literal, optionally converted: "k", "k".to_owned(), "k".into(), "k".to_string(), String::from("k")
LIT = r'(?:String::from\(\s*"(%s)"\s*\)|"(%s)"(?:\.(?:to_owned|into|to_string)\(\))?)' % (IDENT, IDENT)
#: a key written as a constant: NAME (an upper-case identifier)
CONSTREF = r'([A-Z][A-Z0-9_]*)'
ELEM = r'(?:String::from\(\s*"%s"\s*\)|"%s"(?:\.(?:to_owned|into|to_string)\(\))?|[A-Z][A-Z0-9_]*)' % (IDENT, IDENT)
TUPLE_KEY = re.compile(r'\(\s*(?:%s|%s)\s*,' % (LIT, CONSTREF))
#: `&["a", "b"]`, `["a"]`, `vec!["a"]`, `: [&str; N] = ["a", "b"]`; not an index expression `x["a"]`
ARRAY = re.compile(r'(?<![\w\)\]])\[\s*(%s(?:\s*,\s*%s)*)\s*,?\s*\]' % (ELEM, ELEM))
ELEM_RE = re.compile(r'String::from\(\s*"(%s)"\s*\)|"(%s)"|\b([A-Z][A-Z0-9_]*)\b' % (IDENT, IDENT))
CONST_DEF = re.compile(r'\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:\s*&(?:\'static\s+)?str\s*=\s*"(%s)"\s*;' % IDENT)
CFG_TEST = re.compile(r'#\[\s*cfg\s*\(\s*test\s*\)\s*\]')
MARSHALLED = re.compile(rb'(?=([\x04-\x7f])\x00\x00\x00([A-Za-z_][A-Za-z0-9_]*))')
LIST_MAX = 60


def skip_rust(path):
    parts = path.replace(os.sep, '/').split('/')
    return parts[-1] == 'tests.rs' or 'tests' in parts[:-1]


def blank(text, a, b):
    """text with [a, b) replaced by spaces, newlines kept so line numbers stay true."""
    return text[:a] + re.sub(r'[^\n]', ' ', text[a:b]) + text[b:]


def strip_cfg_test(text):
    """Remove every `#[cfg(test)]` item: through its matching `}` (braces inside strings, chars
    and comments do not count), or through its `;` for an item with no body."""
    while True:
        m = CFG_TEST.search(text)
        if not m:
            return text
        i, n = m.end(), len(text)
        depth, end = 0, n
        while i < n:
            c = text[i]
            if text.startswith('//', i):
                j = text.find('\n', i)
                i = n if j < 0 else j
            elif text.startswith('/*', i):
                j = text.find('*/', i + 2)
                i = n if j < 0 else j + 2
            elif c == '"':
                i += 1
                while i < n and text[i] != '"':
                    i += 2 if text[i] == '\\' else 1
                i += 1
            elif c == "'" and re.match(r"'(?:\\.[^']*|[^\\'])'", text[i:i + 12]):
                i += re.match(r"'(?:\\.[^']*|[^\\'])'", text[i:i + 12]).end()
            elif c == '{':
                depth += 1
                i += 1
            elif c == '}':
                depth -= 1
                i += 1
                if depth == 0:
                    end = i
                    break
            elif c == ';' and depth == 0:
                end = i + 1
                break
            else:
                i += 1
        text = blank(text, m.start(), end)


def server_keys(src):
    """key -> [first 'file:line', sites]; files in sorted order so the first site is stable.
    `src` is the crates dir (its `eve-server/src` is read: that is what answers the client) or
    a source dir itself. Keys: `("k", v)` and `("k".to_string(), v)` tuples, a constant naming a
    key (`(KEY, v)` with `const KEY: &str = "k";` anywhere in the crate), and the array forms
    `&["a", "b"]`, `["a"]`, `vec!["a"]` and `[&str; N]` initialisers. `#[cfg(test)]` items
    are skipped."""
    keys = {}
    inner = os.path.join(src, 'eve-server', 'src')
    if os.path.isdir(inner):
        src = inner
    texts = []
    consts = {}
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(root, f)
            if not f.endswith('.rs') or skip_rust(os.path.relpath(p, src)):
                continue
            with open(p, encoding='utf8', errors='replace') as fh:
                text = strip_cfg_test(fh.read())
            texts.append((os.path.relpath(p, src).replace(os.sep, '/'), text))
            for m in CONST_DEF.finditer(text):
                consts.setdefault(m.group(1), set()).add(m.group(2))
    # a constant defined with two different values is ambiguous: not resolved
    consts = {k: next(iter(v)) for k, v in consts.items() if len(v) == 1}
    for rel, text in texts:
        hits = []
        for m in TUPLE_KEY.finditer(text):
            k = m.group(1) or m.group(2) or consts.get(m.group(3))
            if k:
                hits.append((m.start(), k))
        for m in ARRAY.finditer(text):
            for e in ELEM_RE.finditer(m.group(1)):
                k = e.group(1) or e.group(2) or consts.get(e.group(3))
                if k:
                    hits.append((m.start(), k))
        for pos, k in sorted(hits):
            if len(k) < MIN_KEY:
                continue
            site = '%s:%d' % (rel, text.count('\n', 0, pos) + 1)
            if k in keys:
                keys[k][1] += 1
            else:
                keys[k] = [site, 1]
    return keys


def census(tree, keys):
    """key -> number of modules that hold it as a whole marshalled string."""
    hit = dict.fromkeys(keys, 0)
    for root, dirs, files in os.walk(tree):
        dirs.sort()
        for f in sorted(files):
            if not f.endswith('.pyc'):
                continue
            with open(os.path.join(root, f), 'rb') as fh:
                b = fh.read()
            seen = set()
            for m in MARSHALLED.finditer(b):
                n = m.group(1)[0]
                word = m.group(2)
                if len(word) >= n:
                    w = word[:n].decode('ascii')
                    if w in hit:
                        seen.add(w)
            for w in seen:
                hit[w] += 1
    return hit


def _recheck():
    spec = importlib.util.spec_from_file_location('port_recheck', os.path.join(_HERE, 'port-recheck.py'))
    mod = importlib.util.module_from_spec(spec)
    sys.modules.setdefault('port_recheck', mod)
    spec.loader.exec_module(mod)
    return mod


def _module_files(tree, key):
    """Relative names of the modules in tree holding key as a whole marshalled string."""
    want = len(key).to_bytes(4, 'little') + key.encode()
    out = []
    for root, dirs, files in os.walk(tree):
        dirs.sort()
        for f in sorted(files):
            if f.endswith('.pyc'):
                p = os.path.join(root, f)
                with open(p, 'rb') as fh:
                    if want in fh.read():
                        out.append(os.path.relpath(p, tree)[:-4].replace(os.sep, '/'))
    return out


def _strings(code):
    out = set()
    for k in code.consts or ():
        stack = [k]
        while stack:
            x = stack.pop()
            if isinstance(x, (bytes, bytearray)):
                out.add(bytes(x).decode('utf8', 'replace'))
            elif isinstance(x, (tuple, list)):
                stack.extend(x)
    for n in code.names or ():
        out.add(n.decode('utf8', 'replace') if isinstance(n, bytes) else n)
    return out


def candidates(old, new, key, rc):
    """New string constants and names in the code objects that held `key` in OLD, by the
    same dotted path in NEW (port-recheck's keying). A function that moved shows nothing."""
    cand = []
    for rel in _module_files(old, key):
        a = rc.Tree(old).module(rel)
        b = rc.Tree(new).module(rel)
        if a is None or b is None:
            continue
        for k in a.uses(key):
            ea, eb = a.entries.get(k), b.entries.get(k)
            if ea is None or eb is None:
                continue
            for s in sorted(_strings(eb.code) - _strings(ea.code)):
                if s not in cand and s != key and len(s) >= MIN_KEY and re.match(r'^[A-Za-z_]\w*$', s):
                    cand.append(s)
    return cand


def served_count(path):
    try:
        with open(path, encoding='utf8') as fh:
            data = json.load(fh)
        return len(data)
    except (OSError, ValueError):
        return None


def tsv(cols):
    return '\t'.join(str(c).replace('\t', ' ').replace('\n', ' ') for c in cols) + '\n'


def run(old, new, src, outdir, served=None):
    keys = server_keys(src)
    o, n = census(old, keys), census(new, keys)
    rc = None
    rows = []
    for k in sorted(keys):
        if o[k] > 0 and n[k] == 0:
            if rc is None:
                rc = _recheck()
            cand = candidates(old, new, k, rc)
            detail = 'read in %d module(s), read nowhere in NEW' % o[k]
            if cand:
                detail += '; candidate rename: ' + ', '.join(cand[:8])
            rows.append(('B', 'SERVED gone', k, keys[k][0], o[k], n[k], detail))
    os.makedirs(outdir, exist_ok=True)
    with open(os.path.join(outdir, 'server-keys.tsv'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write(tsv(('key', 'file:line', 'sites')))
        for k in sorted(keys):
            fh.write(tsv((k, keys[k][0], keys[k][1])))
    with open(os.path.join(outdir, 'shapes.tsv'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write(tsv(('pass', 'status', 'key', 'server_site', 'old_readers', 'new_readers', 'detail')))
        for r in rows:
            fh.write(tsv(r))
    md = ['# Answer shapes: %s -> %s' % (old, new), '']
    md.append('Verdict: %s.' % (('%d server key(s) no longer read by the client (exit 3)' % len(rows)) if rows
                                else 'nothing we emit moved (exit 0)'))
    md.append('')
    md.append('- Pass A: %d server answer keys (server-keys.tsv)' % len(keys))
    md.append('- Pass B: client modules reading them, OLD vs NEW; a key read in OLD and nowhere in NEW is listed')
    if served:
        sc = served_count(served)
        md.append('- served set: %s' % ('%d entries in %s' % (sc, served) if sc is not None else '%s unreadable' % served))
    md.append('- Pass C (consumer shapes at served call sites) is not run by this tool')
    md.append('')
    md.append('## Needs a reader')
    md.append('')
    if not rows:
        md.append('- none')
    for r in rows[:LIST_MAX]:
        md.append('- **SERVED gone** `%s` at `%s`: %s' % (r[2], r[3], r[6]))
    if len(rows) > LIST_MAX:
        md.append('- ... and %d more (shapes.tsv has all of them)' % (len(rows) - LIST_MAX))
    md += ['', '## Known limits', '',
           "- keys built at run time (`'%sID' % x`) are not seen;",
           '- answers passed through generic scroll-node dicts more than one hop away: a rename there is found by '
           'Pass B, a key that is newly required is not;',
           '- a key the client also reads for another reason hides its own rename.', '']
    with open(os.path.join(outdir, 'shapes.md'), 'w', encoding='utf8', newline='\n') as fh:
        fh.write('\n'.join(md))
    return rows


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('old')
    ap.add_argument('new')
    ap.add_argument('--server', default='server/crates', help='the Rust crates to read answer keys from')
    ap.add_argument('--served', help='served.json from port-surface (noted in shapes.md)')
    ap.add_argument('--out', help='default: <dir of NEW>/shapes')
    try:
        a = ap.parse_args(argv)
    except SystemExit as e:
        return 2 if e.code else 0
    for d in (a.old, a.new, a.server):
        if not os.path.isdir(d):
            print('answer-shapes: no such directory: %s' % d, file=sys.stderr)
            return 2
    out = a.out or os.path.join(os.path.dirname(os.path.abspath(a.new)), 'shapes')
    try:
        rows = run(a.old, a.new, a.server, out, a.served)
    except Exception as e:  # a tool failure is never a pass
        print('answer-shapes: failed: %s: %s' % (type(e).__name__, e), file=sys.stderr)
        return 1
    print('answer-shapes: %d server key(s) gone from the client; %s/shapes.md' % (len(rows), out), file=sys.stderr)
    return 3 if rows else 0


if __name__ == '__main__':
    sys.exit(main())
