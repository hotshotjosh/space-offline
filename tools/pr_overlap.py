#!/usr/bin/env python3
"""Open pull requests and branches that touch the same files or lines as your change.

Stdlib only; read-only. Run it before you open a pull request:

    python3 -I tools/pr_overlap.py [--base REF] [--branches] [--pr N] [--json]
    python3 -I tools/pr_overlap.py --paths p1,p2       # plan: whole files, no git diff
    python3 -I tools/pr_overlap.py hot < names.txt     # which names are hot files

Ours is `git diff -U0` from the merge base with `--base` (default: the first of
`upstream/main`, `origin/main`, `origin/master` that exists) to the working tree, as
base-side line ranges. Theirs is every other open PR's file list with its patch, plus
(with `--branches`) the branches that have no open PR. A file in both is `line` when the
ranges are within 3 lines of each other, else `file`. A file without a patch counts as
the whole file (`file`). Append-only files are skipped. `hot` rows name your paths that
are in the hot-file table of docs/guides/contributing.md section 10; they are
information, not a block.

Output is TSV `kind pr author path ours theirs`, then `overlap=none|file|line`. It never
prints a PR title or body. When two PRs overlap on a line, agree an order with the other
author; the later PR says `Depends on: #N`.

Exit codes: 0 no overlap (hot rows alone also exit 0); 1 file-level overlap only; 3 a line
overlap; 2 a tracker, git or config error.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import fnmatch
import importlib.util
import json
import os
import re
import subprocess
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def _sibling(name):
    """A module of tools/ loaded by path (tools/ holds a dis.py that shadows the stdlib),
    shared through sys.modules so tests and other tools see one copy."""
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, name + ".py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


tracker = _sibling("tracker")

CONTEXT = 3
CONTRIBUTING = os.path.join(ROOT, "docs", "guides", "contributing.md")
SERVER_SRC = "server/crates/eve-server/"

# Append-only files: two changes may both touch them.
EXEMPT = [
    "docs/status/progress.tsv",
    "docs/status/*.svg",
    "docs/status/README.md",
    "docs/status/waves.tsv",
    "tools/client-ci/regress.list",
    "docs/packets.tsv",
]

HINT = ('line overlap: agree an order with the author; the later PR says "Depends on: #N"')


def exempt(p):
    return any(p == e or fnmatch.fnmatch(p, e) for e in EXEMPT)


# ---------------------------------------------------------------- hot files


def hot_table(path=CONTRIBUTING):
    """Every backticked path in the first cell of the hot-files table of `## 10.`.

    The table follows a `### ...hot...` heading or a `**Hot files.**` paragraph; the area
    table before it is not read, and reading stops where the table ends. Its `src/...`
    paths are under `server/crates/eve-server/` (the doc says so), so they are returned as
    repo paths. The "registration tables" row names no path in its first cell on purpose:
    only one line of those files is maintainer-only, not the file. None when the doc or
    the table cannot be found.
    """
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return None
    m = re.search(r"^## 10\.[^\n]*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not m:
        return None
    h = re.search(r"^(?:#{3,}[^\n]*\bhot\b|\*\*hot files)[^\n]*$(.*?)(?=^#{2,} |\Z)",
                  m.group(1), re.M | re.S | re.I)
    if not h:
        return None
    out, started = [], False
    for line in h.group(1).split("\n"):
        if not line.startswith("|"):
            if started:
                break
            continue
        started = True
        first = line.strip("|").split("|", 1)[0]
        for t in re.findall(r"`([^`]+)`", first):
            out.append(SERVER_SRC + t if t.startswith("src/") else t)
    return out if started else None


def hot_files(path=CONTRIBUTING):
    """The hot paths; none (not an error) when the table cannot be read: a hot row grants
    nothing, so a reworded doc only loses the information."""
    return hot_table(path) or []


def is_hot(path, hot):
    p = path.rstrip("/")
    return any(p == h.rstrip("/") or fnmatch.fnmatch(p, h) or p.startswith(h.rstrip("/") + "/")
               for h in hot)


# ---------------------------------------------------------------- ranges

HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+\d+(?:,\d+)? @@", re.M)


def parse_hunks(patch):
    """Base-side [first, last] ranges of every `@@ -a,b +c,d @@` hunk. A pure insertion
    (`b` is 0) counts as line `a`. [] when there is no hunk."""
    out = []
    for m in HUNK.finditer(patch or ""):
        a = int(m.group(1))
        b = int(m.group(2)) if m.group(2) is not None else 1
        out.append((a, a + max(b, 1) - 1))
    return out


def merge_ranges(old, new):
    """Union of two range lists; None (the whole file) absorbs everything."""
    if old is None or new is None:
        return None
    return sorted(set(old) | set(new))


def add(files, path, ranges):
    files[path] = merge_ranges(files[path], ranges) if path in files else ranges


def parse_diff(text):
    """{path: ranges or None} from `git diff -U0` output. None means the whole file (a
    binary file, a mode change, a pure rename). A rename is listed under both names."""
    files = {}
    rec = None

    def close():
        if not rec:
            return
        ranges = rec["hunks"] or None
        for name in rec["names"]:
            add(files, name, ranges)

    def name_of(s):
        s = s.strip().strip('"')
        if s == "/dev/null":
            return None
        return s[2:] if s[:2] in ("a/", "b/") else s

    for line in text.split("\n"):
        if line.startswith("diff --git "):
            close()
            rec = {"names": [], "hunks": []}
            m = re.match(r'diff --git "?a/(.*?)"? "?b/(.*?)"?$', line)
            if m:
                for n in (m.group(1), m.group(2)):
                    if n not in rec["names"]:
                        rec["names"].append(n)
        elif rec is None:
            continue
        elif line.startswith(("rename from ", "rename to ", "copy from ", "copy to ")):
            n = line.split(" ", 2)[2].strip('"')
            if n not in rec["names"]:
                rec["names"].append(n)
        elif line.startswith(("--- ", "+++ ")):
            n = name_of(line[4:])
            if n and n not in rec["names"]:
                rec["names"].append(n)
        elif line.startswith("@@ "):
            rec["hunks"] += parse_hunks(line)
    close()
    return files


def ranges_text(r):
    return "all" if r is None else ",".join(f"{a}-{b}" for a, b in r)


def close_ranges(a, b):
    """True when some range of `a` is within CONTEXT lines of some range of `b`."""
    return any(x1 <= y2 + CONTEXT and y1 <= x2 + CONTEXT for x1, x2 in a for y1, y2 in b)


def compare_files(ours, theirs):
    """Hits between two {path: ranges or None} maps: [(path, 'line'|'file', ours, theirs)]."""
    hits = []
    for path, o in sorted(ours.items()):
        if path not in theirs or exempt(path):
            continue
        t = theirs[path]
        kind = "line" if (o is not None and t is not None and close_ranges(o, t)) else "file"
        hits.append((path, kind, o, t))
    return hits


# ---------------------------------------------------------------- ours


class GitError(Exception):
    pass


def run_git(*args):
    try:
        r = subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True,
                           timeout=120, encoding="utf-8", errors="replace")
    except (OSError, subprocess.SubprocessError):
        raise GitError("git did not run") from None
    if r.returncode != 0:
        raise GitError(f"git {args[0]} failed")
    return r.stdout


def default_base(git=run_git):
    for ref in ("upstream/main", "origin/main", "origin/master"):
        try:
            git("rev-parse", "--verify", "--quiet", ref + "^{commit}")
            return ref
        except GitError:
            continue
    raise GitError("no base ref: fetch upstream or origin, or pass --base REF")


def ours_ranges(base=None, git=run_git):
    """{path: ranges or None} of the change from the merge base to the working tree."""
    base = base or default_base(git)
    if base.startswith("-"):
        raise GitError("--base must be a ref")
    mb = git("merge-base", base, "HEAD").strip()
    text = git("-c", "core.quotepath=off", "diff", "-U0", "--find-renames", "--no-ext-diff",
               "--no-color", "--src-prefix=a/", "--dst-prefix=b/", "--no-textconv", mb)
    return parse_diff(text)


# ---------------------------------------------------------------- theirs


def file_ranges(files):
    """{path: ranges or None} from GitHub file objects (`filename`, `previous_filename`,
    `patch`). A file without a patch counts as the whole file."""
    out = {}
    for f in files:
        patch = f.get("patch")
        ranges = parse_hunks(patch) if patch else None
        ranges = ranges or None
        for name in (f.get("filename"), f.get("previous_filename")):
            if name:
                add(out, name, ranges)
    return out


def printable_path(p):
    if any(ord(c) < 32 or ord(c) == 127 or c in tracker.BIDI
           or tracker.unicodedata.category(c) in ("Cc", "Cf", "Co", "Cs", "Zl", "Zp")
           for c in p):
        return "<unprintable path>"
    return p


def author_of(login):
    return login if tracker.is_login(login) else "?"


def theirs_prs(tr, skip_number):
    """[(label, author, {path: ranges})] for every open PR except `skip_number`."""
    out = []
    for p in tr.open_pulls():
        if p.get("number") == skip_number:
            continue
        files = file_ranges(tr.pull_file_list(p["number"]))
        out.append((f"#{p['number']}", author_of((p.get("user") or {}).get("login")), files))
    return out


def theirs_branches(tr, open_heads, mine):
    """Branches with no open PR, compared with the default branch."""
    info = tr.get(tr.r) or {}
    default = info.get("default_branch") or "main"
    out = []
    for b in tr.paged(f"{tr.r}/branches"):
        name = b.get("name") or ""
        if not name or name == default or name in open_heads or name == mine:
            continue
        q = urllib.parse.quote
        cmp_ = tr.get(f"{tr.r}/compare/{q(default, safe='/')}...{q(name, safe='/')}") or {}
        commits = cmp_.get("commits") or [{}]
        who = ((commits[-1].get("author") or {}).get("login"))
        label = "branch:" + tracker.clean_text(name, 60).replace(" ", "_")
        out.append((label, author_of(who), file_ranges(cmp_.get("files") or [])))
    return out


# ---------------------------------------------------------------- report


def build_rows(ours, theirs, hot):
    rows = []
    for path in sorted(ours):
        if is_hot(path, hot):
            rows.append({"kind": "hot", "pr": "-", "author": "-", "path": printable_path(path),
                         "ours": ranges_text(ours[path]), "theirs": "-"})
    worst = "none"
    for label, author, files in theirs:
        for path, kind, o, t in compare_files(ours, files):
            rows.append({"kind": kind, "pr": label, "author": author,
                         "path": printable_path(path), "ours": ranges_text(o),
                         "theirs": ranges_text(t)})
            if kind == "line" or worst == "none":
                worst = kind
    return rows, worst


def parser():
    ap = argparse.ArgumentParser(prog="pr_overlap.py", description=__doc__.split("\n")[0])
    ap.add_argument("--base")
    ap.add_argument("--branches", action="store_true")
    ap.add_argument("--pr", type=int, help="our own PR number (excluded from the others)")
    ap.add_argument("--paths", help="comma-separated paths: plan a change as whole files")
    ap.add_argument("--json", action="store_true")
    sub = ap.add_subparsers(dest="cmd")
    sub.add_parser("hot", help="print the names on stdin that are hot files")
    return ap


def cmd_hot(stdin, out, hot=None):
    hot = hot_files() if hot is None else hot
    for line in stdin.read().split("\n"):
        name = line.strip()
        if name and is_hot(name, hot):
            print(printable_path(name), file=out)
    return 0


def main(argv=None, tr=None, out=None, err=None, stdin=None, git=run_git, hot=None):
    out = out or sys.stdout
    err = err or sys.stderr
    a = parser().parse_args(argv)
    if a.cmd == "hot":
        return cmd_hot(stdin or sys.stdin, out, hot)
    try:
        if tr is None:
            cfg = tracker.config()
            tr = tracker.Tracker(cfg, tracker.Http(cfg, tracker.read_token(api=cfg["api"])))
        if a.paths is not None:
            ours = {p.strip(): None for p in a.paths.split(",") if p.strip()}
            if not ours or any(p.endswith("/") or p.startswith("./") or set(p) & set("*?[")
                               for p in ours):
                print("--paths takes file paths: no globs, no directories, no ./ prefix",
                      file=err)
                return 2
            branch = None
        else:
            ours = ours_ranges(a.base, git)
            try:
                branch = git("rev-parse", "--abbrev-ref", "HEAD").strip()
            except GitError:
                branch = None
        pulls = tr.open_pulls()
        mine = a.pr
        if mine is None and branch:
            p = tracker.pr_for_branch(pulls, branch, tracker.origin_owner())
            mine = p["number"] if p else None
        theirs = theirs_prs(tr, mine)
        if a.branches:
            heads = {(p.get("head") or {}).get("ref") for p in pulls}
            theirs += theirs_branches(tr, heads, branch)
        rows, worst = build_rows(ours, theirs, hot_files() if hot is None else hot)
    except tracker.TrackerError as e:
        print(str(e), file=err)
        return 2
    except GitError as e:
        print(f"git error: {e}", file=err)
        return 2
    if a.json:
        print(json.dumps({"rows": rows, "overlap": worst}), file=out)
    else:
        print("kind\tpr\tauthor\tpath\tours\ttheirs", file=out)
        for r in rows:
            print("\t".join(r[k] for k in ("kind", "pr", "author", "path", "ours", "theirs")),
                  file=out)
        print(f"overlap={worst}", file=out)
        if worst == "line":
            print(HINT, file=out)
    return {"none": 0, "file": 1, "line": 3}[worst]


if __name__ == "__main__":
    sys.exit(main())
