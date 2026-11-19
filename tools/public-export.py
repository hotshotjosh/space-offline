"""The public export's path filter and leak scan.

Stdlib only. Reads the tracked files of a repo (git ls-files, or a walk when
there is no .git, as in a lane's `git archive` copy), drops every private
path, and scans what is left for pointers to one maintainer's own setup.

  python3 -I tools/public-export.py --check          # list leaks (exit 1 if any) and prose mentions of private docs (information only)
  python3 -I tools/public-export.py --list           # the paths that would export
  python3 -I tools/public-export.py --out DIR        # copy them to DIR (must not exist); refuses on a leak

What is private:
  - everything under `private/` (the maintainer folder), always;
  - each path or prefix listed in `private/export-private-paths.txt`, one per line.

What counts as a leak, in an exported file:
  - built in: a home path (`/home/<user>/`; the invented users alice, user, you and
    example are fine), a home LAN address (192.168.x.x; the jail's own 10.x subnet and
    libvirt's default bridge 192.168.122.1 are by design and not a leak),
    the agent secret folder (the dot-claude one);
  - every pattern is matched against the file's relative path as well as its lines,
    and a file that is not UTF-8 is read as latin-1, never skipped;
  - each regex in `private/export-scan.txt`, one per line: names, user names,
    host names. They live in the private folder so this script names nobody.

`--out` also writes `tools/doc-private-prefixes.txt` (one `sha256:<hex>` per `docs/` line of the
private list, so the exported doc lint knows which docs are absent on purpose without the
file naming them) and drops the
private rows, and any section they leave with only its table header, from the exported
`docs/README.md`. `--check` lists every surviving
prose mention (`docs/...` in a `.md` file) of a private doc as `mention:` lines; they
do not change the exit code, because a mention is a dead link to fix, not a leak.

The licence policy, also a failure (`policy()`):
  - `LICENSE.md`, `LICENSE-additions.md`, `CONTRIBUTING-terms.md` and `NOTICE` must be in the export;
  - no `FUNDING.yml` may exist anywhere in the tree (no money, ever: LICENSE-additions.md section 1);
  - no exported text file may call this project "open source" or "OSI". A mention
    inside quotes or backticks (a third party's words, a GitHub topic name) and a
    negated one ("not open source") are fine.

Lines starting with `#` in either file are comments.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import hashlib
import subprocess
import sys
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PRIVATE_DIR = "private/"
DISASM_LINE = r"^\s*0x18[0-9a-f]{7}\s+(?:call|mov|lea|jmp|cmp|xor|test)\b"
BYTECODE_LINE = r"^\s*(?:\d+\s+)*(?:LOAD_(?:FAST|ATTR|GLOBAL|CONST|NAME)|STORE_FAST|CALL_FUNCTION)\s+\d*\s*\w"
BUILT_IN = [
    r"/home/(?!(?:alice|user|you|example)/)[a-z_][a-z0-9_-]*/",
    r"\b192\.168\.(?!122\.1\b)\d{1,3}\.\d{1,3}\b",
    r"\.claude" + r"/secrets",
    # Credential shapes. A real one in the tree is a leak whoever owns it.
    r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
    r"https://(?:\w+\.)?disc" r"ord(?:app)?\.com/api/(?:v\d+/)?webhooks/\d+/[\w-]+",
    r"-----BEGIN [A-Z ]*PRIVATE KEY(?: BLOCK)?-----",
    r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
    r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*",
    r"(?i)\bx-api-key\s*:",
    r"\b(?:ssh-(?:ed25519|rsa|dss)|ecdsa-sha2-nistp\d+) AAAA[0-9A-Za-z+/]{20,}",
    r"\bsk-ant-[A-Za-z0-9_-]{20,}",
    r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
    r"\bAIza[0-9A-Za-z_-]{35}",
    r"\bptr_[A-Za-z0-9+/=]{40,}",
    # Client-data shapes (shapes only; they cannot catch a paraphrase or a bare value).
    r"(?i)c:/build" r"agent/work/",                                # the vendor's build-agent paths
    DISASM_LINE,                                                   # a disassembly listing line
    r"^\s*(?://[/!]?\s*)?def \w+\(self\b.*#\s*:?\d+\s*$",       # line-numbered client Python
    BYTECODE_LINE,                                                 # a bytecode listing
    r"SHAPE " r"ONLY, plus .* sample rows",                           # a dumped table's header
]
# A fixture that must hold a credential-shaped string, per file: the patterns it may match.
ALLOWED = {
    # Listings of Python code that WE assemble ourselves (no client code): its tests and docs
    # show the bytecode and the disassembly of that output.
    "server/crates/eve-server/src/pycode.rs": [DISASM_LINE, BYTECODE_LINE],
    "tools/test_tunnel_config.py": [r"-----BEGIN [A-Z ]*PRIVATE KEY(?: BLOCK)?-----"],
}
INFLATE_MAX = 64 << 20   # bytes inflated per capture file; more is not scanned, and is a hit
SKIP_DIRS = {".git", "out", "var", "logs", "tmp", "target", "__pycache__"}


def tracked(root: str) -> list[str]:
    if os.path.exists(os.path.join(root, ".git")):
        out = subprocess.run(
            ["git", "-C", root, "ls-files", "-z"], check=True, capture_output=True
        ).stdout
        return sorted(p.decode() for p in out.split(b"\0") if p)
    files = []
    for d, dirs, names in os.walk(root):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        for n in names:
            files.append(os.path.relpath(os.path.join(d, n), root))
    return sorted(files)


def lines_of(root: str, rel: str) -> list[str]:
    p = os.path.join(root, rel)
    if not os.path.exists(p):
        return []
    with open(p, encoding="utf-8") as f:
        return [s.strip() for s in f if s.strip() and not s.lstrip().startswith("#")]


def exported(root: str) -> list[str]:
    private = lines_of(root, PRIVATE_DIR + "export-private-paths.txt")
    keep = []
    for rel in tracked(root):
        if rel.startswith(PRIVATE_DIR):
            continue
        if any(rel == p or rel.startswith(p.rstrip("/") + "/") for p in private):
            continue
        keep.append(rel)
    return keep


def read_text(path: str) -> str | None:
    """The file's text: UTF-8, else latin-1 (a binary file is still scanned); None if unreadable."""
    try:
        with open(path, "rb") as f:
            data = f.read()
    except (IsADirectoryError, FileNotFoundError):
        return None
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = data.decode("latin-1")
    binary = b"\0" in data[:4096]
    if path.endswith(".evecap") or binary:
        streams = inflated(data)
        if streams:
            text += "\n" + "\n".join(streams)
    if data[:2] == b"\x1f\x8b":
        text += "\n" + gunzipped(data)
    if binary:
        # UTF-16 text in a binary file reads as letters split by NULs in the lines above.
        text += "\n" + data.decode("utf-16-le", "ignore") + "\n" + data[1:].decode("utf-16-le", "ignore")
    return text


