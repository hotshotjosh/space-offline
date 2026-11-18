# Sov over the gateway: hub upgrades, resources, skyhooks, mercenary dens

Implementation spec for issue #77 (SOV-UPG-0). It maps every public-gateway
message the client sends or listens for in the Equinox-era sovereignty
features, the machoNet calls and slim-item fields that go with them, what this
server answers today, and a numbered build plan of claimable pieces. Research
only: no code was written.

**Evidence markers.**

- `[V]` verified against the pinned client (the `pinned` row of
  `tools/builds.tsv`): a message shape read from the serialized
  `FileDescriptorProto` inside the client's own `*_pb2` module, or a line of
  the client's Python.
- `[I]` inferred from `[V]` facts; says from what.
- `[A]` an assumption we choose to ship; says what falsifies it. An `[A]` value
  never goes into a test.
- EvEJS (the unpublished EvEJS inventory note) is a hint source only. Anything taken from it stays
  `[A]` here until a client line confirms it.

**Line numbers.** `path:line` is a line of the decompiled client source
(uncompyle6 output of the pinned build, five-line header included; the tree is
local and gitignored under `out/`). They are not `co_lnotab` lines; they locate
the code, they do not match a traceback. Message shapes and field numbers were
read with a stdlib-only descriptor walker over the `serialized_pb` bytes of
each `eveProto/generated/eve_public/sovereignty/**/_pb2` module.

**Public-doc rule.** This file names client types and constants by their
symbolic names (`invConst.typeColonyReagentIce`, `SKYHOOK_CLAIM_FEE`), never
by value. Every number the server needs that is not a field number is `[A]`
and authored.

Contents: 0 headline, 1 the transport, 2 the wire map, 3 the machoNet side,
4 installing an upgrade, 5 power, workforce and reagents, 6 the skyhook
lifecycle, 7 the mercenary-den lifecycle, 8 what the server has today,
9 corrections to the drafted split, 10 the build plan, 11 open questions.

---

## 0. Headline

1. Seven public namespaces carry the feature: `hub.upgrade.api`,
   `hub.fuel.api`, `hub.api` (resources), `hub.workforce.api`,
   `resource.planet.api` + `resource.star.api`, `skyhook.api`, and
   `mercenaryden.api` + `mercenaryden.activity.api`. 34 request types (four of
   them QA-only admin requests, and two never sent) and 21 notice types. Each
   namespace has exactly one client messenger; all are listed in §2.
2. **The client simulates, the server stores.** The hub window computes every
   upgrade's power state itself (`sovereignty/client/sovHub/hubUtil.py:15`),
   the fuel left (`sovereignty/fuel/client/data_types.py:47`), the skyhook's
   reagent stock (`sovereignty/resource/client/reagent_split_production_simulation.py:8`)
   and a den's evolution (`sovereignty/mercenaryden/client/repository.py:308`).
   The server must run the **same rules** or the window says "configuration
   saved, but not the same" (§4.4). §5 states each rule.
3. **Hub fuel is colony reagents, not fuel blocks.** The reagent bay deposits
   `invConst.typeColonyReagentIce` and `invConst.typeColonyReagentLava` through
   `hub.fuel.api.AddRequest`, one request per stack
   (`eve/client/script/ui/shared/sov/reagentBay/controllers.py:21`, `:101`)
   `[V]`. Our stand-in (`src/sov_hub.rs`, fuel blocks by `/sovfuel`) predates
   the gateway; §9 and §11 Q1.
4. Several draft names do not exist in the client: there is no
   `VerifyInstallRequirements` or `AuthorizeManage` request in `eve_public`
   (they are in the server-to-server `eve.` namespace only), no skyhook
   `GetState` (it is `Get`), and no `resource.transfer` request for reagents.
   §9.
5. Three machoNet seams are needed besides the gateway:
   `beyonce.CallComponentFromClient` (not served at all today; skyhook and den
   extraction and skyhook theft go through it), `sovMgr.AcquireSkyhooks` /
   `DestroySkyhooks`, and `colonyResourcesMgr.GetMyCorporationSkyhooks`. §3.

---

## 1. The transport, once

- Every request in this document goes through
  `publicGateway.send_character_request(request, response_class, timeout)`
  and is answered by our `gateway::answer`, which consults
  `gateway_ext::CHAIN` first (`src/gateway_ext.rs`). An arm takes
  `(type name, body, pilot, ground)` and returns `Some(Reply)` for its own
  types.
- The sov messengers treat **any status but 200** as an error and branch on
  the code (§2 tables give each branch) `[V]`
  (`sovereignty/client/base_external_messenger.py:16-22`). A 200 with an
  unset payload reads as `None` and raises; our `answer` already sends an
  empty *typed* message for unknown classes.
- Timeouts: the hub, fuel, workforce, resource and skyhook messengers wait
  10 s (`base_external_messenger.py:8`, `resource/client/messenger.py:17`,
  `skyhook/client/messenger.py:16`); the den messenger 2 s, 5 s for the den
  list (`mercenaryden/client/request_messenger.py:15-16`) `[V]`.
- The skyhook messenger wraps every call in `backoff_if_error`
  (`publicGateway/grpc/backoff.py`): after an error the same call raises
  `BackedOffException` locally for 0.1 s, doubling to 300 s `[V]`. **An error
  answer to a skyhook request silences that request for up to five minutes.**
  Only `Get`'s 403 is exempt (`skyhook/client/messenger.py:26`).
- Notices go out through `gateway_out::queue` under the world lock and
  `gateway::publish` after it, to a `Target` (character, corporation,
  alliance, solar system) `[V]` (`src/gateway_out.rs`, `src/gateway.rs`
  `Target`). A notice is a "refresh now"; the client re-reads state when it
  opens a window.
- Writes that move items (upgrade install, fuel deposit) need the item store,
  not just `Deltas`: `Ground::write` hands an arm only `&mut Deltas`
  (`src/gateway.rs` `Ground::write`). `src/skins.rs:784` shows the pattern for
  an arm that needs the whole `Registry` (`Ground::Server(w)`).

**Identifiers.** Every id below is a one-field message, field 1 `sequential`:
`uint64` for hub, upgrade type, planet, skyhook, star, solar system, dungeon,
generic item and generic item type, and the two reagent and infomorph item
type ids; **`uint32`** for character, corporation and mercenary-den type `[V]`
(descriptors). A den activity id is `bytes uuid` (16 bytes) `[V]`.
A hub's `sequential` is the hub ball's item id: the hub window is opened with
the ball's `itemID` (`eve/client/script/ui/services/menuSvcExtras/openFunctions.py:177`)
and sends it unchanged (`eve/client/script/ui/shared/sov/sovHub/hubController.py:36`) `[V]`.

---

## 2. The wire map

Columns: the request and its fields (`n name:type`), the response fields the
client **reads**, who sends it, the status codes the client distinguishes, and
what our server answers today with the effect. "Empty" means our default
200 + empty typed message. Package prefix `eve_public.sovereignty.` omitted.

### 2.1 `hub.upgrade.api` — `sovereignty/upgrades/client/external_messenger.py`

Shared shapes (`hub/upgrade/upgrade.proto`) `[V]`:

- `upgrade.Identifier`: `1 hub:hub.Identifier`, `2 installation_type:genericitemtype.Identifier`,
  `3 upgrade_type:upgrade_type.Identifier`.
