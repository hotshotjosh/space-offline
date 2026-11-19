"""Tests for `tools/check_deps.py` (review V6 F11; packet IP20).

Each test builds a synthetic server tree in a temp dir: a clean tree that
passes, plus one defect. Invented crate names only; the real tree is never
read, so this file says nothing about what the server depends on.

Run::

    python3 -I tools/test_check_deps.py
"""

from __future__ import annotations

import os
import pathlib
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last, so nothing in tools/ shadows the stdlib

import check_deps  # noqa: E402

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"

ROOT = """\
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.dependencies]
"""

CRATE = """\
[package]
name = "{name}"
version = "0.0.1"

[dependencies]
"""

CONFIG = """\
[source.crates-io]
replace-with = "distro"

[source.distro]
directory = "/usr/share/cargo/registry"

[net]
offline = true
"""

LOCK = """\
version = 4

[[package]]
name = "alpha"
version = "0.0.1"
"""


class CheckDeps(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.server = pathlib.Path(self._tmp.name) / "server"
        self.write("Cargo.toml", ROOT)
        self.write("deps.allow", "# none\n")
        self.write("Cargo.lock", LOCK)
        self.write(".cargo/config.toml", CONFIG)
        self.write("crates/alpha/Cargo.toml", CRATE.format(name="alpha"))
        self.write("crates/alpha/src/lib.rs", "")

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, rel, text):
        p = self.server / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def append(self, rel, text):
        p = self.server / rel
        p.write_text(p.read_text() + text)

    def errors(self):
        return check_deps.check(self.server)[0]

    def row(self, name, kind="direct", version="1.0.0", licence="MIT", build="no", unsafe="none", used_in=None):
        if used_in is None:
            used_in = f"alpha/src/{name}_wrap.rs" if kind == "direct" else "-"
        return "\t".join((name, version, kind, licence, build, unsafe, used_in)) + "\n"

    def admit(self, *rows, direct=("zeta",), locked=None):
        """Allowlist `rows` (lines from row()), declare `direct` in the workspace, lock them,
        and write a matching deps.digest and THIRD-PARTY.txt."""
        self.write("deps.allow", "".join(rows))
        self.write("Cargo.toml", ROOT + "".join(f'{d} = "1"\n' for d in direct))
        lock = LOCK
        digest = ""
        for r in rows:
            name, version = r.split("\t")[:2]
            lock += (f'\n[[package]]\nname = "{name}"\nversion = "{version}"\n'
                     f'source = "{CRATES_IO}"\nchecksum = "Could not get crate checksum"\n')
            digest += f"{name}\t{version}\t{'0' * 64}\n"
        self.write("Cargo.lock", lock)
        self.write("deps.digest", digest)
        self.write("THIRD-PARTY.txt", "notices\n")

    def assertFails(self, *needles):
        errs = self.errors()
        self.assertTrue(errs, "expected a failure, got none")
        joined = "\n".join(errs)
        for n in needles:
            self.assertIn(n, joined)

    # --- the clean tree --------------------------------------------------

    def test_a_clean_std_only_tree_passes(self):
        self.assertEqual(self.errors(), [])

    def test_main_exit_status_follows_the_errors(self):
        orig = check_deps.SERVER
        try:
            check_deps.SERVER = self.server
            with open(os.devnull, "w") as null:
                out, err = sys.stdout, sys.stderr
                sys.stdout = sys.stderr = null
                try:
                    clean = check_deps.main()
                    self.write(".cargo/config.toml", CONFIG.replace("offline = true", ""))
                    dirty = check_deps.main()
                finally:
                    sys.stdout, sys.stderr = out, err
        finally:
            check_deps.SERVER = orig
        self.assertEqual((clean, dirty), (0, 1))

    # --- Cargo.lock sources ----------------------------------------------

    def test_a_lock_package_with_any_source_fails_while_the_allowlist_is_empty(self):
        self.append("Cargo.lock", f'\n[[package]]\nname = "zeta"\nversion = "1.0.0"\nsource = "{CRATES_IO}"\n')
        self.assertFails("Cargo.lock", "'zeta'", "not in deps.allow")

    def test_a_git_lock_source_fails_even_when_the_name_is_allowlisted(self):
        self.admit(self.row("zeta"))
        self.write("Cargo.lock", LOCK + '\n[[package]]\nname = "zeta"\nversion = "1.0.0"\n'
                   'source = "git+https://example.invalid/zeta#0123"\nchecksum = "x"\n')
        self.assertFails("Cargo.lock", "'zeta'", "not the distro registry")

    def test_an_allowlisted_crates_io_lock_package_passes(self):
        self.admit(self.row("zeta"))
        self.write("crates/alpha/src/zeta_wrap.rs", "fn f() { zeta::go(); }\n")
        self.assertEqual(self.errors(), [])

    # --- [patch] and [replace] -------------------------------------------

    def test_patch_in_the_workspace_manifest_fails(self):
        self.append("Cargo.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails("server/Cargo.toml", "[patch]")

    def test_replace_in_the_workspace_manifest_fails(self):
        self.append("Cargo.toml", '\n[replace]\n"zeta:1.0.0" = { path = "../zeta" }\n')
        self.assertFails("server/Cargo.toml", "[replace]")

    def test_patch_in_a_crate_manifest_fails(self):
        self.append("crates/alpha/Cargo.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails("crates/alpha/Cargo.toml", "[patch]")

    def test_patch_in_the_cargo_config_fails(self):
        self.append(".cargo/config.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails(".cargo/config.toml", "[patch]")

    # --- .cargo/config.toml ----------------------------------------------

    def test_a_missing_offline_line_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace("offline = true", ""))
        self.assertFails("offline = true")

    def test_offline_false_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace("offline = true", "offline = false"))
        self.assertFails("offline = true")

    def test_a_missing_replace_with_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace('replace-with = "distro"\n', ""))
        self.assertFails('replace-with = "distro"')

    def test_replace_with_another_source_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace('"distro"\n', '"mirror"\n', 1))
        self.assertFails('replace-with = "distro"')

    def test_a_distro_source_that_is_not_a_directory_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace(
            'directory = "/usr/share/cargo/registry"', 'registry = "https://example.invalid/index"'))
        self.assertFails("[source.distro]")

    def test_a_missing_cargo_config_fails(self):
        (self.server / ".cargo" / "config.toml").unlink()
        self.assertFails(".cargo/config.toml", "missing")

    # --- build scripts ---------------------------------------------------

    def test_an_unlisted_build_rs_fails(self):
        self.write("crates/alpha/build.rs", "fn main() {}\n")
        self.assertFails("crates/alpha/build.rs", "BUILD_RS_ALLOW")

    def test_a_build_key_naming_another_script_fails(self):
        self.write("crates/alpha/Cargo.toml",
                   CRATE.format(name="alpha").replace('version = "0.0.1"\n', 'version = "0.0.1"\nbuild = "gen.rs"\n'))
        self.assertFails("crates/alpha/Cargo.toml", "build = 'gen.rs'")

    def test_an_allowlisted_build_rs_passes(self):
        self.write("crates/alpha/build.rs", "fn main() {}\n")
        orig = check_deps.BUILD_RS_ALLOW
        try:
            check_deps.BUILD_RS_ALLOW = frozenset({"alpha"})
            self.assertEqual(self.errors(), [])
        finally:
            check_deps.BUILD_RS_ALLOW = orig

    # --- the manifest rules that predate IP20 ----------------------------

    def test_a_registry_dependency_in_a_crate_fails(self):
        self.append("crates/alpha/Cargo.toml", 'zeta = "1"\n')
        self.assertFails("'zeta'", "path or `workspace = true`")

    def test_a_git_workspace_dependency_fails(self):
        self.admit(self.row("zeta"), direct=())
        self.write("Cargo.toml", ROOT + 'zeta = { git = "https://example.invalid/zeta" }\n')
        self.assertFails("'zeta'", "distro registry")

    def test_an_unused_allowlist_entry_fails(self):
        self.admit(self.row("zeta"), direct=())
        self.assertFails("deps.allow", "unused")

    # --- the deps.allow table (phase 0 of the crate policy) ---------------

    def passing(self):
        """A getrandom-shaped admission with invented names: zeta direct, eta and theta via zeta."""
        self.admit(self.row("zeta", unsafe="os-ffi"), self.row("eta", "via zeta", build="yes"),
                   self.row("theta", "via zeta"))
        self.write("crates/alpha/src/zeta_wrap.rs", "pub fn f() { zeta::go(); }\n")

    def test_a_direct_and_via_admission_passes(self):
        self.passing()
        self.assertEqual(self.errors(), [])

    def test_a_via_row_needs_its_parent_row(self):
        self.passing()
        self.admit(self.row("zeta"), self.row("eta", "via nowhere"))
        self.write("crates/alpha/src/zeta_wrap.rs", "")
        self.assertFails("'eta'", "'nowhere'", "not a row")

    def test_a_via_row_missing_from_the_lock_fails(self):
        self.passing()
        self.append("deps.allow", self.row("iota", "via zeta"))
        self.assertFails("'iota'", "not in Cargo.lock")

    def test_a_workspace_dependency_without_a_direct_row_fails(self):
        self.passing()
        self.append("Cargo.toml", 'kappa = "1"\n')
        self.assertFails("'kappa'", "no `direct` row")

    def test_a_direct_row_that_is_not_a_workspace_dependency_fails(self):
        self.passing()
        self.write("Cargo.toml", ROOT)
        self.assertFails("'zeta'", "unused")

    def test_a_lock_version_other_than_the_row_fails(self):
        self.passing()
        self.write("Cargo.lock", self.server.joinpath("Cargo.lock").read_text().replace(
            'name = "zeta"\nversion = "1.0.0"', 'name = "zeta"\nversion = "1.0.1"'))
        self.assertFails("'zeta' 1.0.1", "not in deps.allow", "(allowed: 1.0.0)")

    def test_a_lock_package_without_a_checksum_key_fails(self):
        self.passing()
        self.write("Cargo.lock", self.server.joinpath("Cargo.lock").read_text().replace(
            'checksum = "Could not get crate checksum"\n', "", 1))
        self.assertFails("no checksum key")

    def test_a_malformed_row_fails(self):
        self.passing()
        self.append("deps.allow", "lambda\t1.0.0\tdirect\tMIT\n")
        self.assertFails("deps.allow", "line", "tab-separated")

    def test_a_bad_build_flag_or_kind_fails(self):
        self.passing()
        self.append("deps.allow", "mu\t1.0.0\tindirect\tMIT\tmaybe\tnone\t-\n")
        self.assertFails("build must be yes or no", "kind must be")

    def test_a_direct_row_needs_a_using_file_and_a_via_row_takes_none(self):
        self.passing()
        self.write("deps.allow", self.row("zeta", used_in="-") + self.row("eta", "via zeta", used_in="x.rs"))
        self.write("Cargo.lock", self.server.joinpath("Cargo.lock").read_text())
        self.assertFails("direct crate 'zeta' needs a used-in", "takes used-in '-'")

    # --- licences (rule 7) ------------------------------------------------

    def test_an_or_expression_with_one_accepted_term_passes(self):
        self.assertTrue(check_deps.licence_ok("MIT OR Apache-2.0"))
        self.assertTrue(check_deps.licence_ok("GPL-3.0-only OR MIT"))
        self.assertTrue(check_deps.licence_ok("(MIT OR Apache-2.0) AND Unicode-3.0"))

    def test_copyleft_unknown_and_and_expressions_fail(self):
        for bad in ("GPL-3.0-only", "LGPL-2.1-or-later", "MPL-2.0", "MIT AND GPL-2.0-only", "SomethingElse"):
            self.assertFalse(check_deps.licence_ok(bad), bad)

    def test_a_row_with_an_unaccepted_licence_fails(self):
        self.passing()
        self.write("deps.allow", self.row("zeta", licence="GPL-3.0-only") + self.row("eta", "via zeta", build="yes")
                   + self.row("theta", "via zeta"))
        self.assertFails("'zeta'", "GPL-3.0-only", "accepted list")

    def test_an_owner_exception_by_crate_name_passes(self):
        self.passing()
        self.write("deps.allow", self.row("zeta", licence="MPL-2.0") + self.row("eta", "via zeta", build="yes")
                   + self.row("theta", "via zeta"))
        orig = check_deps.LICENCE_EXCEPT
        try:
            check_deps.LICENCE_EXCEPT = frozenset({"zeta"})
            self.assertEqual(self.errors(), [])
        finally:
            check_deps.LICENCE_EXCEPT = orig

    # --- one using file (rule 8) ------------------------------------------

    def test_another_file_naming_the_crate_fails(self):
        self.passing()
        self.write("crates/alpha/src/lib.rs", "fn g() { zeta::go(); }\n")
        self.assertFails("alpha/src/lib.rs", "'zeta'", "only in alpha/src/zeta_wrap.rs")

    def test_use_and_extern_crate_count_as_naming(self):
        for text in ("use zeta::go;\n", "use zeta;\n", "extern crate zeta;\n"):
            self.passing()
            self.write("crates/alpha/src/lib.rs", text)
            self.assertFails("alpha/src/lib.rs", "'zeta'", "which may be used only")
            self.write("crates/alpha/src/lib.rs", "")

    def test_comments_and_longer_names_do_not_count(self):
        self.passing()
        self.write("crates/alpha/src/lib.rs", "// zeta::go is wrapped in zeta_wrap\nfn g() { zeta_ext::go(); my_zeta::x(); }\n")
        self.assertEqual(self.errors(), [])

    def test_a_hyphenated_crate_name_matches_its_underscored_path(self):
        self.admit(self.row("zeta-core", used_in="alpha/src/zeta_wrap.rs"), direct=("zeta-core",))
        self.write("crates/alpha/src/zeta_wrap.rs", "fn f() { zeta_core::go(); }\n")
        self.write("crates/alpha/src/lib.rs", "fn g() { zeta_core::go(); }\n")
        self.assertFails("alpha/src/lib.rs", "'zeta-core'")

    def test_a_via_crate_named_in_our_code_fails(self):
        self.passing()
        self.write("crates/alpha/src/lib.rs", "fn g() { eta::go(); }\n")
        self.assertFails("'eta'", "no file of ours")

    def test_a_missing_using_file_fails(self):
        self.passing()
        (self.server / "crates/alpha/src/zeta_wrap.rs").unlink()
        self.assertFails("used-in", "does not exist")

    # --- digest and notices: host consistency -----------------------------

    def test_a_digest_file_that_names_other_crates_fails(self):
        self.passing()
        self.append("deps.digest", f"stray\t1.0.0\t{'0' * 64}\n")
        self.assertFails("deps.digest")

    def test_a_missing_digest_or_notices_file_fails(self):
        self.passing()
        (self.server / "deps.digest").unlink()
        (self.server / "THIRD-PARTY.txt").unlink()
        self.assertFails("deps.digest: missing", "THIRD-PARTY.txt: missing")