class TooBig(Exception):
    """A capture inflates past INFLATE_MAX."""


CHUNK = 1 << 16   # the decompressor is fed this much at a time
ZLIB_HEAD = re.compile(b"\x78[\x01\x5e\x9c\xda]")


def _feed(d, view, start: int, total: int):
    """Run decompressor d over view from start in CHUNK windows: (parts, next offset, failed).
    The bytes decoded before a zlib error are kept. TooBig past INFLATE_MAX in all."""
    parts, at, used = [], start, 0
    try:
        while not d.eof and at < len(view):
            chunk = view[at:at + CHUNK]
            at += len(chunk)
            limit = INFLATE_MAX - total - used + 1
            raw = d.decompress(chunk, limit)
            if len(raw) >= limit:
                raise TooBig()
            parts.append(raw)
            used += len(raw)
    except zlib.error:
        return parts, at, True
    return parts, at, False


def inflated(data: bytes) -> list[str]:
    """Every zlib stream inside a capture, as latin-1 text, so the scan reads them. Bounded:
    at most INFLATE_MAX bytes in all (TooBig past that), no copy of the rest of the file per
    stream, and a consumed stream is skipped. A truncated stream gives what it holds; a stream
    that fails partway keeps what was decoded before the error, and one whose checksum is
    corrupt is read again without the checksum, so its text is still scanned."""
    view, out, pos, total = memoryview(data), [], 0, 0
    while True:
        m = ZLIB_HEAD.search(data, pos)
        if not m:
            return out
        d = zlib.decompressobj()
        parts, at, failed = _feed(d, view, m.start(), total)
        if failed and not parts:   # a bad checksum fails after the text was produced
            d = zlib.decompressobj(-zlib.MAX_WBITS)
            parts, at, _ = _feed(d, view, m.start() + 2, total)
        raw = b"".join(parts)
        if not raw:
            pos = m.start() + 1
            continue
        total += len(raw)
        out.append(raw.decode("latin-1"))
        pos = m.start() + 1 if failed else (at - len(d.unused_data) if d.eof else len(data))


