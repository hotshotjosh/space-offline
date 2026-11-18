#!/usr/bin/env python3
"""List `[V]` marks that carry no evidence, in the areas a change touched (item V2).

Stdlib only; read-only. A `[V]` claims "verified against the client, with evidence"
(AGENTS.md). At a re-pin the cited ones are re-checked by port-recheck.py; a `[V]`
with nothing to check is the gap this tool lists, for a reader:

    python3 -I tools/v-audit.py [--base REF] [--added-only] [--paths p1,p2] [--all]
                                [--files-from FILE] [--tsv OUT] [--repo DIR]

Which files: with `--base REF`, the files that differ from REF (`git diff --name-only`,
plus untracked); `--paths` names files or directories; `--files-from` reads one path per
line (for a container without git); `--all` scans the repo's text files. With
`--added-only`, only a mark on a line the change added counts (needs `--base`).

A mark is `[V]`, `[V ...]` or `[V:...]` (`[V lane C §C2b]`, `[V prototype]`). It is **cited**
when the mark itself, its line, up to 6 earlier lines of the same paragraph, or up to 3 later
ones hold evidence:
- a `file:line` (`pyfile.py:123`, `safety.rs:160-165`);
- a named `.pyc` or `.pyj` file (`widget.pyc`);
- a path under `findings/`, `out/` or `scratch/`, or a file path under `docs/` or `tools/`
  (`docs/guides/contributing.md`; the bare directory is not evidence);
- `recheck.md`, `surface.md` or `shapes.tsv`;
- a section (`§3`, `section 4`).
Not marks: `[V]` inside backticks or quotes (the convention being described), and `[V,` or
`[V;` (code, such as a slice of a vector named V).

Output: TSV `path line mark text` of the uncited marks, then a summary line. Exit codes: 0
no uncited mark; 3 at least one (a reader needs to look); 2 usage or a git error.

How a gate or `tools/new-build.sh --report` can call it: run it on the host (it is stdlib only
and needs git) after the re-pin with `--base <the pre-cut-over commit>` and `--added-only`, treat
exit 3 as "reader needed" and not as a failure, and list the TSV in the patch-day issue. In a
container without git, pass the file list with `--files-from`.
"""
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import re
import subprocess
import sys

TEXT = ('.rs', '.py', '.md', '.sh', '.tsv', '.txt', '.toml', '.json')
SKIP_DIRS = {'.git', 'target', 'out', 'node_modules', '__pycache__', 'private'}
MARK = re.compile(r'(?<![`\w"\'])\[V(?=[\]\s:])([^\]\n]*)\]?')
EVIDENCE = [
    re.compile(r'[\w./-]+\.(?:py|pyc|pyj|rs|tsv|md|txt|json|sh|proto|ini|toml):\d+'),
    re.compile(r'\w\.py[cj]\b'),
    re.compile(r'(?:^|[\s`(\'"])(?:findings|out|scratch)/[\w./-]+'),
    re.compile(r'(?:^|[\s`(\'"])(?:docs|tools)/[\w./-]*\w\.\w+'),
    re.compile(r'recheck\.md|surface\.md|shapes\.tsv'),
    re.compile(r'§\s*\w|\bsection\s+\d', re.I),
]
WINDOW = 3
BEFORE = 6


def has_evidence(text):
    return any(p.search(text) for p in EVIDENCE)


def marks_of(lines, only=None):
    """(line number, mark text, line text) of each uncited mark; `only` limits to those lines."""
    out = []
    for i, line in enumerate(lines):
        for m in MARK.finditer(line):
            if only is not None and (i + 1) not in only:
                continue
            if has_evidence(m.group(0)):
                continue
            if line[:m.start()].count('`') % 2:
                continue  # inside a code span
            window = [line]
            for nxt in lines[i + 1:i + 1 + WINDOW]:
                if not nxt.strip():
                    break
                window.append(nxt)
            for prev in lines[max(0, i - BEFORE):i][::-1]:
                if not prev.strip():
                    break
                window.append(prev)
            if has_evidence('\n'.join(window)):
                continue
            out.append((i + 1, m.group(0), line.strip()))
    return out


def git(repo, *args):
    r = subprocess.run(('git', '-C', repo, '-c', 'core.quotePath=false') + args, capture_output=True)
    if r.returncode != 0:
        raise RuntimeError('git %s: %s' % (' '.join(args), r.stderr.decode('utf8', 'replace').strip()))
    return r.stdout.decode('utf8', 'replace')


