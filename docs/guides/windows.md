# The Windows client

[getting-started.md](getting-started.md) runs the client on Linux, in the jail,
through Proton, or on a separate Windows machine. This is a third shape: **one Windows PC**, the server in WSL2 and the
**native Windows client** talking to it over `127.0.0.1`. It is what the
README's "a similar setup on Windows is wanted" asks for.

What it gives you, verified on build 3707965 (V24.02; the pinned build is now
3720952, the same line, not yet re-run here): sign-up and login, undock,
in-game chat, and the AIR new-player experience from character creation through
scene 2.4. What it does
**not** give you is the one thing the Linux path does — a network jail. A native
Windows client has no namespace to put it in. [No jail on Windows](#no-jail-on-windows)
is not an appendix; read it before you point the client at a server you do not own.

Paths on the WSL side are relative to the repository root in your WSL home.
Paths on the Windows side are yours; this guide writes them as `C:\EVE\tq-offline`.
Every flag and `EVE_*` variable is in
[launch-and-env.md](../reference/launch-and-env.md).

## 1. Prerequisites

The client comes from you, as in [getting-started.md](getting-started.md):
nothing in this repository downloads it. On a stock Windows 11 PC, in order:

- **WSL2 with Ubuntu.** A stock Windows 11 install has no WSL (`wsl --status`
  says "not installed"). In an administrator PowerShell run `wsl --install`,
  reboot, and create the Ubuntu user when it asks. Ubuntu 24.04 or newer
  ([getting-started.md](getting-started.md#supported-platforms)).
- **The Linux toolchain, inside WSL.** The server, the build and the content
  extraction run in WSL exactly as [getting-started.md](getting-started.md)
  describes; nothing in the server changes for Windows. Inside the Ubuntu
  shell you need `git` (for the clone), `python3`, and Docker. **Docker
  Engine installed in the distro** (`/usr/bin/docker`, the
  `docker.io` or Docker CE packages) is what this guide was run with; Docker
  Desktop's WSL integration is not verified here. Confirm the clone builds
  first (getting-started.md §2).
- **A Windows EVE install, at an accepted build.** Get the launcher from
  `https://www.eveonline.com/download`. Its installer is signed by **Fenris
  Creations hf.**, not "CCP ehf"; that is expected. Install EVE with it and let it
  download every resource file. The build must be the `pinned` row of
  `tools/builds.tsv`, or one the registry marks `equivalent`
  ([build-compat.md](build-compat.md)). The launcher installs the current
  Tranquility build, so a fresh install matches once the registry has caught
  up ([tracking-tq.md](tracking-tq.md)). Check it:

  ```
  type "C:\path\to\EVE\tq\start.ini" | findstr /b build
  ```
- **Log the game into Tranquility once.** Start the game from the launcher
  with an EVE account (a free Alpha account is enough) and get as far as
  character selection. Some client data is not in the install at all: the
  client downloads it from CCP on its first Tranquility login and keeps it in
  its user cache under `%LOCALAPPDATA%\CCP\EVE\` (for example the Career
  Program achievement definitions, `cache\Storage\achievements\data.bin`).
  Without that cache the achievements are empty, offline as well.
- **WSL2 forwards loopback.** A server that binds `127.0.0.1` inside WSL2 is
  reachable at `127.0.0.1` from Windows, and the reverse — this is what makes
  the same-box setup work with no tunnel. The three ports below all arrive this
  way.

## 2. Run the server in WSL2

Build the server and the content bundle in WSL per
[getting-started.md](getting-started.md) §2 and §4, then run it **headless** —
no jail, no Linux client — and leave it running. A dev world is the quickest:

```sh
# in WSL, at the repo root
tools/dev-world.sh none        # dev server on 127.0.0.1:26000, no bots
```

Three things matter for the Windows client:

- **The gateway (port 8443) must be on.** Without it the client's achievements,
  goals and licences silently fail. `tools/dev-world.sh` turns it on for the
  main world automatically (it adds `--gateway` on port 26000); if you start
  `eve-offline-server` another way, pass `--gateway` yourself. The Windows
  client is pointed at it in step 6. See
  [launch-and-env.md](../reference/launch-and-env.md) (`--gateway`).
- **It binds `127.0.0.1`.** The listeners come up on loopback — `26000` (game),
  `5222` (chat), `8443` (gateway) — which WSL2 forwards to Windows. A listener a
  new player signs up against needs `signup=on`
  ([dev-mode.md](dev-mode.md), [hosting.md](hosting.md)).
- **An unverified build** needs its own bundle and `--client-build <N>` on the
  server ([content-bundle.md](content-bundle.md)); a pinned build needs neither.

Leave this running and switch to Windows for the rest.

## 3. Make the offline client copy

As on Linux ([jail.md](jail.md#we-run-a-copy-never-the-install-you-play-on)),
run a **copy**, never the install you play on — the client is modified
(`cryptoPack`), and a live install can be revalidated and reverted under you.

With EVE closed, copy the install directory *beside its ResFiles* so the
client's `../ResFiles` still resolves:

```
C:\path\to\EVE\tq           ->  C:\EVE\tq-offline
C:\path\to\EVE\ResFiles      (left where it is, shared)
```

The copy only needs `tq`; it shares the one `ResFiles` store, which is tens of
GB and is not worth duplicating. Keep `tq-offline` and `ResFiles` under a common
parent so `tq-offline\..\ResFiles` points at it, the same layout the launcher
assumes.

## 4. cryptoPack = Placebo, in common.ini

The client has to be told to stop encrypting against CCP's key:
`cryptoPack = Placebo` makes `EncryptedRead`/`EncryptedWrite` the identity
([jail.md](jail.md#one-client-file-does-have-to-change)).

On Linux this means editing `start.ini`, which `manifest.dat` covers, so the
Linux path also has to re-hash or neutralise the manifest check. **On Windows
there is a cleaner way:** put the setting in a new `common.ini` at the install
root, which the client reads as an override and which the manifest does **not**
cover. `start.ini` stays stock, the manifest still matches, and no `blue.dll`
patch is needed.

`C:\EVE\tq-offline\common.ini`:

```ini
[main]
cryptoPack = Placebo
```

The launcher in step 7 writes this file. **Do not** edit `start.ini` by hand to
do it — that trips the manifest check and the client refuses to start (the trap
is the same one [jail.md](jail.md#getting-placebo-past-the-manifest-check)
documents on Linux).

## 5. The chat-TLS line in prefs.ini

In-game chat (XMPP) otherwise tries TLS and fails against the local server. Add
`chatserver_disable_tls=1` to the client's `prefs.ini`.

The file is under `%LOCALAPPDATA%`, in a directory named after the install path
and the server address with separators flattened to underscores, e.g.

```
%LOCALAPPDATA%\CCP\EVE\c_eve_tq-offline_127.0.0.1\settings\prefs.ini
```

The client writes this directory the first time it launches, so if it is not
there yet, launch once and then add the line.

**The trap:** `prefs.ini` has no trailing newline. A plain "append the line"
glues it onto the last line — once this produced `port=26000chatserver_disable_tls=1`
on one line, and chat stayed broken with no error. Add a newline first, or let
the launcher do it (it appends on a byte boundary and backs the file up to
`prefs.ini.eveoffline-orig`).

## 6. Log in with a sign-up token

The native client is launched straight past the official launcher with a token:

```
bin64\exefile.exe /server:127.0.0.1 /port:26000 /ssoToken=<user>:<password> /noCrashReportUpload
```

Against a listener with `signup=on`, a token for an unknown user **creates** the
account; against one without, the user must already exist
([hosting.md](hosting.md), [dev-mode.md](dev-mode.md)). `/noCrashReportUpload`
turns off Sentry crash uploads — the first of the two phone-homes the no-jail
section relies on. Run the client with its **working directory set to the
install root**, so `..\ResFiles` resolves.

The client also needs the gateway variable in its environment:

```
set EVE_PUBLIC_GATEWAY_ADDRESS=127.0.0.1:8443
```

Without it the client dials CCP's `{tier}-public-gateway.evetech.net` and the
goals/achievements/licence calls go nowhere (and, were there a route, to CCP) —
see [phone-home.md](../reference/phone-home.md) (N4). With it they go to our
`--gateway`.

## 7. The launcher

`tools/windows_launch.py` does steps 4, 5 and 6 in one go — stdlib only, no
dependencies, so it fits the repo's rules and the gate tests it:

```
py -3 tools\windows_launch.py --root C:\EVE\tq-offline --token me:mypass
py -3 tools\windows_launch.py --root C:\EVE\tq-offline --token me:mypass --dry-run
```

It writes `common.ini`, appends the chat line to `prefs.ini` safely (backing it
up first), and launches `exefile.exe` with the flags above and
`EVE_PUBLIC_GATEWAY_ADDRESS` set, cwd at the install root. `--dry-run` prepares
and launches nothing and prints the command and the gateway variable.

| Flag | Meaning |
|---|---|
| `--root DIR` | the offline install root (holds `bin64\exefile.exe`) |
| `--token user:pass` | the sso token; a `signup=on` listener makes the account |
| `--port N` | server port (default `26000`) |
| `--gateway host:port` | `EVE_PUBLIC_GATEWAY_ADDRESS` (default `127.0.0.1:8443`) |
| `--no-gateway` | do not set the gateway variable |
| `--prefs PATH` | prefs.ini path, if the derived one is wrong |
| `--dry-run` | prepare nothing, launch nothing, print the command |

It edits only the copy, backs up every file it changes to `.eveoffline-orig`,
and never touches the install you play on.

## No jail on Windows

The Linux client runs in a network namespace with **no route to anything but
this server** ([jail.md](jail.md)). There is no equivalent here. A native
Windows client runs with your full network access, and two facts from the Linux
docs carry over unchanged and matter more without the jail:

- **The server can run code in the client.** Under `Placebo` a server — or
  anyone who can rewrite its traffic — can hand the client Python that it runs,
  as you ([jail.md](jail.md#the-server-can-run-code-in-the-client)). This is
  true on every platform. On Linux the jail bounds what that code can reach on
  the network; on Windows nothing does. **Only connect to a server whose owner
  you trust, and never over a raw internet path** — the traffic is plaintext.
- **The client phones home.** `/noCrashReportUpload` stops Sentry and
  `EVE_PUBLIC_GATEWAY_ADDRESS` keeps the gateway pointed at us, which covers the
  two loudest ([phone-home.md](../reference/phone-home.md), N4 and Sentry). Other
  calls in that document have no route blocked, because nothing isolates the
  network.

### The optional firewall rule

You can get part of the jail's egress control back with a host firewall rule.
Windows Firewall does not filter loopback, so an **outbound block** on the
client binary stops it reaching the internet while `127.0.0.1` still works. In
an elevated PowerShell:

```powershell
New-NetFirewallRule -DisplayName "eve-offline: client no egress" -Direction Outbound `
  -Program "C:\EVE\tq-offline\bin64\exefile.exe" -Action Block -Profile Any
New-NetFirewallRule -DisplayName "eve-offline: crashmon no egress" -Direction Outbound `
  -Program "C:\EVE\tq-offline\bin64\eve_crashmon.exe" -Action Block -Profile Any
```

Remove them with `Remove-NetFirewallRule -DisplayName "eve-offline: *"`.

Be clear about what this is and is not:

- it blocks the client's **own** outbound sockets, so a local server still
  works and a phone-home does not;
- `eve_crashmon.exe` is the Crashpad handler, a separate process, so it needs
  its own rule ([jail.md](jail.md) names it);
- it is **weaker than the namespace jail**. It is a host firewall rule, not
  isolation: anything running as you — including code the server runs in the
  client — can add, remove or route around it, and it does not touch other
  processes. It is a backstop against the client's routine phone-homes, not a
  boundary you can trust a hostile server not to cross.

The launcher does not create this rule (it needs admin and is a policy choice);
it is here for you to apply by hand.

## The display mode is saved, and it can save the wrong one

Run the client fullscreen at your monitor's native resolution. The client keeps
its fullscreen mode in
`%LOCALAPPDATA%\CCP\EVE\<settings dir>\settings\core_public__.yaml`, as
`device: FullScreenSettings: [<stamp>, {adapter, height, presentInterval, width}]`,
and it saves whatever size its window last had. Two things have made it save a
smaller one: a tool resizing the game window (the client takes the new size as
its mode), and the displays going to sleep and waking while the client ran (it
came back minimized at 1280x720). Either way the monitor itself switches to the
smaller mode at the next launch, and the UI layout is squeezed to it.

The fix, with the client closed: set `width` and `height` in that entry back to
the native resolution (back the file up first), then relaunch. Window positions
laid out at the small size stay wrong until **Esc > Reset Settings > Reset to
Default Window Positions**. Raising the Windows display-sleep timeout (Settings >
System > Power) keeps the second cause away during long sessions.

## What is verified, and what is not

**Verified on build 3707965 (V24.02):** sign-up and login, undock, in-game chat,
and the AIR new-player experience through scene 2.4, with the server in WSL2 and
the native client over loopback. The rest of act 2 needs the AIR NPE work still
in progress (#205). The `common.ini` Placebo route and the prefs-line trap are
what this setup actually used. Not yet re-run on the pinned 3720952.

**A fresh-PC run, partial (19NOV2026):** Windows 11 Home with no WSL, git or
Docker and no EVE install. It measured the gaps §1 now covers: `wsl --install`,
the toolchain inside WSL, where the launcher comes from and who signs it. It did
not go past §1.

**Not covered here:** the Windows firewall rule has been reasoned, not run on a
fresh machine end to end; treat it as the backstop described, not a proven jail.
`tools/windows_launch.py` is tested on its file and argv logic only (the gate
runs on Linux); its launch path is the one this setup used by hand.