- `upgrade.Definition`: `1 installation_type`, `2 power_required:u64`,
  `3 workforce_required:u64`, `4 fuel_type:genericitemtype.Identifier`,
  `5 fuel_consumption_per_hour:u32`, `6 fuel_startup_cost:u32`,
  `7 mutually_exclusive_group:string`, `8 power_produced:u64`,
  `9 workforce_produced:u64`.
- `upgrade.Attributes`: `1 definition:Definition`, `2 power_state:PowerState`.
- `PowerState` enum: `UNSPECIFIED 0, OFFLINE 1, ONLINE 2, LOW 3, PENDING 4`.
  The client maps 1-4 and turns 0 into `None` (`sovereignty/upgrades/client/proto_parsers.py:13-20`).
- `HubUpgrades`: `1 hub`, `2 upgrades: repeated Upgrade{1 identifier, 2 attributes}`,
  `3 last_updated:Timestamp`.

| request (fields) | response read | sender | codes | today |
|---|---|---|---|---|
| `GetDefinitionsRequest` () | `1 definitions[]`: every field of `Definition`, keyed by `installation_type` (`:26-39`) | `UpgradesStaticDataSource.prime_data`, once per session (`sovereignty/upgrades/client/external_data.py:23`); hub window `PrimeData` | 404 → `SovUpgradeNotFoundError`; else `DataUnavailable` | Empty → cache primed with **no** types. Dropping an upgrade into the window then raises `AttributeError` in `_IsInMutuallyExclusiveGroup` (`hubController.py:220-222`, `get_static_data` returns `None`) `[I]` |
| `GetHubUpgradesRequest` (`1 hub`) | `1 hub_upgrades`: per upgrade `identifier.hub`, `identifier.upgrade_type`, `attributes.definition.installation_type`, `attributes.power_state`; `last_updated` (`:41-53`, `proto_parsers.py:20`) | hub window, reagent bay, alliance sov-hub list (`sovHubEntryController.py:465`), local cache on system change (`external_data.py`) | 403 → access restricted; else unavailable | Empty → "no upgrades installed" (honest) |
| `ProcessConfigurationRequest` (`1 new_upgrades: repeated genericitem.Identifier`, `2 configuration: repeated {1 upgrade_type:genericitemtype.Identifier, 2 online:bool}`, `3 hub`) | `1 hub_upgrades` as above (`:55-75`) | Save in the hub window (`hubController.py:341-365`) | 400, 403, 404, 409, 500 each a named error; all show `SovHubChangeNotSaved` | Empty → result `[]`; the client compares with its own list, finds them different and shows `ConfigurationSavedNotSame` (`hubController.py:362`). Nothing is stored; the upgrade item stays in cargo |
| `UninstallRequest` (`1 upgrade: upgrade.Identifier` with **`hub` and `upgrade_type` set**, `installation_type` unset) (`:77-90`) | nothing | Delete on an installed upgrade, after `ConfirmDestroyUpgrade` (`hubController.py:292-317`) | 403, 404, 500 | Empty → client removes it locally; it comes back on reopen |
| `GetRequest` (`1 upgrade`) → `1 attributes` | — | **not sent**: no client messenger builds it `[V]` (grep) | — | — |

### 2.2 `hub.fuel.api` — `sovereignty/fuel/client/external_messenger.py`

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `GetRequest` (`1 hub`) | `1 fuels[]: {1 fuel_type, 2 amount:u32, 3 burned_per_hour:u32}`, `2 last_updated` (`:19-33`) | hub window, reagent bay (`reagentBay/controllers.py:59`), alliance list | 403, 500 | Empty → no reagents; the bay pads both reagent types with 0 (`controllers.py:63-64`) |
| `AddRequest` (`1 fuel_item:genericitem.Identifier`, `2 amount:u32`, `3 hub`) → `AddResponse` () | nothing | reagent bay deposit, **one request per cargo stack** of a reagent type, `amount ≤ stacksize` (`reagentBay/controllers.py:84-113`) | 403, 404, 500 | Empty → the client believes the deposit; the stack stays in the hold. **A lie today** |

The bay is offered on any hub to a pilot in space, not warping, within range
(`menucheckers/celestialCheckers.py:1058`); who may deposit is the fuel access
group on `sovMgr` (§3.1), which we already serve.

### 2.3 `hub.api` (resources) — `sovereignty/resource/client/messenger.py`

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `GetResourcesRequest` (`1 hub`) | `1 power{1 available, 2 allocated, 3 local_harvest}`, `2 workforce{1 available, 2 allocated, 3 local_harvest}`, all `u64` (`:87-98`) | hub window `_PrimeAvailableResources` (`hubController.py:129-136`), alliance list | 403 → forbidden; else error | Empty → all zero; every upgrade the window simulates is "low power" |

### 2.4 `hub.workforce.api` — `sovereignty/workforce/client/external_messenger.py`

Shapes (`hub/workforce/workforce.proto`) `[V]`:

- `Configuration` oneof `mode`: `1 inactive:bool`, `2 import_settings{1 sources[]{1 source:solarsystem}}`,
  `3 export_settings{2 amount:u64; oneof export_destination: 3 destination_system | 4 no_destination:bool}`
  (field 1 `destination` exists and is **not read**), `4 transit:bool`.
- `State` oneof `mode`: `1 inactive:bool`, `2 import_state{1 sources[]{1 source, 2 amount:u64}}`,
  `3 export_state{oneof exporting: 5 connected{1 destination_system, 2 exported_quantity:u64} | 6 disconnected{1 local_reserve:u64}}`,
  `4 transit:Transit{}`.
- **An unset `mode` makes `create_from_proto` return `None`**
  (`sovereignty/workforce/client/data_types.py:61`, `:205`) and callers raise.

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `GetConfigurationRequest` (`1 hub`) | `1 configuration` (`:22-35`) | hub window workforce tab, sov map (`evemap/workforceUtil.py:94`) | 403, 500 | `inactive` set (`src/gateway.rs`) |
| `GetStateRequest` (`1 hub`) | `1 state` (`:74-87`) | same, map marker (`mapMarkerSovHub.py:102`), alliance list | 403, 500 | `inactive` set |
| `ConfigureRequest` (`1 hub`, `2 configuration`) → () | nothing; success = 200 (`:37-49`) | workforce tab save (`sovHubSvc.py:99-103`) | 403, 500 | Empty → client applies the transport visual (`UpdateWorkforceTransportState`) for a change never stored. **A lie** |
| `GetNetworkableHubsRequest` (`1 hub`) | `1 hubs[]{1 hub, 2 system, 3 configuration, 4 state}` (`:89-99`) | workforce tab (`editSections.py:326`), sov map **once per hub** (`evemap/workforceUtil.py:62-78`) | any → error | Empty → "no hubs in range" |

### 2.5 `resource.planet.api`, `resource.star.api` — same messenger

Shapes (`resource/planet/planet.proto`, `reagent.proto`) `[V]`:

- `planet.Definition`: `1 planet`, `2 power:u64`, `3 workforce:u64`,
  `4 reagent_definitions[]{1 type:genericitemtype, 3 reagent_type:reagent.itemtype, 2 definition:reagent.Definition}`.
- `reagent.Definition`: `1 amount_per_cycle:u32`, `2 cycle_period:Duration`,
  `3 secured_percentage:u32` (0-100), `4 secured_capacity:u32`,
  `5 unsecured_capacity:u32`.
