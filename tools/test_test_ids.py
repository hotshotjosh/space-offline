"""No real client id in test code.

Tests use invented ids and rows (AGENTS.md, docs/content/policy.md). These
tests now use invented ids for the rows listed below; ids that other tests or
non-test code still hold are not on the list. The list of ids that were taken
out of the tests is itself client data, so it is not in git: it is read from
`out/test-id-denylist.txt` (gitignored), one integer per line, `#` lines and
blanks ignored. The file is looked for in this checkout's `out/` and in the
`out/` beside the repository's common git dir, because the gate runs in a
throwaway worktree that has no `out/` of its own. Without the file (a public
clone) every check here skips with a note.

Test code is: any Rust file under a `tests/` directory or with `test` in its
file name, a file declared as `#[cfg(test)] mod name;`, the body of a
`#[cfg(test)] mod name { ... }` block, and `tools/test_*.py`. A match is a whole
integer token, with or without `_` digit groups. A `file.py:123` line reference
(`.py`, `.rs`, `.md`, `.tsv`) is not an id and is skipped. Integer suffixes
(`i64`, `u32`, `usize`, `f64`, ...) and comma-grouped numbers (`424,242`) match too.

Stdlib only; the gate runs every `tools/test_*.py`.
"""

import os
import re
import subprocess
import sys
import unittest

sys.path.append(os.path.dirname(os.path.abspath(__file__)))
import testenv  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DENYLIST = os.path.join(ROOT, "out", "test-id-denylist.txt")
TOKEN = re.compile(
    r"(?:(?<![\w.])|(?<=\\[tnr]))(\d[\d_]*?)_?(?:[iu](?:8|16|32|64|128|size)|f32|f64)?\b"
)
LINE_REF = re.compile(r"\.(py|rs|md|tsv):$")
COMMA_GROUPS = re.compile(r"(?<=\d),(?=\d{3}(?!\d))")


