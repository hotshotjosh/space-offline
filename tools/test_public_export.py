"""Tests for tools/public-export.py: the scanner and the exported-tree helpers.

Invented strings only; every case builds a throwaway tree. Run::

    python3 -I tools/test_public_export.py
"""

import contextlib
import importlib.util
import io
import os
import shutil
import re
import tempfile
import time
import unittest
import zlib
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("public_export", os.path.join(HERE, "public-export.py"))
pe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pe)


class Scan(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_public_export-")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, data):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode("utf-8"))

    def scan(self, *pats):
        self.put("private/export-scan.txt", "# c\n" + "\n".join(pats) + "\n")
        return pe.leaks(self.root, pe.exported(self.root))

    def test_a_non_utf8_file_is_scanned_as_latin1(self):
        self.put("a.bin", b"\xff\xfe zzquux \x80")
        self.assertEqual(self.scan(r"zzquux"), ["a.bin:1: matches /zzquux/"])

    def test_a_file_name_is_matched(self):
        self.put("docs/zzquux-notes.md", "clean text\n")
        hits = self.scan(r"zzquux")
        self.assertEqual(hits, ["docs/zzquux-notes.md: path matches /zzquux/"])

    def test_a_clean_tree_has_no_hits(self):
        self.put("a.md", "nothing here\n")
        self.assertEqual(self.scan(r"zzquux"), [])

    def test_case_insensitive_and_spelling_variants(self):
        self.put("a.md", "Host ZZHOST\nname Zqaenzzz\n")
        hits = self.scan(r"(?i)\bzzhost\b", r"(?i)zq(ä|a|ae)nzzz")
        self.assertEqual(len(hits), 2, hits)

    def test_built_in_home_path_still_matches_but_invented_users_do_not(self):
        home = "/" + "home/"
        self.put("a.md", home + "zed/x\n" + home + "alice/x\n")
        self.assertEqual(self.scan(r"zzquux"), ["a.md:1: matches /" + pe.BUILT_IN[0] + "/"])


class Credentials(unittest.TestCase):
    """The built-in credential shapes. Every sample is built at runtime and invented."""

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_public_export-")
        self.addCleanup(shutil.rmtree, self.root)

    def hits(self, name, data):
        p = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if isinstance(data, bytes) else data.encode("utf-8"))
        return [h for h in pe.leaks(self.root, [name]) if h.startswith(name)]

    def test_each_shape_is_refused(self):
        samples = {
            "github": "t = " + "ghp" + "_" + "a1" * 18,
            "github-fine": "t = " + "github" + "_pat_" + "Ab1" * 8,
            "webhook": "u = https://" + "disc" + "ord.com/api/web" + "hooks/123456/abc-DEF_9",
            "webhook-v": "u = https://" + "disc" + "ord.com/api/v10/web" + "hooks/123456/abc-DEF_9",
            "key": "-----" + "BEGIN OPENSSH PRIVATE" + " KEY-----",
            "key-block": "-----" + "BEGIN PGP PRIVATE KEY" + " BLOCK-----",
            "aws": "id " + "AK" + "IA" + "ABCDEFGHIJKLMNOP",
            "jwt": "ey" + "Jabcdefgh.ey" + "Jabcdefgh.sig",
            "header": "X-Api" + "-Key: abc",
            "ssh": "ssh-" + "ed25519 " + "AAAA" + "C3NzaC1lZDI1NTE5AAAAIabcdef",
            "ssh-ecdsa": "ecdsa-sha2-" + "nistp256 " + "AAAA" + "E2VjZHNhLXNoYTItabcdefgh",
            "anthropic": "k = " + "sk-" + "ant-" + "a1b2c3d4e5f6g7h8i9j0",
            "slack": "k = " + "xox" + "b-" + "1234567890-abc",
            "google": "k = " + "AI" + "za" + "A" * 35,
            "ptr-shape": "k = " + "ptr" + "_" + "A" * 44,
        }
        for k, v in samples.items():
            self.assertEqual(len(self.hits(f"{k}.txt", v + "\n")), 1, k)

    def test_clean_prose_is_not_a_shape(self):
        self.assertEqual(self.hits("a.md", "send the x-api-key to nobody; ghp is a prefix\n"), [])

    def test_the_tunnel_fixture_may_hold_a_private_key_header_and_nothing_else(self):
        key = "-----" + "BEGIN OPENSSH PRIVATE" + " KEY-----\n"
        self.assertEqual(self.hits("tools/test_tunnel_config.py", key), [])
        self.assertEqual(len(self.hits("tools/other.py", key)), 1)
        self.assertEqual(len(self.hits("tools/test_tunnel_config.py", "AK" + "IA" + "ABCDEFGHIJKLMNOP\n")), 1)

    def test_the_real_tunnel_fixture_holds_exactly_one_header_line(self):
        head = re.compile("-----" + "BEGIN [A-Z ]*PRIVATE" + " KEY(?: BLOCK)?-----")
        with open(os.path.join(HERE, "test_tunnel_config.py"), encoding="utf-8") as f:
            self.assertEqual(sum(1 for line in f if head.search(line)), 1)

    def test_an_evecap_stream_is_inflated_before_the_scan(self):
        secret = "id " + "AK" + "IA" + "ABCDEFGHIJKLMNOP"
        blob = b"EVEOFFLINECAP\n\x01" + zlib.compress(b"junk " + secret.encode()) + b"\x00tail"
        self.assertEqual(len(self.hits("c/a.evecap", blob)), 1)
        self.assertEqual(self.hits("c/b.evecap", b"EVEOFFLINECAP\n" + zlib.compress(b"clean")), [])
        # A binary file of any name is inflated too (a zlib stream inside it is read).
        self.assertEqual(len(self.hits("c/c.bin", b"\0EVEOFFLINECAP\n" + zlib.compress(secret.encode()))), 1)

    def test_gzip_and_utf16_text_are_scanned(self):
        import gzip
        secret = "id " + "AK" + "IA" + "ABCDEFGHIJKLMNOP"
        self.assertEqual(len(self.hits("c/e.dat", gzip.compress(secret.encode()))), 1)
        self.assertEqual(self.hits("c/f.dat", gzip.compress(b"clean text")), [])
        self.assertEqual(len(self.hits("c/g.bin", secret.encode("utf-16-le"))), 1)
        self.assertEqual(len(self.hits("c/h.bin", b"x" + secret.encode("utf-16-le"))), 1)
        self.assertEqual(self.hits("c/i.bin", "clean text".encode("utf-16-le")), [])
        with mock.patch.object(pe, "INFLATE_MAX", 1 << 10):
            self.assertEqual(self.hits("c/j.dat", gzip.compress(bytes(1 << 12))),
                             ["c/j.dat: a zlib stream inflates past INFLATE_MAX bytes; not scanned"])

    def test_gzip_keeps_text_before_a_bad_checksum_and_reads_every_member(self):
        import gzip
        secret = ("id " + "AK" + "IA" + "ABCDEFGHIJKLMNOP").encode()
        good = gzip.compress(b"clean")
        bad = bytearray(gzip.compress(secret))
        bad[-8] ^= 0xFF   # corrupt the CRC after the text was produced
        self.assertEqual(len(self.hits("c/k.dat", bytes(bad))), 1)
        self.assertEqual(len(self.hits("c/l.dat", good + gzip.compress(secret))), 1)

    def test_our_own_bytecode_assembler_may_hold_listings_and_nothing_else_may(self):
        listing = "  12  LOAD" + "_FAST  0 (self)\n  0x18" + "0001234  mov rax, rbx\n"
        self.assertEqual(self.hits("server/crates/eve-server/src/pycode.rs", listing), [])
        self.assertEqual(len(self.hits("server/crates/eve-server/src/other.rs", listing)), 2)
        self.assertEqual(len(self.hits("server/crates/eve-server/src/pycode.rs",
                                       "at " + "C:/build" + "agent/work/x.py\n")), 1)

    def test_client_data_shapes_are_refused_and_prose_is_not(self):
        shapes = [
            "at " + "C:/build" + "agent/work/abc123/x.py line 4",
            "  0x18" + "0001234  mov rax, rbx",
            "/// def Invented" + "Call(self, a, b):  # 123",
            "  12  LOAD" + "_FAST  0 (self)",
            "  LOAD" + "_ATTR 3 name",
            "SHAPE " + "ONLY, plus 12 sample rows",
        ]
        for line in shapes:
            self.assertEqual(len(self.hits("a.md", line + "\n")), 1, line)
        for line in ("a def in prose: def f(self) is described here", "the load order is fine",
                     "SHAPE ONLY is a word", "0x1 is a number"):
            self.assertEqual(self.hits("a.md", line + "\n"), [], line)

    def test_two_streams_in_one_capture_are_both_read(self):
        a = "AK" + "IA" + "ABCDEFGHIJKLMNOP"
        blob = zlib.compress(b"x " + a.encode()) + b"\x00" + zlib.compress(b"y " + a.encode())
        self.assertEqual(len(self.hits("c/d.evecap", b"HEAD" + blob)), 2)

    def test_a_huge_stream_is_one_fail_closed_hit(self):
        blob = b"HEAD" + zlib.compress(bytes(4 << 20))
        with mock.patch.object(pe, "INFLATE_MAX", 1 << 20):
            self.assertEqual(self.hits("c/big.evecap", blob),
                             ["c/big.evecap: a zlib stream inflates past INFLATE_MAX bytes; not scanned"])

    def test_a_truncated_stream_and_a_lone_header_do_not_raise(self):
        full = zlib.compress(b"some text " * 50)
        self.assertEqual(self.hits("c/t.evecap", b"HEAD" + full[:-6]), [])
        self.assertEqual(self.hits("c/h.evecap", b"HEAD\x78\x9c"), [])

    def test_a_stream_with_a_corrupt_checksum_still_reports_its_secret(self):
        secret = "id " + "AK" + "IA" + "ABCDEFGHIJKLMNOP"
        z = bytearray(zlib.compress(b"junk " + secret.encode()))
        z[-1] ^= 0xFF
        self.assertEqual(len(self.hits("c/e.evecap", b"HEAD" + bytes(z))), 1)

    def test_bad_headers_scale_linearly(self):
        # CPU time of this process, so load from other processes does not count.
        t = time.process_time()
        self.assertEqual(self.hits("c/k.evecap", b"\x78\x9c\xff" * 200000), [])
        self.assertLess(time.process_time() - t, 2)

    def test_many_bad_headers_finish_quickly(self):
        t = time.process_time()
        self.assertEqual(self.hits("c/j.evecap", b"\x78\x9c\xff" * 100000), [])
        self.assertLess(time.process_time() - t, 2)


