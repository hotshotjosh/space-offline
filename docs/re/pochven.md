# Pochven, end to end

Implementation spec for issue #72 (POCH-0): what it takes to make Pochven a
region players and bots can reach, live in and leave. Read off the pinned
client's decompiled Python (build 3662168; line numbers are the decompiled
files', header included), its shipped static data (`tools/fsd.py`,
`tools/loctable.py`, run in `tools/analysis.sh`), the content bundle
(`out/content/`, counts and names only here) and this server's code.

**Evidence markers.** `[V]` verified against this client's code or shipped
data, with the place. `[V-data]` a count or a name read from the bundle or a
static table. `[I]` inferred from `[V]` facts, with what would falsify it.
`[A]` an assumption we choose to ship, with its falsifier; an `[A]` value
never goes into a test. EvEJS (`docs/evejs/`) was consulted for hints only;
nothing here rests on it.

Nothing here has been run against a client. §9 lists what the first client
run settles.

---

## 0. The answer in one screen

1. **The region is in the bundle, whole, and sealed.** Pochven is region
   10000070: 27 systems in three constellations (Krai Perun, Krai Svarog,
   Krai Veles, nine systems each), every one with wormhole class 25, faction
   500026 (Triglavian Collective), security −1.0 `[V-data]`. It has **60
   stargates, all internal** (30 pairs, a loop of 27 with one chord per
   Krai), **0 gates out**, 53 NPC stations, 0 belts `[V-data]`.
2. **A gate jump inside Pochven should work today** `[I]`: every gate has a
   partner in the extract, so `celestial.rs` serves it and `jump.rs` jumps
   it, constellation change included. Nobody can get there except a GM
   (`/tr`). The jump delay is the same 4,000 ms as anywhere
   (`ballparkCommon/jumpdelay.py:9-15` `[V]`).
3. **The client decides "Pochven" by one test**: the system's `factionID`
   is `factionTriglavian` (`idCheckers.IsTriglavianSystem:141-145` `[V]`).
   It reads that from its own static map, so the security label, the
   delayed Local, the route text and the jump tunnel all change without the
   server doing anything. What the server must do is enforce the rules the
   client only displays.
4. **Ways in and out are wormholes and filaments, nothing else.** 37
   wormhole types (10 names) lead in, out or across `[V-data]`; 17
   Triglavian filaments (group 4087) carry fleets in (12 types) and out
   (5 types) `[V-data]`, through the same `randomJumpMgr` call our Needlejack
   code already answers (and refuses for this group today).
5. **Standings decide survival and access.** The client ships one station
   standings-restriction table, and its only owner is the Triglavian
   Collective `[V-data]`, checked through the system's faction
   (`evestations/standingsrestriction.py:44-51` `[V]`). The UI draws the
   "attacked in space" threshold at 0.0 for the Triglavians and EDENCOM
   (`standingsBar.py:161-162` `[V]`). The Agency text says some conduit
   gates need standing too `[V text]`.
6. **What spawns there** is Triglavian content: 57 Triglavian-faction
   dungeons (9 of them archetype 65, the Triglavian sites the client asks
   for with `GetTriglavianSiteInstances`), anomalies holding the Triglavian
   ores, and Triglavian NPCs our NPC table does not yet carry (0 rows with
   faction 500026 in `npc/types.tsv` `[V-data]`).
7. **Build order:** POCH-1 the region model and a first client proof;
   POCH-2 filaments; POCH-3 wormholes; then the rules (Local, cyno, home
   station, deploy), station standings, conduit gates, sites and NPCs, the
   Triglavian weapon FX, drops, bots (§8).

---

## 1. What Pochven is in the data

### 1.1 The region [V-data]

From `out/content/universe/{systems,constellations,regions,celestials}.tsv`:

| fact | value |
|---|---|
| region | 10000070 "Pochven", `faction_id` 500026, `wormhole_class_id` 25 |
| constellations | 20000787 Krai Perun, 20000788 Krai Svarog, 20000789 Krai Veles; class 25, faction 500026, each other's only neighbours |
| systems | 27, nine per Krai; `faction_id` and `owner_faction` 500026 on all; `security` −1.0 on all; `wormhole_class_id` 25 on all; `disallow_scanning` 0 |
| celestials | 27 stars, 220 planets, 1,095 moons, 53 stations, 60 gates, 0 belts |
| gates | type 56317 (group 10, stargate) on all 60; every `dest_system` is another Pochven system |
| stations | 53: owners Perun Clade (21), Veles Clade (19), Svarog Clade (7), The Convocation of Triglav (6); all four corporations are faction 500026 and name AEGIS (EDENCOM) as their enemy (`npc/corporations.tsv`) |

**The gate graph** `[V-data]`: each Krai is a chain of nine; the three
chains join end to end into one loop of 27 (Otanuomi–Urhinichi,
Sakenta–Arvasaras, Ahtila–Senda), and each Krai has one extra internal
chord (degree 3 at Nalvula and Otela, Harva and Raravoss, Angymonne and
Vale). 30 edges. This is CCP's "Conduit Loop" (Agency text
`UI/Agency/TriglavianSpace/ConduitLoopDescription` `[V]`).

