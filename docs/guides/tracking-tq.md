# Tracking Tranquility: how the server follows the client build

**Policy.** eve-offline stays compatible with the client that the Tranquility (TQ) launcher
currently ships, at all times. Each move to a new build is one clean cut-over. We never serve two
release lines side by side, and we never freeze on an old build. On the content side we track what
a client adds (items, ships, modules, rigs, NPCs, sites, missions, the map, SKINs, mechanics)
through CCP's public changelogs and diffs of our own builds, and we update the server, the bots
and the content as we go. A friend's launcher updates on its own, so while the server lags a
build, that friend cannot log in.

This replaces the older "freeze and hold the build" stance. Evidence marks follow the repo: `[V]`
is verified (with a file, row or line), `[I]` is inferred, `[A]` is assumed (with its falsifier).
Sizes: S is under a lane-day, M one to three, L more.

**Which tools exist.** Today: `tools/import-build.sh`, `tools/new-build.sh`, `tools/build-delta.py`,
`tools/port-surface.sh`, `tools/port-recheck.py`, `tools/bundle-diff.py`, `tools/re-pin.sh`,
`tools/client-ci-regress.sh`, `tools/regress-compare.py`,
`tools/content-report.py`, `tools/census.sh`, `tools/tq-lag.py`,
`tools/client-setup.sh refresh` (with `tools/pins/<build>.sha256` for `tools/pin-client.sh`) and
`tools/client-ci/cutover.list`. Planned (each is named where it is used, and tracked on #290):
`tq-watch.py`, `build-triage.py`, `answer-shapes.py`,
`client-ci/fci-surface.py`, `intake-issues.py`, `retire-build.sh`,
`v-audit.py`, and the `recompiled` class of `build-delta.py`. Until a planned tool lands, its
step is done by hand from the same inputs.

## 1. Words

| word | meaning | where it lives |
|---|---|---|
| build | `start.ini` `build` (for example 3707965) | `tools/builds.tsv`, `eve_marshal::build::BUILDS` |
| line | the release line: `codename`, `version`, `region` and machoVersion (V24.02 is 24.02, `ccp`, 496) | `ReleaseLine` in `server/crates/eve-marshal/src/build.rs` |
| served line | the one line the server speaks; one H1 serves one line | `REFERENCE_LINE` |
| pinned build | the build the RE corpus (`out/pyc`, `findings/`, every `[V]`) is anchored to; exactly one | `builds.tsv` status `pinned` |
| served builds | every registered build the server accepts by default: `pinned`, `equivalent` and `provisional` (section 4.2) | `Status::accepted()` |
| TQ build | the newest build the TQ launcher ships | `docs/status/tq-builds.tsv` |
| lag | hours from "TQ build first seen" to "TQ build served" | `tools/tq-lag.py` |
| tier | how big a build's delta is, T0 to T3 (section 3) | `tools/build-triage.py` (planned) |
| intake | the content and code work a build brings beyond login | tracker issues labelled `tq-intake` |

Two invariants, which the registry test enforces:

1. **The pinned build is always on the served line.** A new line therefore means a re-pin at
   cut-over. The bots (`eve-bot` sends `REFERENCE_LINE`), the probe and client CI all follow the
   pinned build.
2. **Every served build is on the served line.** One process, one line: no dual-line serving. Two
   builds of one line may both be served for the hours of a cut-over; that is not dual-line
   serving, because the client accepts any server build not higher than its own [V `build.rs`
   header; the `handshake.rs` test `an_older_client_build_is_refused_but_a_newer_one_is_not`].

## 2. Detection: how we learn a new build exists

| # | source | what it gives | latency | who |
|---|---|---|---|---|
| D1 | CCP's public patch notes (`eveonline.com/news/t/patch-notes`, RSS at `eveonline.com/rss`) | a page per **line** ("Patch Notes - Version 24.02") with dated revisions such as `2026-11-17.1`, and headings like Features & Changes, Defect Fixes, Factional Warfare. **No build numbers.** Expansions also get "Expansion Notes". [V, the public pages, read 2026-11-18] | usually at or before the deploy [I] | anyone |
| D2 | the public ESI status endpoint (`/status/`, field `server_version`) | TQ's current build as a string | minutes after the deploy | a script, network only, no client code |
| D3 | the launcher updating the maintainer's online install | the client's own `start.ini` `build` | when the maintainer next runs the launcher | the maintainer only |
| D4 | a friend reports "cannot log in" | the symptom | too late; D1 to D3 exist to prevent this | anyone |

- `[A]` ESI `server_version` equals the client `start.ini` `build` of the same deploy. Falsifier:
  on the next launcher update, `start.ini` `build` differs from the `server_version` read the same
  day. Until that is checked, D2 is a **trigger** to look at D3, never a value we register.
- Cadence `[I]`: two expansions a year (Catalyst 2025-11-14, Cradle of War 2026-06-05, Fractured
  Skies 2026-11-10, from the public notes index). The line changes with each expansion and
  sometimes between them: 24.01 is dated 2026-11-04 and 24.02 2026-11-10. 24.02 had five dated
  revisions in eight days. Whether each revision shipped a new client build is `[A]`; falsifier:
  `docs/status/tq-builds.tsv` after a month shows fewer builds than revisions. Plan for several
  builds a week after an expansion, and about one a week or fewer otherwise.

**The watcher** (`tools/tq-watch.py`, planned, S) will fetch D2 and D1 with the standard library
only (a generic `User-Agent`, no token) and read the one `start.ini` the maintainer points it at.
It appends to `out/tq-watch/state.tsv` and exits 3 (a source shows a newer build or notes
revision), 4 (the online install holds a build that `builds.tsv` lacks: the import is due), 5
(the notes announce a new line, a T2 warning) or 1 (a fetch failed, never a pass). It never runs
client code.

**A patch day** starts when the watcher fires, or when anyone reads patch notes for a new
revision:

| step | who | what |
|---|---|---|
| 1 | coordinator | ask the maintainer to update the launcher: "TQ build N is out" |
| 2 | **maintainer only** | run the launcher with "download all resources" on, then close EVE and the launcher. No one else runs the launcher |
| 3 | coordinator | `tools/import-build.sh tq-<N> --check`, then `tools/import-build.sh tq-<N>` |
| 4 | coordinator | `tools/new-build.sh tq-<N> --report`, then triage (section 3) and the tier's sequence |
| 5 | lanes | the tier's work items in parallel, each from a build copy |
| 6 | coordinator | the cut-over (section 4), then the intake issues (sections 5 and 6) |

A contributor without the maintainer's installs can still help: read the triage counts on the
patch-day issue (counts and names only) and take `tq-intake` issues.

## 3. Triage: tiers by delta class

**Prerequisite.** Today `tools/build-delta.py` calls a module `changed` when only its
`co_filename`, `co_firstlineno` or `co_lnotab` differ, so a recompile of the whole client looks
like thousands of changes. [I, measured locally on 3670638 to 3707965 (the delta file is not committed): 12,723 modules `changed`
and 1,130 call-surface hits, while only 379 modules change in meaning.] A `recompiled` class
(compare code objects the way `tools/port-recheck.py` does; ignore filename, first line number,
lnotab and the `absolute_import` future flag; emit call-surface rows only for modules changed in
meaning) is a planned change (S-M); exit codes stay 0 equivalent, 3 differs, 1 check mismatch, 2
not an install.

**The classifier** (`tools/build-triage.py BUILD`, planned) reads the delta, the port-surface and
port-recheck outputs, the answer shapes and the bundle diff, and exits 20 (T0), 21 (T1), 22 (T2)
or 23 (T3). A missing input is `skipped`, never a pass. The highest tier that fires wins, and a
higher tier's sequence includes the lower tiers' items:

| tier | name | fires when (any of) |
|---|---|---|
| **T3** | native protocol change | `blue.dll`, `_destiny.dll`, `exefile.exe` or `python27.dll` is `code-differs` (not `signature-only`); `eveMachoNetVersion.machoVersion`, `__HANDSHAKE_SECRET_KEY` or `handShakePaddingLength` changed; a net, marshal, destiny, ballpark or michelle module changed in meaning; the gateway's `eve_public.gateway.*` strings changed; a patch rule that `eve_patch_author.py locate` cannot place |
| **T2** | new release line | `start.ini` `codename`, `version` or `region` differs |
| **T1** | code change, same line | any module changed in meaning; port-surface exit 3; an answer-shapes `SERVED` row; a port-recheck `changed` or `gone` row |
| **T0** | data only, same line | none of the above: resources, identical schema, static data, `recompiled` modules and signature-only natives |

### 3.1 The sequences

Every sequence starts with **S0**, the read-only part, run by the coordinator:

```sh
tools/import-build.sh tq-<N> --check          # exit 0, or MISSING blobs: back to the launcher
tools/import-build.sh tq-<N>                  # copy, link, md5, pin -> out/new-build/<N>/
tools/new-build.sh tq-<N> --report            # steps 1 to 9; stops with exit 10 at each decision
python3 -I tools/build-triage.py <N>          # exit 20..23 = T0..T3 (planned)
```

The census control copy is made **straight after the import, before any patch**
(`docs/guides/build-compat.md` step 8; `tools/re-pin.sh` refuses without `tq-<N>-control`).

**T0: data only, same line.** Target: provisional in 2 h, served on its own bundle the same day.

| step | what | decision point | proof |
|---|---|---|---|
| T0.1 | patch manifest: `eve_patch_author.py locate /client --merge tools/patches/<pinned>.json` to `tools/patches/<N>.json` (`locate --merge` already sets `build`, `identify` and the comment from `start.ini`) | a rule refuses: T3 | `eve_patch_author.py check` exit 0; `tools/test_eve_patch.py` |
| T0.2 | `eve_patch_author.py hosts /client` | a new host: read it. `*.mmdb` blobs are skipped, because the GeoLite databases hold string fragments that look like hosts | exit 0, or a human-read list |
| T0.3 | registry row, status `provisional` (section 4.2): `tools/builds.tsv` and `BUILDS` | none | `the_table_matches_tools_builds_tsv` |
| T0.4 | land T0.1 and T0.3 together | a red gate: fix | the gate stamp |
| T0.5 | bundle: `content.py ... --build <N>`, then `--check --against out/content` | equal `data_hash`: record it and go to T0.7; differs: T0.6 | `content.py --check` exit 0 |
| T0.6 | expected counts (`content_types.EXPECT_BY_BUILD`, `content_dungeons.EXPECT_BY_BUILD`); `bundle-diff.py OLD NEW --uses` | a `REMOVED` or `CLASS` flag on an id we use: it is T1 work | `--check` with no UNVERIFIED note |
| T0.7 | row `provisional` to `equivalent`, with `data_hash`; land | the proof below passed | section 4.4 |
| T0.8 | intake: content report (section 5) to issues | | |

The proof for T0 is the gate, plus the `cutover.list` subset with `EVE_CLIENT_DIR=tq-<N>`, plus a
jailed login. No human is needed when the subset is green.

**T1: code change, same line.** Target: login-capable within 24 h, must-serve within 72 h. The T0
steps, plus:

| step | what | decision point | proof |
|---|---|---|---|
| T1.1 | read `surface.md`, `recheck.md`, the answer shapes and the fci-surface report; sort each row into must, should, ignore or no work (section 6.1); write the table on the patch-day issue | a `must` on the **login path** blocks `provisional`: the row goes to `unverified` and the fix comes first | the sorted table |
| T1.2 | one lane per area for the must items, each with server tests and an fci step | | each lane's tests; the fci step green on `tq-<N>` |
| T1.3 | fci churn: fci-surface rows become scenario fixes (section 6.3) | a scenario that cannot be fixed in 24 h: mark `@skip-<N>` in `regress.list` with an issue | `regress-compare.py` against the last pinned run |
| T1.4 | `shifts.sh` from port-recheck (citation lines only), after reading `recheck.md` | | port-recheck exit 0 after the re-run |
| T1.5 | re-pin within the line? Yes when section 7.1 says so | | section 7 |

**T2: new release line.** Target: login within 24 h of the import, must-serve within 72 h. The T1
steps, plus:

| step | what | decision point | proof |
|---|---|---|---|
| T2.1 | a new `ReleaseLine` const in `build.rs` (macho, version, codename, region, secret, padding, `codename_at_region`) from `start.ini` and the disassembly of `eveMachoNetVersion.pyc` and `GPS.pyc`; the `BUILDS` row is `unverified` | machoVersion, secret or padding moved: T3 | registry tests; a new `handshake.rs` test (H1 carries the new identity, H2 with the old one is refused) |
| T2.2 | the probe and bots take the line from H1 (needed once, then permanent) | | the `TWO_LINES` fixture test |
| T2.3 | must fixes land **before** the cut-over, written to serve both shapes where the client tells them apart (both key sets; arity-based branches) | a fix that cannot serve both waits for the cut-over commit | unit tests for both shapes |
| T2.4 | the **cut-over commit** = the re-pin (section 4.3): `REFERENCE`, `REFERENCE_LINE`, statuses (new `pinned`, old `unverified`), `CFSD_REFERENCE_BUILD`, `tools/client-pin.sha256`, the `out/pyc` and `out/content` swap; then `tools/re-pin.sh tq-<N>` | | gate; `cutover.list` green on the new `tq-offline`; one jailed human login |
| T2.5 | the old line stops being served at this commit. There is no transition listener | | |

A T2 with only an identity change (machoVersion, secret and padding unchanged) is S for the wire;
the time goes into the T1-class must items and the bundle.

**T3: native protocol change.** Best effort, a status every 24 h, typically L.

1. Announce on the patch-day issue: "server lags TQ; friends cannot log in until ...", and post an
   estimate after step 2.
2. A research wave (several read-only lanes): `tools/dis.py` diffs of the changed native, the
   GPS/machoNet handshake, and `docs/reference/login-path.md`, `framing.md` and
   `object-wrapper.md` against the new build. Each lane delivers an implementation spec.
3. Patcher relocation by hand (`tools/client-patch-manifest.py`) for any rule `locate` refused.
4. Wire implementation lanes, then the T2 steps.
5. Proof: an `eve-probe` login against the new line, then `cutover.list`, then a human login.

### 3.2 Target latency, measured

| tier | first served | intake issues filed | must items closed |
|---|---|---|---|
| T0 | 2 h (provisional), same day (own bundle) | 24 h | n/a |
| T1 | 24 h | 24 h | 72 h |
| T2 | 24 h | 48 h | 72 h |
| T3 | best effort; daily status | 48 h | per the research wave |

The clock starts at the first `docs/status/tq-builds.tsv` sighting (D1 to D3), not at the import.
The launcher run is inside the lag on purpose: it shows when the ping arrived late.
`python3 -I tools/tq-lag.py --check` prints each unserved build's lag against its target and exits
3 when one is over; `--record STEP BUILD` sets a cell and never moves one backwards. The status
page carries the "Tracking TQ" block.

## 4. The cut-over playbook

### 4.1 Order of steps (every tier)

1. **Registry and line:** the `builds.tsv` row, the `BUILDS` row, and for T2 the `ReleaseLine`.
2. **Patches:** `tools/patches/<N>.json` (check and test).
3. **Bundle:** extract, check, record `data_hash`; staged where the server will look.
4. **Server code:** the must fixes, landed while still serving the old build (written for both
   shapes).
5. **Proof** (section 4.4) on a patched copy of the new build.
6. **Cut-over commit:** the status flips (and the re-pin for T2), landed through `tools/land.sh`.
7. **Deploy** follows the landing.
8. **Players' copies:** the client kit and `client-setup.sh refresh` (section 4.5).
9. **Second client and CI slots:** `client2-setup.sh --remove` then set up again;
   `slot-prefix.sh remove all` (both are in `re-pin.sh`; for a same-line build, step 9 runs only if
   `tq-offline` changed).
10. **RE corpus:** the re-pin (T2 always; T0 and T1 per section 7.1).
11. **Retire the old build**, after the rollback window (section 4.6).

### 4.2 Status `provisional`

- **Meaning:** a build of the served line that has not been measured beyond triage T0 or T1 with
  no login-path must. It is **served by default** and opens on the line's pinned or equivalent
  bundle.
- **Why:** strict mode (`Config::strict_build`, on whenever accounts are on, which a hosted server
  uses `[I]`) refuses a build outside the accepted set. A friend whose launcher took a hotfix is
  refused until a row lands. `provisional` turns that row into a two-hour step that needs no
  bundle.
- **Code:** `eve_marshal::build::Status::Provisional` (`accepted()` is true, `name()` is
  `provisional`); `content::bundle_accepted` accepts, for a provisional build B, a bundle whose
  `client_build` is a `pinned` or `equivalent` build **on B's line**, and logs once at start
  `serving provisional build B on the bundle of build M`. A provisional row on a line other than
  `REFERENCE_LINE` is refused (invariant 2).
- `[A]` a provisional build served on the previous same-line bundle causes no login-path failure.
  Falsifier: a `cutover.list` red that goes green once the build's own bundle is served.

### 4.3 The re-pin at a cut-over

Use `tools/re-pin.sh tq-<N>` (see "Re-pinning" in `docs/guides/build-compat.md`), with these
points:

- `re-pin.sh` checks that the registry's pinned row is on `REFERENCE_LINE` (it reads `build.rs`)
  and refuses with exit 2 if not, so a T2 re-pin cannot land with half the line change.
- `fsd.py` `CFSD_REFERENCE_BUILD = <N>`; the loader layouts follow build-compat step 5.
- The regenerated tables run in the analysis container in one lane; with the port-surface cache
  warm that is about 10 minutes of `scan_calls` [V `port-surface.sh` header].

### 4.4 Proof before the cut-over: `tools/client-ci/cutover.list`

A `regress.list`-format file that `tools/client-ci-regress.sh --list FILE` reads. It holds the
smallest set that covers the login path and the most-churned UI: the base six, undock and warp,
dock, a market scenario, a fitting scenario, chat, asset safety, the FW enlist scenario, and one
bots row. Every row is copied verbatim from `regress.list`; `tools/test_client_ci_parallel.py`
keeps it so.

```sh
EVE_CLIENT_DIR=tq-<N> tools/client-ci-regress.sh --list tools/client-ci/cutover.list --jobs 4
python3 -I tools/regress-compare.py var/regress-<last pinned run> var/regress-<this run>
```

`tq-<N>` is a patched copy (`eve_patch.py apply --client`); `client-ci.sh` already picks
`out/content-<N>` for a non-pinned build.

| tier | the gate | `cutover.list` | full `regress.list` | jailed human login |
|---|---|---|---|---|
| T0 | yes | yes, no new reds | within a week (nightly) | no |
| T1 | yes | yes | before the must issues close | if a must row is UI-visible |
| T2 | yes | yes, on the re-pinned `tq-offline` | within 48 h | **yes**: log in, undock, open the overview and a market window |
| T3 | yes | yes | yes | yes |

### 4.5 Players' copies

A friend lets the launcher update and then refreshes; nothing holds the build. The kit's target
is the newest **served** build. `tools/client-setup.sh check` says one of three things: "up to
date", "refresh: run `tools/client-setup.sh refresh`" (the install is a newer served build than
the `tq-offline` copy), or "the server has not caught up with build N yet: wait, see the
patch-day issue". `refresh` keeps the old `tq-offline` as `tq-<oldbuild>` and runs `copy` and
`patch` again; it changes only `tq-offline`, refuses before it writes, and does nothing when the
install is not a served build. `tools/pin-client.sh` verifies against the committed
`tools/pins/<build>.sha256` of the install's build (hashes only); add one per served build
(`pin-client.sh --write`). See `docs/guides/remote-clients.md` for the player's side.

