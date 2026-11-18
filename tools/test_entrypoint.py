"""deploy/entrypoint.sh picks the content bundle by the reference build.

A fake `eve-offline-server` on PATH (written here) answers `--reference-build`
with an invented number; the roles after the selection are not run (the `*`
arm execs its arguments). Run: `python3 -I tools/test_entrypoint.py -q`.
"""

import os
import shutil
import subprocess
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENTRY = os.path.join(ROOT, "deploy", "entrypoint.sh")
REF = "9900001"  # invented


class EntrypointBundle(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(prefix="entrypoint-test-")
        self.addCleanup(shutil.rmtree, self.d, True)
        self.bin = os.path.join(self.d, "bin")
        os.makedirs(self.bin)
        self.content = os.path.join(self.d, "content")
        os.makedirs(self.content)

    def fake_server(self, body):
        p = os.path.join(self.bin, "eve-offline-server")
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body + "\n")
        os.chmod(p, 0o755)

    def run_entry(self):
        env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"],
                   EVE_CONTENT_ROOT=self.content)
        r = subprocess.run(["bash", ENTRY, "true"], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.stderr = r.stderr
        return r.stdout

    def manifest(self, *parts):
        d = os.path.join(self.content, *parts)
        os.makedirs(d, exist_ok=True)
        open(os.path.join(d, "MANIFEST.tsv"), "w").close()
        return d

    def test_a_per_build_directory_wins(self):
        self.fake_server(f'[ "$1" = --reference-build ] && echo {REF}')
        self.manifest()
        sub = self.manifest(REF)
        self.assertIn(f"content bundle {sub} (reference build {REF})", self.run_entry())

    def test_a_flat_bundle_is_the_fallback(self):
        self.fake_server(f'[ "$1" = --reference-build ] && echo {REF}')
        self.manifest()
        self.manifest("1234567")  # another build's directory is not ours
        self.assertIn(f"content bundle {self.content} (reference build {REF})", self.run_entry())
        self.assertIn(f"expected {self.content}/{REF}/MANIFEST.tsv", self.stderr)

    def test_a_flat_bundle_alone_says_nothing_extra(self):
        self.fake_server(f'[ "$1" = --reference-build ] && echo {REF}')
        self.manifest()
        self.run_entry()
        self.assertEqual(self.stderr, "")

    def test_an_empty_per_build_directory_is_not_a_bundle(self):
        self.fake_server(f'[ "$1" = --reference-build ] && echo {REF}')
        self.manifest()
        os.makedirs(os.path.join(self.content, REF))
        self.assertIn(f"content bundle {self.content} ", self.run_entry())
        self.assertIn("exists without a MANIFEST.tsv", self.stderr)

    def test_an_old_server_binary_without_the_flag_falls_back(self):
        self.fake_server("exit 2")
        self.manifest()
        self.manifest(REF)
        self.assertIn(f"content bundle {self.content} (reference build unknown)", self.run_entry())


if __name__ == "__main__":
    unittest.main()