- `semanticversion.Specification`: `1 major, 2 minor, 3 patch:u32`,
  `4,5 repeated string` tags.

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `GetAllDefinitionsRequest` (oneof `version`: `1 known_version` \| `2 no_known_version:bool`) | `1 definitions[]`: `planet`, `power`, `workforce`, and per reagent **`reagent_type` (field 3)** + `definition`; `2 version` (`:29-47`, `_parse_definitions :60-71`) | `PlanetaryResourcesCache`, 15-minute TTL, re-asked with `known_version` (`sovereignty/resource/shared/planetary_resources_cache.py`) | **304 → "unchanged"**, cache renewed (`:39`) | Empty → version 0.0.0, no planets: every planet reads power/workforce `None`, the hub's max power and workforce are 0 |
| `GetDefinitionsVersionRequest` (`1 version`) | `1 definitions[]` | only when a skyhook names a version **older** than the newest held (`planetary_resources_cache.py:84-97`) | 404 → version not found | Empty |
| planet `GetAllConfigurationsRequest` () | — | **not sent** `[V]` (grep: only the star one is built, `messenger.py:79`) | — | — |
| star `GetAllConfigurationsRequest` () | `1 configurations[]{1 star, 2 power:u64}` (`:78-85`) | `ConfigurationDataSource.prime_data`, once | none caught | Empty → star power `None` → 0 |

**Version hazard `[V]`:** a skyhook answer carries
`planet_resources_definitions_version`; the cache raises
`VersionNumberTooLargeError` for a version **newer** than the newest
definitions it holds (`planetary_resources_cache.py:87`). The server must
never hand out a skyhook version ahead of its definitions version. Keeping one
version for the whole universe and bumping it only when definitions change is
enough `[I]`.

### 2.6 `skyhook.api` — `sovereignty/skyhook/client/messenger.py`

Shared shapes `[V]`: `skyhook.TheftVulnerability{1 vulnerable:bool, 2 start, 3 end:Timestamp}`,
`skyhook.Workforce` oneof `available`: `1 amount:u64` \| `2 none:bool`,
`reagent.Simulation{1 secured_stock:u32, 2 unsecured_stock:u32, 3 last_cycle:Timestamp}`.

A skyhook record (in `GetResponse` and each list entry) is:
`skyhook`, `reagent_simulations[]`, `3 active:bool`, `reagent_definitions[]`,
oneof `vulnerability` (`5 theft_vulnerability` \| `6 unknown:bool`),
`workforce`, `planet_resources_definitions_version`. The client reads only
`reagent_type` and `simulation` of a simulation and `reagent_type` and
`definition` of a definition (`sovereignty/skyhook/data_type.py:14-29`).

**Field-number trap `[V]`.** The `reagent_type` field number differs per
message. A server that writes the wrong one sends reagent type 0, which the
client **silently drops** (`data_type.py:64`, `:72`): the skyhook shows no
reagent.

| message | simulation `reagent_type` / `simulation` | definition `reagent_type` / `definition` | workforce field |
|---|---|---|---|
| `GetResponse` | 5 / 4 | 5 / 2 | `7 workforce` |
| `GetAllLocalResponse.Skyhook` | 5 / 4 | **3** / 2 | `7 workforce` |
| `GetAllByCorporationResponse.Skyhook` | **1** / 4 | **1** / 2 | `7 workforce` |
| `AllInSolarSystemNotice.Skyhook` | 5 / 4 | **6** / 2 | `9 effective_workforce` (the one read); oneof `7 workforce_available`/`8 none` not read |
| `ReagentSimulationsNotice` | 6 / 4 | — | — |
| `ReagentDefinitionsNotice` | — | 3 / 2 | — |

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `GetRequest` (`1 skyhook`) | the record (`:27-46`) | `sovereigntyResourceSvc._GetSkyhookData` (`eve/client/script/ui/services/sovereigntyResourceSvc.py:316`), stale-vulnerability refresh (`skyhook/client/data_sources.py:70-94`) | 403 → forbidden (no backoff) | Empty → skyhook 0, inactive, no reagents |
| `GetAllLocalRequest` () | `1 solar_system`, `2 skyhooks[]` (`:48-65`) | local cache prime | any → backoff | Empty → `solar_system` 0 ≠ session system, so the cache **never primes** and every lookup re-asks (`data_sources.py:64`) `[V]` |
| `GetAllByCorporationRequest` (`1 corporation:u32`) | `1 skyhooks[]` (`:67-84`) | structure browser skyhook tab (`skyhookBrowserController.py:44`) | backoff | Empty → none |
| `ActivateRequest` / `DeactivateRequest` (`1 skyhook`) → () | nothing | skyhook window (`eve/client/script/ui/shared/skyhook/uiController.py:252`, `:266`) | 404, 403 (+500 on deactivate) | Empty → client believes it. **A lie** |
| `GetSolarSystemsWithTheftVulnerableSkyhooksRequest` () | `1 solar_systems[]` | map filter "vulnerable skyhooks" (`mapFilterVulnerableSkyhooks.py:25`, `mapViewWorld.py:850`) | backoff | Empty → none (honest) |
| `GetTheftVulnerableSkyhooksInSolarSystemRequest` (`1 solar_system`) | `1 skyhooks[]{1 skyhook, 2 planet, 3 expiry, 4 start}` | same, per listed system | backoff | Empty |
| `admin.ModifyReagentsRequest`, `admin.ScheduleTheftVulnerabilityWindowRequest` | — | QA tools only (`admin_modify_reagents`, `:138`) | — | answer 403 `[A]` (falsifier: a player-reachable menu that sends them) |

**Staleness rule `[V]`** (`data_type.py:132-158`): a skyhook that has a reagent
simulation but **no** theft vulnerability is "stale", and so is one whose
`vulnerable` flag disagrees with its window (`vulnerable` but now is outside
`[start, end]` plus 30 s, or not vulnerable but now is inside it). Every
lookup of a stale skyhook re-asks the server (`data_sources.py:70-94`). So
**every reagent skyhook must carry `theft_vulnerability`**, and when not
vulnerable its window must be the **next** one (or a past one), never one that
contains now.

### 2.7 `mercenaryden.api`, `mercenaryden.activity.api` — `sovereignty/mercenaryden/client/request_messenger.py`

Shapes `[V]`:

- `mercenaryden.Attributes`: `1 owner:character(u32)`, `2 skyhook`, `3 solar_system`,
  `4 planet`, `5 type:mercenarydentype(u32)`.
- `Evolution{1 definition:EvolutionDefinition, 2 simulation:EvolutionSimulation}`.
- `EvolutionSimulation`: oneof `state` `1 paused:bool` \| `2 started:bool`;
  `3 development{1 level:u32, 2 stage:DevelopmentStage, oneof 3 paused_at | 4 simulated_at}`;
  `4 anarchy` the same with `AnarchyStage`. Stages are enums `1..5`, `0` unspecified.
- `EvolutionDefinition`: `1 development{1 unit_increase_time_seconds, 2 stages[]{1 stage, 2 level_lower_bound, 3 level_upper_bound}}`,
  `2 anarchy{… same, plus 4 workforce_consumption:u64 per stage}`.
