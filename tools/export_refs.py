"""Exported live files that name a tracked path the public export drops (#292, M4).

A script or test that runs or opens a withheld file works on the maintainer's tree and
fails on a contributor's ("No such file"). No test executes every such line, so this
check reads them. Standard library only.

    python3 -I tools/export_refs.py --check

Scope (exported files only): `.py` (comments and docstrings are ignored; other string
tokens are live), `.sh`, `tools/hooks/*`, `containers/*`, `*.yml` (comment lines ignored).
`.md` hits print as `note:` lines and never fail. `.rs` is out of scope (the build fails
on a missing include). A path that is not a tracked file is never a hit.

The allowlist `tools/export-refs-allow.tsv` has rows `file<TAB>path<TAB>why`, keyed by
file and path (not line). A row that matches nothing is a failure (`stale allow row`).
"""

import ast
import importlib.util
import io
import os
import re
import sys
import tokenize

HERE = os.path.dirname(os.path.abspath(__file__))
ALLOW = os.path.join(HERE, "export-refs-allow.tsv")

# The CI directory's name is split: the export's leak scan rejects the plain word.
_CI_DIR = r"\.forg" + "ejo"
TOKEN = re.compile(
    r"(?<![\w.-])((?:private|deploy|docs|tools|findings|containers|" + _CI_DIR + r")/[A-Za-z0-9_./-]*[A-Za-z0-9_])"
)