OS = "open" + " source"  # split, so this file does not trip its own check


class Policy(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="test_public_export-")
        for rel in pe.REQUIRED:
            self.put(rel, "invented\n")

    def tearDown(self):
        shutil.rmtree(self.root)

    def put(self, rel, text):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)

    def check(self):
        return pe.policy(self.root, pe.exported(self.root))

    def test_a_clean_tree_passes(self):
        self.put("README.md", "Source available. No money, ever.\n")
        self.assertEqual(self.check(), [])

    def test_a_missing_licence_file_fails(self):
        os.remove(os.path.join(self.root, "LICENSE.md"))
        self.assertEqual(self.check(), ["LICENSE.md: missing from the export (the licence must ship)"])

    def test_a_tree_without_the_licence_additions_fails(self):
        os.remove(os.path.join(self.root, "LICENSE-additions.md"))
        self.assertEqual(self.check(), ["LICENSE-additions.md: missing from the export (the licence must ship)"])

    def test_missing_contributor_terms_and_notice_fail(self):
        os.remove(os.path.join(self.root, "CONTRIBUTING-terms.md"))
        os.remove(os.path.join(self.root, "NOTICE"))
        self.assertEqual(len(self.check()), 2)

    def test_a_licence_file_kept_private_fails(self):
        self.put("private/export-private-paths.txt", "LICENSE.md\n")
        self.assertEqual(len(self.check()), 1)

    def test_a_funding_file_anywhere_fails(self):
        self.put(".github/FUNDING.yml", "invented: x\n")
        self.put("private/sub/funding.yaml", "invented: x\n")
        hits = self.check()
        self.assertEqual(len(hits), 2, hits)
        self.assertTrue(all("funding file" in h for h in hits))

    def test_calling_the_project_open_source_fails(self):
        self.put("README.md", "An " + OS + " server.\nIt is OSI approved.\nAn " + OS.replace(" ", "-") + " tool\n")
        self.assertEqual(len(self.check()), 3)

    def test_quoted_and_negated_mentions_pass(self):
        self.put("docs/a.md", 'Their README says "fully ' + OS + '".\n'
                 "Leave the `" + OS.replace(" ", "-") + "` topic unset.\n"
                 "Source available, not " + OS + ".\n"
                 "This is not an OSI licence.\n")
        self.assertEqual(self.check(), [])

    def test_open_source_claim_fails_main_check(self):
        self.put("README.md", "An " + OS + " server.\n")
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(pe.main(["--root", self.root, "--check"]), 1)
        self.assertIn("README.md:1:", out.getvalue())


