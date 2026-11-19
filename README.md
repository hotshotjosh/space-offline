# space-offline

## What it is

space-offline is an emulator for our favourite online space MMO, written in
Rust, mostly with the standard library: a few allowlisted crates and no `unsafe`
in our own code. It targets one client
build: the `pinned` row of `tools/builds.tsv`. The goal is co-op and PvP with
500 to 1,000 or more player-like bots that mine, fight, build and trade beside
you. The build tools run in Docker and the client can run in a network jail, so
nothing needs installing on the host. This project is not affiliated with or
endorsed by CCP Games.

The repository holds no client files and no copy of CCP's art, sound, game
data files or published Static Data Export (SDE); we do not use the SDE. The
tools read a client you installed yourself, and what they extract stays in
`out/`, which git ignores. Everything the server needs to start is extracted
from your own client install for the pinned build: the universe, item types and
their attributes, skills, NPCs, agents, industry and dungeons all come out of
the client's own static files and ResFiles, in a container with no network
([`docs/guides/content-bundle.md`](docs/guides/content-bundle.md)).

What the repository does hold is our own work: the server code, the tools,
tables we authored for policy and tuning that no client file gives us
(`content/`), and notes on how the client talks to a server, which is what
interoperability needs. Those notes and the code comments describe the
client's behaviour and cite where in the client it is found; some still carry
short excerpts of it. A few small portions come from CCP's MIT-licensed
`carbonengine/destiny` and other permissively licensed sources; [`NOTICE`](NOTICE)
lists each one. The rule for tests is invented ids and rows, never extracted
values.

## Status at a glance

- Feature by feature, built or proved: [`docs/status/README.md`](docs/status/README.md).
  It is generated from [`docs/status/progress.tsv`](docs/status/progress.tsv) by
  `tools/status-page.py`, with charts of each system and what is left by theme.
- To check one feature, grep the table:

```sh
grep -n -i 'mining' docs/status/progress.tsv
```

The status page is the only place for progress figures. This README quotes none.

## Quick start

The full path, with a fix for each failure, is
[`docs/guides/getting-started.md`](docs/guides/getting-started.md). Every flag
and `EVE_*` variable is in
[`docs/reference/launch-and-env.md`](docs/reference/launch-and-env.md).

For now the instructions here and in the guides are for Linux only. A pull
request with instructions for a similar setup on Windows is wanted and much
appreciated.

### Prerequisites