def _pe():
    spec = importlib.util.spec_from_file_location("public_export", os.path.join(HERE, "public-export.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def private_names(root: str, pe=None) -> set[str]:
    """Tracked files the export drops, their directories that no exported file lives under
    (a directory that also holds exported files is not private, so naming it is fine),
    the private list entries under the same rule, and `private`."""
    pe = pe or _pe()
    kept = pe.exported(root)
    kept_set = set(kept)
    kept_dirs = set()
    for rel in kept:
        parts = rel.split("/")
        for i in range(1, len(parts)):
            kept_dirs.add("/".join(parts[:i]))
    names = {"private"}
    for rel in pe.tracked(root):
        if rel in kept_set:
            continue
        names.add(rel)
        parts = rel.split("/")
        for i in range(1, len(parts)):
            d = "/".join(parts[:i])
            if d not in kept_dirs:
                names.add(d)
    for p in pe.lines_of(root, pe.PRIVATE_DIR + "export-private-paths.txt"):
        p = p.rstrip("/")
        if p not in kept_dirs:
            names.add(p)
    return names


def in_scope(rel: str) -> str | None:
    """'py', 'sh' (shell rule), 'md', or None."""
    if rel.endswith(".py"):
        return "py"
    if rel.endswith(".sh") or rel.startswith("tools/hooks/") or rel.startswith("containers/") or rel.endswith(".yml"):
        return "sh"
    if rel.endswith(".md"):
        return "md"
    return None


def _hits(text: str, names: set[str], lineno: int, out: list):
    for i, line in enumerate(text.split("\n")):
        for m in TOKEN.finditer(line):
            tok = m.group(1)
            if tok in names or tok.rstrip("/") in names:
                out.append((lineno + i, tok))


def _shell_hits(text: str, names: set[str]) -> list:
    out = []
    for n, line in enumerate(text.split("\n"), 1):
        if line.lstrip().startswith("#"):
            continue
        _hits(line, names, n, out)
    return out


def _literal(tok_string: str):
    """The value of a plain string literal token, or None (bytes, f-strings, py2 forms)."""
    try:
        v = ast.literal_eval(tok_string)
    except (ValueError, SyntaxError):
        return None
    return v if isinstance(v, str) else None


_MIDDLE = tuple(getattr(tokenize, n) for n in ("FSTRING_MIDDLE", "TSTRING_MIDDLE") if hasattr(tokenize, n))


def _py_hits(text: str, names: set[str]) -> list:
    out = []
    try:
        toks = list(tokenize.generate_tokens(io.StringIO(text).readline))
    except (tokenize.TokenError, IndentationError, SyntaxError):
        return _shell_hits(text, names)
    # A STRING that is a whole expression statement is a docstring: previous significant
    # token is NEWLINE/INDENT/DEDENT/start and next is NEWLINE/ENDMARKER.
    skip = {tokenize.NL, tokenize.COMMENT}
    sig = [t for t in toks if t.type not in skip]
    n = len(sig)
    for i, t in enumerate(sig):
        if t.type in _MIDDLE:  # the literal text of an f-string (3.12+)
            _hits(t.string, names, t.start[0], out)
            continue
        if t.type != tokenize.STRING:
            continue
        before = sig[i - 1].type if i else tokenize.NEWLINE
        after = sig[i + 1].type if i + 1 < n else tokenize.ENDMARKER
        if before in (tokenize.NEWLINE, tokenize.INDENT, tokenize.DEDENT) and after in (tokenize.NEWLINE, tokenize.ENDMARKER):
            continue
        _hits(t.string, names, t.start[0], out)
        # A run of plain literals separated only by `,` or `/` is a path built in parts:
        # os.path.join(ROOT, "deploy", "x.sh") or ROOT / "deploy" / "x.sh". Test it joined.
        parts = []
        v = _literal(t.string)
        if v is None or (i >= 2 and sig[i - 1].type == tokenize.OP and sig[i - 1].string in (",", "/") and sig[i - 2].type == tokenize.STRING):
            continue  # not a plain literal, or the middle of a run already tested from its start
        parts.append(v)
        j = i + 1
        while j + 1 < n and sig[j].type == tokenize.OP and sig[j].string in (",", "/") and sig[j + 1].type == tokenize.STRING:
            v = _literal(sig[j + 1].string)
            if v is None:
                break
            parts.append(v)
            j += 2
        if len(parts) > 1:
            for k in range(len(parts) - 1):  # every suffix run, so a leading part may be non-private
                _hits("/".join(parts[k:]), names, t.start[0], out)
    return sorted(set(out))


def read(path: str) -> str:
    with open(path, "rb") as f:
        data = f.read()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("latin-1")


def load_allow(path: str = ALLOW) -> list[tuple[str, str, str]]:
    rows = []
    if not os.path.exists(path):
        return rows
    for line in read(path).split("\n"):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        f = line.split("\t")
        if len(f) < 3 or not f[2].strip():
            raise ValueError(f"export-refs allow row needs file, path, why: {line!r}")
        rows.append((f[0], f[1], f[2]))
    return rows


def scan(root: str, pe=None) -> tuple[list, list]:
    """(live hits, notes), each (file, line, path)."""
    pe = pe or _pe()
    names = private_names(root, pe)
    live, notes = [], []
    for rel in pe.exported(root):
        kind = in_scope(rel)
        if kind is None:
            continue
        p = os.path.join(root, rel)
        if not os.path.isfile(p):
            continue
        text = read(p)
        if kind == "md":
            h = []
            _hits(text, names, 1, h)
            notes += [(rel, n, t) for n, t in h]
        elif kind == "py":
            fn = _py_hits  # scenarios (python 2.7) fall back to the shell rule on a TokenError
            live += [(rel, n, t) for n, t in fn(text, names)]
        else:
            live += [(rel, n, t) for n, t in _shell_hits(text, names)]
    return live, notes


def check(root: str, allow_path: str = ALLOW, out=None) -> int:
    out = out or sys.stdout
    live, notes = scan(root)
    allow = load_allow(allow_path)
    allowed = {(f, p) for f, p, _ in allow}
    used = set()
    bad = 0
    n_allowed = 0
    for f, n, t in live:
        key = None
        for cand in (t, t.rstrip("/")):
            if (f, cand) in allowed:
                key = (f, cand)
                break
        if key:
            used.add(key)
            n_allowed += 1
        else:
            print(f"{f}:{n}: names {t}", file=out)
            bad += 1
    for f, p, _ in allow:
        if (f, p) not in used:
            print(f"stale allow row: {f}\t{p}", file=out)
            bad += 1
    for f, n, t in notes:
        print(f"note: {f}:{n}: names {t}", file=out)
    print(f"export-refs: {len(live) - n_allowed} live, {n_allowed} allowed, {len(notes)} notes", file=out)
    return 1 if bad else 0


def main(argv: list[str]) -> int:
    if argv[1:] != ["--check"]:
        print(__doc__, file=sys.stderr)
        return 2
    root = os.path.dirname(HERE)
    if not os.path.isfile(os.path.join(root, "private", "export-private-paths.txt")):
        print("export-refs: this tree is the export; nothing is excluded here")
        return 0
    return check(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