class Docs(unittest.TestCase):
    PRIV = ["docs/secret/", "docs/hidden.md"]

    def test_filter_index_drops_private_rows_only(self):
        text = ("| path | role |\n|---|---|\n| `docs/a.md` | GUIDE |\n"
                "| `docs/hidden.md` | PLAN |\n| `docs/secret/x.md` | PLAN |\n"
                "| `docs/guides/b.md` | other mentions docs/hidden.md |\n\nprose\n")
        out = pe.filter_index(text, self.PRIV).split("\n")
        self.assertIn("| `docs/a.md` | GUIDE |", out)
        self.assertIn("| `docs/guides/b.md` | other mentions docs/hidden.md |", out)
        self.assertEqual([l for l in out if "hidden.md`" in l or "secret" in l], [])

    def test_filter_index_drops_a_section_left_with_only_its_header(self):
        text = ("# Index\n\nintro\n\n## Kept\n\n| doc | role |\n|---|---|\n| `docs/a.md` | GUIDE |\n\n"
                "## Emptied\n\n| doc | role |\n|---|---|\n| `docs/secret/x.md` | PLAN |\n\n"
                "## Half\n\n| doc | role |\n|---|---|\n| `docs/hidden.md` | PLAN |\n| `docs/b.md` | RE |\n\n"
                "## Empty on purpose\n\n| doc | role |\n|---|---|\n")
        out = pe.filter_index(text, self.PRIV)
        self.assertNotIn("## Emptied", out)
        self.assertIn("## Kept", out)
        self.assertIn("## Half", out)
        self.assertIn("| `docs/b.md` | RE |", out)
        self.assertIn("## Empty on purpose", out)   # lost no row: kept as written
        self.assertTrue(out.startswith("# Index\n\nintro\n"))

    def test_mentions_lists_a_surviving_prose_mention(self):
        root = tempfile.mkdtemp(prefix="test_public_export-")
        try:
            os.makedirs(os.path.join(root, "private"))
            os.makedirs(os.path.join(root, "docs"))
            with open(os.path.join(root, "private/export-private-paths.txt"), "w") as f:
                f.write("docs/hidden.md\ndocs/secret/\ntools/other.sh\n")
            with open(os.path.join(root, "docs/a.md"), "w") as f:
                f.write("see docs/hidden.md.\nand docs/secret/x.md and docs/public.md\n")
            got = pe.mentions(root, ["docs/a.md"])
            self.assertEqual(got, ["docs/a.md:1: mention: docs/hidden.md",
                                   "docs/a.md:2: mention: docs/secret/x.md"])
        finally:
            shutil.rmtree(root)

    def test_out_writes_the_prefix_file_and_filters_the_index(self):
        root = tempfile.mkdtemp(prefix="test_public_export-")
        try:
            def put(rel, text):
                p = os.path.join(root, rel)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "w") as f:
                    f.write(text)
            put("private/export-private-paths.txt", "docs/hidden.md\ntools/other.sh\n")
            put("docs/README.md", "| `docs/a.md` | x |\n| `docs/hidden.md` | y |\n")
            put("docs/a.md", "a\n")
            put("docs/hidden.md", "h\n")
            for rel in pe.REQUIRED:
                put(rel, "invented\n")
            out = os.path.join(root, "out-tree")
            self.assertEqual(pe.main(["--root", root, "--out", out]), 0)
            self.assertFalse(os.path.exists(os.path.join(out, "docs/hidden.md")))
            with open(os.path.join(out, "docs/README.md")) as f:
                self.assertNotIn("hidden", f.read())
            with open(os.path.join(out, "tools/doc-private-prefixes.txt")) as f:
                lines = [l.strip() for l in f if not l.startswith("#") and l.strip()]
            import hashlib
            self.assertEqual(lines, ["sha256:" + hashlib.sha256(b"docs/hidden.md").hexdigest()])
            self.assertNotIn("hidden", open(os.path.join(out, "tools/doc-private-prefixes.txt")).read())
        finally:
            shutil.rmtree(root)


if __name__ == "__main__":
    unittest.main()
