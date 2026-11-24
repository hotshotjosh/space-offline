# The jail

These tools network-isolate the modified client, and we recommend them. Inside the
jail the client has no path to CCP: not "is configured not to phone home", but no
route to anywhere except your server.

## What the jail protects, in one paragraph

This is the paragraph to read before you play, and the one the remote-clients page
quotes word for word:

> **The jail stops the client from reaching anything but this server. It does
> not protect your files from this server.** The server can run any Python it
> likes inside the client. That code runs as you, inside the jail: it can read
> and write everything the jail lets the client see, which includes the Wine
> prefix the client lives in, and it can send keystrokes and clicks to other
> windows on your X display. Only connect to a server whose owner you trust,
> and only over the tunnel.

The rest of this document is the detail behind those sentences:

- what blocks the network: [Design](#design);
- what the client can see on disk: [The client's view is an
  allowlist](#the-clients-view-is-an-allowlist);
- how the server runs code in the client: [The server can run code in the
  client](#the-server-can-run-code-in-the-client).

## Design

The client runs in its own network namespace (`eveoffline`) joined to the host
by a veth pair. The namespace has **no default route**. The only address that
exists on the other end is the host side of the veth, and nftables inside the
namespace restricts even that to the ports our own server listens on.

```
  namespace eveoffline                    host
  ┌──────────────────────────┐
  │ exefile.exe (via Proton) │
  │ eve-ns   10.137.0.2/30 ──┼── eve-host  10.137.0.1/30
  │                          │      ├─ 26000/tcp  machoNet
  │ no default route         │      ├─ 5222/tcp   in-game chat (XMPP)
  │ no global IPv6           │      └─ 53/udp+tcp stub resolver
  │ output policy: drop      │
  └──────────────────────────┘      forwarding for 10.137.0.0/30: dropped
```

Three independent layers, so a mistake in one does not open a path:

1. **No route.** Nothing outside `10.137.0.0/30` is routable. This alone is
   sufficient; the rest is belt and braces.
2. **Default-deny egress** inside the namespace — `policy drop` on output, with
   an allow-list of `10.137.0.1` on the game ports, DNS, and ICMP echo.
3. **Forwarding refused on the host.** This one matters: the host almost
   certainly has `ip_forward=1` because Docker and libvirt are installed, so
   without an explicit drop the namespace could route out *through* the host.
   `jail-setup.sh` installs an `inet eve_offline_jail` table at forward
   priority −300 — ahead of Docker's rules — that drops anything touching the
   jail subnet or the veth.

Graphics, audio and input are unix sockets and file descriptors, so a network
namespace does not touch them. That is why X11/Wayland, PipeWire and the DRI
device work inside the jail. It is also a hole: every *other* pathname socket
crosses the namespace too. Before the mount allowlist, the client could reach the session
D-Bus (and through it a running host browser), systemd-resolved and avahi
(a security review found this). The network layers above do not close that. The mount namespace
does: see [The client's view is an allowlist](#the-clients-view-is-an-allowlist).

## Server redirection without touching a client file

The client's install files carry integrity data, so we leave them alone to
redirect the server address. Instead the jail's `resolv.conf` points at a stub resolver on `10.137.0.1` which answers
only for the EVE hostnames — `*.servers.eveonline.com` → our server — and
NXDOMAINs everything else.

`serverInfo._GetServerIP` builds `"{}.servers.eveonline.com".format(serverName)`
and resolves it, so a stub answer is all it takes. No client file is modified,
and DNS is simultaneously proof of isolation: if a name outside our stub ever
resolves, the jail is broken.

### The resolver exists now, and the hostname is confirmed

`eve-dns` (`server/crates/eve-dns`) is the stub: std-only, zero dependencies,
UDP and TCP, answering `*.servers.eveonline.com` and NXDOMAIN for everything
else. It is small enough to read.

The hostname was confirmed against the client rather than assumed: it builds
the server name into a `servers.eveonline.com` host name (China's regions use a
different domain), and a stock install names Tranquility. So the one name that
has to resolve is:

```
tranquility.servers.eveonline.com  ->  10.137.0.1
```

Verified end to end against `dig` — a client implementation we did not write —
with the resolver running on a high port:

| query | result |
|---|---|
| `tranquility.servers.eveonline.com A` | `10.137.0.1`, over UDP and over TCP |
| `servers.eveonline.com A` (the apex) | `10.137.0.1` |
| `tranquility.servers.eveonline.com AAAA` | NOERROR, 0 answers |
| `tranquility.servers.eveonline.com MX` | NOERROR, 0 answers |
| `www.google.com A` | NXDOMAIN |
| `notservers.eveonline.com A` | NXDOMAIN |

That last row is the one worth having: matching is label-wise, so a name that
merely *ends* in the suffix as text does not match.

### On naming the server "Singularity"

It does nothing. `IsLiveServer` is referenced by no module but `serverInfo`
itself, and no client module references `IsTestServer` or `singularity` at all.
The Singularity sandbox was **server-side** — the dev and GM tools came from
the role mask the server granted, not from a flag the client reads. So there is
nothing to unlock here; it has to come from us.

## The client must be patched to play

To play on this server the client must be patched. That is not part of this
repository, and these pages do not describe it; the public guides are the
[new users guide](https://wiki.evemu.dev/wiki/New_Users_Guide#Client_Setup) and
[blue_patcher](https://github.com/stschake/blue_patcher). Always prepare the
**copy** (below), never the install you play on.

## Use

```sh
sudo -v && tools/up.sh         # build, jail, resolver, server, client
sudo -v && tools/up.sh --no-client   # services only
```

`up.sh` tears the whole stack down on exit, including the jail, unless `--keep`.
It refuses to continue if a service dies on startup, and — when `dig` is
present — refuses to launch a client until it has seen the resolver answer
correctly.

The pieces, if you would rather drive them yourself:

```sh
sudo tools/jail-setup.sh      # once per boot
sudo tools/jail-verify.sh     # assert no egress; non-zero exit if leaking
tools/run-client.sh           # launches only if verify passes
sudo tools/jail-teardown.sh   # remove
```

Services run on the **host** end of the veth pair, not inside the namespace:
the namespace has no default route, so the host end is the only address the
client can reach, and nftables narrows that to ports 26000, 5222 and 53.

`run-client.sh` runs `jail-verify.sh` itself and refuses to launch on failure,
so the client cannot be started with a live network path by accident. It passes
`/ssoToken=` to skip the launcher.

## `proton run` returns before the client exits

A failure worth naming, because it reads as a server bug and is not one:

> unable to connect to server

`run-client.sh` used to end with `exec ... "$PROTON" run exefile.exe`. But
`proton run` returns as soon as it has handed the executable to `wineserver`
— the game is still running when that command finishes. `up.sh` treats
`run-client.sh` returning as "the session is over", so its `cleanup` trap
killed `eve-server` and `eve-dns` and ran `jail-teardown.sh`, while the client
was still playing. The client then had no resolver, no server, and no veth,
and said it could not connect.

The symptom is distinctive once you know it: `/var/run/netns/` is empty, no
`eve-server` process is running, and yet an `exefile.exe` is still alive and
still appending to its log. The server log stops after the first couple of
connections because there was nothing left to connect to.

Two changes:

- `run-client.sh` no longer `exec`s. It launches, then polls for the client's
  own process and only returns once that is gone. The pattern it matches is
  built from the client directory name (`tq-offline`), so it waits for *our*
  copy and ignores a real EVE the owner may have open.
- `run-client.sh` also refuses to start if such a client is already running,
  and `up.sh`'s cleanup warns if one somehow outlives it rather than quietly
  pulling the network out from under it.

If you hit this before the fix, the client left behind is unrecoverable —
closing it is the only option, since the namespace it was launched into no
longer exists.

## The host firewall has to let the jail reach our services

This is the subtlety that is easy to miss and bites per-machine. The services
(eve-dns, eve-server) run on the **host** end of the veth, at `10.137.0.1`.
When the jailed client sends a DNS query or a connection there, it arrives as
*input to the host* — and a default-deny host firewall drops it before our
services see it. The client then reports "could not connect" or fails to
resolve, while the services sit there working. On the machine this was first
run on, `ufw` was active and did exactly that.

`jail-setup.sh` detects an active `ufw` (or a raw-nftables input hook) and opens
exactly one hole: input **on the veth interface only** (`ufw allow in on
eve-host`, or an early-priority nft accept for that interface).
`jail-teardown.sh` removes it. If no host firewall filters input, nothing is
changed.

This does **not** weaken the jail. Egress control lives on the *namespace* side
— the output chain's allow-list plus the absence of any default route — and
that is untouched. The host accepting input on the veth only lets the jail
reach `10.137.0.1`; the namespace still cannot address anything else, which
`jail-verify.sh` confirms by failing to reach the host's real LAN address.

(If you would rather not have the setup touch the host firewall at all, the
alternative is to run both the services and the client *inside* the namespace
on loopback, with no veth — then only `lo` exists, egress is impossible by
construction, and no host firewall is involved. That is a cleaner end state and
is noted here as a future option; the current design keeps the services on the
host so their logs and captures land in the repo without a namespace hop.)

## The build is pinned, and now it is checked

`tools/pin-client.sh` records a sha256 of every file in the jailed install and
verifies it. **Why it matters more than it looks:** every `[V]` mark in this
repository is measured against the pinned build — the `pinned` row of `tools/builds.tsv`
(each re-pin moves it; older installs stay on disk under their own `tq-<build>` names). Every constant read from the client, every wire layout,
every formula is anchored to it. Steam moved its own copy once already (to
3628513), and until 2026-10-24 nothing noticed that the two had diverged. Each
new launcher build repeats the re-pin.

```sh
tools/pin-client.sh            # verify (232 files, ~1.2 s)
tools/pin-client.sh --quick    # 10 load-bearing files only
tools/pin-client.sh --write    # re-record, after a deliberate change
```

It reads the offline copy only. With no offline copy at
`$PREFIX/tq-offline` it exits 2 with a message; it never falls back to the
online (Steam) prefix. Name another install with `--path DIR`.

Falsified both ways when it was written: pointed at `tq` it reports
`BUILD MOVED -- manifest pins 3619879, install says 3628513` and exits 1 (the
numbers are those of the day it was written; the manifest pins the `pinned` build now);
pointed at the second prefix it passes, because **`tq-offline-2` is
byte-identical to `tq-offline` across all 232 files**, so one manifest covers
both clients (after a re-pin, rebuild the second client with `client2-setup.sh`).

A failure here means the notes in this repository no longer describe what we
run, or that the copy was replaced by a different build.

## What the jail helper grants

Entering a network namespace, binding port 53 and changing firewall rules need root.
`tools/install-jail-helper.sh` therefore installs one root-owned helper,
`/usr/local/sbin/eve-offline-jail`, plus a sudoers entry that lets you run its fixed
verbs without a password (this is what makes unattended client runs possible). Read
`tools/install-jail-helper.sh --diff` before you install it. Installing it is your
choice; without it, `sudo -v` first and the tools fall back to the repo's own jail
scripts.

**Why the design is narrow.** A passwordless rule on a script or binary that you can
write is the same as handing out root, because anything running as you could edit the
file and have root execute it. So:

- the verbs take no arguments, which makes the sudoers match exact;
- the helper holds its own copy of the jail logic and never sources, executes or reads
  anything from the repo; changing the jail means reinstalling it, and that step is
  where you read the diff;
- the resolver is started as you, with only the capability to bind a low port, not as
  root;
- `client` needs root only to enter the namespace, then drops to you with
  `setpriv --clear-groups --no-new-privs` and runs a command the helper builds from a
  fixed template, never one the caller supplies;
- the helper exits non-zero if it or any parent directory is writable by anyone but
  root.

**What it still grants.** Anything running as your user can, without a prompt: bring
the jail up and down (adding and removing one `ufw` rule and a set of `nft` rules on
the host); bind `10.137.0.1:53`; launch the client inside the namespace with the
allowlisted view of the filesystem; stop the resolver it started; and run the census
(a TUN device the namespace routes into, two repo scripts run as you, nothing reaches
a real network). It does not grant writing outside the jail's own configuration,
installing packages, touching other users or running any repo binary as root, and it
does not survive a reboot.

**What it does not bound.** If your account is in the `docker` or `libvirt` group, it
already has root by another door, so the helper's narrowness does not limit what
software running as you can do. Every launch path drops supplementary groups for the
client (the rootless jail cannot drop them, so it masks the docker and libvirt sockets
instead, and those must fail to connect from inside), and the client refuses to start
if it still holds `docker`, `libvirt`, `sudo` or `lxd`, or can see the docker socket.

**Check an install.** `/usr/local/sbin` and the helper are `root root 755`;
`sudo -l` lists only the helper's verbs; `sudo -n /usr/local/sbin/eve-offline-jail up
extra` must be refused; and both `sudo -n tools/jail-setup.sh` and
`sudo -n ip netns exec eveoffline /bin/sh` must still ask for a password.

## We run a copy, never the install you play on

`EVE_CLIENT` defaults to `.../CCP/EVE/tq-offline` inside the offline
client's own prefix (next section), a copy of `tq` that sits beside that
prefix's `ResFiles` — so `../ResFiles` resolves exactly as the client
expects and no drive mapping changes. 392 MB. `run-client.sh` **refuses** to launch if `EVE_CLIENT`
points at `.../tq`, unless `EVE_OFFLINE_ALLOW_REAL_CLIENT=1`.

Two reasons, and the second is the one that matters:

1. The client has to be modified to play here. Doing that to a live install
   means Steam can revalidate and silently revert it in the middle of a debugging session.
2. **If the jail ever leaked, whatever phoned home would be the client sitting
   next to your real EVE state** — settings, cache, and whatever the prefix
   holds. A copy keeps the game directories apart.

Only the copy is modified. The real install is left stock.

Making the copy by hand is `cp -a .../CCP/EVE/tq .../CCP/EVE/tq-offline` inside
the online prefix, with EVE closed, before `tools/offline-prefix.sh`. A remote player
does not do that by hand: `tools/client-setup.sh copy` does it and then runs
`offline-prefix.sh --link-resfiles`. The prefix copy leaves out the launcher's
login state (`AppData/Roaming/EVE Online`, `AppData/Local/CCP/EVE/Launcher`):
the prefix is read-write for a client the server can drive.

## The client has its own Wine prefix

**A copy of the game directory is not a separate prefix.** Until it got its own prefix,
`tq-offline` sat inside prefix 8500, the Wine prefix the owner's online,
Steam-launched EVE uses, and the jail bound that prefix in read-write. So a
compromised jailed client, or a hostile server driving it (the server can run
Python in the client: [below](#the-server-can-run-code-in-the-client)), could
plant a DLL in the prefix that the online client loads later, outside the
jail. So the offline client gets a prefix of its own.

**Where.** `~/.local/share/eve-offline/prefix`, decided in one place,
`tools/prefix-lib.sh`. Outside Steam's `compatdata` on purpose: Steam's own
prefix management (cleanup on uninstall, "delete compatibility data", a
Proton upgrade) never reaches it. On the same filesystem as the Steam library,
so ResFiles can be hard-linked (below).

**Made once**, by `tools/offline-prefix.sh`, as a copy of 8500:

```sh
tools/offline-prefix.sh                  # a full copy
tools/offline-prefix.sh --link-resfiles  # the same, ResFiles hard-linked
tools/offline-prefix.sh --check          # what is there; changes nothing
```

- It copies everything in `compatdata/8500` (bottle, registry, Wine user
  profile, `tq-offline`, the
  client's settings) **except** the online installs `CCP/EVE/tq` and
  `CCP/EVE/sisi`, which the offline client never runs.
- It only reads 8500. Every write goes to `<dest>.partial`, which is renamed
  into place only once the copy is complete. It refuses while a Wine or Proton
  process is using 8500, refuses an existing copy unless given `--force`
  (and then only a copy it made itself, which it recognises by its
  `eve-offline-prefix.txt`), and refuses a copy with a symlink that leads
  back into 8500.
- **Size.** On one test machine (ext4, 2026-10-26) prefix 8500 is 57G, and
  48G of that is `ResFiles`. ext4 has no reflinks, so a full copy costs about
  56G. `--link-resfiles` hard-links the store instead and costs about 8G. On
  btrfs or xfs, `cp --reflink=auto` makes the full copy almost free.
- **Hard-linked ResFiles are the online store's own inodes**, so the jail
  must not be able to write them. It cannot: both jails bind
  `<client>/../ResFiles` **read-only on top of** the read-write prefix (a
  read-only entry inside a read-write one is no longer skipped as covered),
  and `jail-client.sh` refuses to launch if ResFiles is writable from inside.
  The second client has always run with a read-only ResFiles.

**Why an imported build is a real copy and its blobs are hard links.**
`tools/import-build.sh` brings a later online
build into the offline prefix. The install directory is a real copy: the
client writes `cache:` inside it and a
hard-linked install would write through to the online one. The ResFiles blobs
are hard links, because the store is 48G on a disk that cannot hold two. Only
names matching `^[0-9a-f]{2}/[0-9a-f]{16}_[0-9a-f]{32}$` are linked, never over
an existing blob, each checked against the index (size, md5) first. The cost of
sharing an inode is that a launcher that rewrites a blob in place changes ours
too, so `--sweep` re-checks the md5 monthly. The tool never names the copy
`tq` or `sisi`, refuses while a Wine or Proton process uses 8500, and reads
8500 only.

**Every jailed launch path uses it.** `run-client.sh`, `up.sh`,
`client-ci.sh` (sudo and `--rootless`) and `client-headless.sh` default to
it (`EVE_PREFIX_ID=offline`); `--second` is still prefix 8501, which
`client2-setup.sh` now fills from the offline prefix and whose `ResFiles`
symlink it repoints there. The tools that read the client (`pin-client.sh`, `analysis.sh`,
`check-resources.sh`, `client-log.sh`, `evidence.sh`, `destiny-ref.sh`) point
at the copy too.

**The online prefix is invisible in the jail**, not even read-only. Four
places refuse it, each on its own:

| where | refuses |
|---|---|
| `run-client.sh`, `client-ci.sh`, `client-headless.sh` (`tools/prefix-lib.sh`) | a prefix, client or ResFiles under `*/steamapps/compatdata/8500` |
| the root helper's `bind_back` (`eve-offline-jail.in`) | any view entry that is, is inside, or **holds** the online prefix (a prefix of `~/.local/share/Steam` would bind it), checked after the path is resolved and before anything is mounted |
| `jail-rootless.sh`'s `bind_back` | the same, for the rootless CI jail |
| `jail-client.sh`, inside the jail | the same paths again, the online prefix being visible at all, and a writable ResFiles |

The root helper is the one that matters, since the launch-env it reads is
yours to write. It changed, so **reinstall it**: `sudo
tools/install-jail-helper.sh` (until then `install-jail-helper.sh --check`
says STALE, `run-client.sh` falls back to sudo with the current template, and
`client-ci.sh` refuses).

**Limits.** The "holds" check knows where your own Steam library is
(`~/.local/share/Steam`); a library elsewhere is caught by its
`steamapps/compatdata/8500` path only. `client-headless.sh smoke` runs a
client with no network but no mount jail either: it sees your whole home,
online prefix included, and is a display test with no server to drive it.
The old `tq-offline` inside 8500 is no longer used; removing it is safe and
yours to do.

## Why the egress probes aim at your own LAN

`jail-verify.sh` makes **real connection attempts** from inside the namespace —
you cannot prove a jail is sealed without trying to escape it. That makes the
choice of target a decision rather than a detail, because a probe that leaks is
a packet genuinely sent.

So every target is on this machine or this LAN: the default gateway, `docker0`,
the libvirt bridge, and the host's *other* addresses (the allowlist names
exactly one, so reaching any other means it is not holding). Plus a UDP probe
to `10.137.0.1:123`, which tests the **port allowlist** rather than the routing
table — a different failure mode.

This is also the stronger test. The host has `ip_forward=1` (docker, libvirt),
so the realistic leak is the namespace being routed out *through* the host onto
the LAN, which is exactly what these probe. The namespace has no default route
and no LAN route, so a public address and a LAN address fail in the same place.

An earlier version probed `1.1.1.1`, `8.8.8.8`, `example.com` and **Sentry's
address — which is CCP's crash-reporting endpoint**, so a leak there was a leak
to CCP. Replaced. The DNS checks are safe as they stand: they resolve through
our own stub, which has no upstream at all, so a query never leaves the machine
even when it names a CCP host.

## The client's view is an allowlist

The allowlist is on both launch paths. It has not yet been proved running as
root; a human check that it holds is the proof still owed.

**How it is built.** The helper's `client` verb runs `ip netns exec NS
unshare --mount --propagation private`, then `build_view`:

1. Each allowlist entry is vetted (absolute path, resolves, right type, owned
   by the caller or root) and staged before anything is hidden.
2. An empty tmpfs goes over `/run`, `/var/run`, `/tmp`, the caller's `$HOME`
   and the runtime dir.
3. Each entry is bound back at its own host path, `nosuid,nodev`, read-only
   where the table says so. `/dev/shm` is replaced with an empty one.

| entry | mode | why |
|---|---|---|
| `/run/udev` | ro | input devices (controllers) |
| the jail's `resolv.conf` | ro | DNS goes only to our stub |
| `$XDG_RUNTIME_DIR/pulse/native`, `pipewire-0` | rw | audio |
| `$XDG_RUNTIME_DIR/wayland-N` | rw | display |
| `$XAUTHORITY` | ro | X cookie |
| `/tmp/.X11-unix/X<N>` | rw | display |
| Proton's directory | ro | Proton |
| `STEAM_COMPAT_DATA_PATH` (the prefix) | rw | the offline client's own prefix, never 8500 |
| `EVE_CLIENT`, `../ResFiles` | rw, ro | the game; ResFiles read-only on top of the prefix |
| `tools/jail-client.sh`, `out/client-ci/launch-env` | ro | the shim and its settings |
| `tools/client-launch-env.tsv` | ro | the fixed launch environment |

**It refuses to launch** when:

- `scan_view` finds any socket in `/run`, `/tmp`, `/dev/shm`, `$HOME` or the
  runtime dir that is not on the list, or any of docker, the system bus,
  resolved, avahi, snapd, cups or libvirt;
- the client user, after `setpriv --clear-groups --no-new-privs`, still
  holds `docker`, `libvirt`, `sudo` or `lxd`, or `/run/docker.sock` exists.
  `jail-client.sh` repeats this check, so a stale helper is caught too.

`WINEDLLOVERRIDES=winebrowser.exe=d` stops the browser chain before it
starts.

**Both launch paths get the same view.** The non-helper branch of
`run-client.sh` generates the helper for this checkout and runs its own
`v_in_jail_client`, so there is no second allowlist to drift.
`install-jail-helper.sh --check` reports an installed helper that is stale
or belongs to another checkout, and `client-ci.sh` then refuses to run.

**What the allowlist does not stop:**

- **X11 input injection.** The client talks to your X server, so it can send
  keystrokes and clicks to any other X11 window, and remote-control an
  already-running X11 browser. Nothing here prevents that. Wayland-only
  sessions narrow it; this repo does not enforce one.
- **Files outside the masked trees.** The client can still read anything you
  can read outside `/run`, `/tmp`, `/dev/shm`, `$HOME` and the runtime dir:
  other disks, `/etc`, `/srv`.
- **A world-writable socket outside those trees** is not scanned. A one-off
  `sudo find / -xdev -type s -perm -o=w` shows whether any exists.
- ~~The prefix is shared with the online client~~ Fixed: the client
  has [its own prefix](#the-client-has-its-own-wine-prefix), and every
  jail refuses the online one.

## The server can run code in the client

*Corrected 2026-10-25.* This section used to say "There is no code
execution", citing `debug.Eval` and `debug.Exec`. **That was wrong.** Those
two do disarm themselves on a client, but the
server has two other ways to hand the client a code object that it runs,
and our own server uses both:

- **A character-settings reply** after login, which the client treats as code.
- **A handshake field**, which the client evaluates once its check is satisfied.

So **a server, or anyone who can rewrite its traffic, can run arbitrary
Python in the client.** It runs as the client user, inside the jail. On a
patched client the traffic is not protected, so an on-path attacker can do this too.
That is why we strongly recommend that a remote player reach a server only through the
SSH tunnel, never over a raw internet path: this is a real security risk.

Besides that, `machoNet.HandleMessage` on a client dispatches any inbound
`CallReq` to the exported set: 103 classes declare `__exportedcalls__`, 81
non-empty, 608 methods (our census of the exported calls, not published). The client makes no
role check on calls from the server. That channel is bounded, but with
`GetSettingsInfo` available the bound no longer matters.

**What bounds a compromised server is therefore the jail, not the client.**
The code it runs can:

- reach nothing on the network but this server;
- read and write what [the allowlist](#the-clients-view-is-an-allowlist)
  shows it: the prefix, the game directory, audio and display;
- read what the allowlist does not mask;
- inject input through X11.

That is the plain statement at the top of this document.

## What the jail can and cannot record, and the census

The jail stops a phone-home; it does not see most of them. With no route,
`connect()` to anything outside `10.137.0.0/30`, and to any IPv6 address,
fails with ENETUNREACH **before** every netfilter hook, so an attempt at an
IP literal leaves no trace at all. The `counter` at the end of each jail
chain sees only traffic to `10.137.0.1` on a port we did not allow. And
there is no kernel log: nf_log is suppressed in every network namespace but
the init one (`net.netfilter.nf_log_all_netns=0`), so the rate-limited
`log` rules the chains used to carry could never write a line. They are
gone; `tools/jail-drops.sh` reads the counters and the resolver's log.
"Nothing in `eve-dns.log`" therefore means "no name was looked up", not
"nothing was attempted".

The census ([census.md](census.md)) is what records the rest: for the length of a
run it gives the namespace default routes (v4 and v6) into `census0`, a TUN
device read by a sink that opens no socket. A TUN device has no wire, so
the jail still reaches nothing; every attempt becomes a recorded packet.
The helper's verbs are `census-up`, `census-down`, `census-dump` and
`census-verify` (rootless: `jail-rootless.sh --census`). While a census is
up, `verify` fails by design (it asserts the /30 is the only route); it
runs before `census-up` and again after `census-down`, and must pass both
times. In census mode the client's view also gets the browser stand-ins
and the canary script (read-only) and the two census logs the client's side
appends to; the sink's and the proxy's own logs stay outside it.

## What is verified, and what is not

**Verified end to end, 2026-10-19.** `jail-setup.sh` and `jail-verify.sh` have
now actually run, as root, on a test machine. Every assertion passed.
This section used to describe a design; it now reports a result.

```text
== egress must be impossible (targets are all local to this machine/LAN)
  ok    blocked the LAN gateway (192.0.2.1:53)
  ok    blocked the LAN gateway (192.0.2.1:80)
  ok    blocked docker0 on this host (172.17.0.1:53)
  ok    blocked libvirt bridge (192.168.122.1:53)
== the host's OTHER addresses must be unreachable
  ok    blocked the host's LAN address (192.0.2.20:22)
  ok    blocked the host's LAN address (192.0.2.20:26000)
== udp egress must be impossible too
  ok    blocked udp 10.137.0.1:123 (only 53 is allowed)
== dns must not resolve anything outside our stub
  ok    cannot resolve sentry.io
  ok    cannot resolve clientresources.eveonline.com
== there must be no default route
  ok    no default route
== no global IPv6 address
  ok    no global IPv6

JAIL OK - no egress path found.
```

The host-address checks are the sharpest of these: the allowlist names exactly
one address, and the host has several, so `192.0.2.20:22` being unreachable
from inside while the namespace can still reach `10.137.0.1:26000` is the
allowlist doing precisely what it claims. The UDP check matters separately —
every TCP probe above fails at *routing*, while UDP to `10.137.0.1:123` is a
routable address on a non-allowlisted port, so it tests the nftables policy
instead.

What that run did **not** exercise: the jail with a client actually inside it
generating traffic. Egress was proven impossible with the namespace idle.

**Not covered:** the client cannot download resources it is missing, because
`clientresources.eveonline.com` is unreachable by design. The install must be
complete before it goes in the jail. Crash reporting (Sentry via `exefile.exe`,
and the Crashpad handler `eve_crashmon.exe`) is native and always on, and is
stopped by the jail rather than by patching — it will try, and fail, silently.