### 4.6 Retiring the old build, and rollback

**Rollback window:** the previous build's install, patched copy, `out/pyc-<old>` and
`out/content-<old>` stay until the **next** cut-over has passed its proof. Then
`tools/retire-build.sh <old> [--dry-run]` (planned, S) prints build-compat "Retiring a build"
steps 1 to 3 as edits and deletes the local copies (`tq-<old>`, `tq-<old>-control`,
`out/pyc-<old>`, `out/content-<old>`, `out/new-build/<old>`). It lands nothing itself, and it
refuses when `<old>` is `pinned`, is in the last green `cutover.list` run, or the path is inside
the online prefix. Exit 0 done, 2 refused or usage, 1 a step failed. The
`findings/build-delta-*-<old>.tsv` files stay as history. Until the tool exists, do the steps by
hand and never delete the pinned build's copies.

**Rollback** (the new build is served and something is badly wrong). TQ clients cannot go back, so
rolling back the build is never an option. The options are:

1. **The server code broke:** `git revert` the offending commits and land through `tools/land.sh`.
2. **The new bundle broke:** set the row back to `provisional` and land; the server then reopens
   the line's previous bundle (section 4.2). T0 and T1 only: at T2 the previous bundle is on
   another line.
3. **The world broke** (a migration or a removed type): the maintainer restores the world backup
   taken before the cut-over and sets the build back to provisional, or reverts.
