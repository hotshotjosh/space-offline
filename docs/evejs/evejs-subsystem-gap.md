# What EvEJS implements that we do not

A **scope map**, not a design document. It lists what the EvEJS fork has code for
and what we have, so a lane can see where the gaps are. The operating constraints
of `docs/archive/evejs-261018-reference.md` §0 still apply: this file names
subsystems and paths, never code. Where a row looks useful, the next step is the
**client's** bytecode, not their source. EvEJS is AGPL; only facts are kept here.

> **Rewritten 2026-11-07** (issue #210). The 2026-10-20 and 2026-10-21 versions
> compared a file listing of the 261018 archive with our tree at `139c0ad`. They
> called our market, contracts, corps, structures and space handling stubs or
> "none". That is wrong now: those are client-proved. This page is rebuilt from a
> fresh comparison of two trees and from `docs/status/progress.tsv`.

Contents: 1 what was compared, 2 the headline, 3 subsystems in both, 4 gaps
(theirs only), 5 ours only, 6 caveats, 7 history.

---

## 1. What was compared

| side | tree | client build |
|---|---|---|
| EvEJS CIEFA fork | commit `df3ca41`, 2026-10-22 | 3396210 |
| eve-offline | master at `1bc58793`, 2026-11-07 | 3670638 (the `pinned` row of `tools/builds.tsv`) |

Nothing was run on either side. The fork's tree was read as files. Where the old
notes in `docs/evejs/` disagreed with the fork, the fork won. Upstream EvEJS may
have moved since 2026-10-22; that was not checked.

Our states below are the words in `docs/status/progress.tsv`: `client-proved`,
`done`, `built-unproved`, `partial`, `stubbed`, `missing`, `parked`, `unknown`.
EvEJS has no such grades. For it we say only that code exists and roughly how big
it is.

---

## 2. The headline

**EvEJS is wider. We are proven.**

- It registers about 202 machoNet services; we register about 149. About 145 are
  in both. (It counts names in `super("...")` calls; we count the `SERVICES`
  lists with comments removed. Both are breadth counts, not work estimates.)
- It has code for almost everything the client has UI for. It goes deeper than
  we do on Upwell structure combat, moon mining, structure services and
  industry, skyhooks, mercenary dens, sov hub upgrades, abyss depth, Pochven,
  mutaplasmids, time dilation, POS, and a set of live-service extras.
- Our table at this commit has 268 feature rows: 179 `client-proved`, 8 `done`,
  41 `built-unproved`, 13 `partial`, 2 `stubbed`, 17 `missing`, 3 `parked`,
  2 `unknown`. The proof comes from about 200 automated real-client scenarios
  under `tools/client-ci/scenarios/`. EvEJS has no client CI; its proof is hand
  checks and headless gateway sessions.
- For PvP and bots we lead: two-client PvP, CONCORD, kills and pods, doctrines,
  camps, dictors and HICs are client-proved here. Our bot crate is far larger,
  and runs hundreds of bots in one system.

---

## 3. In both

Both sides have these, and ours are `client-proved` unless noted. The old page
listed several as stubs. They are not.

| area | our state | EvEJS paths (facts only) |
|---|---|---|
| Login, session, characters | client-proved | `server/src/network/tcp/`, `services/login/`, `services/character/` |
| Stations, docking, inventory | client-proved | `services/station/`, `services/inventory/` |
| Fitting and dogma | client-proved; rig caps and stacks `built-unproved` | `services/dogma/`, `services/fitting/` |
| Space and destiny (undock, warp, gates, autopilot) | client-proved | `server/src/space/destiny/` |
| Combat, ewar, tackle, drones, fighters | client-proved | `space/combat/`, `space/modules/`, `services/drone/`, `services/fighter/` |
| Fleets | client-proved; reconnect `built-unproved` | `services/fleets/` |
| Corps and alliances | client-proved; ads and votes `partial` (#152, #153) | `services/corporation/` |
| Market and contracts | client-proved, including two-client trade | `services/market/`, `services/contracts/`, `services/trade/` |
| Industry (manufacturing, invention, reactions) | client-proved | `services/industry/` |
| Mining, PI, missions, exploration, wormholes | client-proved | `services/mining/`, `services/planet/`, `services/agent/`, `services/exploration/` |
| Sov claim, campaigns, entosis; Upwell anchor, dock, relog, clone, revive | client-proved | `services/sovereignty/`, `services/structure/` |
| Chat, mail, calendar, skills, New Eden Store, PLEX | client-proved | `services/chat/`, `services/mail/`, `services/skills/`, `services/newEdenStore/` |
| Persistence | `done` (`docs/reference/world-state.md`) | `server/src/gameStore/` |

---

## 4. Gaps: what EvEJS has and we do not

Ranked by value to our goal (co-op and PvP with many player-like bots).
Each is a hint to re-derive from the pinned client, never from their code. The
tracker issue numbers are the ones to claim.

| # | gap | our state (progress.tsv) | issues | EvEJS paths |
|---|---|---|---|---|
| 1 | Structure combat and reinforcement: hit points, lock, reinforce timers, vulnerability windows, destruction loot | `missing` (structure state machine and timers are `partial`) | #40, #43-48 | `structure/structureVulnerabilityService.js`, `structureState.js`, `structureDestructionLootState.js` |
| 2 | Structure service modules; industry in structures; fuel and deed bays | modules `missing`; industry in structures `missing`; fuel bays `partial` | #41, #42, #56-59 | `structure/structureServiceModules.js`, `structureIndustryJobs.js` |
| 3 | Moon drills and Metenox | `missing` | #52-55 | `structure/moonExtractionsService.js`, `autoMoonMiningService.js`, `metenox*.js` |
| 4 | Sov extras: hub upgrades, mercenary dens, skyhooks | upgrades `stubbed`, dens `stubbed`, skyhooks `missing` | #77-84 | `services/sovereignty/` (`skyhook*.js`, `mercenaryDenState.js`, `sovHubUpgradeInstall.js`) |
| 5 | Abyss depth: later rooms, weather, loot, tiers | `partial` | #5, #6, #62 | `server/src/abyssal/` |
| 6 | Mutaplasmids | `missing` | #68-71 | `services/dogma/dynamicItemService.js` |
| 7 | Pochven and Triglavian or Drifter NPCs | `missing` | #72-74 | `space/pochvenPolicy.js`, `space/npc/trigDrifter/` |
| 8 | Proving Grounds. Their side is thin (a queue and a leaderboard) | `missing` | #63-67 | `activity/pvpFilamentMgrService.js` |
| 9 | Tick and per-system metrics, wider GM breadth | admin web page `built-unproved`; 37 GM commands | #201 | `monitoring/`, `services/chat/commands/` |
| 10 | Daily goals | no progress.tsv row | #202 | `services/dailyGoals/` |
| 11 | HyperNet raffles | no row | #203 | `services/raffles/` |
| 12 | POS (old starbases): read arms only is the small first step | `missing` | #204 | `services/starbase/`, `planet/posMgrService.js` |
| 13 | New-player experience (NPE) | no row | #205 | `services/npe/` |
| 14 | Homefront operations | no row | #206 | `services/homefront/` |
| 15 | Evermarks and corp paint | no row | #207 | `services/evermarks/` |
| 16 | Certificates | no row | #208 | `services/character/` (`certificateMgr`) |
| 17 | Persona store and attention bubble for bots. An idea to weigh, not a feature gap | research only | #209 | `brain/src/store`, `brain/src/bubble`, `brain/src/paper` |
| 18 | Time dilation | `missing`, advised against at our scale; revisit only if the 200v200 work (#100, `partial`) cannot hold the tick | none | `network/nativeTimeDilation.js` |

Other open rows in our table that EvEJS also has, and that this page does not
rank: war headquarters (`missing`), Ansiblex gates and asset safety (`partial`),
FW capture points (`partial`), incursions (`parked`; their side is a few
hundred lines).

Order of work: 1 gates 2, 3 and the sov items that follow a structure fight. Abyss
depth (5) comes before mutaplasmids (6) and Proving Grounds (8).

---

## 5. Ours only

- **Automated real-client proof.** About 200 scenarios, up to four clients at
  once, two-client PvP, market and fleets. The gate is `tools/land.sh`.
- **A newer client.** 3670638 against 3396210, with re-pin tooling
  (`tools/re-pin.sh`).
- **Bots at scale and in depth.** Doctrines from real killmails, counters,
  hunters, camps, dictor and HIC camps, ESS raids, capital fights, sov bots,
  PI and industry chains. EvEJS bots are a gateway harness and a persona brain
  with a roadmap for the rest.
- **A closed-economy design.** Recipe and PI markup, income multipliers, an
  econ ledger, demand-driven prices.
- **Supply chain and audit.** Std-only Rust, no dependencies, no `unsafe`.
  EvEJS uses npm packages, a native SQLite binding and a Rust market sidecar.
- **Remote-client tooling.** Client kit, jailed-client census, hosted auto-deploy, the
  status page.
- **Proof of features both have in code.** Doomsdays, jump portals, cyno
  inhibitors and ESS theft are proven on a client here only.

---

## 6. Caveats

- **Builds differ.** Scope transfers across builds; wire details do not. The
  18 earlier disagreements with EvEJS were all decided our way
  (`docs/evejs/evejs-surprises.md`).
- **Graded differently.** `client-proved` means an automated client passed. For
  EvEJS we know only that code exists. A large file may still be partial, and
  some of theirs (incursions, Proving Grounds) are visibly thin.
- **Service counts are breadth, not progress.** A name can hide one feature or
  a hundred handlers; see `docs/port-tracker.md` §10.5.
- **Not determined.** Whether their structure combat, moon mining and abyss
  run end to end on a client. Whether our 41 `built-unproved` rows hold on one.
  Whether our bots feel like players (`unknown`).
- **Licence.** AGPL: no code, no prose. Our tree holds no EvEJS code.

---

## 7. History

The first versions (2026-10-20, re-measured 2026-10-21) compared the service
names in the 261018 archive file listing with ours: 206 theirs, 87 ours, 73 in
both. They also carried a `server/src/space/` file map and a log of what moved.
That method is retired. Its counts are not comparable with section 2, and its
"our state" column is wrong for the rows named at the top. The recipe and the
old tables are in git history before this commit.