def gunzipped(data: bytes) -> str:
    """The text of every gzip member in `data` as latin-1, at most INFLATE_MAX bytes in all
    (TooBig past that). What was decoded before an error is kept."""
    out, total, pos = [], 0, 0
    while data[pos:pos + 2] == b"\x1f\x8b":
        d = zlib.decompressobj(16 + zlib.MAX_WBITS)
        try:
            for at in range(pos, len(data), CHUNK):
                raw = d.decompress(data[at:at + CHUNK], INFLATE_MAX + 1 - total)
                while True:
                    total += len(raw)
                    if total > INFLATE_MAX:
                        raise TooBig
                    out.append(raw.decode("latin-1"))
                    if not d.unconsumed_tail:
                        break
                    raw = d.decompress(d.unconsumed_tail, INFLATE_MAX + 1 - total)
                if d.eof:
                    break
        except zlib.error:
            # A bad trailer or checksum loses the whole chunk; read the deflate body alone
            # (the plain 10-byte header) so the text is still scanned.
            parts, _, _ = _feed(zlib.decompressobj(-zlib.MAX_WBITS), memoryview(data), pos + 10, total)
            out.extend(x.decode("latin-1") for x in parts)
            break
        if not d.eof:
            break
        pos = len(data) - len(d.unused_data)
    return "\n".join(out)


def leaks(root: str, files: list[str]) -> list[str]:
    pats = [re.compile(p) for p in BUILT_IN + lines_of(root, PRIVATE_DIR + "export-scan.txt")]
    hits = []
    for rel in files:
        for p in pats:
            if p.search(rel):
                hits.append(f"{rel}: path matches /{p.pattern}/")
                break
        try:
            text = read_text(os.path.join(root, rel))
        except TooBig:
            hits.append(f"{rel}: a zlib stream inflates past INFLATE_MAX bytes; not scanned")
            continue
        if text is None:
            continue
        allowed = ALLOWED.get(rel, [])
        for n, line in enumerate(text.split("\n"), 1):
            for p in pats:
                if p.pattern not in allowed and p.search(line):
                    hits.append(f"{rel}:{n}: matches /{p.pattern}/")
                    break
    return hits


REQUIRED = ("LICENSE.md", "LICENSE-additions.md", "CONTRIBUTING-terms.md", "NOTICE")
OPEN_SOURCE = re.compile(r"(?i:\bopen[\s-]+source\b)|\bOSI\b")
QUOTED = re.compile(r'"[^"\n]*"|\u201c[^\u201d\n]*\u201d|`[^`\n]*`')
NEGATED = re.compile(r"(?i)\b(?:not|never|no|isn't|nor)\b[^.;:!?]{0,24}$")


def open_source_claims(line: str) -> list[str]:
    """The unquoted, un-negated `open source` or `OSI` words in a line."""
    spans = [m.span() for m in QUOTED.finditer(line)]
    out = []
    for m in OPEN_SOURCE.finditer(line):
        if any(a <= m.start() and m.end() <= b for a, b in spans):
            continue
        if NEGATED.search(line[: m.start()]):
            continue
        out.append(m.group(0))
    return out


def policy(root: str, files: list[str]) -> list[str]:
    """Licence-policy failures: missing licence files, a funding file, a claim of an OSI-style licence."""
    hits = [f"{r}: missing from the export (the licence must ship)" for r in REQUIRED if r not in files]
    for rel in tracked(root):
        if os.path.basename(rel).lower() in ("funding.yml", "funding.yaml"):
            hits.append(f"{rel}: a funding file (no money, ever: LICENSE-additions.md section 1)")
    for rel in files:
        text = read_text(os.path.join(root, rel))
        if text is None or "\0" in text:
            continue
        for n, line in enumerate(text.split("\n"), 1):
            words = open_source_claims(line)
            if words:
                hits.append(f"{rel}:{n}: calls the project {words[0]!r} (it is source available)")
    return hits