4. **A T2 must is missed:** fix forward. There is no old line to fall back to.

## 5. Content intake ("datamining")

**The report** (`tools/content-report.py OLD_BUNDLE NEW_BUNDLE [--notes FILE] [--uses]`, planned,
M) compares two bundles from the **same extractor commit** (the manifest's `extractor` hash must
match or is missing on either side, else exit 2; `--allow-extractor-mismatch` goes on with a warning in the report), reusing `bundle-diff.py`'s loader. It writes `content-report.md` and
`intake.tsv` (`area`, `kind`, `what`, `count`, `sample_ids` at most 5, `server_readers`,
`in_notes`, `verdict` of data-driven, needs work, notes-only or `review`,
`suggested_title`) under `out/new-build/<N>/content/`. The files hold CCP names and ids, so they
stay under `out/` and are never committed. Exit 0 no differences; 1 differences, none needing
work; 3 at least one needs work or `review` row; 2 usage or an extractor mismatch.

A row is needs work for: a new type group or metagroup; a `REMOVED` id with a `--uses` hit; a
modifier function the server does not know; a dungeon that loses every spawn row; a new NPC
component kind or a component count dropping to zero; an industry check failure; a renamed or
re-owned system named in `content/`; a corp that changed faction; an unpriced reprocess output; and
every new table.