### 1.2 What the client's own static data adds [V]

Read with `tools/fsd.py` from `solarsystemcontent.static` (the schema is in
`docs/re/static-corpus.md` §6.3):

- **`visualEffect = 'TRIGLAVIAN_HOME'` on exactly the 27 Pochven systems**,
  none else. `environmentSvc.RefreshSystemWideEnvironment:99-105` turns that
  into environment template 2570 (`environmentSvc.py:21`). Client-only: the
  server sends nothing for it.
- **`factionID` is not in `solarsystemcontent`** for these systems; the
  client reads it from `mapSystemCache` (`systems.static`), which is where
  our `systems.tsv` faction column comes from.
- **`disallowCyno` is unset** on all 27; the cyno ban is a code rule (§2).
- **`disruptedStargates`**: 130 rows universe-wide; 64 sit in k-space and
  point into Pochven, 64 sit in Pochven and point out, 2 point between two
  Pochven systems. They are the cut gates of the invasion story. No client
  Python reads the field `[V: grep]`, so they are server-placed scenery
  `[I]` (falsified by a client file that reads `disruptedStargates`). They
  matter to us as **the list of k-space systems that used to border
  Pochven** (§4.2, POCH-2).

### 1.3 Constants the code uses [V]

| name | value | where |
|---|---|---|
| `factionTriglavian` | 500026 | `eve/common/lib/appConst.py:604` |
| `TRIGLAVIAN_WORMHOLE_CLASS_ID` | 25 | `appConst.py:1597` |
| `POCHVEN_REGION_ID` | 10000070 | `jobboard/client/features/dungeons/triglavian_sites/provider.py:11` |
| `groupTriglavianJumpKeys` | 4087 | `inventorycommon/const.py:1078` |
| `typeTriglavianSpaceTrace` | 56693 | `inventorycommon/const.py:2351` |
| `dunArchetypeInvasionSites` | 65 | `appConst.py:467` |
| `factionEDENCOM` | 500027 | `appConst.py:605` |

---

## 2. How the client treats a Pochven system

Everything below keys on `IsTriglavianSystem(id)`: `mapSystemCache[id]
.factionID == 500026` (`idCheckers.py:141-145`) `[V]`. Because the test is
client-side and data-driven, the **server's job is to agree** with it, which
`service/live_misc.rs:120-140` already does for interference.