- `Infomorphs{1 definition{1 infomorph_type, 2 generation_tick_seconds:u64, 3 generation_bands[]{1 stage:DevelopmentStage, 2 lower_band, 3 upper_band}, 4 cargo_capacity:u32}, 2 contents{1 infomorphs_count:u32, 2 last_generation_tick:Timestamp}}`.
- Activity: `activity.Attributes{1 activitytemplate{1 name, 2 description: localization message ids, 3 dungeon, 4 development_impact:i32, 5 anarchy_impact:i32, 6 infomorph_bonus:u32}, 2 mercenary_den, 3 started:bool, 4 expiry:Timestamp, 5 solar_system}`;
  list entries are `{1 id:{1 uuid:bytes}, 2 attributes}`.

| request | response read | sender | codes | today |
|---|---|---|---|---|
| `api.GetAsOwnerRequest` (`1 id`) | `1 attributes` (all five), `2 enabled`, `3 evolution`, `4 infomorphs`, `5 cargo_extraction_enabled`, `6 skyhook_owner:corporation` (`:23-44`, `data/mercenary_den.py:102`) | den window (`sovereignty/mercenaryden/client/ui/controller.py:85`), "My dens" list (`mercDenUiController.py:40`) | 200 / **404 → `UnknownMercenaryDen`** / else generic | 404 (`is_not_found`) → `MercenaryDenUnknown` dialog |
| `api.GetAllOwnedRequest` () | `1 id[]` (`:144-164`) | "My dens" (`mercDenUiController.py:25`), job board adverts | non-200 → generic | Empty → no dens |
| `api.GetMaximumForCharacterRequest` () | `1 current_maximum`, `2 absolute_maximum` (u32) | "My dens" (`mercDenUiController.py:61`) | non-200 → internal error | Empty → "0 of 0" |
| `activity.api.GetAllRequest` () | `1 activities[]` (`:46-74`) | on character selection, async, 10 tries (`repository.py:88-147`); the job board in the space menu | non-200 → generic | Empty → none (honest; this is the call that used to stall the space menu) |
| `activity.api.GetForMercenaryDenRequest` (`1 mercenary_den`) | `1 activities[]`, `2 next_generation_at` (`:76-101`) | den window activities tab (`ui/controller.py:358`) | 404 → unknown den | 404 |
| `activity.api.StartRequest` (`1 id{uuid}`) | `1 attributes` (`:103-125`) | den window, objectives task (`objectives/client/objective_task/sovereignty_tasks.py:82`) | 404 unknown, **409 already started**, **403 validation failed** | 404 |
| `activity.api.GetCapacityRequest` () | `1 capacity:u32` (`:127-142`) | activity limit, memoized per load (`repository.py:269-272`) | non-200 → generic | Empty → capacity 0 |
| `api.admin.*` (anarchy, development) | — | QA only | — | 403 `[A]` |

### 2.8 Notices (server → client)

All arrive on `Notices/Consume`; the type name is the full proto name.