- Linux with Docker, bash, coreutils and `python3` (standard library only, always `python3 -I`).
- For the client: Steam with Proton, and `sudo` for the jail (optional, but
  strongly recommended; see [The jail](#the-jail)).
- EVE Online installed through Steam at the pinned build, with every resource file downloaded.
- About 12 GB free RAM for a build, and 8-11 GB of disk for `server/target`.

### 1. Build the server

```sh
tools/dev.sh --image
EVE_MAX_JOBS=6 tools/dev.sh cargo build --locked --release --workspace
```

Ready: cargo prints its `Finished` line. The binaries are in
`server/target/release/`: `eve-offline-server`, `eve-server`, `eve-bots`, `eve-dns`.
`tools/dev.sh --image` is the only networked step.

### 2. Make the offline client copy

Close EVE first. The server and the tools use a copy in its own Wine prefix,
never the install you play on.

```sh
tools/offline-prefix.sh --link-resfiles
tools/offline-prefix.sh --check
python3 -I tools/eve_patch.py apply --client "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline"
python3 -I tools/eve_patch.py verify --client "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE/tq-offline"
tools/pin-client.sh
```

Ready: `--check` lists the copy and its ResFiles, and `verify` and `pin-client.sh`
exit 0. [Getting-started step 3](docs/guides/getting-started.md#3-make-the-offline-client-copy)
has the `cp -a` that makes the `tq-offline` copy.

### 3. Build the content bundle

```sh
tools/analysis.sh --build
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content
tools/analysis.sh python3 -I /work/tools/content.py \
    --client /client --resfiles /client-root/ResFiles --out /work/out/content --check
```

Ready: one line per section, then `out/content/MANIFEST.tsv` exists and the
`--check` run exits 0. `tools/up.sh` and `tools/serve.sh` re-extract a missing
or stale bundle themselves. Details: [`docs/guides/content-bundle.md`](docs/guides/content-bundle.md).

### 4. Start a dev server

A dev world on loopback, with no client, no jail and no bots. Ctrl-C stops it
and writes the world.

```sh
tools/dev-world.sh none
```

Ready: a `# DEV MODE` banner, then
`eve-server listening on 127.0.0.1:26000 in Gps mode (...)`.
The world is `var/dev/world.dat`; `rm -r var/dev` wipes it. Details:
[`docs/guides/dev-mode.md`](docs/guides/dev-mode.md).

### 5. Set up the jail and launch the client

Once per machine: read what will run as root, install the jail helper, and
write your login.

```sh
tools/install-jail-helper.sh --diff | less
sudo tools/install-jail-helper.sh
tools/account.sh login
```

The first launch signs you up. Quit the client, grant yourself GM with the
server stopped, then launch again.

```sh
sudo -v && tools/up.sh --signup
tools/account.sh grant 'Your Name' gm
sudo -v && tools/up.sh
```

Ready: `== game server accepting after ~Ns`, then the client window. It logs in
with the name in `$HOME/.config/eve-offline/login`. In game, `/whoami` says GM.
`tools/up.sh` builds, sets up the jail, starts the resolver and the server, then
the client, and tears it all down on exit unless `--keep`.

### 6. A second client

```sh
tools/client2-setup.sh
tools/account.sh login --second
sudo -v && tools/up.sh --keep
tools/run-client.sh --second
```

Ready: `== login: <second name>`, then `jail ok`, then a second client window.
The second client has its own prefix and its own login.

### 7. Bots

```sh
sudo -v && tools/bots.sh 3
tools/bots.sh --no-client 5
tools/dev-world.sh world50
tail -f logs/eve-bots.log
```

Ready: `== 3 miners -> 10.137.0.1:26000 (log: .../logs/eve-bots.log)`. The first
form also launches your client. In game, a GM drives bots with `/bots help`,
`/bots 10 allied miners` and `/bots despawn all`
([`docs/guides/gm-commands.md`](docs/guides/gm-commands.md)).

## The jail

The jail is optional, but we strongly recommend it. The client phones home: it
has crash reporting that is always on (Sentry, and the Crashpad handler
`eve_crashmon.exe`), it fetches resources from CCP's servers, and it may make
other calls we have not found. Without the jail, any of that can reach CCP
while you play on this server.

What the jail does: it starts the client in its own network namespace with no
default route. The only address it can reach is the host end of a veth pair,
`10.137.0.1`, on the server's ports (26000 for the game, 5222 for chat, 53 for
our own resolver), and everything else is dropped. So the client cannot reach
CCP, or anything else on your network or the internet; a phone-home attempt
simply fails. The check `jail-verify.sh` proves there is no way out before the
client starts, and `run-client.sh` refuses to launch if there is.

What it does not do: the server can still run code inside the client, as you,
and that code can read and write what the client can see. So connect only to a
server whose owner you trust. The jail also stops the client from downloading
missing resources, so download every resource file before the first jailed
launch.

`tools/up.sh` sets the jail up for you. The pieces, to drive them by hand:

```sh
sudo tools/jail-setup.sh
sudo tools/jail-verify.sh
tools/run-client.sh
sudo tools/jail-teardown.sh
```

Ready: `jail-verify.sh` prints `JAIL OK - no egress path found.` and exits 0.
`run-client.sh` runs the same check and refuses to launch on a leak. A client
against a server on another host goes through `tools/jail-forward.py`, so the
jail stays closed. Without root, use `tools/jail-rootless.sh`. Everything else:
[`docs/guides/jail.md`](docs/guides/jail.md).

## For contributors and their agents

Point your agent at [`AGENTS.md`](AGENTS.md). It is a short guide: pick a task, check
overlap, run the gate, and open a pull request that a maintainer imports.

```sh
python3 -I tools/pr_overlap.py
tools/land.sh --no-push
```

Hard rules:

- No extracted client data in git, tests included; tests use invented ids and rows.
- Crates only through `server/deps.allow`, and no `unsafe` in our code.
- `[A]` marks an assumption, with what would falsify it; an `[A]` value never goes into a test.
- The gate decides, by exit code: `tools/land.sh --no-push`. Never pipe a test command into `&&`.
- A feature change updates `docs/status/progress.tsv` in the same PR.
- No secrets in the repo, an issue, a PR or a log.

More: [`docs/guides/contributing.md`](docs/guides/contributing.md),
[`docs/guides/working-together.md`](docs/guides/working-together.md),
[`docs/guides/issue-tracking.md`](docs/guides/issue-tracking.md).

## Dependencies

The server was std-only until 2026-11. It now takes a few crates, where doing it
ourselves would be the riskier choice. The first is `getrandom`: real OS randomness
on Linux and Windows, which keys, salts and tokens need. The standard library has no
stable API for it. Crypto and TLS crates follow, for remote play with the client's
own encryption.

- Every crate, and every crate it pulls in, is listed with its version and licence in
  `server/deps.allow`, and its bytes are hash-locked in `server/deps.digest`.
  `tools/check_deps.py` checks the table in the gate. Its `--registry` mode checks the
  bytes, the licences and `server/THIRD-PARTY.txt` against the build image's crates;
  it runs before the cargo build, in the gate and in the deploy image's build.
- Crates come from the build image's signed Ubuntu packages. Builds run offline in
  Docker, and nothing is fetched from crates.io.
- Our code still forbids `unsafe`. A crate may use it, for OS calls or CPU
  instructions, and each crate is used from one module only.
- Licences: permissive only (MIT, Apache-2.0, BSD, ISC, Zlib and the like); no
  copyleft. Their notices ship in `server/THIRD-PARTY.txt`.
- Adding a crate is an owner decision. The criteria are in
  [`docs/reference/dependencies.md`](docs/reference/dependencies.md).

## Repository map

| Path | What lives there |
|---|---|
| `server/` | The Rust workspace: the server, the wire format, the bots. A few allowlisted crates. |
| `tools/` | Shell and Python tools: build, content extraction, jail, client launch, client CI, gate. |
| `content/` | Tables we author: policy and tuning no client file gives us. |
| `containers/` | Docker image definitions; the only place network access happens. |
| `docs/` | Guides, reference, status and design notes. Index: `docs/README.md`. |
| `findings/` | Inventories of the client's services and calls, in TSV and JSON. |
| `captures/` | Kept recordings of a real client session, for regression tests. |
| `out/`, `var/`, `logs/` | Generated: content bundle, worlds, logs. Gitignored. |

## Docs map

| Doc | Read it for |
|---|---|
| [`AGENTS.md`](AGENTS.md) | The short guide for contributors and their agents. |
| [`docs/status/README.md`](docs/status/README.md) | What is built and what is proved, feature by feature. |
| [`docs/guides/getting-started.md`](docs/guides/getting-started.md) | From a fresh clone to playing, step by step. |
| [`docs/reference/launch-and-env.md`](docs/reference/launch-and-env.md) | Every launch example, flag and `EVE_*` variable. |
| [`docs/guides/jail.md`](docs/guides/jail.md) | The jail: design, use, and what it does not protect. |

Everything else: [`docs/README.md`](docs/README.md).

## Acknowledgements

We thank EvEJS, an independent EVE server project.

- It was a reference for protocol and behaviour facts, and a hint source.
- Each fact taken from it was verified against the client.
- Its data files are not used as content inputs.
- Our content data (dungeons, sites, spawns, agents, missions, loot) is extracted by our own tools from the client's static files.

We thank Eden (https://github.com/CJ-SPT/Eden, MIT), an independent EVE server emulator.

- It is a hint source for protocol and behaviour facts.
- Each fact taken from it is verified against the client.
- None of its data files are used.

## Licence

This project is source available, not open source. You may use, study, change,
share and run it for free, under the PolyForm Noncommercial License 1.0.0
(`LICENSE.md`) with the additional terms in `LICENSE-additions.md`.

- **No money, ever.** Nobody may receive money, or anything worth money, in
  connection with it: no sales, donations, tips, ads, sponsorship, paid access or
  shared running costs.
- Videos, streams and articles about it are free to make and publish anywhere,
  including on channels that earn money, as long as they sell nothing connected
  with it (`LICENSE-additions.md` 1.5).
- If you share a changed version, or run one that other people connect to, you
  share its source under the same terms.
- The licence: [`LICENSE.md`](LICENSE.md) and
  [`LICENSE-additions.md`](LICENSE-additions.md). The terms for contributions:
  [`CONTRIBUTING-terms.md`](CONTRIBUTING-terms.md).
- Some files carry third-party portions under MIT or zlib terms; see
  [`NOTICE`](NOTICE).
- This project is not affiliated with, endorsed by or sponsored by CCP.
- Releases before this change are under the No Money Share-Alike Licence 1.1.