| client behaviour | where [V] | server work |
|---|---|---|
| Security label "Pochven", range −1.0 "unclassified", its own icon and text; null-sec excludes it | `security/client/const.py:26-37`, `:69-70` | none |
| Local channel is named `triglavian_<systemID>`, not `local_<id>` | `chatutil/channelnames.py:22-34` (`CATEGORY_TRIGLAVIAN`, `chatutil/const.py:22`) | **yes**: `chat.rs` must make that room (POCH-4) |
| Local is delayed (no member list until you speak) | `chat/common/util.py:15-16` `is_local_chat_delayed` | **yes**: `chat.rs:197` already lists `triglavian` among the no-presence categories; the room itself is missing |
| Stargate jump delay 4,000 ms (`ABYSSAL_JUMP_DELAY_MS`) vs `invulnerabilityJumping` 4,000 elsewhere | `ballparkCommon/jumpdelay.py:9-15`, `appConst.py:1447` | none: `jump.rs:67` already waits 4,000 ms |
| Autopilot waits 13 timer cycles after a jump into Pochven, 5 elsewhere | `autopilot.py:350-353` | none |
| Jump tunnel plays the Triglavian curve set when `graphicInfo[0]` is a Pochven system | `effects/Jump.py:390-391` | none: `JumpFx` already sends `(dest,)` (`jump.rs` §`JumpFx`) |
| Route to a Pochven or wormhole system says "no gate-to-gate route" | `evePathfinder/pathfinder.py:145-146` | none |
| Gate confirm "jump to Triglavian system" only for systems in `GetTriglavianMinorVictorySystems()` | `stargate/client/jump_checks.py:74-84` | none: Pochven systems are not minor-victory systems; `service/world.rs:95-97` answers `[]` |
| Cyno suppressed: `get_cyno_suppressed_error` returns `'CFGInTriglavianSystem'` | `eveuniverse/solar_systems.py:130-134` (common code; no client caller `[V: grep]`, so the server's rule `[I]`) | **yes**: `cyno.rs:432-444` refuses high-sec and J-space only (POCH-5a) |
| No "Set home station" on a station or structure in Pochven; validation error `TRIGLAVIAN_SYSTEM = 6` | `menucheckers/celestialCheckers.py:1368-1380`, `homestation/validation.py:17` | **yes**: `clones.rs:839` has no such check (POCH-5b) |
| Asset safety disabled (as in J-space) | `assetSafetySvc.py:78-79` | none until asset safety exists |
| Structures may not be deployed: `UserError('CannotBeDeployedInSystemOwnedByFaction', {deployTypeID, factionID})` | `services/structure/structureDeployment.py:96-97` | **yes**, server-side repeat (POCH-5c) |
| Deployables with `disallowInTriglavianSpace` show the ban | `spacecomponents/client/components/deploy.py:44-45` | **yes**, same piece |
| Capital manufacturing refused at structures in Pochven | `industry/industrySvc.py:164-166` | none (no structures there) |
| Station model shows its `triglavianOwned` state | `spaceObject/station.py:30-31` | none |
| Job Board "Triglavian sites" provider enables in Pochven and lists the whole region | `triglavian_sites/provider.py:31-38` | answered by POCH-8c |
| Anomaly tracker asks `dungeonInstanceCacheMgr.GetTriglavianSiteInstances()` only when in Pochven; archetype 65 | `evedungeons/client/anomalyTracker.py:305-315` | answered `{}` today (`service/sites.rs:112`); POCH-8c fills it |
| Loot chance 0.9 (same as low, null, J-space) | `idCheckers.py:344-352` | none |

`GetTriglavianMinorVictorySystems`, `GetEdencomFortressSystems` and
`GetEdencomMinorVictorySystems` (`maps/mapSvc.py:478-489`) are **not**
Pochven calls: they list k-space systems of the invasion story. `[]` stays
right `[I]`, falsified by a map filter that shows Pochven under one of them.

---

## 3. Moving inside Pochven

### 3.1 Gate jumps work today [I]

- `celestial.rs` serves a gate whose `dest` is in the extract (module doc,
  "a stargate with no partner"); all 60 Pochven gates have one `[V-data]`.
- `jump::session_change` (`jump.rs:484-510`) adds `constellationid` when the
  Krai changes, so the three Krai-crossing pairs move the session right.
  The region never changes inside Pochven.
- **Falsified by** a GM `/tr` into a Pochven system followed by a gate jump
  that fails. That is the first client scenario (POCH-1).

### 3.2 Conduit gates that need standing [V text, rule A]

The Agency says: "Some conduit gates require a certain level of standings
with the Triglavian Collective in order to pass through them"
(`ConduitLoopDescription` `[V]`). Nothing in the client says which gates or
what level: the check is server-side `[I]` (`gateJumpSvc.JumpThroughGate:91-117`
checks only emanation locks and FW tolls before it calls
`CmdStargateJump`). The client has a message body for it, with one token
`{standingsMissing}` ("You do not fulfill the standings requirements to
activate this gate!", message 529038) but no label naming the `UserError`
`[V]`.

- **Rule [A]:** a gate whose far side is a **Home** system (§4.3) refuses a
  pilot whose effective standing toward 500026 is below 0.0. Falsified by a
  TQ player naming another gate set or level.
- **Refusal [A]:** `CustomNotify` with the text above, until the message's
  name is found. Falsified by finding it in the dialog data
  (`prompt_player_fsd_dialog` names live outside the localization labels).

---

## 4. Getting in and out

### 4.1 Wormholes [V-data]

All are group 988, `wormholeTargetSystemClass` (1381) as listed. Masses and
lifetimes are the types' own attributes, read at runtime as for every hole
(an internal research note, not in the public repo).

| name | types | 1381 (target class) | role [I] |
|---|---|---|---|
| F216, U372, R327, O753, I078, L687, O546 | 9 (F216 ×2) | 25 | into Pochven from outside |
| X450 | 1 | 9 | Pochven → null-sec |
| R081 | 1 | 4 | Pochven → class-4 J-space |
| C729 | **27** | −1 | Pochven-internal (R-WH §1.2's reading of the attribute's description) |

- R-WH §1.3 counts C729 ×28; the bundle has **27**, one per Pochven system.
- **Each C729 type has its own `wormholeTargetDistribution` (1457)**, 27
  distinct values `[V-data]`, so a C729 type names one destination `[I]`.
  The distribution table itself is not shipped `[I]`; which type goes to
  which system is ours to choose `[A]` (sorted types onto sorted systems;
  falsified by a capture of a C729's far end).
- **The R-WH source-block rule does not cover these.** They sit in 36
  archetype-38 dungeons of several factions and shapes, not in the blocks of
  nine `[V-data]`, so the source classes are ours `[A]` (POCH-3).
- **Slim fields** (R-WH §3). Two change for Pochven:
  - `isDestTriglavian`: `True` when the far system is a Pochven system. It
    is a bare controller variable (`clientControllerRegistry.py:156` `[V]`);
    `wormhole.rs:820` sends `False` everywhere today.
  - `otherSolarSystemClass`: 25 for a Pochven far end, **never −1**. The
    info window does `wormholeClasses.get(otherSolarSystemClass)` over a
    dict whose keys are 0-9, 12-18 and 25, then `GetByLabel` of the result
    (`infoContentBase.py:1466-1484` `[V]`); −1 gives `GetByLabel(None)` `[I]`
    (falsified by an info window that renders a C729 sent with −1).

### 4.2 Filaments [V-data]

Group 4087, 17 types, all published, in market groups "Triglavian Space
Outbound" (5) and "Triglavian Space Inbound" (12). Every one carries
`microJumpPortalMaxShipCandidates` (2832: 1, 5 or 15 pilots),
`microJumpPortalRadius` (2067), `filamentSpoolupTimeSeconds` (5783),
`filamentDescriptionMessageID` (3026) and `locationListID` (3096); the two
Proximity types add `lightYearDistanceMax` (3097) `[V-data]`
(`dogma/const.py:633-634`, `:1339-1340` for the names `[V]`).

| family | names | direction | lands, per its description [V text] |
|---|---|---|---|
| Glorification 'Devana' | -1, -5, -15 | out | a k-space system "in which the Triglavians maintain an ongoing interest" (Agency: a Triglavian minor-victory system) |
| Proximity 'Extraction' | -5, -15 | out | a k-space system near the Pochven system it is activated in (3097 is the reach) |
| Cladistic 'Krai Perun' / 'Krai Svarog' / 'Krai Veles' | -5, -15 each | in | a system of the named Krai |
| Border / Internal / Home 'Pochven' | -5, -15 each | in | a Pochven system of that class |

- **The destination lists are server data.** 3096 points into
  `locationLists.fsdbinary`, whose loader is server-side
  (`locationlists/loader.py:16-18` `[V]`); the client's own
  `clientlocationlists.fsdbinary` is 1,588 bytes and not decoded by our
  tools yet `[V]`. So every landing set below is ours `[A]`.
- **The client's Agency tooltip lists are stale**: `CLADE_FILAMENTS` and
  `SYSTEM_TYPE_FILAMENTS` name type ids 37-42 and 46-51, which are minerals
  or nothing (`contentPageTriglavianFilaments.py:35-44` `[V]`). A tooltip
  bug in the client, not ours to fix; the two outbound lists are right.
- **The call is the Needlejack call** (`randomJump/client/randomJumpSvc.py:17`
  `randomJumpMgr.ActivateRandomJumpFilament(itemID)` through
  `PerformSessionChange` `[V]`). The validator accepts both key groups
  (`clientRandomJumpValidator.py:50` `[V]`) and the item menu offers both
  (`menucheckers/itemCheckers.py:33` `[V]`).
- **The errors** (`randomJump/error.py:38-39` `[V]`):
  `TRIGLAVIAN_SPACE_ONLY = 30` ("may only be activated within the Pochven
  region") and `TRIGLAVIAN_SPACE_DISALLOWED = 31` ("only outside"). The
  client never raises them: the server does, in the same `RandomJumpError`
  list shape `random_jump.rs` already uses `[I]`.
- **The trace** (`typeTriglavianSpaceTrace`, group 1991): a ball others can
  follow, activated by `beyonce.CallComponentFromClient(itemID,
  'filamentSpoolup', 'JumpThroughRandomFilamentGate')`
  (`menuSvcExtras/movementFunctions.py:355-356`, `:504-507` `[V]`). Optional;
  `random_jump.rs` sends no trace for Needlejacks either.
- **Where they come from** (Agency `InboundFilamentsDescription` /
  `OutboundFilamentsDescription` `[V text]`): System Class filaments from
  "engaging various enemies in Known Space"; Krai filaments from wrecks of
  that Clade's ships; Extraction from wrecks of EDENCOM ships invading
  Pochven; Devana from Triglavian ships in Pochven. No LP store offers any
  of the 17 `[V-data]` (the clades' two LP tables hold 9 offers, none a
  filament). This matches the owner's "nothing from nowhere" rule: drops
  only (POCH-10).

### 4.3 Home, Internal, Border [A]

The three inbound classes need a class per system, and no shipped table
gives one `[V: only the location lists would]`. All 27 carry the same
`visualEffect`, so that does not split them either.

- **Home [A]:** Kino (Krai Perun), Niarja (Krai Svarog), Archee (Krai
  Veles), one per Krai. The Vyraj Anchorage descriptions put one anchorage
  "at the heart of" each Krai `[V text]`. Falsified by a TQ player or a
  decoded location list.
- **Internal / Border [A]:** by gate hops from the Krai's home along the
  Krai's own gates: ≤ 2 hops Internal, else Border. Falsified as above.

### 4.4 What we choose for landings [A]

| filament | lands in [A] | falsifier |
|---|---|---|
| Cladistic 'Krai X' | a system of constellation X | none needed: the description says so `[V text]` |
| Home / Internal / Border | a system of that class (§4.3) | the class table |
| Proximity 'Extraction' | a k-space (high, low or null) system within the type's `lightYearDistanceMax` of the origin's star, by `universe/centers.tsv`; none in reach → the nearest k-space system | a player landing farther out on TQ |
| Glorification 'Devana' | a k-space system that held a disrupted gate into Pochven (§1.2), since this world has no minor-victory systems | the owner, or a minor-victory list |

The landing is in deep space, as `random_jump.rs` does for Needlejacks.

---

## 5. What spawns there

### 5.1 Sites [V-data]

`dungeons/dungeons.tsv` has 57 dungeons with faction 500026, by archetype:
51 ×12, 65 ×9, 2 ×9, 25 ×8, 24 ×7, 27 ×7, 38 ×2, and one each of 31, 45,
50. Archetype 65 (`dunArchetypeInvasionSites`, tags
`feature_triglavian, activity_combat, activity_fleet`) is what
`TriglavianSiteTracker` lists. Archetype 24 are combat anomalies, 27 ore
anomalies.

- **Ores** in those dungeons: Bezdnacine, Rakovene, Talassonite (with their
  II- and III-Grade forms), Spodumain, and one Mercoxit `[V-data]`; the
  client's `TRIGLAVIAN_ORES` are Bezdnacine, Rakovene, Spodumain,
  Talassonite (`mining/client/ore_const.py:146-150` `[V]`).
- **No belts**: Pochven has 0 belt rows `[V-data]`, so C3's belt rats have
  nowhere to be.
- **Today** `population.rs:425-440` gives every system its band's rows; a
  Pochven system is band Null with no pirate faction, so it can draw the
  faction-less null rows `[I]` (falsified by a population census with no
  Pochven site). POCH-8c replaces that with Triglavian rows.

### 5.2 NPCs [V-data]

- **Hulls**: Triglavian NPC hulls are group 4028 "Invasion NPCs" (142
  types, mixed with EDENCOM's) and others; the Triglavian names start
  Raznaborg, Hospodar, Zorya's, Liminal, Starving, Harrowing, Blinding,
  Renewing, Anchoring, Ghosting, Tangling, Sudenic. `npc/types.tsv` carries
  **none** of group 4028 or faction 500026 today.
- **Spawners**: `npc/reactive-spawn.tsv` has 87 rows owned by 500026 and 173
  by 500027 (EDENCOM): gate, pocket, dungeon, conduit, World Ark, harvester
  and invasion-phase spawners, and one "Pochven Wormholes - Enemy Spawner"
  (C6 §2.3). These are the live-event spawners C6 says not to rebuild; we
  author squads by the C6 grammar instead (POCH-8b).
- **Werposts** (`typeTriglavianDisintegratorWerpost`, `…DamagedWerpost`,
  group 383) are Triglavian gun posts the client fits with dogma turrets
  (`spaceObject/sentryGun.py:24-28` `[V]`). Where CCP places them is not in
  the client `[V: no placement table]`.

### 5.3 The weapon FX trap [V]

Triglavian guns (group 1986 `groupPrecursorTurret`) fire the effect type
`TriglavianBeam` (`effects/Repository.py:48`; guid `effects.TriglavianBeam`
in `effects.fsdbinary` `[V-data]`). Its constructor does
`self.graphicInfo['mulitplierBonusPerCycle']` (CCP's spelling) and
`self.graphicInfo['multiplierBonusMax']`
(`effects/triglavianBeam.py:17-18`). This server sends `graphicInfo=None`
on every module FX (`modules.rs:1079-1082`), so **any disintegrator FX,
NPC or player, raises in the client** `[I]` (falsified by a client log with
no traceback after a Triglavian ship fires). The values the server should
send are the gun's `damageMultiplierBonusPerCycle` (2733) and
`damageMultiplierBonusMax` (2734) `[I]` (`dogma/const.py:281-282`). The
damage ramp itself is not modelled in `dogma.rs` either `[V: grep]`.

### 5.4 System effects [I]

The Pochven description says "environmental effects shape combat"
(`UI/Map/Filters/Security/secRangeDescriptionPochven` `[V text]`), and the
client has four `Triglavian System Effect` dbuffs (max locked targets, warp
speed, remote armor repair, remote shield boost; `dbuff/const.py:136-139`,
`dbuffs/collections.tsv` `[V-data]`). Which systems carry which values is
server data `[I]`; `universe/system-effects.tsv` has no k-space row
`[V-data]`. Left to the owner (§8, POCH-12).

---

## 6. Standings

### 6.1 Station services [V]

- `check_station_standings_restriction(service_id, station_id, who)` checks
  the station owner's restrictions, then **the restrictions of the system's
  faction** (`evestations/standingsrestriction.py:44-51`).
- The table (`stationStandingsRestrictions.fsdbinary`, 296 bytes) has one
  owner key, **500026** `[V-data]`, and seven rules (`docs/re/evejs-data.md`
  §4 and `corp_standings.rs:42-47` agree) over the seven
  `_restricted_services` (`:24-31`): reprocessing, repair, factory,
  fitting, **docking**, office rental, jump clone `[V]`.
- **The refusal** (`:126-139`): `UserError('StandingsRestrictionGeneric',
  {'from_id', 'to_id', 'current_standing', 'required_standing'})`, or
  `StandingsRestrictionCorporationGeneric` for office rental; it compares
  `round(current, 2) < round(required, 2)`, with the standing **with skill
  bonus** for a character (`:147-159`) `[V]`.
- The Agency says the same in words: "prevents its enemies from docking
  and only allows access to certain services to those who have proven
  themselves" (`StationAccessDescription` `[V text]`).
- `tools/fsd.py` has no columns for this table yet (`no confirmed columns`)
  `[V]`; the values come from it, never from a hand-typed copy.

### 6.2 Who shoots whom [V UI, rule A]

`standingsBar.GetThresholdValue` puts the "attacked in space" threshold at
**0.0** for `factionTriglavian` and `factionEDENCOM`
(`standingsBar.py:161-162`) `[V]`. The rule we ship `[A]`: a Triglavian NPC
or Werpost engages a pilot whose effective standing toward 500026 is below
0.0, and an EDENCOM NPC one whose standing toward 500027 is below 0.0.
Falsified by a TQ player at exactly 0.0 being shot (then `<=`).

### 6.3 Gaining and losing it [A]

`standings.rs` changes standings only through missions and `/standing`
today (`mission_rewards.rs:439`). Pochven needs a kill rule `[A]`: killing
an EDENCOM NPC raises 500026 and lowers 500027; killing a Triglavian NPC
the reverse; through `kill_hooks` (a maintainer line). Falsified by the
owner's tuning, or a TQ standings log.

---

## 7. What this server has today

| piece | today | where |
|---|---|---|
| Region, systems, gates, stations in the bundle | present | `out/content/universe/*`, `stations/stations.tsv` |
| Pochven recognised | as band Null; class 25 known; faction 500026 known | `celestial.rs:603-618`, `:904-914` |
| Internal gate jumps | built, unproved for Pochven `[I]` | `jump.rs` |
| No interference in Pochven | right `[V]` | `service/live_misc.rs:120-140` |
| Sov never claimable | right `[I]` (faction set) | `sov.rs:345-360` |
| Minor-victory and fortress lists | `[]`, right | `service/world.rs:95-97` |
| `GetTriglavianSiteInstances` | `{}` | `service/sites.rs:112` |
| Wormholes | Pochven holes left out; `isDestTriglavian = False`; `statics_per_system(25) = 0` | `wormhole.rs:146-153`, `:820`; `content/wormholes/statics.tsv:22` |
| Filaments | group 4087 accepted as a key group, refused `NOT_ACTIVE`; Pochven excluded as a Needlejack landing | `random_jump.rs:110-114`, `:371`, `:430-447` |
| Filament drops | abyss and Needlejack kinds only | `filament_drops.rs:198-200`, `:234` |
| Local chat | `triglavian_` rooms not produced | `chat.rs:215-216` |
| Cyno in Pochven | allowed (bug) | `cyno.rs:432-444` |
| Home station in Pochven | allowed (bug) | `clones.rs:839` |
| Station standings restrictions | not held | `corp_standings.rs:42-47` |
| Triglavian NPCs, sites, Werposts | none | `npc/types.tsv`, `npc_data.rs:751` |
| Disintegrator FX | `graphicInfo=None` (client raise `[I]`) | `modules.rs:1079-1082` |
| Status row "Pochven" | `missing`, issues #72, #73, #74 | `docs/status/progress.tsv` |

---

## 8. Build plan

Each piece is one claim. Paths without a crate are under
`server/crates/eve-server/src/`. Registration lines in hot tables
(`lib.rs` `mod`, `world_ext.rs` `SLOTS`, `content_tables.rs` `TABLES`,
`kill_hooks.rs`, `slash.rs` `COMMANDS`, `rollover_steps.rs` `STEPS`) go in
the PR text for the maintainer, never into the diff. Tests use invented ids
(a fake faction-500026 region of four systems, invented types); no `[A]`
value goes into a test, only its mechanism (a test passes the threshold in
as a parameter). Every piece edits its own row in `docs/status/progress.tsv`
(theme 3.5) and regenerates the page. The status row "Pochven" names #73
and #74; which two pieces they become is the maintainer's call (the issue's
starting guess is POCH-1 and POCH-2).

**POCH-1. The region model, and the first client proof.** *Size S-M.*
- `pochven.rs` (new, crate root): `is_pochven(&Universe, system)` (faction
  500026, the client's own test); `krai_of(system)`; `class_of(system) ->
  Home | Internal | Border`; `systems_of_class`, `systems_of_krai`; loaded
  from `content/pochven/systems.tsv` (new: `system_name, krai, class, why`,
  names not ids, resolved through the universe table, the §4.3 `[A]`s, each
  row with its falsifier); a `TableSpec` for `content_tables.rs` in the PR
  text. Schema row in `docs/content/schema.md` (proposed in the PR if that
  file is claimed).
- `tools/client-ci/scenarios/fci_pochven_gate.py` (new): `/tr me` to a
  Pochven system's station system, undock, jump a gate into the next Krai,
  assert the session's `solarsystemid` and `constellationid`, check the
  Local room name is unchanged-or-`triglavian_` (records today's state), no
  traceback.
- Owns: `server/crates/eve-server/src/pochven.rs`, `content/pochven/`,
  `tools/client-ci/scenarios/fci_pochven_gate.py`.
- Tests: an invented faction-500026 region classifies; a system of another
  faction is not Pochven; a name in the table that the universe lacks is
  dropped with a line; the class table round-trips.
- `[A]`: §4.3 classes (falsifier there).

**POCH-2. Triglavian filaments.** *Size M. Depends POCH-1.*
- `random_jump.rs`: serve group 4087. Direction by family, read from the
  type's attributes and name family, never by id: inbound
  (`TRIGLAVIAN_SPACE_DISALLOWED` when activated in Pochven), outbound
  (`TRIGLAVIAN_SPACE_ONLY` when activated outside). Landings per §4.4 through
  `pochven.rs`. Same spool, fleet and range rules as Needlejacks;
  wormhole/abyss refusals unchanged. Proximity uses `centers.tsv` and 3097.
- The Devana landing set needs the k-space ends of the disrupted gates:
  `tools/content.py` gains `universe/disrupted-gates.tsv` (`system_id,
  target_system_id`) from `solarsystemcontent.disruptedStargates`. If
  `tools/content.py` is claimed, split it out as POCH-2a.
- Owns: `random_jump.rs`, `random_jump/tests.rs` (and `tools/content.py`'s
  universe section for 2a).
- Tests: each refusal code in its list shape for an invented key of group
  4087; an inbound class key lands in a system of that class; a Krai key in
  that Krai; a Proximity key never beyond the reach (reach a parameter);
  nothing is consumed on a refusal.
- `[A]`: the landing sets of §4.4 (falsifiers there); outbound into high
  sec allowed (falsified by a TQ player who never landed in high sec).
- fci: `fci_pochven_filament.py`: GM-give a Krai filament, activate, land in
  that Krai; activate an Extraction filament there, land in k-space.

**POCH-3. Pochven wormholes.** *Size M. Depends POCH-1.*
- `wormhole.rs`: `systems_of(25)` = Pochven; C729 (−1) draws a Pochven
  system other than its own, by the type's sorted slot (§4.1 `[A]`);
  `isDestTriglavian` true for a Pochven far end; `otherSolarSystemClass` 25
  for a Pochven far end and never −1; the loader accepts −1 as "Pochven" in
  `dest_class`.
- `content/wormholes/statics.tsv`: rows for the Pochven holes `[A]` — each
  Pochven system: one C729 and one of X450/R081; k-space null and low:
  F216/U372/R327/O753 at a per-system chance; J-space C2-C4: I078/L687/O546
  at a per-system chance; a `POCHVEN_HOLE_CHANCE` const in `wormhole.rs`.
  Falsified by players not finding a way in within an evening (raise) or
  Pochven crowded with holes (lower).
- Owns: `wormhole.rs`, `content/wormholes/statics.tsv`.
- Tests: an invented class-25 system draws a Pochven far end for a −1 row;
  the slim says `isDestTriglavian` true and class 25; no slim ever carries
  −1; a 25-row's far end is always Pochven.
- fci: `fci_pochven_wh.py`: find a hole into Pochven, read its info window
  (no traceback), jump, land in Pochven.

**POCH-4. Triglavian Local.** *Size S.*
- `chat.rs`: in a Pochven system the Local room is `triglavian_<id>`
  (`chatutil/channelnames.py:22-34`), delayed like `wormhole_` (already in
  the no-presence list at `:197`).
- Owns: `chat.rs`. Tests: the room name for an invented faction-500026
  system; no presence broadcast in it. fci: speak in Local in Pochven.
- `[A]`: none.

**POCH-5. The rules the client only shows.** *Size S each; can be three
claims.*
- 5a `cyno.rs`: refuse a cyno in Pochven. Error name `[A]`
  `CFGInTriglavianSystem` (the common code's own key), falling back to
  `JumpCantUseCFG` if the client has no message for it (falsified by a
  "message not found" box). Test: an invented Pochven system refuses.
- 5b `clones.rs`: `home_station_candidates` leaves out Pochven stations and
  `set_home_station` refuses one. Refusal `[A]` `CustomNotify` (the real
  error is a `ChangeHomeStationValidationError` subclass,
  `homestation/validation.py:10-22`, which `eve_proto` cannot build).
  Test: invented station in an invented Pochven system.
- 5c the structure deploy path (`structure.rs` or the deploy module that
  owns it): refuse with `CannotBeDeployedInSystemOwnedByFaction`,
  `{'deployTypeID', 'factionID'}` `[V shape]`; and mobile deployables whose
  type has `disallowInTriglavianSpace`.
- `[A]`s as named.

**POCH-6. Station standings restrictions.** *Size M.*
- `tools/fsd.py` decoder columns for `stationstandingsrestrictions`, and a
  `tools/content.py` section writing `stations/standings-restrictions.tsv`
  (`owner_id, service_id, required_standing`); then `station_standing.rs`
  (new): `check(reg, service, station, character) -> Option<Refusal>` with
  the client's rule (§6.1: owner, then system faction; 2-dp rounding; skill
  bonus as `standings.rs` computes it, or raw standing `[A]` until it
  does); called from the dock path and the six service paths (one line
  each, in the PR text if the files are claimed).
- Owns: the decoder and section, `station_standing.rs`.
- Tests: invented owner and threshold; below refuses with the four-key
  dict; equal after rounding passes; the system-faction branch applies to a
  station whose owner has no rule.
- fci: dock at a Pochven station at standing −1 (`/standing`): refused with
  the client's own text; at 0: docked or refused per the table.

**POCH-7. Conduit gates.** *Size S. Depends POCH-1.*
- `pochven.rs`: `gate_refusal(reg, actor, gate)`; one line in
  `jump::cmd_stargate_jump` beside the crimewatch refusal (`JUMP_GUARDS`
  take no gate id, `movement_guard.rs:37`).
- Owns: `pochven.rs` (and the `jump.rs` line, or the PR text).
- Tests: with an invented threshold parameter, below refuses and at it
  passes; a gate to a non-Home system never refuses.
- `[A]`: §3.2 (Home gates, 0.0, `CustomNotify`).

**POCH-8. Triglavians in space.** *Size L; four claims.*
- 8a **Disintegrator FX** (`modules.rs`, and the NPC fire path): send
  `graphicInfo = {'mulitplierBonusPerCycle': f, 'multiplierBonusMax': g}`
  for a turret whose effect guid is `effects.TriglavianBeam`, from 2733 and
  2734. A player-ship bug today too `[I]`; the PR says so. Test: the FX of
  an invented type with that guid carries both keys, CCP's spelling.
  fci: fit an Entropic Disintegrator, shoot a target, no traceback.
- 8b **Triglavian and EDENCOM NPC rows**: the `tools/content.py` npc
  section tags group 4028 (and the other Triglavian NPC groups) with faction
  500026 or 500027 by the name grammar of §5.2, class by hull; `npc_data.rs`
  reads them. Owns: the npc section, `npc_data.rs`. Tests: invented rows
  classify. `[A]`: name-prefix → faction (falsified by a type whose own
  `factionID` disagrees, once the extract carries it).
- 8c **Pochven sites**: `content/population.tsv` gains Pochven rows (the
  archetype 65, 24 and 27 dungeons of faction 500026), a `pochven` band or
  filter in `population.rs` so generic null rows stop landing there;
  `GetTriglavianSiteInstances` answers the live archetype-65 sites in the
  `agency_instances` shape (`service/sites.rs`). Owns: `population.rs`,
  `content/population.tsv`, the sites arm (PR text if `service/sites.rs` is
  claimed). `[A]`: counts per system (falsified by a census that finds
  Pochven empty or crowded).
- 8d **Hostility and standing change**: an NPC brain rule that Triglavian
  and EDENCOM NPCs engage by §6.2, and the §6.3 kill rule as a `kill_hooks`
  row (maintainer line). Owns: `pochven.rs` (or `npc_threat.rs` if free).
  Tests with invented thresholds.

**POCH-9. Werposts** *(Size M, later)*: gun posts at Pochven gates and
stations, built on `sentry.rs`'s post machinery, firing by §6.2.
`[A]` placement (falsified by a TQ capture of a Pochven gate).

**POCH-10. Triglavian filament drops.** *Size S. Depends POCH-2.*
`filament_drops.rs` gains two kinds, `trig-in` and `trig-out`, chosen by
§4.2's sources (`trig-in` from k-space rats and Triglavian wrecks, `trig-out`
from EDENCOM and Triglavian wrecks in Pochven); rows in
`content/pve/filament-drops.tsv`, every rate `[A]` with its falsifier.

**POCH-11. Bots in Pochven** *(Size M, after 2, 3, 8)*: an `eve-bot` role
that buys nothing it cannot find (owner rule), carries Krai filaments, runs
Pochven sites in a fleet and leaves by Extraction. Owns: an `eve-bot` role
file; `roles.rs` `ROLES` line in the PR text.

**POCH-12. Pochven system effects** *(owner decision)*: whether the four
Triglavian dbuffs apply in Pochven, and at what values. `needs-human`
until decided.

---

## 9. What a client run settles

| question | settled by | if it fails |
|---|---|---|
| Internal gates jump, Krai crossings move `constellationid` (§3.1) | `fci_pochven_gate` | read the client log for the gate's slim; compare with a k-space gate |
| The Local room is `triglavian_<id>` and delayed (POCH-4) | speaking in Local in Pochven | the channel name the client joins is in its log |
| A C729 info window with class 25 renders (§4.1) | `fci_pochven_wh` | send the far end's own class |
| `isDestTriglavian = True` drives the model without error | the same | send `False` and file the controller's need |
| `TriglavianBeam` with `graphicInfo` keys plays (§5.3) | POCH-8a's fci | the traceback names the missing key |
| `CFGInTriglavianSystem` has a message box (POCH-5a) | lighting a cyno in Pochven | switch to `JumpCantUseCFG` |
| The standings refusal text shows with the four keys (§6.1) | POCH-6's fci | compare with `StandingsRestrictionGeneric`'s tokens |
| The §4.3 classes and §3.2 gate rule match TQ | a player who has lived there | edit `content/pochven/systems.tsv` |