| notice (package) | fields read | client effect |
|---|---|---|
| `hub.api.ResourcesSimulatedNotice` | `hub`, `solar_system`, `power{…}`, `workforce{…}` | `OnSovHubResourcesSimulated` event (`sovereigntyResourceSvc.py`, `_OnResourcesSimulatedNotice`) |
| `hub.upgrade.api.HubUpgradesSimulatedNotice` | `hub_upgrades` | hub cache updated, the hub model's upgrade visuals redrawn, `OnHubUpgradesStateChanged` (`sovHubSvc.py:63-67`) |
| `hub.upgrade.api.HubUpgradesConfiguredNotice` | parsed, but **no listener is connected** (`sovHubSvc.py:54`) | none `[V]` |
| `hub.upgrade.api.UninstalledNotice` | not subscribed | none |
| **`resource.transfer.workforce.api`**`.ConfiguredNotice` / `.StateChangedNotice` | `solar_system`, `hub`, old/new config or state | `OnHubWorkforceConfigureChanged` / `…StateChanged` (`sovHubSvc.py:69-73`). **The package is `eve_public.sovereignty.resource.transfer.workforce.api`, not `hub.workforce.api`** `[V]` (descriptor of `hub/workforce/api/notices.proto`) |
| `skyhook.api.AllInSolarSystemNotice` | `solar_system` (dropped unless it is the session's), `skyhooks[]` | flush and re-prime the local cache (`skyhook/client/notice_listener.py:62-80`) |
| `skyhook.api.ReagentSimulationsNotice` / `ReagentDefinitionsNotice` | `skyhook`, `planet`, simulations or definitions (+ version) | cache update, `OnReagentsUpdated`, skyhook model silo levels |
| `skyhook.api.ActivationNotice` | `skyhook`, `active` | cache |
| `skyhook.api.WorkforceChangedNotice` | `skyhook`, `workforce` | cache |
| `skyhook.api.TheftVulnerabilityWindow{Scheduled,Started,Ended}Notice` | `skyhook`, times | cache; the skyhook model's raiding state |
| `mercenaryden.activity.api.{Added,ExpiryChanged,Started,Completed,Removed}Notice` | as in the descriptor | activity cache and den window (`mercenaryden/client/notice_listener.py`) |
| `mercenaryden.api.EvolutionStateChangedNotice` / `DefinitionsUpdatedNotice` | den, simulation / definitions | den window |

---

## 3. The machoNet side

### 3.1 Calls

| call | args → answer | sender | today |
|---|---|---|---|
| `sovMgr.GetSovHubFuelAccessGroup` / `SetSovHubFuelAccessGroup` / `IsOnLocalSovHubFuelAccessGroup` | `[V]` `eve/client/script/ui/services/sovSvc.py:371-381` | hub window, reagent bay | served (`src/service/sov.rs`, `sov_hub::fuel_access`) |
| `sovMgr.AcquireSkyhooks(skyhookIDs, accessListID)` → list of acquired ids | `sovSvc.py:386` | the post-claim cleanup window, `ProcessSkyhooks` (`skyhooksCleanup/cleanupWndController.py:119-133`) | `no_method` |
| `sovMgr.DestroySkyhooks(skyhookIDs)` → list of destroyed ids | `sovSvc.py:383` | same | `no_method` |
| `colonyResourcesMgr.GetMyCorporationSkyhooks()` → rows with `skyhookID`, `planetID`, `solarSystemID`, `typeID`, `state` | `sovereigntyResourceSvc.py:251`; row attributes read in `skyhookBrowserController.py` and `sovHubEntryController.py:137` | structure browser, alliance hub list (count per system) | not served |
| `beyonce.CallComponentFromClient(itemID, componentName, method, *args)` | bound ballpark call | see 3.2 | **not served** (no arm in `src/service/space.rs` `beyonce`) |

The cleanup window lists the previous holder's skyhooks from the **ballpark's
slim items** (`cleanupSovSystemWindow.py:79`), prices each acquisition at
`sovereignty.hub.const.SKYHOOK_CLAIM_FEE` from the corp wallet, and has a
deadline of `ALLOWED_DURATION_AFTER_SOVHUB_CLAIM` after the hub's
`claimTime` (`cleanupWndController.py:114-117`) `[V]`. Our ledger already has
the "Skyhook claim fee" entry label (`src/ledger.rs`).

### 3.2 Component calls (`CallComponentFromClient`)

| component | method, args → answer | sender |
|---|---|---|
| `orbitalSkyhook` | `IsCharacterAuthorised` () → bool | skyhook window (`spacecomponents/client/components/orbitalSkyhook.py:238`) |
| `orbitalSkyhook` | `ExtractReagentsToShip` (`flagID`, `qty`) → qty moved | skyhook window take (`orbitalSkyhook.py:244`, `skyhook/uiController.py:392`) |
| `mercenaryDen` | `ExtractInfomorphsToShip` (`flagID`, `qty`) → qty moved; client pre-clips `qty` to the hold's free volume and raises `MercenaryDenNoUnitsFitInShipCargo` itself (`repository.py:335-353`) | den window cargo tab (`spacecomponents/client/components/mercenaryDen.py:100`) |
| `linkWithShip` | `InitiateLink` / `InitiateLinkQuick` () | the theft link on a skyhook's reagent silo (`spacecomponents/client/components/linkWithShip.py:233-240`) |

### 3.3 Server notifies (macho)

| notify | args | client |
|---|---|---|
| `OnSkyhookFirstActivation` | `(skyhookID, planetID)` | flushes the skyhook cache, re-reads vulnerability (`sovereigntyResourceSvc.py:74-78`), PI window. **Nothing in the client scatters it** `[V]` (grep), so it is a server notify `[I]` |
| `OnMercenaryDenRegistrationSuccess` / `OnMercenaryDenRegistrationInvalid` | `(notifyMsg, notifyArgs)`; `notifyArgs['itemID']` | a dialog by message name, and the den added to the lists (`repository.py:98-106`) |
| `OnMyMercenaryDenRemoved` | `(notifyMsg, notifyArgs)` with `itemID` | den removed from lists (`repository.py:93-96`) |

### 3.4 Slim-item fields

| ball | fields the client reads |
|---|---|
| skyhook (category orbital, `invConst.groupSkyhook`) | `planetID`, `skyhook_state` (`orbitalSkyhook.const` `STATE_*`: shield/armor/hull vulnerable and reinforce), `skyhook_timer`, `component_activate` = `(done:bool, activation time)`, `ownerID` / corp (menu checks) (`orbitalSkyhook.py:40-63`, `inflight/selectedItemTimers.py:46`, `:72`) |
| reagent silo (`invConst.typeSkyhookReagentSilo`) | `component_skyhookReagentSilo_skyhookID`, `component_linkWithShip` = `(addedAt, linkState, completeAt, linkedShipID)` with `LINKSTATE_IDLE/RUNNING/COMPLETED/DISABLED` (`skyhookReagentSilo.py:41-43`, `linkWithShip.py:115-120`, `spacecomponents/common/components/linkWithShip.py:6-9`) |
| mercenary den (`invConst.groupMercenaryDen`) | `component_activate`, `ownerID` (= the pilot: `OfferAccessMercenaryDen` needs `IsOwnedByMe`, `celestialCheckers.py:185-191`) |

---

## 4. Installing an upgrade, end to end

1. The pilot opens **Configure hub** on the hub ball (needs corp ownership and
   station-manager role, `celestialCheckers.py:1076-1084`). The window primes
   four things in parallel: `GetHubUpgrades`, `GetResources`,
   `GetDefinitions` (once) and fuel `Get` (`hubController.py:96-108`) `[V]`.
   It also reads the system's maximum power and workforce from the resource
   definitions (`sovereigntyResourceSvc.GetMaxPowerAndWorkforce`, `:235`).
2. **The item comes from the pilot's ship.** The pilot drags an inventory
   item into the window. The drop is refused unless (`hubController.py:170-190`) `[V]`:
   the type is in `upgrades.const.UPGRADE_GROUP_IDS` (four groups,
   `sovereignty/upgrades/const.py:16`); that type is not already installed;
   no installed upgrade shares its `mutually_exclusive_group`; the ship is
   within `const.maxCargoContainerTransferDistance` of the hub; the system has
   been held long enough for the type's `attributeDevIndexSovereignty`
   (`devIndexUtil.GetTimeIndexLevelForDays` over `claimTime`); and
   `rec.locationID == session.shipid` (in the ship, not a station).
   The power check is a stub that always passes (`:217`).
3. The window adds the item locally as "new" and lets the pilot reorder and
   toggle online/offline. **Order is priority**: the simulation walks the list
   top-down (§5.2).
4. **Save** sends one `ProcessConfigurationRequest`: `new_upgrades` = the item
   ids dropped this session, `configuration` = every upgrade in window order
   as `(type id, online)`, `hub` (`hubController.py:348-352`) `[V]`. The
   server must:
   - move each `new_upgrades` item out of the pilot's ship into the hub
     (it is consumed; uninstall destroys, `ConfirmDestroyUpgrade`) `[I]`;
   - re-check the step-2 rules server-side and answer 400 (bad request),
     403 (not allowed), 404 (item or hub gone), 409 (conflict, e.g. the
     configuration changed under it) `[A]` mapping (falsifier: a client
     dialog that names a different cause for one of these codes);
   - store the list in that order with `online` flags, run §5.2, deduct
     startup reagents, and answer `HubUpgrades` **in the same order**.
5. The client compares `[(type, state)]` before and after
   (`hubController.py:349-362`). Equal → "configuration saved"; different →
   `ConfigurationSavedNotSame` and a logged exception. So the server's state
   algorithm must match the client's (§5.2) on the same inputs.
6. The server then publishes `HubUpgradesSimulatedNotice` (the connected one;
   not `Configured`, §2.8) so other open windows and the hub model update
   `[A]` target: the system's pilots (falsifier: the hub model in space not
   updating for a pilot in the system).

Uninstall is a separate `UninstallRequest` keyed by `(hub, upgrade_type)`, and
the `upgrade_type` value the client sends is whatever the server put in
`HubUpgrades.Upgrade.identifier.upgrade_type` (`proto_parsers.py:20`). Use the
installation type id there `[A]` (an internal key; falsifier: none needed, the
client only echoes it).

---

## 5. Power, workforce and reagents

### 5.1 The system budget

- **Maximum power** = sum over the system's planets of the definition `power`
  + the star configuration `power`; **maximum workforce** = sum of planet
  `workforce` (`sovereigntyResourceSvc.py:235-249`) `[V]`. Planets come from
  the client's own map cache, so definitions must use real planet item ids.
- A planet yields **one** kind: reagent if it has a reagent definition, else
  power, else workforce — that is the order the info panels read it
  (`sovereigntyResourceSvc.GetPlanetResourceInfo`, `:218-231`) `[I]`
  (falsifier: a planet info panel showing two kinds at once).
- `GetResources.power.available` and `workforce.available` are what the hub
  window starts its simulation from (`hubUtil.py:17-18`, via
  `hubController._availableLocalPower`); `allocated` is display;
  `local_harvest` is display ("harvested here") `[V]` reads, meaning `[I]`.
  The window shows **total available workforce** =
  `available + produced by online upgrades` and **total maximum** =
  `max + produced + imported` (`hubController.py:483-508`) `[V]`.
- What makes a planet's power or workforce "available" is server-only. The
  Equinox rule is that a planet's yield is harvested by an **active skyhook**
  over it `[A]` (falsifier: the skyhook window's power/workforce text for an
  inactive skyhook showing it counted). Until SKY-1 lands, SOV-RES ships
  "available = maximum" `[A]` (falsifier: owner ruling).
