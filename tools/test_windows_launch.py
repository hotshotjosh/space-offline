#!/usr/bin/env python3
r"""Tests for tools/windows_launch.py (the Windows client launcher).

**Synthetic only.** Every path here is a temp directory and every byte is
invented; nothing reads a client and nothing launches one. The real launch
path (subprocess.Popen) is Windows-only and is not exercised -- these check the
pure file and argv logic, so they pass in the Linux gate container too.

Run::

    python3 -I tools/test_windows_launch.py
    tools/analysis.sh python3 -I /work/tools/test_windows_launch.py
"""

from __future__ import annotations

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import windows_launch as wl  # noqa: E402


class PrefsAppend(unittest.TestCase):
    def test_a_line_appends_to_a_file_with_no_trailing_newline(self) -> None:
        # the trap: prefs.ini ends mid-line, so a naive append glues on.
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "prefs.ini")
            with open(p, "wb") as fh:
                fh.write(b"[chat]\r\nport=26000")  # no trailing newline
            changed = wl.append_line_safely(p, wl.CHAT_TLS_LINE)
            self.assertTrue(changed)
            with open(p, "rb") as fh:
                out = fh.read()
            self.assertEqual(out, b"[chat]\r\nport=26000\r\nchatserver_disable_tls=1\r\n")
            self.assertNotIn(b"26000chatserver", out)  # not glued

    def test_the_append_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "prefs.ini")
            with open(p, "wb") as fh:
                fh.write(b"port=26000")
            self.assertTrue(wl.append_line_safely(p, wl.CHAT_TLS_LINE))
            self.assertFalse(wl.append_line_safely(p, wl.CHAT_TLS_LINE))  # second run no-ops
            with open(p, "rb") as fh:
                self.assertEqual(fh.read().count(b"chatserver_disable_tls=1"), 1)

    def test_editing_backs_the_file_up_once(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "prefs.ini")
            with open(p, "wb") as fh:
                fh.write(b"port=26000")
            wl.append_line_safely(p, wl.CHAT_TLS_LINE)
            bak = p + wl.BACKUP_SUFFIX
            self.assertTrue(os.path.exists(bak))
            with open(bak, "rb") as fh:
                self.assertEqual(fh.read(), b"port=26000")  # the original, untouched


class CommonIni(unittest.TestCase):
    def test_a_missing_common_ini_is_created_with_placebo(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            changed, path = wl.ensure_common_ini(d)
            self.assertTrue(changed)
            with open(path, "rb") as fh:
                self.assertEqual(fh.read(), wl.PLACEBO)

    def test_an_existing_placebo_is_kept(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            wl.ensure_common_ini(d)
            changed, _ = wl.ensure_common_ini(d)  # second run
            self.assertFalse(changed)

    def test_a_common_ini_without_cryptopack_is_not_clobbered(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            with open(os.path.join(d, "common.ini"), "wb") as fh:
                fh.write(b"[main]\r\nsomethingElse = 1\r\n")
            with self.assertRaises(RuntimeError):
                wl.ensure_common_ini(d)


class Command(unittest.TestCase):
    def test_the_argv_carries_the_server_port_token_and_no_crash_upload(self) -> None:
        argv, env = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", "127.0.0.1:8443")
        self.assertTrue(argv[0].endswith(os.path.join("bin64", "exefile.exe")))
        self.assertIn("/server:127.0.0.1", argv)
        self.assertIn("/port:26000", argv)
        self.assertIn("/ssoToken=me:pw", argv)
        self.assertIn("/noCrashReportUpload", argv)
        self.assertEqual(env["EVE_PUBLIC_GATEWAY_ADDRESS"], "127.0.0.1:8443")

    def test_no_gateway_leaves_the_variable_unset(self) -> None:
        saved = os.environ.pop("EVE_PUBLIC_GATEWAY_ADDRESS", None)
        try:
            _, env = wl.build_command("ROOT", "127.0.0.1", 26000, "me:pw", None)
            self.assertNotIn("EVE_PUBLIC_GATEWAY_ADDRESS", env)
        finally:
            if saved is not None:
                os.environ["EVE_PUBLIC_GATEWAY_ADDRESS"] = saved


class PrefsPath(unittest.TestCase):
    def test_the_settings_dir_name_flattens_the_install_path(self) -> None:
        os.environ["LOCALAPPDATA"] = os.path.join(os.sep, "Local")
        p = wl.prefs_path(r"C:\EVE\tq-offline")
        # the client flattens separators to underscores and appends the server
        self.assertIn("127.0.0.1", p)
        self.assertTrue(p.endswith(os.path.join("settings", "prefs.ini")))
        self.assertIn("tq-offline", p.lower())


if __name__ == "__main__":
    unittest.main()
