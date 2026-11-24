# Build compatibility: adding, checking and retiring a client build

**The reference is the `pinned` row of `tools/builds.tsv`** (`eve_marshal::build::REFERENCE`);
that table names every registered build and its status, so this page does not repeat them.
Older builds stay registered as `unverified`: seen, kept, refused unless `--client-build`
names them. Several `provisional` rows can be open at once (a second hotfix before the first
is measured); each is served on the pinned bundle until it is measured. A newer launcher build
may need a re-pin: see "Re-pinning" below. The server supports the pinned build and
any registered build whose status is `pinned`, `equivalent` or `provisional`. The
standing policy (follow the TQ build, one clean cut-over per move, no dual-line serving) is
`docs/guides/tracking-tq.md`; this page is the procedure it calls. This is the
repeatable procedure for a newer build. One command does the read-only part:

```sh
tools/new-build.sh tq-new            # a dir name under the EVE/ prefix, or a path
tools/new-build.sh tq-new --dry-run  # print what each step would run
```

It stops (exit 10) at each step that needs a decision and prints what to do;
run it again afterwards. It never launches the client; the analysis container
has no network and mounts the installs read-only.

Some steps use maintainer-side tools and tables that are not published:
`build-delta.py` and the client-call, client-callable and push-event tables under
`findings/`. Those steps run on the maintainer's side.

## The procedure

1. Install the new build as a **separate** dir beside `tq-offline`. Update
   the online client with the launcher (download all resources on), close
   EVE and the launcher, then `tools/import-build.sh tq-<name> --check` and
   `tools/import-build.sh tq-<name>`. It copies `tq` out of prefix 8500 as a
   real copy, hard-links the ResFiles blobs the indexes name, checks each new
   blob's size and md5 against the index, lists any MISSING blob and exits
   non-zero, runs `check-resources.sh`, and writes the pin and a provenance
   record to `out/new-build/<build>/`. Once a month,
   `tools/import-build.sh tq-<name> --sweep` re-checks the md5 of its blobs
   (a launcher run can rewrite a shared inode). Never launch the copy, never
   give it network. A second client of it: `EVE_SECOND_SRC_DIR=tq-<name>
   EVE_SECOND_CLIENT_DIR=tq-<name>-2 tools/client2-setup.sh`. Tests:
   `tools/test_import_build.py`; the lane note is the unpublished import-tool lane note.
2. Hash manifest, read-only: `tools/pin-client.sh --write --path DIR
   --manifest out/new-build/<build>/pin.sha256`. The script does this.
3. Delta against the **pinned** build (the baseline, not the previous new
   one): `build-delta.py` writes `findings/build-delta-<pinned>-<build>.tsv`.
   Exit 0 equivalent, 3 differs, 1 check mismatch, 2 not an install.
4. If `differs`, read the reason rows: a protocol file means re-diff
   the native protocol libraries; call surface means compare
   `findings/client-calls.tsv`, `client-callable.tsv`, `push-events.tsv`,
   `session-keys.tsv` and implement the calls; schema means
   `tools/pyd_schema.py` against `docs/content/schema.md`. The script stops
   here unless given `--go-on`.
5. Loader layout and counts, **before the data**: `content.py` refuses a
   build with no `fsd.CFSD_BUILD_LAYOUTS` entry (`schema_guard`). Regenerate
   only the changed loaders as an overlay,
   `pyd_schema.py tsv --client /client --tables T1,T2 --out
   findings/pyd-schemas-<build>.tsv`, recheck the presence-bit fixes by hand and
   add the line. Row counts live in `content_types.EXPECT_BY_BUILD` and
   `content_dungeons.EXPECT_BY_BUILD`; a build in neither is **unverified**:
   `content.py --check` notes each count ("UNVERIFIED build") instead of
   failing, and the closures still run. After the first extraction read the
   notes and add the build's rows.
6. Register: one row in `tools/builds.tsv` and one entry in
   `eve_marshal::build::BUILDS`; the test `the_table_matches_tools_builds_tsv`
   fails if they disagree. Commit the delta TSV with it. This also comes
   before the data: `content-lib.sh` refuses an unregistered build.
7. Data: `tools/content.py ... --build <build>`, then `--check --against`
   the old bundle. Equal `data_hash` lets the server open the old bundle.
8. Hosts: check that every host the new build contacts is in the reviewed list. **Make the census control copy now**, from the stock files:
   once the install is modified there is no stock copy.
9. Gate: `EVE_MAX_JOBS=6 tools/dev.sh cargo test --locked --workspace`,
   clippy `-D warnings`, then `tools/land.sh --head`.

`--report` also runs `tools/port-surface.sh`, `bundle-diff.py` and
`port-recheck.py` when they exist and prints their exit codes. Exit 3 from
`port-recheck.py` means read `out/new-build/<build>/recheck/recheck.md`: the
table rows, citations and constants that moved. Its `shifts.sh` is a comment-only
list of `sed` lines for citations in code that did not change.

Then the human check: launch a copy of the new build, jailed, log
in, undock, open the overview and a market window, compare with the pinned
build.

## Registry fields per build (`tools/builds.tsv`)