**Patch notes outline.** The notes text is CCP's. We keep a **local** outline only (headings and
one line per item, under `out/tq-notes/<version>.txt`), never committed. Tracker text quotes at
most item names and short phrases from it. The report joins it to changed rows by capitalised
names.

### 5.1 How each kind of addition flows into the game

| addition | lands in | automatic? | the work when it is not |
|---|---|---|---|
| new types, ores, minerals, hulls, modules, charges | bundle `typedata.tsv`; market, industry, reprocess, compression | yes, data-driven | a hard-coded group list refusing a new group: grep `--uses`; fix with a test using invented ids |
| new rigs or rig families | `rigs.rs` (by slot effect) | yes `[I]` | set-bonus semantics: a dogma lane |
| new dogma effects and attributes | bundle; `dogma.rs` | yes, if the modifier funcs are known | a new func or a Python-guid effect: a dogma lane with a client-verified formula |
| NPC loot | `content/pve/*-loot.tsv`, `content/pochven/loot.tsv`, `content/econ/geode-loot.tsv`; bundle `explore/loot` | partly: the bundle's tables come over, our authored tables do not | an issue per authored table that should carry the item. A new item must be makeable or dropped before bots may use it |
| market and NPC seeding | `content/econ/*`; bundle `econ/prices` | prices come with the bundle | an unpriced output: a price fallback; NPC-sold items: `content/agents/lp-overrides.tsv` or `store/offers.tsv` |
| industry (blueprints, reactions) | bundle `industry/*`; `content/econ/recipe-markup.tsv` | yes | a closure failure: fix the check, with a test |
| bot doctrines and fits | `eve-bot` doctrine and fit code (`tools/killfits.py` for real fits) | hulls and modules join the catalog when makeable | two to three weeks after a build with new hulls or modules, re-run `killfits.py` on new archives and check `tools/doctrine-report.py` (a dated follow-up) |
| missions and agents | bundle `agents/*`; `content/agents/rewards.tsv` | yes for rows | a new mission kind or objective: the pve area |
| sites and dungeons | bundle `dungeons/*`; `dungeon_data.rs`; `site_spawn.rs` | they load; nothing spawns a new archetype | a seeding rule per new archetype; a dungeon with no spawns must not complete as "cleared" |
| map (gates, stations, ownership) | bundle `universe/*`; `celestial.rs`, `jump.rs` | yes, the graph is built from data | authored tables naming systems (`content/facwar/warzones.tsv`, `content/npc/region-pirates.tsv`, `content/sov/*`) need a re-check; a kept world gains a new station only on reseed `[I]` |
| SKINs and cosmetics | bundle `skins/*` | yes | none |
| new mechanics (a window, service or push) | section 6 | no | section 6 |