- Workforce moves between hubs by the workforce configuration (§2.4): an
  exporting hub gives `amount`, an importing hub receives from its sources,
  `transit` passes through `[I]` from the message names. The amounts and the
  range of "networkable" hubs are server design `[A]`.

### 5.2 Upgrade power states — the rule the server must reproduce `[V]`

`hubUtil.UpdateEffectiveStatesForUpgrades` (`sovereignty/client/sovHub/hubUtil.py:15-72`),
stated as a rule (not copied):

- Walk the upgrades in configured order with running totals of power,
  workforce and per-reagent fuel used, starting from the hub's available power
  and workforce.
- An upgrade lacks power (workforce) if its requirement plus what is already
  used exceeds the running available total.
- If it has a fuel type: when it is new, or was pending or offline before, its
  `fuel_startup_cost` is due; it lacks startup reagent if that cost plus the
  reagent already used exceeds the hub's current amount. Its fuel cost is the
  startup cost (if due) plus **one hour** of burn if it burns at all; it lacks
  fuel if that plus the used amount exceeds the stock.
- A configured-online upgrade that lacks nothing **uses** its power, workforce
  and fuel cost.
- Its state: configured offline → `OFFLINE`; lacking fuel, power or workforce
  → `LOW` (the client shows `OFFLINE` instead while it has unsaved edits);
  lacking only the startup reagent → `PENDING`; otherwise `ONLINE`, and an
  online upgrade **adds** its `power_produced` / `workforce_produced` to the
  running available totals for the upgrades after it.

The server answers with the states this rule gives on its own stored inputs.
Write a parity test that feeds the same invented inputs to the Rust port and
checks the states by hand-worked expectation.

### 5.3 Reagent fuel `[V]`

- The hub holds reagents by type; fuel `Get` returns
  `(amount at last_updated, burned_per_hour)`; the client shows
  `amount_now = max(0, amount − int(elapsed_hours × burned_per_hour))`
  (`fuel/client/data_types.py:47-52`). The server must burn the same way
  (continuously, truncated per read), not in day-sized steps, or the
  displayed amount jumps when the server catches up `[I]`.
- `burned_per_hour` of a reagent = sum of `fuel_consumption_per_hour` over
  functional (online) upgrades using it (`hubUtil.GetConsumptionPerHour`,
  `:96-106`) `[V]` as the client displays it; the server value should equal
  it `[I]`.
- Startup cost: due for every upgrade going from offline or pending to on,
  paid once (`hubUtil.GetStartupCostForUpgrades`, `:137-153`) `[V]`.
- An online upgrade whose reagent runs out goes `LOW` at the next simulation
  `[I]`; the server re-simulates on burn-out and publishes
  `HubUpgradesSimulatedNotice` `[A]` (falsifier: the client showing a stale
  ONLINE after its reagent hit 0 when the window reopens).

### 5.4 Skyhook reagent harvest `[V]`

`reagent_split_production_simulation.get_reagent_split_production_data_at_time`
(`:8-33`), as a rule: whole cycles since `last_cycle` (`cycle_period`);
each cycle adds `amount_per_cycle`; the secured share is
`int(secured_percentage/100 × produced)`, the rest unsecured; each pool is
capped at its capacity and overflow spills into the other pool's free room,
then is lost; `last_cycle` advances by whole cycles only. The server stores
`(secured, unsecured, last_cycle)` and applies the same rule at any read.
**`cycle_period` must never be 0** (the client divides by it, `:11`).

---

## 6. The skyhook lifecycle