def private_docs(root: str) -> list[str]:
    """The `docs/` lines of the private list: files and prefixes absent from the export."""
    return [p for p in lines_of(root, PRIVATE_DIR + "export-private-paths.txt") if p.startswith("docs/")]


def is_private_doc(path: str, prefixes: list[str]) -> bool:
    return any(path == p or path.startswith(p.rstrip("/") + "/") for p in prefixes)


def filter_index(text: str, prefixes: list[str]) -> str:
    """docs/README.md without the table rows whose first cell is a private doc, and
    without a `## ` section that lost rows and has nothing left but its table header."""
    keep = []
    dropped = set()  # indices in `keep` of the headings whose section lost a row
    heading = None
    for line in text.split("\n"):
        if line.startswith("## "):
            heading = len(keep)
        if line.startswith("|"):
            cells = re.split(r"(?<!\\)\|", line.strip())
            first = cells[1] if len(cells) > 2 else ""
            m = re.search(r"((?:docs/)?[A-Za-z0-9_./-]+)", first)
            if m:
                path = m.group(1)
                path = path if path.startswith("docs/") else "docs/" + path
                if is_private_doc(path, prefixes):
                    if heading is not None:
                        dropped.add(heading)
                    continue
        keep.append(line)
    starts = [i for i, l in enumerate(keep) if l.startswith("## ")]
    out = keep[:starts[0]] if starts else keep
    for i, j in zip(starts, starts[1:] + [len(keep)]):
        body = [l for l in keep[i + 1:j] if l.strip()]
        if i in dropped and len(body) <= 2 and all(l.startswith("|") for l in body):
            continue
        out += keep[i:j]
    return "\n".join(out)


DOC_MENTION = re.compile(r"docs/[A-Za-z0-9_./-]*[A-Za-z0-9_/]")


def mentions(root: str, files: list[str]) -> list[str]:
    """Prose in an exported .md file that names a private doc (what a reader could not follow)."""
    prefixes = private_docs(root)
    out = []
    for rel in files:
        if not rel.endswith(".md"):
            continue
        text = read_text(os.path.join(root, rel))
        if text is None:
            continue
        if rel == "docs/README.md":
            text = filter_index(text, prefixes)
        for n, line in enumerate(text.split("\n"), 1):
            for m in DOC_MENTION.finditer(line):
                if is_private_doc(m.group(0), prefixes):
                    out.append(f"{rel}:{n}: mention: {m.group(0)}")
    return out


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--check", action="store_true")
    g.add_argument("--list", action="store_true")
    g.add_argument("--out")
    ap.add_argument("--root", default=ROOT)
    a = ap.parse_args(argv)
    files = exported(a.root)
    if a.list:
        print("\n".join(files))
        return 0
    hits = leaks(a.root, files) + policy(a.root, files)
    for h in hits:
        print(h)
    ments = mentions(a.root, files)
    for m in ments:
        print(m)
    by_file = len({h.split(":", 1)[0] for h in hits})
    print(f"public-export: {len(files)} files exported, {len(hits)} leak lines in {by_file} files, "
          f"{len(ments)} mentions of private docs", file=sys.stderr)
    if a.check or hits:
        return 1 if hits else 0
    if os.path.exists(a.out):
        print(f"public-export: {a.out} exists, refusing", file=sys.stderr)
        return 2
    prefixes = private_docs(a.root)
    for rel in files:
        dst = os.path.join(a.out, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        if rel == "docs/README.md":
            with open(os.path.join(a.root, rel), encoding="utf-8") as f:
                text = filter_index(f.read(), prefixes)
            with open(dst, "w", encoding="utf-8", newline="") as f:
                f.write(text)
        elif os.path.islink(os.path.join(a.root, rel)):
            os.symlink(os.readlink(os.path.join(a.root, rel)), dst)  # keep a link a link
        else:
            shutil.copy2(os.path.join(a.root, rel), dst)
    # The exported doc lint reads this when private/ is absent (tools/test_docs.py).
    dst = os.path.join(a.out, "tools", "doc-private-prefixes.txt")
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(dst, "w", encoding="utf-8") as f:
        f.write("# Docs the public export omits on purpose, as sha256 of each path (written by tools/public-export.py --out).\n")
        f.write("".join("sha256:" + hashlib.sha256(p.rstrip("/").encode()).hexdigest() + "\n" for p in prefixes))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