### 5.2 Intake issues

`tools/intake-issues.py out/new-build/<N>/content/intake.tsv --out DIR` (planned, S) writes one
issue draft per (area, theme) group and an `index.tsv`. It never posts: filing is the
maintainer's. The title is a plain sentence of what should become true, for example "V24.02: the
asset safety window lists wraps with the new row keys". The body has: **Build** (number, line,
tier), **Area** and **Verdict** (`must`, `should` or `content`), **What changed in the client**
(counts and public names, no bulk ids), **What the server does today** (reader and effect, with
file and line), **Spec**, **Proof** (unit tests with invented ids; the fci scenario and step),
**Status row**, **Evidence** (`[V]`, the tool and row), and **[A] values** with falsifiers.

Labels: `tq-intake`, one `area/<slug>` (`docs/guides/contributing.md`), and `status/ready` once
the spec is in the body. The patch-day issue is labelled `tq-build` and holds the triage counts,
the must/should table and the lag line; each intake issue says `Part of #<patch-day issue>`.

## 6. Code intake: the surface and behaviour diff

### 6.1 Sorting rows

The sources are port-surface G1 to G6 (`surface.md`, served-and-changed rows first), answer shapes
(section 6.2), port-recheck (`recheck.md`) and fci-surface (section 6.3).