def unquote(p):
    """A path as git prints it in a diff header: quoted with C escapes when it holds odd bytes."""
    if len(p) >= 2 and p[0] == '"' and p[-1] == '"':
        return p[1:-1].encode('latin-1', 'replace').decode('unicode_escape').encode('latin-1', 'replace').decode('utf8', 'replace')
    return p


def resolve(repo, base):
    """(repo top, commit id of BASE). Paths from git are relative to the top."""
    top = git(repo, 'rev-parse', '--show-toplevel').strip()
    sha = git(top, 'rev-parse', '--verify', '--end-of-options', base + '^{commit}').strip()
    return top, sha


DIFF = ('diff', '--no-color', '--no-ext-diff', '--no-textconv', '--src-prefix=a/', '--dst-prefix=b/')


def added_lines(repo, base):
    """{path: set of added line numbers} from `git diff -U0 BASE`, plus whole untracked files."""
    out = {}
    path = None
    for line in git(repo, *DIFF, '-U0', base, '--').splitlines():
        if line.startswith('+++ '):
            path = unquote(line[4:])
            path = path[2:] if path.startswith('b/') else None
        elif line.startswith('@@') and path:
            m = re.search(r'\+(\d+)(?:,(\d+))?', line)
            if m:
                start, n = int(m.group(1)), int(m.group(2) if m.group(2) is not None else 1)
                out.setdefault(path, set()).update(range(start, start + n))
    for p in git(repo, 'ls-files', '-z', '--others', '--exclude-standard').split('\0'):
        if p:
            out[p] = None  # None: every line
    return out


def walk(repo, rel):
    p = os.path.join(repo, rel)
    if os.path.isfile(p):
        yield rel
        return
    for dp, dn, fn in os.walk(p):
        dn[:] = sorted(d for d in dn if d not in SKIP_DIRS)
        for f in sorted(fn):
            yield os.path.relpath(os.path.join(dp, f), repo)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--base')
    ap.add_argument('--added-only', action='store_true')
    ap.add_argument('--paths')
    ap.add_argument('--files-from')
    ap.add_argument('--all', action='store_true')
    ap.add_argument('--tsv')
    ap.add_argument('--repo', default=os.getcwd())
    try:
        a = ap.parse_args(argv)
    except SystemExit:
        return 2
    repo = a.repo
    only = {}
    try:
        if a.added_only and not a.base:
            sys.stderr.write('v-audit: --added-only needs --base\n')
            return 2
        if a.base:
            repo, sha = resolve(repo, a.base)
            names = set(git(repo, *DIFF, '--name-only', '-z', sha, '--').split('\0'))
            names |= set(git(repo, 'ls-files', '-z', '--others', '--exclude-standard').split('\0'))
            files = sorted(n for n in names if n)
            if a.added_only:
                only = added_lines(repo, sha)
        elif a.paths:
            files = [f for p in a.paths.split(',') if p for f in walk(repo, p)]
        elif a.files_from:
            with open(a.files_from) as fh:
                files = [ln.strip() for ln in fh if ln.strip()]
        elif a.all:
            files = list(walk(repo, '.'))
        else:
            sys.stderr.write('v-audit: name the files: --base, --paths, --files-from or --all\n')
            return 2
    except (RuntimeError, OSError) as e:
        sys.stderr.write('v-audit: %s\n' % e)
        return 2
    rows = []
    scanned = 0
    for rel in files:
        if not rel.endswith(TEXT) or set(rel.split('/')) & SKIP_DIRS:
            continue
        try:
            with open(os.path.join(repo, rel), encoding='utf8', errors='replace') as fh:
                lines = fh.read().split('\n')
        except OSError:
            continue
        scanned += 1
        lim = only.get(rel, None) if a.added_only else None
        if a.added_only and rel not in only:
            continue
        for ln, mark, text in marks_of(lines, lim):
            rows.append((rel, ln, mark, text[:160]))
    for r in rows:
        print('%s\t%d\t%s\t%s' % r)
    if a.tsv:
        with open(a.tsv, 'w') as fh:
            fh.write('path\tline\tmark\ttext\n')
            for r in rows:
                fh.write('%s\t%d\t%s\t%s\n' % r)
    print('uncited [V] marks: %d in %d file(s) scanned' % (len(rows), scanned))
    return 3 if rows else 0


if __name__ == '__main__':
    sys.exit(main())