1. **Deploy.** A skyhook item is category orbital, so the cargo menu offers
   "Launch for Corporation" (`menusvc.py:3329-3340`, `eveMisc.LaunchFromShip`
   with the corp as owner) → `ship.Drop`, the same call our hub launch already
   answers (`src/service/inventory.rs` `"Drop"` chain) `[I]` (falsifier: the
   item's menu showing another launch entry). Placement rules (at a planet,
   one per planet, in a system the corp's alliance holds) are server design
   `[A]` (falsifier: a client-side refusal message for a specific rule).
2. **Activation.** The ball carries `component_activate`; the model builds over
   the activation duration (`orbitalSkyhook.py:38-53`). On completion the
   server sends `OnSkyhookFirstActivation(skyhookID, planetID)` (§3.3) and
   `ActivationNotice` / `AllInSolarSystemNotice`.
3. **Running.** `active` true; the planet's yield counts toward the system
   (§5.1); a reagent planet fills the two pools (§5.4). The owner turns it on
   and off with `Activate` / `Deactivate`, and takes reagents with
   `ExtractReagentsToShip` (authorised by `IsCharacterAuthorised`). The
   skyhook window shows workforce from the record's `workforce` oneof.
4. **Theft windows.** Each reagent skyhook has a theft-vulnerability window
   (`TheftVulnerability`, the map filter requests, the three window notices).
   While vulnerable, a hostile pilot links to the skyhook's **reagent silo**
   ball (`InitiateLink`); the silo's `component_linkWithShip` drives the
   "being raided" visual (`orbitalSkyhook.py:69-99`). The unsecured pool is
   what is at risk: the silo controller variable for it is named
   `RaidablesiloValue` (`orbitalSkyhook/const.py`, `SILO_INSECURE_VALUE`) `[I]`.
   Window length, frequency and link duration are server design `[A]`
   (EvEJS models these, the unpublished EvEJS inventory note "Sov:
   skyhooks"; hints only).
5. **Combat states.** `skyhook_state` / `skyhook_timer` follow the
   shield → armor reinforce → armor → hull reinforce → hull cycle of
   `orbitalSkyhook.const.STATES` `[V]` names; timers are `[A]`.
6. **Sov change.** When a new holder claims the system, for
   `ALLOWED_DURATION_AFTER_SOVHUB_CLAIM` it may acquire (paying
   `SKYHOOK_CLAIM_FEE` each from the corp wallet) or destroy the old
   holder's skyhooks (§3.1) `[V]`. What happens to the rest after the deadline
   is server design `[A]`.

---

## 7. The mercenary-den lifecycle

1. **Deploy.** A den is owned by a **character** (`Attributes.owner` is a
   character id; the access menu needs `IsOwnedByMe`) and sits at a **planet
   with a skyhook** (`Attributes.skyhook`, `Attributes.planet`,
   `GetAsOwnerResponse.skyhook_owner` is a corporation;
   `celestialCheckers.py:185`) `[V]` shapes,
   `[I]` meaning. Launch is "Launch for Self" → `ship.Drop` `[I]`. The server
   validates after the drop and answers with
   `OnMercenaryDenRegistrationSuccess` or `…Invalid` (§3.3). The number of
   dens a pilot may own is `GetMaximumForCharacter` (`current` grows with the
   den-management skill `typeMercenaryDenManagementSkill` `[I]` from
   `mercDenWnd.py:330`; values `[A]`).
2. **Workforce.** A den consumes workforce by anarchy stage
   (`AnarchyStage.workforce_consumption`); the den window shows it as a share
   of the planet's workforce (`repository.py:327-333`) `[V]`. So the den's
   planet should be a workforce planet `[I]` (falsifier: a den window showing
   0 % on a reagent planet as normal).
3. **Evolution.** Development and anarchy levels rise by one per
   `unit_increase_time_seconds` while not paused; the client extrapolates
   `level = base + elapsed / unit` from `simulated_at`
   (`repository.py:288-312`) `[V]`. Stages come from the level bands.
   Activities move the levels by their impacts on completion.
4. **Infomorphs.** Generated every `generation_tick_seconds`, a count within
   the band of the current development stage, up to `cargo_capacity`; the
   owner extracts them to the ship with the component call (§3.2).
   `cargo_extraction_enabled` gates that `[I]`.
5. **Activities.** The server generates activities per den (from templates:
   name and description message ids, a dungeon, impacts, bonus) with an
   `expiry`, announces them (`AddedNotice`), the owner starts one
   (`StartRequest`; 409 if started, 403 if invalid), the dungeon runs, and
   completion sends `CompletedNotice` with the impacts. `GetCapacity` is the
   pilot's concurrent-activity limit `[I]`. Templates and values are server
   design `[A]` (EvEJS uses a default capacity and an absolute maximum,
   `corp-sov-structures.md` "Effort to port"; hints only).
6. **Removal.** Destroyed in space (`__server_removed_from_space_exploding`
   on the slim triggers the client's own signal, `mercenaryDen.py:85-93`) or
   removed → `OnMyMercenaryDenRemoved`.

---

## 8. What the server has today

- `src/gateway.rs` `answer`: workforce `GetConfiguration` and `GetState`
  answer `inactive`; den `GetAsOwner`, `activity.GetForMercenaryDen` and
  `activity.Start` answer 404 (`is_not_found`); everything else in §2 gets
  the empty typed message. Four of those empties **report success for a write
  that did not happen**: fuel `Add`, upgrade `ProcessConfiguration` (followed
  by the mismatch dialog), workforce `Configure`, skyhook `Activate` /
  `Deactivate`. `GetAllLocal`'s empty `solar_system` defeats the client cache
  (§2.6).
- `src/gateway_ext.rs` `CHAIN`: skill plans, PLEX vault, SKINs. No sov arm.
- `src/sov.rs`, `src/sov_hub.rs`, `src/service/sov.rs`, `src/service/sov_hub.rs`:
  claims, the hub item and ball (`launch` via `ship.Drop`), GM capture, the
  fuel access-group trio, and **stand-in fuel**: fuel-block stacks moved in by
  `/sovfuel` / `/boostsov`, `FUEL_PER_DAY` burned at the rollover, the claim
  lapsing after `GRACE_DAYS` offline.
- `src/ledger.rs`: the "Skyhook claim fee" entry label. Nothing else for
  skyhooks or dens.
- `src/service/space.rs` `beyonce`: no `CallComponentFromClient`.
- `src/gateway_out.rs`: the notice outbox, ready to use.

---

## 9. Corrections to the drafted split

| draft | correction |
|---|---|
| SOV-RES | Right scope. Add: answer `GetAllDefinitions` with **304** when `known_version` equals ours; one universe-wide version; publish `ResourcesSimulatedNotice` when a hub's numbers change. `GetResources` needs the active-skyhook rule (§5.1), so it ships "available = maximum" `[A]` until SKY-1. |
| SOV-UPG-1 | Drop `VerifyInstallRequirements` and `AuthorizeManage`: `eve.`-internal, never sent by the client. `Get` is never sent either. The real set: `GetDefinitions`, `GetHubUpgrades`, `ProcessConfiguration` (carries the items), `Uninstall`. The power-state rule is §5.2 and must match the client's. "Burn added to the rollover's fuel use" is wrong: upgrades burn **colony reagents** per hour through `hub.fuel.api`, continuously (§5.3). Split the reagent bay out (new SOV-FUEL). |
| SOV-UPG-2 | Unchanged. |
| SKY-1 | No skyhook `GetState`: the requests are `Get`, `GetAllLocal`, `GetAllByCorporation`, `Activate`, `Deactivate`. Add the field-number table (§2.6) to its tests. The component calls need a `beyonce.CallComponentFromClient` seam that nobody owns (new COMP-CALL). |
| SKY-2 | There is no `resource.transfer` request for reagents; `resource.transfer.workforce.api` is only the **package of the workforce notices**. Reagents reach the hub by the owner extracting to the ship (`ExtractReagentsToShip`) and depositing in the reagent bay (`fuel.Add`). SKY-2 is theft only: windows, the silo ball, `InitiateLink`, the two map-filter requests, the three window notices. |
| (missing) | **Workforce transfer** (`Configure`, `GetNetworkableHubs`, real state, the two notices) has no piece: new SOV-WF. |
| (missing) | **The reagent bay** (`fuel.Get`, `fuel.Add`) has no piece: new SOV-FUEL. It touches owner decision S-2 (§11 Q1). |
| MERC-1 | The placement `[A]` (needs a skyhook at the planet) is now `[I]` from the shapes (§7.1). Add the registration notifies and `ExtractInfomorphsToShip` (needs COMP-CALL). |
| MERC-2 | Add `GetCapacity` and the five activity notices; completion moves evolution levels. |

---

## 10. The build plan

Paths are under `server/crates/eve-server/` unless they say otherwise. Every
piece: unit tests with invented ids and invented rows only; no client value in
a test; a `docs/status/progress.tsv` row update. Each new arm is one
coordinator line in `gateway_ext::CHAIN`; each new module one `lib.rs` line;
each delta kind one `delta_kinds.rs` line (all maintainer lines, listed in the
PR). Content tables are authored, `[A]` in their header, under `content/sov/`
(new directory), registered by one `content_tables.rs` `TABLES` line.

**1. SOV-GW-0 — honest sov stubs (new, S).** Depends: none.
- Owns: `src/sov_gw_stub.rs` (new).
- An arm, placed **last** of the sov arms in `CHAIN`, that answers the four
  lying writes with 403 (fuel `Add`, upgrade `ProcessConfiguration` and
  `Uninstall`, workforce `Configure`, skyhook `Activate`/`Deactivate`) and
  `GetAllLocal` with the pilot's current solar system and no skyhooks; admin
  requests 403. Each later piece's arm sits before it and takes over its types.
- Tests: each type → expected status; `GetAllLocal` field 1 equals the pilot's
  system (invented pilot and system).
- `[A]` 403 reads as "not allowed" in each window (falsifier: a traceback from
  the messenger on 403).
- Status: "Sov hub upgrades" stays `stubbed` (evidence: honest refusals).

**2. SOV-RES (#78, M).** Depends: none (this doc).
- Owns: `src/sov_resources.rs` (new), `content/sov/planet_resources.tsv` (new),
  `tools/client-ci/scenarios/fci_sov_resources.py` (new).
- Arms: `resource.planet.GetAllDefinitions` (304 on equal version),
  `GetDefinitionsVersion` (current version or 404), `resource.star.GetAllConfigurations`,
  `hub.GetResources`. Exports `max_power(system)`, `max_workforce(system)`,
  `planet_yield(planet)` and `RESOURCES_VERSION` for later pieces. Publishes
  `ResourcesSimulatedNotice` on change.
- Tests: encode/decode round trip on field numbers (§2.5); 304 path;
  `max_power` = Σ planets + star on an invented system; a reagent definition
  never has `cycle_period` 0.
- `[A]` yields by planet type from the authored table (falsifier: owner
  balance ruling); `[A]` available = maximum until SKY-1 (falsifier: owner
  ruling or the Equinox rule confirmed in a client string).
- Scenario: the hub window shows non-zero maximum power.

**3. SOV-FUEL — the reagent bay (new, M).** Depends: SOV-RES; owner answer to
§11 Q1.
- Owns: `src/sov_hub_fuel.rs` (new). One call in `src/sov_hub.rs` (the
  rollover) only if Q1 says the stand-in goes; list it in the PR.
- Arms: `hub.fuel.Get`, `hub.fuel.Add` (moves `amount` from the named stack in
  the pilot's ship to the hub; 403 not in the fuel access group, 404 item or hub
  gone). Stock per hub per reagent type with `last_updated`; continuous burn by
  §5.3; exports `burn_rate_set(hub, type, per_hour)` and `stock_at(hub, type, t)`.
- Tests: `amount_now` matches the client formula at hour boundaries and
  between them; a split stack deposit; refusal when out of range.
- `[A]` range = `TRANSFER_RANGE` of `sov_hub.rs` (falsifier: a client range
  check in the bay with a different constant).

**4. SOV-UPG-1 (#79, L).** Depends: SOV-RES, SOV-FUEL.
- Owns: `src/sov_upgrade.rs` (new), `src/sov_upgrade/` (new),
  `content/sov/upgrades.tsv` (new), `tools/client-ci/scenarios/fci_sov_upgrade.py` (new).
- Arms: `GetDefinitions` (from the table), `GetHubUpgrades`,
  `ProcessConfiguration` (§4 steps 4-5; needs the `Registry`, the
  `skins.rs:784` pattern), `Uninstall` (destroys). The §5.2 rule as a pure
  function; startup cost deducted through SOV-FUEL; burn rates set per reagent;
  re-simulate on reagent exhaustion; `HubUpgradesSimulatedNotice`.
- Tests: the §5.2 parity cases (order matters; produced power feeds later
  upgrades; startup-only shortfall gives `PENDING`; unsaved-edit display is
  client-only and **not** ported); response order equals request order; each
  refusal code.
- `[A]` the 400/403/404/409 mapping of §4; `[A]` definitions values from the
  authored table (falsifier: owner balance ruling).

**5. SOV-UPG-2 (#80, M).** Unchanged from the draft. Depends: SOV-UPG-1.

**6. SOV-WF — workforce transfer (new, M).** Depends: SOV-RES.
- Owns: `src/sov_workforce.rs` (new).
- Takes `GetConfiguration`/`GetState` over from `gateway.rs` (one maintainer
  edit removing that `answer` arm), adds `Configure`, `GetNetworkableHubs`;
  state from configuration plus the hubs' workforce; the two notices **under
  the `resource.transfer.workforce.api` package name**.
- Tests: every configuration and state oneof branch encodes with its field
  number; an unset mode is never written; import from two sources sums.
- `[A]` networkable = same alliance within N jumps, N authored (falsifier:
  owner ruling).

**7. COMP-CALL — `beyonce.CallComponentFromClient` (new, S).** Depends: none.
- Owns: `src/space_component.rs` (new). One maintainer arm in
  `src/service/space.rs` `beyonce` routing to it.
- A table `(component name, method) → fn(reg, actor, item_id, args)`, like the
  other registration tables; unknown pairs answer the client's normal
  `no_method` error.
- Tests: routing with an invented component; the item must be in the caller's
  park.

**8. SKY-1 (#81, L).** Depends: SOV-RES, COMP-CALL.
- Owns: `src/skyhook.rs` (new), `src/service/skyhook.rs` (new),
  `tools/client-ci/scenarios/fci_skyhook.py` (new).
- Launch through the `ship.Drop` chain (one maintainer line in
  `src/service/inventory.rs`); the ball and slim (§3.4); activation and
  `OnSkyhookFirstActivation`; gateway `Get`, `GetAllLocal`,
  `GetAllByCorporation`, `Activate`, `Deactivate`; harvest by §5.4; the
  component calls `IsCharacterAuthorised` and `ExtractReagentsToShip`;
  `colonyResourcesMgr.GetMyCorporationSkyhooks`; `sovMgr.AcquireSkyhooks` /
  `DestroySkyhooks` with the claim fee under its ledger label; every reagent
  skyhook carries a non-stale `theft_vulnerability` (§2.6) even before SKY-2
  (always "not vulnerable, next window far ahead" `[A]`). Feeds SOV-RES's
  `available` (active skyhooks only).
- Tests: the §2.6 field-number table per message; the §5.4 harvest parity
  (overflow both ways, partial cycle); version never ahead of SOV-RES's;
  staleness invariant for every emitted record.
- Maintainer lines: `DOMAINS` row for `colonyResourcesMgr`, the two
  `sovMgr` methods, the delta kind, one `CHAIN` line, the drop line.

**9. SKY-2 (#82, M).** Depends: SKY-1, COMP-CALL.
- Owns: `src/skyhook_theft.rs` (new) (the draft's `sov_upgrade.rs` ownership
  goes away: reagents reach the hub through SOV-FUEL).
- Window schedule, the three window notices, the two map-filter requests, the
  silo ball, `InitiateLink`/`InitiateLinkQuick` via COMP-CALL, link
  completion moves unsecured stock to the thief.
- `[A]` window length and cadence, link time, share stolen (falsifier: owner
  ruling; §11 Q3).

**10. MERC-1 (#83, L).** Depends: SKY-1, COMP-CALL.
- Owns: `src/merc_den.rs` (new), `content/sov/merc_dens.tsv` (new),
  `tools/client-ci/scenarios/fci_merc_den.py` (new).
- Launch for self via `ship.Drop`; registration notifies; `GetAsOwner`,
  `GetAllOwned`, `GetMaximumForCharacter`; evolution and infomorph generation
  by §7.3-7.4; `ExtractInfomorphsToShip`; `OnMyMercenaryDenRemoved`.
  Maintainer edit: remove the den rows from `gateway.rs` `is_not_found`.
- Tests: evolution extrapolation parity (paused, unpaused, unit 0); generation
  within the stage band; extract clipped to capacity and count.
- `[A]` maxima, tick lengths, bands (falsifier: owner balance ruling).

**11. MERC-2 (#84, M).** Depends: MERC-1.
- Owns: `src/merc_den/` (new), `content/sov/merc_den_activities.tsv` (new).
- `GetAll`, `GetForMercenaryDen` (with `next_generation_at`), `Start`
  (409/403), `GetCapacity`; the five activity notices; completion applies
  impacts. Templates reference dungeons by id from our dungeon table.
- `[A]` capacity and templates (falsifier: owner ruling).

Order: 1 and 7 any time; 2 → 3 → 4 → 5; 2 → 6; 2 + 7 → 8 → 9; 8 → 10 → 11.

---

## 11. Open questions

1. **Hub fuel (owner decision S-2).** The client's hub burns colony reagents
   per upgrade through the reagent bay; our stand-in burns fuel blocks per hub
   per day. Keep both (blocks keep the claim, reagents run upgrades), or retire
   the stand-in when SOV-FUEL lands? SOV-FUEL and SOV-UPG-1 depend on it.
2. **What makes yield available.** Ship SOV-RES with "available = maximum" and
   gate on active skyhooks in SKY-1, or gate from the start (then nothing works
   until SKY-1)?
3. **Theft and dens in this economy** (already filed as decision 24): do bots
   raid skyhooks; what a den's infomorphs are worth.
4. One client run settles most `[I]`s at once: deploy a skyhook and a den with
   GM help, open each window, and read the client log for messenger
   tracebacks (`evecap --alerts`).