| verdict | rule |
|---|---|
| **must** | a call, push or answer we serve whose arity, answer shape or keys changed in a way that raises or misleads in the client on a path a player can reach with default settings |
| **should** | the same on a rarely reached path, or a silent disagreement with no traceback |
| **ignore** | client code behind a feature flag that defaults off offline; QA and insider calls; server-to-server protos |
| **no work** | client-local scatters, readers of session keys we already set, renames inside the client |

The **login path** is login, character select, station, undock, warp and the HUD: the first rows
of `tools/client-ci/cutover.list`. A must on it blocks `provisional`. Each ignore row gets one line
in the patch-day issue saying why, and no issue.

### 6.2 Answer shapes: `tools/answer-shapes.py` (planned, M)

port-surface G1 sees calls, arity and the first consumer, but not the keys the client reads from
our answers. In 3707965 the `GetItemsInSafetyFor*` row keys were renamed under an unchanged call
(`daysUntilCanDeliverConst` became `daysUntilCanDeliver`, `daysUntilAutoMoveConst` became
`daysUntilAutoMove`, `nearestNPCStationInfo` became `defaultDestinationInfo`, and
`structureTypeID` is new); G1 missed it [V `server/crates/eve-server/src/service/structures/safety.rs`, the row keys we emit; the client side is `[I]`]. The tool will run as port-surface step G7,
in three passes:

- **A, the server key inventory:** every string literal used as an answer key under
  `server/crates/eve-server/src` (tests skipped), identifier-like and four characters or longer.
- **B, the client key census:** per tree, the modules whose marshalled constants hold the key as
  a whole marshalled string. A key read in OLD and nowhere in NEW is `SERVED gone`, listed with
  the new string constants of the code objects that held it as a candidate rename. [I, a
  local stdlib prototype on 3670638 to 3707965: of 2,115 server keys exactly three are gone, all in
  `service/structures/safety.rs`; no false positive.]
- **C, consumer shapes at served call sites:** follow the result in the caller's bytecode
  (unpack, iterate, constant subscript, attribute, `.get`) and diff the shape sets old to new:
  `SHAPE changed`, `KEY added`, `KEY gone`.

Limits: keys built at run time, and answers passed more than one hop through generic dicts.
Exit codes follow port-surface: 0 nothing moved, 3 a row needs a reader, 2 usage, 1 a tool failed.

### 6.3 fci churn: `tools/client-ci/fci-surface.py` (planned, M)

The scenarios run inside the client and reach its internals (`uicore.layer.shipui`,
`sm.GetService('bookmarkSvc')`, module imports). 3707965 added a whole `hud` package (88
modules) [I]; port-recheck checks only the `path.py:N` citations in scenarios. The tool
extracts per scenario and line the imports, service calls, layer chains and `settings.*` keys with
a tokenizer (the scenarios are Python 2.7 source) and checks them against the new pyc tree:
module and names exist, the service is in `client-services.json`, the layer is registered, and
referenced functions are not `changed` or `gone`. Output is `fci-surface.md` and `.tsv` (status
`ok`, `gone`, `changed` or `unresolved`); exit 0, 3 (a row needs a reader), 2, 1. Each `gone` or
`changed` row becomes a scenario fix in T1.3. A scenario that must serve both builds reads
defensively, for example `row.get('daysUntilCanDeliver', row.get('daysUntilCanDeliverConst'))`.

### 6.4 Behaviour diff

When port-recheck reports `changed` on a function we cite as behaviour (a formula, a timer, a
constant), its `recheck.md` row is the work item. The lane re-derives the behaviour from the new
bytecode, then updates the code, the test (invented values) and the `[V]` citation, or downgrades
it (section 7.2).

## 7. Keeping `[V]` claims honest across builds

### 7.1 Re-pin cadence

| event | re-pin? |
|---|---|
| a new line (T2 or T3) | **always**, at cut-over (invariant 1) |
| a same-line build where port-recheck has a `changed` or `gone` row on a cited function or table row, or the surface touches served code (T1) | yes, within 7 days of the cut-over, once its must items are in |
| a same-line T0 build | no. The served build may be ahead of the pinned one; port-recheck between pinned and served must stay exit 0, and the patch-day routine re-runs it |
| 30 days since the last re-pin, and the served build differs from the pinned build | yes (one routine re-pin, so the corpus never drifts) |

Re-pins at a new line, and the routine 30-day re-pin within a line, are standing-approved
(maintainer decision of 2026-11-18). Any other same-line re-pin stays a maintainer decision.

### 7.2 What re-verifies automatically, and what does not

