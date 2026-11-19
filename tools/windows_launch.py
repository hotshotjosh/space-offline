#!/usr/bin/env python3
r"""Launch the native Windows EVE client against a space-offline server (stdlib only).

For the same-machine setup in docs/guides/windows.md: the server runs in WSL2
and the native Windows client reaches it over `127.0.0.1`. This script does the
three client-side steps that guide describes and then launches the client:

  * set `cryptoPack = Placebo` in `common.ini` (an override file, so `start.ini`
    and `manifest.dat` are left untouched -- see docs/guides/jail.md);
  * append `chatserver_disable_tls=1` to the client's `prefs.ini` *safely*
    (the file has no trailing newline, so a naive append glues the line onto
    the previous one);
  * run `bin64\exefile.exe` with the server, port, sso token and
    `/noCrashReportUpload`, with `EVE_PUBLIC_GATEWAY_ADDRESS` in the
    environment so the client dials our gateway and not CCP's.

    py -3 tools\windows_launch.py --root C:\EVE\tq-offline --token me:mypass
    py -3 tools\windows_launch.py --root C:\EVE\tq-offline --token me:mypass --port 26000 --dry-run

It changes only the copied offline client, never the install you play on, and
backs up any file it edits to `<file>.eveoffline-orig` (once). It launches
nothing under `--dry-run`, which prints the command and the gateway variable so
the setup can be checked without a client.

This is the Windows counterpart of tools/run-client.sh. It is NOT a jail: a
native Windows client has no network isolation. Read the no-jail section of
docs/guides/windows.md before you run a server you do not own.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import os
import shutil
import subprocess
import sys

PLACEBO = b"[main]\r\ncryptoPack = Placebo\r\n"
CHAT_TLS_LINE = "chatserver_disable_tls=1"
BACKUP_SUFFIX = ".eveoffline-orig"
GATEWAY_DEFAULT = "127.0.0.1:8443"


def _backup_once(path: str) -> None:
    """Copy `path` to `path.eveoffline-orig` the first time it is edited."""
    bak = path + BACKUP_SUFFIX
    if os.path.exists(path) and not os.path.exists(bak):
        shutil.copy2(path, bak)


def ensure_common_ini(root: str) -> tuple[bool, str]:
    """Write `cryptoPack = Placebo` into `<root>/common.ini`.

    Returns (changed, path). A missing file is created. An existing file that
    already sets Placebo is left alone. An existing file that does NOT mention
    `cryptoPack` is not clobbered -- the caller is told to add it by hand,
    because merging an arbitrary ini safely is out of scope for this tool.
    """
    path = os.path.join(root, "common.ini")
    if not os.path.exists(path):
        with open(path, "wb") as fh:
            fh.write(PLACEBO)
        return True, path
    with open(path, "rb") as fh:
        data = fh.read()
    if b"cryptoPack" in data and b"Placebo" in data:
        return False, path
    raise RuntimeError(
        "%s exists but does not set cryptoPack = Placebo; add it by hand "
        "under [main]" % path
    )


def append_line_safely(path: str, line: str) -> bool:
    """Append `line` to a CRLF text file that may lack a trailing newline.

    Returns True if the file was changed. If `line` is already present the
    file is left alone (idempotent). Otherwise a `\\r\\n` separator is added
    first when the file does not already end in a newline -- the trap this
    function exists for: without it the appended line is glued onto the last
    line (e.g. `port=26000chatserver_disable_tls=1`). Operates on bytes so the
    file's CRLF endings are preserved exactly.
    """
    want = line.encode("ascii")
    if os.path.exists(path):
        with open(path, "rb") as fh:
            data = fh.read()
    else:
        data = b""
    # already there (match the whole line between boundaries)
    for existing in data.replace(b"\r\n", b"\n").split(b"\n"):
        if existing.strip() == want:
            return False
    _backup_once(path)
    out = bytearray(data)
    if out and not out.endswith(b"\n"):
        out += b"\r\n"
    out += want + b"\r\n"
    with open(path, "wb") as fh:
        fh.write(bytes(out))
    return True


def build_command(
    root: str, server: str, port: int, token: str, gateway: str | None
) -> tuple[list[str], dict[str, str]]:
    """Build the (argv, env) for the client. Pure; launches nothing."""
    exe = os.path.join(root, "bin64", "exefile.exe")
    argv = [
        exe,
        "/server:%s" % server,
        "/port:%d" % int(port),
        "/ssoToken=%s" % token,
        "/noCrashReportUpload",
    ]
    env = dict(os.environ)
    if gateway:
        env["EVE_PUBLIC_GATEWAY_ADDRESS"] = gateway
    return argv, env


def prefs_path(root: str) -> str:
    """The client's prefs.ini under %LOCALAPPDATA%.

    The settings directory is named after the install path with separators
    and the dots of the server address flattened to underscores, e.g.
    `e_dl_eve-emu_client-tq_eve_tq-offline_127.0.0.1`. This derives that name
    from `root` and the loopback server; a client launched once writes the
    directory itself, so if this guess is wrong, launch once and point
    `--prefs` at the directory the client created.
    """
    local = os.environ.get("LOCALAPPDATA", os.path.expanduser(r"~\AppData\Local"))
    drive, rest = os.path.splitdrive(os.path.abspath(root))
    flat = (drive[:-1] + rest).replace("\\", "_").replace("/", "_").lstrip("_")
    name = "%s_127.0.0.1" % flat.lower()
    return os.path.join(local, "CCP", "EVE", name, "settings", "prefs.ini")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Launch the Windows EVE client against space-offline.")
    ap.add_argument("--root", required=True, help=r"the offline client install root (holds bin64\exefile.exe)")
    ap.add_argument("--token", required=True, help="ssoToken as user:password (a signup=on listener makes the account)")
    ap.add_argument("--server", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=26000)
    ap.add_argument("--gateway", default=GATEWAY_DEFAULT, help="EVE_PUBLIC_GATEWAY_ADDRESS (default %(default)s)")
    ap.add_argument("--no-gateway", action="store_true", help="do not set the gateway variable")
    ap.add_argument("--prefs", help="path to prefs.ini (default: derived from --root)")
    ap.add_argument("--no-chat", action="store_true", help="skip the chat-TLS prefs line")
    ap.add_argument("--dry-run", action="store_true", help="prepare nothing, launch nothing; print the command")
    args = ap.parse_args(argv)

    exe = os.path.join(args.root, "bin64", "exefile.exe")
    gateway = None if args.no_gateway else args.gateway
    cmd, env = build_command(args.root, args.server, args.port, args.token, gateway)

    if args.dry_run:
        print("would launch:", " ".join(cmd))
        print("cwd:", args.root)
        print("EVE_PUBLIC_GATEWAY_ADDRESS:", env.get("EVE_PUBLIC_GATEWAY_ADDRESS", "(unset)"))
        return 0

    if not os.path.exists(exe):
        print("no exefile.exe at %s -- is --root the install root?" % exe, file=sys.stderr)
        return 2

    changed, common = ensure_common_ini(args.root)
    print("%s common.ini (cryptoPack = Placebo)" % ("wrote" if changed else "kept"))

    if not args.no_chat:
        pf = args.prefs or prefs_path(args.root)
        if os.path.exists(os.path.dirname(pf)):
            wrote = append_line_safely(pf, CHAT_TLS_LINE)
            print("%s %s in %s" % ("added" if wrote else "kept", CHAT_TLS_LINE, pf))
        else:
            print("prefs dir not found (%s); launch once, then re-run or pass --prefs" % os.path.dirname(pf))

    proc = subprocess.Popen(cmd, cwd=args.root, env=env)
    print("launched pid", proc.pid)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
