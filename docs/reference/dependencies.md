# Dependencies: the crate policy

The server was std-only. On 2026-11-19 the owner ended that: crates come in where
they make sense, `getrandom` first, then crypto and TLS for native remote play. This
page is the whole policy. The enforcement is `tools/check_deps.py`; the one-line rule
is in `AGENTS.md`.

## The rule

A crate comes in by an owner decision. It is listed, with every crate it pulls in, in
`server/deps.allow`, and it comes from the build image's apt registry. Our crates keep
`unsafe_code = "forbid"`; a dependency may contain `unsafe`. Each crate is used from
one wrapper module only. Adding a crate changes guarded files, so only a maintainer
does it.

## How crates reach the build

- The registry is the image's `librust-*-dev` packages, never crates.io:
  `server/.cargo/config.toml` replaces crates-io with the directory
  `/usr/share/cargo/registry` and sets `[net] offline = true`.
- `containers/build.Dockerfile` (the gate and `tools/dev.sh`) and `deploy/Dockerfile`
  name the packages. `tools/dev.sh --image` rebuilds the image; it is the only
  networked step. The image also lists its crates in `/etc/eve-offline-crates.txt`.
- `[workspace.dependencies]` in `server/Cargo.toml` is the only place a version is
  named, with `default-features = false` and the features we need. Member crates use
  `workspace = true`.
- The version is whatever Ubuntu packages. Debian patches crates and strips
  dependencies it does not package, so read the crate in the image before relying on
  an upstream claim.
- `Cargo.lock` pins versions (`--locked`). The distro registry carries no checksums,
  and cargo writes the text `Could not get crate checksum` in their place; the checker
  requires exactly that text, so a real-looking checksum is a diff to review. The bytes
  are pinned by `server/deps.digest` instead.

## `server/deps.allow`

Tab-separated, `#` comments, one row per crate in `Cargo.lock`:

    name  version  kind  licence  build  unsafe  used-in

| field | meaning |
|---|---|
| `kind` | `direct` (a row in `[workspace.dependencies]`) or `via PARENT` (pulled in by a row) |
| `licence` | the crate's own `Cargo.toml` `license`; must be accepted (below) |
| `build` | `yes` if the crate has a build script |
| `unsafe` | why it has `unsafe`: `os-ffi`, `cpu-intrinsics`, `asm`, or `none` |
| `used-in` | the one file of ours that names it, relative to `server/crates`; `-` for a `via` crate |

## What `tools/check_deps.py` enforces

Host rules (a pure file check, run by the gate):

1. `direct` rows and `[workspace.dependencies]` match both ways. A `via` row names a
   parent that is a row, and is in `Cargo.lock`.
2. Member crates take `workspace = true` dependencies or a path to another
   `server/crates/*` member, nothing else. The root manifest has no dependency tables.
   Every member has `[lints] workspace = true` and no other `[lints]` entry, and the
   workspace keeps `unsafe_code = "forbid"`, so our code is forbid(unsafe).
3. Every `Cargo.lock` package with a source matches a row on name and version, so a
   version bump is a guarded diff, comes from exactly the crates.io index (which the
   config replaces), and carries the placeholder checksum.
4. No `[patch]` or `[replace]` anywhere.
5. The cargo config keeps offline and the distro directory.
6. No build script in our own crates.
7. Every licence is on the accepted list (`LICENCE_OK`), or the crate is in
   `LICENCE_EXCEPT` by owner decision.
8. A `direct` crate is listed in the manifest of the crate that holds its `used-in`
   file and in no other manifest, so cargo itself refuses the name elsewhere. As a
   second line, a text scan finds no other `.rs` file under `server/crates` naming it
   (`name::` with an optional leading `::` and spaces, `use name`, `extern crate name`),
   and the `used-in` file does not re-export it (`pub use name`, `pub extern crate
   name`). A `via` crate is named nowhere.
9. `server/deps.digest` names the same crates and versions as the rows, and
   `server/THIRD-PARTY.txt` exists.

Registry mode, in the image. The gate runs it before the cargo build, and
`deploy/Dockerfile` runs it in its build stage before `cargo build`, so a crate whose
bytes moved never reaches a build script:

    tools/dev.sh python3 -I /repo/tools/check_deps.py --registry /usr/share/cargo/registry

It takes about two seconds and fails closed: a missing crate directory, an unreadable
`Cargo.toml` or any mismatch is an error, and the exit status is 1. It checks each
row's directory, name, version, licence and build-script flag against the crate's own
`Cargo.toml`, refuses any symlink inside a crate directory (cargo follows them, the
digest does not), the tree digest against `server/deps.digest`, and that
`server/THIRD-PARTY.txt` is what `tools/dep-notices.py` writes now. An image rebuild
that moved a crate's bytes, even under the same version, fails here.