def denylist_paths(root=ROOT):
    """This checkout's out/ first, then the out/ beside the common git dir."""
    paths = [os.path.join(root, "out", "test-id-denylist.txt")]
    try:
        common = subprocess.run(
            ["git", "-C", root, "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True, check=True,
        ).stdout.decode("utf-8", "replace").strip()
    except (OSError, subprocess.CalledProcessError):
        common = ""
    if common:
        paths.append(os.path.join(os.path.dirname(common), "out", "test-id-denylist.txt"))
    return paths


def load_denylist(path=None):
    """The set of integers in the file, or None when it is absent."""
    if path is None:
        for cand in denylist_paths():
            if os.path.isfile(cand):
                path = cand
                break
        else:
            return None
    if not os.path.isfile(path):
        return None
    ids = set()
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.split("#", 1)[0].strip()
            if line.isdigit():
                ids.add(int(line))
    return ids


def tracked(root=ROOT):
    out = subprocess.run(
        ["git", "-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        capture_output=True, check=True,
    ).stdout.decode("utf-8", "replace")
    return [p for p in out.split("\0") if p]


def out_of_line_test_modules(root, files):
    """Files declared as `#[cfg(test)] mod name;`."""
    found = set()
    for f in files:
        if not f.endswith(".rs"):
            continue
        lines = read_lines(root, f)
        for i, line in enumerate(lines):
            if not re.match(r"\s*#\[cfg\(test\)\]", line):
                continue
            for nxt in lines[i + 1:i + 4]:
                m = re.match(r"\s*(?:pub(?:\([a-z]+\))? )?mod (\w+);", nxt)
                if m:
                    d = os.path.dirname(f)
                    base = os.path.basename(f)
                    if base not in ("mod.rs", "lib.rs", "main.rs"):
                        d = d + "/" + base[:-3]
                    found.add(d + "/" + m.group(1) + ".rs")
                    found.add(d + "/" + m.group(1) + "/mod.rs")
    return found


def read_lines(root, rel):
    try:
        with open(os.path.join(root, rel), encoding="utf-8") as fh:
            return fh.read().split("\n")
    except (OSError, UnicodeDecodeError):
        return []


def cfg_test_blocks(lines):
    """(first, end) line indexes of each `#[cfg(test)] mod x { ... }`."""
    blocks = []
    i, n = 0, len(lines)
    while i < n:
        if re.match(r"\s*#\[cfg\(test\)\]", lines[i]):
            j = i + 1
            while j < n and lines[j].strip().startswith("#["):
                j += 1
            if j < n and re.match(r"\s*(pub(\([a-z]+\))? )?mod\b", lines[j]) and "{" in lines[j]:
                depth, k = 0, j
                while k < n:
                    depth += lines[k].count("{") - lines[k].count("}")
                    if depth <= 0:
                        break
                    k += 1
                blocks.append((i, min(k + 1, n)))
                i = k + 1
                continue
        i += 1
    return blocks


def test_regions(rel, lines, whole_files):
    base = os.path.basename(rel)
    if rel.endswith(".py"):
        return [(0, len(lines))] if base.startswith("test_") else []
    if "/tests/" in rel or "test" in base or rel in whole_files:
        return [(0, len(lines))]
    return cfg_test_blocks(lines)


def hits(root, files, deny):
    whole = out_of_line_test_modules(root, files)
    found = []
    for rel in files:
        if not (rel.endswith(".rs") or (rel.startswith("tools/") and rel.endswith(".py"))):
            continue
        lines = read_lines(root, rel)
        for a, b in test_regions(rel, lines, whole):
            for ln in range(a, b):
                text = lines[ln]
                seen = False
                for variant in (text, COMMA_GROUPS.sub("", text)):
                    for m in TOKEN.finditer(variant):
                        if LINE_REF.search(variant[:m.start()]):
                            continue
                        value = int(m.group(1).replace("_", "") or 0)
                        if value in deny and not seen:
                            found.append("%s:%d" % (rel, ln + 1))
                            seen = True
    return found


class TestIdGuard(unittest.TestCase):
    def test_denylist_ids_stay_out_of_tests(self):
        deny = load_denylist()
        if deny is None:
            self.skipTest("out/test-id-denylist.txt is absent (public clone): nothing to check")
        found = hits(ROOT, tracked(), deny)
        self.assertEqual(found, [], "ids from out/test-id-denylist.txt in test code")


class TestGuardMechanics(unittest.TestCase):
    """The scanner finds what it should, on an invented tree."""

    def setUp(self):
        import tempfile
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = self.tmp.name
        os.makedirs(os.path.join(root, "src"))
        with open(os.path.join(root, "src", "a.rs"), "w", encoding="utf-8") as fh:
            fh.write(
                "const REAL: i64 = 424_242;\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    const X: i64 = 424_242;\n"
                "    const Y: i64 = 7;\n"
                "    // see ref.py:424242\n"
                "}\n"
            )
        self.root = root

    def test_only_the_test_block_counts(self):
        found = hits(self.root, ["src/a.rs"], {424242})
        self.assertEqual(found, ["src/a.rs:4"])

    def test_plain_and_grouped_forms_match(self):
        with open(os.path.join(self.root, "src", "b_tests.rs"), "w", encoding="utf-8") as fh:
            fh.write('let s = "424242\\t1";\nlet n = 424_242;\nlet f = 424242.5;\n')
        found = hits(self.root, ["src/b_tests.rs"], {424242})
        self.assertEqual(found, ["src/b_tests.rs:1", "src/b_tests.rs:2", "src/b_tests.rs:3"])

    def _hit(self, text, value=424242):
        with open(os.path.join(self.root, "src", "c_tests.rs"), "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
        return hits(self.root, ["src/c_tests.rs"], {value})

    def test_integer_suffixes_match(self):
        for text in ("let a = 424242i64;", "let a = 424_242_u32;", "let a = 424242usize;",
                     "let a = 424242f64;", "let a = 424_242_i128;"):
            self.assertEqual(self._hit(text), ["src/c_tests.rs:1"], text)

    def test_other_words_and_digits_do_not_match(self):
        for text in ("let a = 4242421;", "let a = x424242;", "let a = 424242abc;"):
            self.assertEqual(self._hit(text), [], text)

    def test_comma_grouped_numbers_match(self):
        self.assertEqual(self._hit("// the row 424,242 here"), ["src/c_tests.rs:1"])
        self.assertEqual(self._hit("// the row 42,4242 here"), [])

    def test_only_file_line_refs_are_skipped(self):
        for text in ("see foo.py:424242", "see a/b.rs:424242", "see x.md:424242", "see t.tsv:424242"):
            self.assertEqual(self._hit(text), [], text)
        for text in ("let id: 424242,", "key:424242"):
            self.assertEqual(self._hit(text), ["src/c_tests.rs:1"], text)

    def test_escaped_tab_neighbours_match(self):
        self.assertEqual(self._hit('let s = "x\\t424242\\ty";'), ["src/c_tests.rs:1"])

    def test_denylist_found_beside_the_common_git_dir(self):
        main = os.path.join(self.root, "main")
        work = os.path.join(self.root, "work")
        os.makedirs(os.path.join(main, "out"))
        env = testenv.hermetic_git_env()
        for cmd in (["git", "init", "-q", main],
                    ["git", "-C", main, "commit", "-q", "--allow-empty", "-m", "x"],
                    ["git", "-C", main, "worktree", "add", "-q", "--detach", work]):
            subprocess.run(cmd, check=True, env=env, capture_output=True)
        with open(os.path.join(main, "out", "test-id-denylist.txt"), "w", encoding="utf-8") as fh:
            fh.write("# invented\n424242\n")
        paths = denylist_paths(work)
        self.assertEqual(os.path.realpath(paths[1]),
                         os.path.realpath(os.path.join(main, "out", "test-id-denylist.txt")))
        self.assertFalse(os.path.exists(paths[0]))
        found = [p for p in paths if os.path.isfile(p)]
        self.assertEqual(load_denylist(found[0]), {424242})

    def test_missing_list_is_none(self):
        self.assertIsNone(load_denylist(os.path.join(self.root, "nope.txt")))


if __name__ == "__main__":
    unittest.main()
