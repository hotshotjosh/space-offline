#!/usr/bin/env python3
"""Publish the private tree's public history to GitHub, and take GitHub's merges back.

    python3 -I tools/publish-github.py plan            # what would be pushed; writes nothing remote
    python3 -I tools/publish-github.py push            # map, check, push GitHub main (fast-forward only)
    python3 -I tools/publish-github.py pull            # fetch GitHub main, merge it into this branch
    python3 -I tools/publish-github.py pull --check    # exit 1 when GitHub has commits not merged here

The private repo is GitHub's history plus the private paths (`private/` and every line of
`private/export-private-paths.txt`). Contributors' PRs merge natively on GitHub; `pull`
merges GitHub main into the private branch. `push` maps the private history to public
commits and pushes them:

  - GitHub holds the maps of what was pushed before, so `pull` merges only what is new
    there (a merged PR), and `push` is a fast-forward;
  - a commit GitHub already has maps to itself, unchanged: its author, hash and signature;
  - any other commit maps to a commit whose tree is its tree without the private paths,
    whose parents are its parents' maps (duplicates and parents already reachable through
    another mapped parent dropped), authored and committed by the alias with the original
    dates in UTC, and whose message keeps every line that names no private path and passes the
    leak scan (`private/export-scan.txt`, `private/post-scan.txt`, the export's built-in
    patterns); a withheld subject becomes "Update <areas>";
  - a commit whose public tree equals its only mapped parent's maps to that parent, so a
    private-only commit publishes nothing.

The map is recomputed from the history on every run (no state), so the same history always
gives the same public commits. The public tree is the private tree minus the private paths
only when no exported file is rewritten on the way out: `public-export.py --check`
(identity()) holds the private tree to that, and `push` refuses without it.

`push` refuses when GitHub main is not an ancestor of the new public tip (run `pull`
first), when GitHub main's tree holds a private path (`pull` refuses a new GitHub commit
that touches one; older history may hold files that were public then), or on a leak. It
pushes with the git remote `github` (or $PUBLISH_REMOTE). Standard library only.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins (`tools/dis.py`
# has the reason).
import sys as _sys
_sys.path.append(_sys.path.pop(0))

import argparse
import importlib.util
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ALIAS_NAME = "thewhitekidney"
ALIAS_EMAIL = "0x81e70020@outlook.com"


def load_export():
    spec = importlib.util.spec_from_file_location("pe", os.path.join(HERE, "public-export.py"))
    pe = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pe)
    return pe


class Git:
    def __init__(self, root: str):
        self.root = root

    def run(self, *args, env=None, input=None, ok=True) -> str:
        e = dict(os.environ)
        e.update(env or {})
        p = subprocess.run(["git", "-C", self.root, *args], env=e, input=input,
                           capture_output=True, text=True)
        if ok and p.returncode != 0:
            raise SystemExit(f"publish-github: git {' '.join(args)}: {p.stderr.strip()}")
        return p.stdout

    def ok(self, *args) -> bool:
        return subprocess.run(["git", "-C", self.root, *args], capture_output=True).returncode == 0


def private_prefixes(root: str, pe) -> list[str]:
    return ["private/"] + pe.lines_of(root, "private/export-private-paths.txt")


def is_private(path: str, prefixes: list[str]) -> bool:
    return any(path == p or path.startswith(p.rstrip("/") + "/") for p in prefixes)


def public_tree(g: Git, commit: str, prefixes: list[str], cache: dict) -> str:
    """The tree of `commit` without the private paths (a tree object id)."""
    tree = g.run("rev-parse", commit + "^{tree}").strip()
    key = (tree, tuple(prefixes))
    if key in cache:
        return cache[key]
    names = [l for l in g.run("ls-tree", "-r", "-z", "--name-only", tree).split("\0") if l]
    drop = [n for n in names if is_private(n, prefixes)]
    if not drop:
        cache[key] = tree
        return tree
    idx = os.path.join(g.run("rev-parse", "--absolute-git-dir").strip(), "publish-index")
    env = {"GIT_INDEX_FILE": idx}
    g.run("read-tree", tree, env=env)
    g.run("update-index", "--force-remove", "-z", "--stdin", env=env, input="\0".join(drop) + "\0")
    out = g.run("write-tree", env=env).strip()
    os.remove(idx)
    cache[key] = out
    return out


def scrub_message(msg: str, prefixes: list[str], pats: list, areas: str) -> str:
    """Keep each line that names no private path and passes the scan."""
    def bad(line):
        # a directory prefix keeps its slash: "private/" names a path, "private" is a word
        return any(p in line for p in prefixes) or any(p.search(line) for p in pats)
    lines = msg.rstrip("\n").split("\n")
    subject = lines[0] if lines and not bad(lines[0]) else f"Update {areas}"
    body = [l for l in lines[1:] if not bad(l)]
    while body and not body[0].strip():
        body.pop(0)
    return subject + ("\n\n" + "\n".join(body) if body else "") + "\n"


def areas_of(g: Git, parent_tree: str | None, tree: str) -> str:
    if parent_tree is None:
        names = [l for l in g.run("ls-tree", "--name-only", tree).split("\n") if l]
    else:
        names = [l for l in g.run("diff-tree", "-r", "--name-only", parent_tree, tree).split("\n") if l]
    tops = []
    for n in names:
        t = n.split("/")[0]
        if t not in tops:
            tops.append(t)
    return ", ".join(tops[:3]) + (f" and {len(tops) - 3} more" if len(tops) > 3 else "") or "files"


def lines_at(g: Git, commit: str, rel: str) -> list[str]:
    """`public-export.lines_of` of a file as it is in `commit` (empty when absent)."""
    out = g.run("show", f"{commit}:{rel}", ok=False)
    return [s.strip() for s in out.split("\n") if s.strip() and not s.lstrip().startswith("#")]


def lists_at(g: Git, commit: str, pe, memo: dict):
    """The private paths and the message scan of `commit`'s own tree, so mapping an old
    commit never depends on how the lists changed later (the map stays deterministic)."""
    blobs = tuple(g.run("rev-parse", "-q", "--verify", f"{commit}:{rel}", ok=False).strip()
                  for rel in ("private/export-private-paths.txt", "private/export-scan.txt",
                              "private/post-scan.txt"))
    if blobs not in memo:
        prefixes = ["private/"] + lines_at(g, commit, "private/export-private-paths.txt")
        pats = [re.compile(p) for p in pe.BUILT_IN + lines_at(g, commit, "private/export-scan.txt")]
        try:
            pats += [re.compile(p) for p in lines_at(g, commit, "private/post-scan.txt")]
        except re.error:
            pats = [re.compile("")]   # a list that does not compile withholds every line
        memo[blobs] = (prefixes, pats)
    return memo[blobs]


def build_map(g: Git, tip: str, upstream: str | None, pe) -> dict:
    """private commit -> public commit, for every commit reachable from tip. Each commit is
    mapped with the private paths and the scan of its own tree."""
    memo: dict = {}
    on_github = set()
    if upstream:
        on_github = set(g.run("rev-list", upstream).split())
    commits = g.run("rev-list", "--topo-order", "--reverse", "--parents", tip).strip().split("\n")
    m: dict[str, str] = {}
    cache: dict[str, str] = {}
    for line in commits:
        c, *parents = line.split()
        if c in on_github:
            m[c] = c
            continue
        mp = []
        for p in parents:
            q = m[p]
            if q not in mp:
                mp.append(q)
        # drop a mapped parent that another mapped parent already contains
        mp = [p for p in mp if not any(o != p and g.ok("merge-base", "--is-ancestor", p, o) for o in mp)]
        prefixes, pats = lists_at(g, c, pe, memo)
        tree = public_tree(g, c, prefixes, cache)
        if len(mp) == 1 and g.run("rev-parse", mp[0] + "^{tree}").strip() == tree:
            m[c] = mp[0]
            continue
        # in UTC, as the old mirror did: a local offset would tell where the maintainer is
        fmt = g.run("log", "-1", "--format=%at%n%ct%n%B", c)
        at, ct, msg = fmt.split("\n", 2)
        adate, cdate = f"@{at} +0000", f"@{ct} +0000"
        ptree = g.run("rev-parse", mp[0] + "^{tree}").strip() if mp else None
        msg = scrub_message(msg, prefixes, pats, areas_of(g, ptree, tree))
        env = {"GIT_AUTHOR_NAME": ALIAS_NAME, "GIT_AUTHOR_EMAIL": ALIAS_EMAIL, "GIT_AUTHOR_DATE": adate,
               "GIT_COMMITTER_NAME": ALIAS_NAME, "GIT_COMMITTER_EMAIL": ALIAS_EMAIL,
               "GIT_COMMITTER_DATE": cdate}
        args = ["commit-tree", tree]
        for p in mp:
            args += ["-p", p]
        m[c] = g.run(*args, env=env, input=msg).strip()
    return m


def github_touches_private(g: Git, upstream: str, base: str | None, prefixes) -> list[str]:
    rng = f"{base}..{upstream}" if base else upstream
    names = g.run("log", "--format=", "--name-only", rng).split("\n")
    return sorted({n for n in names if n and is_private(n, prefixes)})


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("cmd", choices=["plan", "push", "pull"])
    ap.add_argument("--root", default=os.path.dirname(HERE))
    ap.add_argument("--rev", default="HEAD")
    ap.add_argument("--check", action="store_true", help="pull: only say whether a merge is due")
    ap.add_argument("--remote", default=os.environ.get("PUBLISH_REMOTE", "github"))
    ap.add_argument("--branch", default=os.environ.get("PUBLISH_BRANCH", "main"))
    a = ap.parse_args(argv)
    g = Git(a.root)
    pe = load_export()
    prefixes = private_prefixes(a.root, pe)
    pats = [re.compile(p) for p in pe.BUILT_IN + pe.lines_of(a.root, "private/export-scan.txt")]
    try:
        pats += [re.compile(p) for p in pe.lines_of(a.root, "private/post-scan.txt")]
    except re.error:
        print("publish-github: private/post-scan.txt does not compile; refusing", file=sys.stderr)
        return 1

    g.run("fetch", "-q", "--no-tags", a.remote, f"refs/heads/{a.branch}")
    upstream = g.run("rev-parse", "FETCH_HEAD").strip()

    if a.cmd == "pull":
        if g.ok("merge-base", "--is-ancestor", upstream,
                build_map(g, g.run("rev-parse", a.rev).strip(), upstream, pe)[
                    g.run("rev-parse", a.rev).strip()]):
            print(f"publish-github: {a.remote}/{a.branch} ({upstream[:12]}) is already merged")
            return 0
        if a.check:
            print(f"publish-github: {a.remote}/{a.branch} ({upstream[:12]}) has commits not merged here; "
                  "run tools/publish-github.py pull, then land", file=sys.stderr)
            return 1
        bad = github_touches_private(g, upstream, None if not g.ok("merge-base", a.rev, upstream)
                                     else g.run("merge-base", a.rev, upstream).strip(), prefixes)
        if bad:
            print("publish-github: GitHub touches private paths, refusing to merge: " + ", ".join(bad[:10]),
                  file=sys.stderr)
            return 1
        subjects = g.run("log", "--format=%s", f"{a.rev}..{upstream}").strip().split("\n")
        prs = [m.group(1) for s in subjects for m in [re.match(r"Merge pull request (#\d+)", s)] if m]
        msg = f"GitHub main is merged in ({', '.join(prs) if prs else str(len(subjects)) + ' commit(s)'})"
        who = {"GIT_AUTHOR_NAME": ALIAS_NAME, "GIT_AUTHOR_EMAIL": ALIAS_EMAIL,
               "GIT_COMMITTER_NAME": ALIAS_NAME, "GIT_COMMITTER_EMAIL": ALIAS_EMAIL}
        p = subprocess.run(["git", "-C", a.root, "merge", "--no-ff", "--no-edit", "-m", msg, upstream],
                           capture_output=True, text=True, env={**os.environ, **who})
        if p.returncode != 0:
            print(p.stdout + p.stderr, file=sys.stderr)
            print("publish-github: the merge did not go through; resolve it, commit, then land", file=sys.stderr)
            return 1
        print(f"publish-github: merged {a.remote}/{a.branch} at {upstream[:12]}: {msg}")
        return 0

    # GitHub's old history may hold files that were public then and are private now (the
    # paths list grew); what must hold is that its tip has none (pull checks each new commit)
    bad = [f for f in g.run("ls-tree", "-r", "--name-only", upstream).split("\n") if f and is_private(f, prefixes)]
    if bad:
        print("publish-github: GitHub main holds private paths: " + ", ".join(bad[:10]), file=sys.stderr)
        return 1
    ident = pe.identity(a.root)
    if ident:
        print("\n".join(ident), file=sys.stderr)
        print("publish-github: the private tree is not its exported form; refusing", file=sys.stderr)
        return 1
    m = build_map(g, g.run("rev-parse", a.rev).strip(), upstream, pe)
    tip = m[g.run("rev-parse", a.rev).strip()]
    # GitHub holds the maps of what we pushed before; anything else on it (a merged PR)
    # must be pulled in first, or the push would not be a fast-forward
    if not g.ok("merge-base", "--is-ancestor", upstream, tip):
        print(f"publish-github: {a.remote}/{a.branch} ({upstream[:12]}) has commits this tree has not "
              "merged; run publish-github.py pull, gate, then push", file=sys.stderr)
        return 1
    new = g.run("log", "--format=%h %an: %s", f"{upstream}..{tip}").strip()
    # the published files: what the public tip holds
    files = [l for l in g.run("ls-tree", "-r", "--name-only", tip).split("\n") if l]
    leaked = [f for f in files if is_private(f, prefixes)]
    if leaked:
        print("publish-github: a private path reached the public tip: " + ", ".join(leaked[:10]), file=sys.stderr)
        return 1
    print(f"publish-github: public tip {tip[:12]}, {len(new.splitlines()) if new else 0} new commit(s) "
          f"over {a.remote}/{a.branch} {upstream[:12]}")
    if new:
        print(new)
    if a.cmd == "plan" or tip == upstream:
        return 0
    g.run("push", "-q", a.remote, f"{tip}:refs/heads/{a.branch}")
    print(f"publish-github: pushed {tip[:12]} to {a.remote}/{a.branch}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