| field | meaning |
|---|---|
| build | start.ini `build` |
| line | release line (start.ini `codename`); a line shares machoVersion, boot.version, codename and region |
| status | `pinned` (exactly one; the RE corpus is anchored to it), `equivalent` (the delta says so and the human check passed), `provisional` (a build of the served line not yet measured beyond triage: served by default on the line's pinned or equivalent bundle, same line only; registered only with its loader layout and counts, so the content tools take it too, and a player whose launcher has it can build a bundle), `unverified` (seen, refused unless `--client-build` names it) |
| data_hash | a bundle `data_hash` proved equal for this build, or `-` |
| evidence | how the status was earned (findings file, date) |

Beside the row: `findings/build-delta-<pinned>-<build>.tsv`.

## Provisional builds

`provisional` is for a build of the **served line** (the line of `REFERENCE_LINE`) that has been
triaged as T0 or T1 with no must on the login path, and not yet measured further. The server
serves it by default, on the bundle of a `pinned` or `equivalent` build of the same line. When
the served set leaves out the bundle's own build (`--client-build` naming only provisional
builds), it logs `serving provisional build B on the bundle of build M` at start (`builds B1, B2`
when more than one is served); a default run, which serves the bundle's build too, does not. It is a two-hour step that
keeps a friend whose launcher took a hotfix from being refused. Rules:

- same line only: the registry test refuses a `provisional` row on another line, and requires
  every accepted build to be on `REFERENCE_LINE` and exactly one row to be `pinned`;
- it is a stopgap: measure it (steps 3 to 7 of the procedure, then the `cutover.list` proof) and
  move the row to `equivalent`, or back to `unverified` when a must is found on the login path;
- if its own bundle breaks something, setting the row back to `provisional` reopens the line's
  previous bundle (T0 and T1 only; at a new line the previous bundle is on another line);
- a new line is never `provisional`: it is a re-pin.

## Release lines

A new **line** (new codename, e.g. V24.02) is more than a row: it needs a new
`ReleaseLine` and handshake identity in `eve-marshal`/`handshake.rs`, because
the handshake checks (H1/H2/H6/H8) are made against the line. A new build in
an existing line needs only the row.

## The gate

`tools/gate.sh --env-checks` (not the landing gate: it depends on this machine's
installs) runs `tools/new-build.sh --gate-check`: for each committed
`findings/build-delta-<old>-<new>.tsv` whose two installs exist on this
machine it runs `build-delta.py --check` in the analysis container (exit 0 or
3 means the committed rows reproduce; anything else fails the gate, meaning
an install changed). It checks every delta, not only those against the pinned
build, so the older `build-delta-3619879-*.tsv` files stay as history and are
still verified. With either install absent it prints a note and skips.

## Retiring a build

`tools/retire-build.sh <old> [--dry-run]` (`docs/guides/tracking-tq.md` section 4.6)
prints steps 1 to 3 below as edits and deletes the local copies (`tq-<old>`, `tq-<old>-control`,
`out/pyc-<old>`, `out/content-<old>`, `out/new-build/<old>`); it lands nothing and refuses the
`pinned` build, the build in `tq-offline`, the rollback-window build (no registered build lies
between it and the pinned one) and anything under the online prefix. Retire a build only after
the **next** cut-over has passed its proof: until then the previous build is the rollback window.

1. Delete its row in `tools/builds.tsv` and its entry in
   `eve_marshal::build::BUILDS` together (the table test checks both).
2. Keep `findings/build-delta-*`: history; the gate skips a delta whose install is gone.
3. Never retire the `pinned` build; re-pinning means moving the whole RE
   corpus and is an owner decision. Nothing was retired at the 2026-10-31
   re-pin: 3619879 and 3628513 became `unverified`, with their pins and
   deltas.

## Re-pinning

**When.** A new line (T2, T3): always, at the cut-over, because the pinned build must be on the
served line. A same-line build: within 7 days of its cut-over when port-recheck has a `changed`
or `gone` row on a cited function or table row, or the surface touches served code (T1), once
its must items are in; not for a T0 build (port-recheck between the pinned and the served build
must stay exit 0); and once routinely when 30 days have passed since the last re-pin and the
served build differs from the pinned one. Re-pins at a new line, and the routine 30-day re-pin within a line, are standing-approved
(maintainer decision of 2026-11-18). Any other same-line re-pin stays a maintainer decision.

Done for 3662168 (issue #19) and 3670638 (issue #131); `tools/re-pin.sh`
runs these steps. It refuses (exit 2), before it changes anything, when the registry's pinned
row is not on the line that `REFERENCE_LINE` in `build.rs` names, so land the line change with
the re-pin. The coordinator's steps, in this order:

1. Apply the lane's diff (`tools/builds.tsv`, `build.rs`, the regenerated
   `findings/`). The gate stays red until step 4 lands `tools/client-pin.sha256`
   with it.
2. Swap the client copies (**check first that `tq-<old>` does not exist**: a
   `tq-3662168` dir exists in the prefix today, and `mv` would nest `tq-offline` inside it; rename that one aside): `tq-offline`
   becomes `tq-<old>` and the new copy becomes `tq-offline` (renames only, never into compatdata/8500). The
   scripts' default dir stays `tq-offline`, so nothing else is edited.
3. Prepare the new `tq-offline` copy the way the old one was.
4. `tools/pin-client.sh --write`, then commit and land (`tools/land.sh --head`).
5. `out/pyc` to `out/pyc-<old>`, then extract the new `code.ccp` into `out/pyc`
   (from the stock file, so no modified constant enters a finding); `out/content` to `out/content-<old>`, then rebuild
   `out/content` with `content.py` for the new build.
6. The second client: `tools/client2-setup.sh --remove` and set it up again.

The regenerated tables use these tools: `census-calls.sh` (`scan_calls`),
`scan_push.py --old-pyc <old> --seed findings/push-events.tsv` (keeps the
hand-curated cells), `scan_client_callable`, `scan_session_keys`,
`scan_proto_messages`, `scan_bound` (with `--compare docs/reference/bound-objects.md`),
`scan_remote`, `scan_services`, and `pyd_schema.py` for the changed loaders
only. In `fsd.py` the new build's loader layout becomes the base
the unpublished pyd-schemas table; the old one moves to an overlay file
`pyd-schemas-<old>.tsv` and `CFSD_BUILD_LAYOUTS` swaps.