| claim kind | re-verified by | when it moves |
|---|---|---|
| `path.py:N` citations in `server/crates` and fci scenarios | `port-recheck.py` citations | `shifts.sh` fixes lines in code that did not change; changed code goes to a reader |
| table rows of `findings/port-recheck.tsv` | `port-recheck.py` table | `changed` or `gone` goes to the lane named in the row |
| Rust consts against client consts and bundle rows | `port-recheck.py` constants | a reader |
| wire facts (handshake, framing, marshal) | the build-delta protocol rows plus the T3 rules | T3 |
| patch sites | `eve_patch_author.py check` and `locate` | relocation, or T3 |
| content counts | `content.py --check` with `EXPECT_BY_BUILD` | the notes give new counts |
| answer keys we emit | `answer-shapes.py` (planned) | `SERVED gone` is a must |
| citations in docs (`docs/re/*.md`, `docs/reference/*.md`) | **not covered today**; planned: add them to port-recheck's citation roots (#290) | |
| prose `[V]` without a citation | nothing; planned: `tools/v-audit.py` (#290) lists them at re-pin | |

**Rule for a claim that moved:** re-verify it against the new pinned build and keep `[V]` with the
new citation; or downgrade it to `[I]` with `(was [V] at <old build>; moved in <new build>,
recheck.md row X)`. Never leave a `[V]` whose citation `port-recheck` reports `gone`. A `[V]`
with no build named means "verified against the pinned build at the time of the commit"; `git log`
dates it against `builds.tsv`'s evidence column.

## 8. Standing rules and the routine

**Rules every lane follows on a build:**

- **Read-only on installs.** Never launch a client, never give an install network, never write
  into an install, and never touch the online prefix beyond the one `start.ini` read.
- **No client data in git.** Reports, outlines and bundle dumps stay under `out/`. Tests use
  invented ids, builds and lines.
- **Compare like with like:** bundles from one extractor commit, surfaces from one tools commit,
  deltas against the **pinned** build.
- **A must fix serves both shapes** when the client tells them apart (arity, key presence), so it
  can land before the cut-over.
- **Tracker text** (issues, comments, PRs, commit subjects) names builds, lines, public feature
  names, counts and repo paths. It never names local machine paths, host names or maintainer-side
  cleanup.

**Daily** (coordinator; cheap): `tq-watch` (planned; exit 3, 4 or 5 starts a patch day) and
`python3 -I tools/tq-lag.py --check` (exit 3 when a build is over its tier target).

**Patch day**, in order:

1. The watcher fired: ping the maintainer.
2. The maintainer updates the launcher and closes it.
3. S0 (section 3.1).
4. Open the patch-day issue (label `tq-build`) with the triage counts and the tier.
5. Run the tier's sequence, dispatching lanes (`python3 -I tools/pr_overlap.py --paths` first),
   and land.
6. Content report, then intake drafts, then file them.
7. The cut-over (section 4), then `python3 -I tools/tq-lag.py --record served <N>`.
8. Update the `docs/status/progress.tsv` rows that the must items touched; run
   `python3 -I tools/status-page.py`.

**Weekly:** `tools/import-build.sh <dir> --sweep` (the blob md5 check; weekly while builds are
frequent), the nightly full `regress.list`, and `retire-build.sh` for builds outside the rollback
window.

### 8.1 Metrics: `docs/status/tq-builds.tsv` and `tools/tq-lag.py`

`docs/status/tq-builds.tsv` is committed and holds build numbers, lines and UTC times only:
`build`, `line`, `tier`, `seen_utc`, `seen_by` (notes, esi, launcher, report), `imported_utc`,
`provisional_utc`, `served_utc`, `pinned_utc`, `retired_utc`, `issue`, and a `note` that carries
an `[A]` with its falsifier for any time that is not proved. `-` is a step not reached.

- **build lag (h)** is `served_utc` minus `seen_utc`;
- **login lag (h)** is (`provisional_utc` or `served_utc`) minus `seen_utc`;
- **intake open** is the number of open `tq-intake` issues;
- **must age (h)** is the age of the oldest open `tq-intake` must;
- **pin drift** is the number of builds served since the last re-pin.

`python3 -I tools/tq-lag.py --md` prints the "Tracking TQ" block that `tools/status-page.py`
embeds: the current TQ, served and pinned builds, the lag of the last five builds, the median per
tier and the pin drift. The page itself never asks the tracker (it must stay reproducible);
`--md --intake-tsv FILE` adds the open `tq-intake` count per area from a saved
`tools/tracker.py issues` listing, and says "tracker not asked" without one.