## Admitting a crate

The admitting commit body answers each point.

1. **Need.** It replaces something we would get wrong by hand where that is a security
   or correctness risk: crypto, TLS, OS FFI. Not code that is merely tedious. "Ours
   works and has tests" is a reason to keep ours.
2. **Source.** It is packaged as `librust-<name>-dev` in the pinned Ubuntu base, and
   so is everything it pulls in. A crate that is not packaged needs its own owner
   decision and a vendoring design.
3. **Small closure.** About 15 new crates per adoption [A: the owner picks another
   number]. Every crate it pulls in is a `via` row.
4. **No proc-macro sprawl.** No `syn`, `quote` or `proc-macro2` unless the owner
   accepts it for that crate. Derive frameworks are out by default.
5. **Build scripts** only probe the compiler or compile C or asm that ships in the
   crate. Never a download, never `bindgen` against system headers, never `cmake`.
   The row carries `build = yes`.
6. **`unsafe`** has a known reason and is recorded. Prefer `forbid(unsafe_code)`.
7. **Maintained.** A release in the last 18 months or a finished scope, no open
   advisory that affects our use.
8. **Licence** on the accepted list.
9. **MSRV.** `rust-version` at or below the image's rustc (1.93); a distro crate meets
   this by construction.
10. **One wrapper.** One module of ours imports the crate (rule 8 above). Swapping it
    later touches one file. The wrappers live in `server/crates/eve-crypto`.
11. **Minimal features.** `default-features = false` plus named features.

Steps: add the `librust-*-dev` line to both Dockerfiles, run `tools/dev.sh --image`,
read the crate in the image, add the workspace line and the rows, regenerate the
lock (`tools/dev.sh --lock cargo generate-lockfile --offline`), then regenerate the
two registry files:

    tools/dev.sh python3 -I /repo/tools/check_deps.py --registry /usr/share/cargo/registry --print-digest > server/deps.digest
    tools/dev.sh python3 -I /repo/tools/dep-notices.py --registry /usr/share/cargo/registry > server/THIRD-PARTY.txt

## Licences

Our code is PolyForm Noncommercial 1.0.0 plus `LICENSE-additions.md`. A dependency
stays under its own licence, and its notice ships with the binary.

- **Accepted:** MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, 0BSD, BSL-1.0,
  Unicode-3.0, Unicode-DFS-2016, CC0-1.0, Unlicense, and `OR` combinations with at
  least one accepted term. Apache-2.0 also requires carrying the crate's `NOTICE`
  file; `tools/dep-notices.py` does.
- **The owner decides per crate:** MPL-2.0 (file-level copyleft), and terms with an
  advertising clause (older `ring` code is ISC plus OpenSSL-derived terms: read the
  packaged `LICENSE`).
- **Refused:** GPL, LGPL, AGPL, EUPL, SSPL, CDDL, any other noncommercial or
  source-available licence, and a crate with no `license` field. Our no-money and
  noncommercial terms would be further restrictions on them.
- **Notices:** `server/THIRD-PARTY.txt` holds each crate's name, version, licence and
  its `LICENSE*`, `COPYING*` and `NOTICE*` files verbatim. `deploy/Dockerfile` copies
  it to `/usr/share/doc/eve-offline/`. Binaries handed to friends carry it too.

## Vendoring

Not now. If the distro lacks a crate, the fallback is a networked image step that
downloads the exact `.crate` files named in a lock file in the repo, verifies their
checksums and unpacks them next to the apt registry. Nothing is vendored into git. It
needs an owner decision per crate.

## Windows

[V] The packaged `getrandom` 0.2.15 has no dependency besides `cfg-if`, and `libc` on
unix. Its Windows backend is its own code, with no `windows-sys` crate.
A native Windows build of the server or launcher still needs checking in a throwaway
image: `apt-get install libstd-rust-dev-windows gcc-mingw-w64-x86-64`, then
`cargo check --target x86_64-pc-windows-gnu -p eve-crypto`. It is not in the gate.

## Not yet built

`tools/advisories.py` (a reader for a RustSec snapshot) is planned and not written.
Until it is, check RustSec by hand at each admission, and never run `cargo-audit`,
`cargo-deny` or `cargo-vet` on the host.

## Current crates

`getrandom` (OS randomness, used in `server/crates/eve-crypto/src/rand.rs`), with
`libc` and `cfg-if`. `eve-server` reaches it through `eve_crypto::rand::fill`.
Everything else stays ours: SHA-256 and HMAC, the XMPP framer, HTTP/2, marshal,
inflate and deflate, JSON, the store. Reasons are in the admission criteria: ours works
and has tests, and a crate would add a closure for no risk removed.