class RegistryMode(unittest.TestCase):
    """Rule 9 against a fake registry tree with invented crates."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self._tmp.name)
        self.reg = root / "registry"
        self.server = root / "server"
        self.rows = [
            {"name": "zeta", "version": "1.0.0", "licence": "MIT OR Apache-2.0", "build": "no"},
            {"name": "eta", "version": "2.1.0", "licence": "MIT", "build": "yes"},
        ]
        self.crate("zeta", "1.0.0", 'license = "MIT OR Apache-2.0"\nbuild = false\n', {"LICENSE-MIT": "MIT text\n"})
        self.crate("eta", "2.1.0", 'license = "MIT"\nbuild = "build.rs"\n',
                   {"build.rs": "fn main() {}\n", "LICENSE": "eta licence\n", "NOTICE": "eta notice\n"})
        self.build_tree()

    def tearDown(self):
        self._tmp.cleanup()

    def crate(self, name, version, extra, files):
        d = self.reg / f"{name}-{version}"
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src/lib.rs").write_text("")
        (d / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "{version}"\n{extra}')
        for n, t in files.items():
            (d / n).write_text(t)

    def build_tree(self):
        """Write a server tree whose deps.allow, digest and notices match the registry."""
        s = self.server
        for rel, text in {
            "Cargo.toml": ROOT + 'zeta = "1"\n',
            ".cargo/config.toml": CONFIG,
            "crates/alpha/Cargo.toml": CRATE.format(name="alpha"),
            "crates/alpha/src/wrap.rs": "fn f() { zeta::go(); }\n",
        }.items():
            (s / rel).parent.mkdir(parents=True, exist_ok=True)
            (s / rel).write_text(text)
        (s / "deps.allow").write_text(
            "zeta\t1.0.0\tdirect\tMIT OR Apache-2.0\tno\tnone\talpha/src/wrap.rs\n"
            "eta\t2.1.0\tvia zeta\tMIT\tyes\tnone\t-\n")
        (s / "Cargo.lock").write_text(
            "version = 4\n" + "".join(
                f'\n[[package]]\nname = "{n}"\nversion = "{v}"\nsource = "{CRATES_IO}"\nchecksum = "x"\n'
                for n, v in (("zeta", "1.0.0"), ("eta", "2.1.0"))))
        allow_rows, _ = check_deps.parse_allow((s / "deps.allow").read_text())
        (s / "deps.digest").write_text(check_deps.digest_lines(allow_rows, self.reg))
        (s / "THIRD-PARTY.txt").write_text(check_deps.load_notices().render(allow_rows, self.reg))

    def errors(self):
        return check_deps.check(self.server, self.reg)[0]

    def test_a_matching_registry_passes_host_and_registry_modes(self):
        self.assertEqual(self.errors(), [])
        self.assertEqual(check_deps.check(self.server)[0], [])

    def test_a_missing_crate_directory_fails(self):
        import shutil
        shutil.rmtree(self.reg / "eta-2.1.0")
        self.assertIn("eta-2.1.0 does not exist", "\n".join(self.errors()))

    def test_a_changed_file_in_a_crate_changes_the_digest_and_fails(self):
        (self.reg / "zeta-1.0.0/src/lib.rs").write_text("// patched\n")
        joined = "\n".join(self.errors())
        self.assertIn("zeta-1.0.0 digest differs", joined)

    def test_an_added_file_in_a_crate_fails(self):
        (self.reg / "zeta-1.0.0/extra.rs").write_text("")
        self.assertIn("zeta-1.0.0 digest differs", "\n".join(self.errors()))

    def test_the_digest_covers_paths_not_just_contents(self):
        a = self.reg / "zeta-1.0.0"
        before = check_deps.tree_digest(a)
        (a / "src/lib.rs").rename(a / "src/other.rs")
        self.assertNotEqual(before, check_deps.tree_digest(a))

    def test_a_licence_that_differs_from_the_row_fails(self):
        (self.reg / "zeta-1.0.0/Cargo.toml").write_text(
            '[package]\nname = "zeta"\nversion = "1.0.0"\nlicense = "MIT"\nbuild = false\n')
        self.assertIn("licence is 'MIT'", "\n".join(self.errors()))

    def test_a_build_script_the_row_denies_fails_and_one_it_admits_passes(self):
        (self.reg / "zeta-1.0.0/build.rs").write_text("fn main() {}\n")  # build = false still says no
        self.assertNotIn("build script", "\n".join(self.errors()))
        (self.reg / "zeta-1.0.0/Cargo.toml").write_text(
            '[package]\nname = "zeta"\nversion = "1.0.0"\nlicense = "MIT OR Apache-2.0"\n')  # no key: auto-detected
        self.assertIn("build script is present", "\n".join(self.errors()))

    def test_a_stale_third_party_file_fails(self):
        (self.reg / "eta-2.1.0/NOTICE").write_text("eta notice, changed\n")
        joined = "\n".join(self.errors())
        self.assertIn("THIRD-PARTY.txt: stale", joined)

    def test_the_notices_carry_every_licence_file_verbatim(self):
        text = (self.server / "THIRD-PARTY.txt").read_text()
        for needle in ("zeta 1.0.0", "Licence: MIT OR Apache-2.0", "--- LICENSE-MIT ---", "MIT text",
                       "eta 2.1.0", "--- NOTICE ---", "eta notice", "--- LICENSE ---"):
            self.assertIn(needle, text)
        self.assertNotIn("fn main", text)

    def test_print_digest_prints_one_line_per_row(self):
        orig = check_deps.SERVER
        import io, contextlib
        try:
            check_deps.SERVER = self.server
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = check_deps.main(["--registry", str(self.reg), "--print-digest"])
        finally:
            check_deps.SERVER = orig
        self.assertEqual(rc, 0)
        self.assertEqual(buf.getvalue(), (self.server / "deps.digest").read_text())


class PythonFloor(unittest.TestCase):
    def test_the_version_check_comes_before_tomllib(self):
        import ast
        with open(os.path.join(HERE, "check_deps.py"), encoding="utf-8") as f:
            tree = ast.parse(f.read())
        check = toml = None
        for node in tree.body:
            if isinstance(node, ast.If) and "version_info" in ast.dump(node.test) and check is None:
                check = node.lineno
            if isinstance(node, ast.Import) and any(a.name == "tomllib" for a in node.names):
                toml = node.lineno
        self.assertIsNotNone(check, "no sys.version_info check")
        self.assertLess(check, toml)


if __name__ == "__main__":
    unittest.main()
