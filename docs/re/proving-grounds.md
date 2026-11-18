# The Proving Grounds, end to end

Implementation spec for the Proving Grounds (PvP filaments): every call the
client makes to `pvpFilamentMgr`, every push it listens for, the error
vocabulary, the event model it expects, how a queued pilot reaches an arena
and leaves it, and a build plan cut into claimable pieces. Issue #63 (PG-0).

Read off this client's bytecode: the `pvpFilaments` package, the abyss client
service, the menu, the crimewatch timers and the graphics controller registry.
Line numbers are the **real** `co_lnotab` lines (`tools/dis.py`), not the
decompiler's, so they match a client traceback. The shipped static data was
read for shapes and counts only; no ids or values from it appear here.

**Evidence markers.** `[V]` verified in this client's bytecode or shipped data
(`[V-data]` for the data), with the place. `[I]` inferred from `[V]` facts,
with what would falsify it. `[A]` an assumption we choose to ship, with its
falsifier; an `[A]` value never goes into a test. EvEJS was consulted for
hints only (the unpublished EvEJS inventory note, AGPL): nothing below rests on it, and where it
agrees it is noted as a hint, never as evidence.

Nothing here has been run against a client.

---

## 1. Headline

- The feature is **one remote service, `pvpFilamentMgr`, with nine methods**,
  plus **six pushes**, plus the abyss's own pushes and the abyssal session
  change reused for the arena. The client keeps no schedule of its own: every
  event it shows comes from the server's answers `[V]` (§2.3).
- Two of the nine (`GetLeaderboard`, `GetCharacterStatistics`) are
  **request/push**: the call's return is discarded and the answer arrives as
  a push `[V]` (§2.2).
- Queue refusals are `UserError('PVPJumpError', {'errors': [(code, args), ...]})`,
  the same shape as the abyss's `AbyssError` `[V]` (§4).
- The match itself is **an abyssal pocket**: the arena is a one-room abyssal
  dungeon, the pilot is moved there by the server (no client call is pending
  while queued), the PvP timer is the abyss's `AbyssalPvpExpiration`, and the
  way out is the `typeAbyssPvPGate` ("Proving Conduit"), whose menu calls
  `pvpFilamentMgr.AbyssalPVPEndGateActivation` inside an `abyssToKnownJump`
  session change `[V]` (§5).
- **One client trap**: opening the Proving Grounds window from the Neocom with
  **no active event at all** raises `UnboundLocalError` in the window's
  controller `[V]` (§3.4). The server must never return an empty
  `GetActiveEvents`.
- **Server today**: nothing is served. `abyss.rs` knows the PvP gate type
  (`PVP_GATE`) and strips it, and the conduit's room objects, from PvE rooms
  (`is_stray`, used by `site.rs`). The two static tables are not in the
  content bundle (§6).

---

## 2. The wire map

### 2.1 Calls (client → server), all on `sm.RemoteSvc('pvpFilamentMgr')`

All in `pvpFilaments/client/pvpFilamentSvc.py` unless noted. "Svc" is the
client service `svc.pvpFilamentSvc`.

| # | call | args | return the client uses | caller, and what it does with it |
|---|---|---|---|---|
| 1 | `JoinPVPQueue(itemID, typeID)` | the filament's itemID and typeID | **`estimatedQueueTime`**: a number; `< 1` shows "unknown" | `RequestToJoinPVPQueue` :31 `[V]`, from the activation controller's `_JoinQueue` (`pvpFilamentController.py:207`), run in `JoinQueue` :213. A `UserError` whose `msg == 'PVPJumpError'` puts `error.dict['errors']` in the window; any other `UserError` is re-raised; any other exception shows `UNKNOWN_ERROR` `[V]` |
| 2 | `LeavePVPQueue(itemID, typeID)` | same | discarded | `RequestToLeavePVPQueue` :34 `[V]`; from the window's button (`LeaveQueue` :235) and from **closing the window while queued** (`Close` :242) |
| 3 | `GetAllEvents()` | none | a **dict**; the client takes `.values()`, a list of event infos (§3.1) | `GetAllEvents` :102, `@Memoize(5)` `[V]`; `GetAllEventDataByEventID` :112 builds the per-event cache from it **once per session** (never rebuilt) `[V]` |
| 4 | `GetActiveEvents()` | none | a **dict**; `.values()` as above | `GetActiveEvents` :106, `@Memoize(5)` `[V]`; feeds `GetActiveEventDataByTypeID` :118 (keyed by `filamentTypeID`: two active events on one filament collide, last wins) and the event window (§3.4) |
| 5 | `GetMostRecentEvent()` | none | one event info, or `None` | `GetMostRecentEvent` :110, `@Memoize(5)` `[V]`; event window fallback when nothing is active |
| 6 | `GetNextEventDate()` | none | anything printable; shown with `u'{}'.format(...)` | `GetNextEventDate` :126 `[V]`; only in the past-event window (`window.py:296`, `_construct_past_event_left_side`). `None` prints as the text `None` `[I]` |
| 7 | `GetLeaderboard(matchTypeID, scheduleID)` | ints from the event info | **discarded**; the answer is push `OnPVPFilamentsLeaderboard` | `_FetchLeaderboard` :47 `[V]`; at most once a minute per event (`LEADERBOARD_CACHE_TIME`, `_ShouldFetchLeaderboard` :141) |
| 8 | `GetCharacterStatistics(matchTypeID, scheduleID)` | same | **discarded**; the answer is push `OnPVPFilamentsCharacterStatistics` | `GetCharacterStatistics` :78 `[V]`; only while the cached statistics are empty and no fetch is pending |
| 9 | `AbyssalPVPEndGateActivation(itemID)` | the conduit's itemID | discarded; the call **is** a session change | `movementFunctions.RealActivatePVPAbyssalEndGate` :624-625: `sessionMgr.PerformSessionChange('abyssToKnownJump', pvpFilamentMgr.AbyssalPVPEndGateActivation, itemID)` `[V]`. Reached from the gate's menu entry (`menusvc.GetAbyssalGateFunction` :4199, the `typeAbyssPvPGate` arm :4206; `ActivatePVPAbyssalEndGate` :3558, `interactionRange=maxStargateJumpingDistance`) and from double-click (`DockOrJumpOrActivateGate`) `[V]` |

The units of `estimatedQueueTime` are **filetime ticks** `[I]`: the window
formats it with `FormatTimeIntervalShortWritten(long(x))`, the same formatter
it feeds `GetWallclockTime() - queueStart` for the time already queued
(`pvpFilamentController.py:140`, `:147`). Falsified by a client that shows
an answer of `10 * 10**7` as anything other than 10 seconds.

The `findings/client-calls.tsv` rows for the first eight agree on lines and
arity `[V]`. EvEJS answers the same nine names with the same return/push split
(hint only).

### 2.2 Pushes (server → client)

| push | args | handler | what the handler needs |
|---|---|---|---|
| `OnPVPFilamentsLeaderboard` | `info` | `pvpFilamentSvc.py:50` `[V]` | `info['matchTypeID']`, `info['scheduleID']` (both looked up through `GetEventID`, so the event **must** be in `GetAllEvents`, or `KeyError`), `info['entries']`: a **list** of dicts `{'character_id', 'rank', 'wins'}` (`leaderboard.py:67`, `_update_item`). `entries is None` shows the loading spinner forever (`update_entries` :18-20) `[V]` |
| `OnPVPFilamentsCharacterStatistics` | `info` | `pvpFilamentSvc.py:82` `[V]` | `info['matchTypeID']`, `info['scheduleID']`, `info['statistics']`: a dict `{'rank', 'wins', 'losses', 'draws'}`, **never `None`** (it reads `['rank']` first). `rank == 0` means "unranked": the panel stays blank (`character_statistics.py:13`) `[V]` |
| `OnMatchFound` | none | `PVPFilamentActivationController`, `pvpFilamentController.py:294` `[V]` | marks the window finished and out of the queue |
| `OnMatchMakingFailure` | `errors` | `pvpFilamentController.py:299` `[V]` | the same `[(code, args), ...]` list as `PVPJumpError` (§4); shown, and the pilot is out of the queue |
| `OnProvingGroundsMatchFound` | none | `audioService.py:1036` `[V]` | plays the "opponent matched" sound |
| `OnAddExpiryDurationForAbyssalPvpContent` | `end_time` (filetime) | `abyss/client/abyssal_service.py:140` `[V]` | starts the **`AbyssalPvpExpiration`** timer (crimewatch type 14, `crimewatchTimers.py` `SetAbyssalPvpTimer` :803) and the 300/120/30 s collapse warnings; it assumes the content lasted `DEFAULT_PVP_CONTENT_DURATION` (`_set_crimewatch_timer` :152) |

Reused from the abyss (already served by `abyss.rs`, `docs/re/` has none yet;
see its header):

| push | in the arena |
|---|---|
| `OnAbyssalLocationEntered(client_data)` | sets the nebula and `active_content_id` (`abyssal_service.py:83`) `[V]`. It also starts the **PvE objective chain** unconditionally `[V]`: see §5.3, `[A]` 7 |
| `OnAbyssalContentFinished(content_id, timer_type)` | deletes the timer of `timer_type` **only if** `content_id` is the active content (`crimewatchTimers.py:814`) `[V]`. For the arena, `timer_type` is `AbyssalPvpExpiration` (14) `[V]`, not the PvE 11 that `abyss.rs` sends |

**Pushes the activation window also listens to, which the server already
sends for other reasons**: `OnAddFX`/`OnRemoveFX` (cloak, tether),
`OnInvulnCancelled`, `OnInvulunOnUndockingUpdated`,
`OnRestoringInvulnUpdated`, `OnPvpTimerUpdate`, `OnSafetyLevelChanged`,
`OnSessionChanged` (a dock or a structure entry **finishes** the window,
:281), warp start/finish, `ProcessActiveShipChanged`. Each just re-runs the
client-side validation `[V]`. Nothing new to build for them.

### 2.3 Where the client reaches all this

- **The filament's menu**: `OfferPVPFilamentKey` (`menucheckers/itemCheckers.py:87`):
  group `groupPVPfilamentKeys`, owned by me, in my location →
  `menuFunctions.ActivatePVPfilamentKey` :596 → `OpenPVPfilamentWindow` →
  `PVPFilamentActivationWindow` `[V]`. The window asks `isEventActive(typeID)`
  (→ `GetActiveEvents`); inactive shows "Proving Grounds inactive" and no
  button `[V]`.
- **The Neocom / call-to-action**: `eveCommands.OpenPVPFilamentEventWindow`
  and `newFeatures/callToActionCommands.py` → the event window with no
  filament (§3.4) `[V]`.
- **The client never reads the two static tables itself.** The only importer
  of `pvpFilaments/common/util.py` and `fsd_loaders.py` is that package; no
  client module calls `get_active_events` and its siblings `[V]` (grep of the
  whole package tree). That code is the shared module the real server runs;
  it is the best description of the server's own rules we have (§3.2), and
  its date helpers use the client's local clock.

---

## 3. The event model

### 3.1 One event info

Built by the shared `_construct_info` (`util.py:150`) as a `utillib.KeyVal`
`[V]`. The client reads it **both** as attributes (`eventData.info.matchTypeID`)
and as items (`eventInfo['filamentTypeID']`), and `KeyVal` does both `[V]`
(`utillib.KeyVal.__getitem__`). The server's `pyutil::keyval` is exactly that
class.

| key | type | from | read by the client |
|---|---|---|---|
| `matchTypeID` | int | event date row | event id, both data calls |
| `scheduleID` | int | the event date row's **key** | event id, both data calls |
| `filamentTypeID` | int | match type | activation window, event window, market button |
| `numberOfFleets`, `fleetSize` | int | match type | event window (format, e.g. teams × pilots) |
| `shipRestrictions` | int, a **typelist id** | match type | `evetypes.GetTypeIDsByListID` (event window) |
| `rewards` | list of `KeyVal(typeID, amount)` | match type | reward icons (`reward.typeID`, `reward.amount`) |
| `eventNameID`, `eventDescriptionID`, `filamentDescriptionID`, `restrictionsDescriptionID`, `rewardsHintID`, `shipRestrictionsHintID`, `rulesHintID` | int, message ids | match type | `GetByMessageID` |
| `startDate`, `endDate` | **`datetime.date`** | event date row, parsed from `'YYYY-MM-DD'` | `datetime_to_filetime` (accepts a `date` or a `datetime`), `FmtDate`, and the sort key |
| `QA` | int/bool | event date row | `!= 0` / truthiness: hides the event from the window's list |
| `moduleAndImplantMetaLevelRestrictions`, `maxOfShipTypeRestriction`, `maxOfShipGroupRestriction`, `minimumQueueSizeMultiplier`, `provingGroundLootTableID`, `bannedModulesTypeListID`, `limitedAmountOfModulesTypeListID`, `numberOfLimitedModules` | int or `None` (optional fields) | match type | **not read by the client** `[V]`; the server's matchmaking and validation |

The client reads every key in the first three blocks with `[...]` or `.`, so
**all must be present**; the last block may be omitted `[I]` (no reader;
falsified by a client traceback naming one of them).

**`datetime.date` on the wire.** It is in the client's `globalsWhitelist`
(`eve-rowset/src/whitelist.rs`) `[V]`; nothing in the server sends one yet.
CPython 2.7's `date.__reduce__` is `(date, (state,))` with a 4-byte state
(year high, year low, month, day), and `date(state)` rebuilds it, so the value
is an `ObjectForm::Call` (tag `0x22`) with header
`(Global("datetime.date"), (Bytes(4),))` `[I]` (CPython's `date_new` pickle
branch; falsified by a client unmarshal error on the first event list). A
`datetime.datetime` (10-byte state) works for every reader too `[I]`.

### 3.2 The schedule rules (the shared code's, to copy)

From `util.py` `[V]`:

- An event is a **(match type, event date)** pair; the date row has
  `matchTypeID`, `startDate`, `endDate` (strings `'YYYY-MM-DD'`), `region`
  (empty means `'ccp'`; rows of another region are invisible) and `QA`.
- **Active** (`_is_event_active` :119): region matches; a `QA` row is
  **always** active; otherwise `start <= today <= end`, where the first day
  starts and the last day ends **at downtime**, not midnight.
- **Over** (`_is_event_over` :105): ended before today, or today after
  downtime; `QA` rows are never over.
- **Most recent** (:64): the over event with the latest end.
- **Next date** (:53, `_next_date` :90): the `startDate` **string** of the
  earliest-starting non-QA event that is running or yet to start.
- **By filament** (:78): the active event whose match type has that
  `filamentTypeID`.

For us the dates are ours, not CCP's: every row in the shipped table is a
past TQ event or a QA row `[I]` (a 2026 client and event dates written for
TQ's calendar; falsified by a decoded row in the future). So the schedule is
an owner decision (§8, `[A]` 1): an authored schedule, GM commands, or an
always-on rotation, with the shipped rows as the catalogue of formats.

### 3.3 Formats and ship rules

What the match type carries, and therefore what the server enforces
`[V]` for the fields, `[I]` for their meaning (from the error labels in §4 and
the field names):

- **Format**: `numberOfFleets` teams of `fleetSize` pilots.
  `minimumQueueSizeMultiplier` (optional): how many teams must be queued
  before a match is made, as a multiple of `numberOfFleets` `[I]`.
- **Hulls**: `shipRestrictions`, a typelist of allowed hulls;
  `maxOfShipTypeRestriction` (at most N of one hull per team) and
  `maxOfShipGroupRestriction` (at most N of one group per team).
- **Fit**: `moduleAndImplantMetaLevelRestrictions` (a meta level cap on
  modules and implants), `bannedModulesTypeListID`,
  `limitedAmountOfModulesTypeListID` with `numberOfLimitedModules`, and the
  "frigate escape bay" refusal (code 38).
- **Loot**: `provingGroundLootTableID` names a loot table the client does not
  ship (`docs/re/dungeon-model.md` §6) `[V]`. Rewards are the `rewards` list.
- **Durations** (`pvpFilaments/common/constants.py`) `[V]`: content lasts
  `DEFAULT_PVP_CONTENT_DURATION` = 15 min (plus 18 s jump delay in the
  server-side variant); the activation window waits `ACTIVATION_ERROR_DELAY`
  (4 s) after an error before the button works again; the proximity rule is
  `FILAMENT_PROXIMITY_RESTRICTION_RANGE` (1000 km, as the abyss) and the
  "trace" rule `FILAMENT_ANCHOR_RESTRICTION_RANGE` (10 km). `STALEMATE_WALLCLOCK_TIME`
  (480000, milliseconds `[I]`) and `AWARDED_REWARDS`/`UN_REWARDED` have no
  client reader `[V]`: server-side stalemate rule and a per-pilot reward flag
  `[I]`.

The static tables' layouts are in `findings/pyd-schemas.tsv`:
`pvpfilamentmatchtypes` (stride 96, every field above, `rewards` a list of
`{amount, typeID}`) `[V-data]`, and `pvpfilamenteventdates` (stride 40) which
lists **four** fields where the client reads **five**: `QA` is missing and the
`matchTypeID` row is a 1-byte field with a note of doubt `[V-data]`. So that
row is incomplete: one 1-byte field is probably `QA` and `matchTypeID` sits at
another offset `[I]`. Falsified by a decode where every `matchTypeID` keys
into the match types (then the row is right as it stands).

### 3.4 The event window's paths, and the trap

`PVPFilamentEventInfoController.__init__` (`controller.py:7`) `[V]`:

- With a filament: `GetActiveEventByTypeID(typeID)` (a `KeyError` if that
  filament has no active event; the activation window only offers the button
  when it has one).
- Without (Neocom): `activeEvents = GetActiveEvents()`; **if it is empty,
  line 17 reads `sortedEvents`, which was only assigned inside the `if`
  (lines 14-16): `UnboundLocalError`** `[V]` (bytecode: the `STORE_FAST
  sortedEvents` at :15 is jumped over). Non-empty but all `QA`: falls back to
  `GetMostRecentEvent()`; `None` there gives the "inactive" window, an event
  gives the "final results" window with `GetNextEventDate()` `[V]`
  (`window.py:46`).

So **`GetActiveEvents` must always hold at least one event**: the trap is
`[V]`, and how we guarantee it is `[A]` 2.

---

## 4. Refusals: `PVPJumpError` and the codes

**Shape** `[V]`: `UserError('PVPJumpError', {'errors': [(code, args), ...]})`
raised by `JoinPVPQueue`; the same list goes in `OnMatchMakingFailure(errors)`.
`args` is a tuple; the window formats at most five distinct pairs
(`displayed_errors`) through `pvpFilaments.error.get_error_label`
(`error.py:51`). `findings/user-errors.tsv` has the key with no FSD message
(the client builds the text itself) `[V]`. The abyss builds the same shape
(`abyss::abyss_error`); a `PVPJumpError` twin is a one-liner.

The codes are `PVPJumpError`'s class constants (`error.py:5`) `[V]`. "C" =
the client checks it before calling (`ClientPVPFilamentValidator.perform_base_validation_checks`,
:142); "S" = only the server can `[I]` (no client check exists for it).
`args` are what the label formats, so the server must send exactly these:

| code | name | args | who |
|---|---|---|---|
| 1 | IN_ABYSS_SYSTEM | (charID,) | C |
| 2 | INVALID_KEY_TYPE | (charID,) | S (`_validate_key` :38 exists, not called) |
| 3 | INVALID_OWNER | (charID,) | S (same) |
| 4 | INVALID_SHIP_TYPE | (charID,) | S |
| 5 | ITEM_NOT_REACHABLE | (typeID,) | S |
| 6 | KEY_NOT_FOUND | () | S |
| 7 | PROXIMITY_RESTRICTION | (typeID, charID) | S |
| 8 | PVP_TIMER_ACTIVE | (charID,) | C |
| 9 | SAFETY_ENGAGED | (charID,) | S (`is_blocked_by_safety` exists, not called) |
| 10 | SESSION_CHANGE_IN_PROGRESS | (charID,) | C |
| 11 | INVALID_KEY_AMOUNT | () | S ("additional filaments needed") |
| 12 | SHIP_IN_DUNGEON | (charID,) | S |
| 13 | SHIP_IN_FORCE_FIELD | (charID,) | S |
| 14 | SHIP_IN_WARP | (charID,) | C |
| 15 | SHIP_IS_CLOAKED | (charID,) | C |
| 16 | SHIP_IS_DOCKED | (charID,) | C |
| 17 | SHIP_IS_INVULNERABLE | (charID,) | C |
| 18 | SHIP_IS_SELF_DESTRUCTING | (charID,) | C |
| 19 | SHIP_IS_TETHERED | (charID,) | C |
| 20 | SHIP_IS_WARP_SCRAMBLED | (charID,) | S |
| 21 | SHIP_NOT_FOUND | () | C |
| 22 | UNKNOWN_ERROR | () | either |
| 23 | SHIP_NOT_IN_FLEET | () | S (`_validate_fleet_required` :52 exists, not called) |
| 24 | TOO_MANY_IN_FLEET | (amount,) | S |
| 25 | FLEET_MEMBER_NOT_IN_RANGE | (charID, distance) | S |
| 26 | DISABLE_MODULE_BEFORE_JUMPING | (charID, typeID) | S |
| 27 | OVERLOADED_CARGO | (charID,) | S |
| 28 | FLEET_ROLES_NEEDED | () | S |
| 29 | TOO_FEW_IN_FLEET | (amount,) | S |
| 30 | PROXIMITY_TRACE_ANCHOR | (distance, charID) | S |
| 31 | FLEET_SIZE_CHANGED | () | S (matchmaking failure) |
| 32 | ALREADY_IN_QUEUE | () | S |
| 33 | EVENT_NOT_ACTIVE | () | S |
| 34 | CONGESTION_BAN | () | S |
| 35 | MODULE_AND_IMPLANT_RESTRICTIONS | (typeID, charID, amount) | S |
| 36 | MAX_OF_SHIP_TYPE_RESTRICTIONS | (amount, typeID) | S |
| 37 | MAX_OF_SHIP_GROUP_RESTRICTIONS | (amount, groupID) | S |
| 38 | FRIGATE_ESCAPEBAY_RESTRICTIONS | (charID,) | S |
| 39 | BANNED_MODULES_RESTRICTIONS | (typeID, charID) | S |
| 40 | LIMITED_NUMBER_OF_MODULES_RESTRICTION | (typeID, charID, amount) | S |

Most of the shared checks (docked, cloak, warp, timer, tether, scramble,
proximity, session change) already exist as `abyss::base_errors` over
`abyss::Facts`; the abyss's codes are different numbers, so PG maps the same
facts to this table rather than reusing the abyss's list `[I]`.

A server should **re-run the client's checks** (a modified client skips them)
and add the S rows. Which fleet rules apply to a 1-pilot format is `[A]` 4.

---

## 5. The pocket flow

### 5.1 Queue

1. The pilot, in space, opens the filament's window; the client validates
   (§4 "C") and calls `JoinPVPQueue(itemID, typeID)` `[V]`.
2. The server validates, finds the active event for `typeID`
   (`EVENT_NOT_ACTIVE` if none), puts the pilot (or the pilot's fleet, for
   `fleetSize > 1`, `[A]` 4) in that event's queue, and returns an estimate
   (`0` = unknown is always correct) `[V]` for the shape.
3. While queued the window ticks locally; nothing is polled `[V]`. The
   window stays open; closing it, docking, or entering a structure leaves the
   queue (`Close` → `LeavePVPQueue`; `OnSessionChanged` with
   `stationid`/`structureid` marks it finished but does **not** call
   `LeavePVPQueue`) `[V]`. So the server must also drop a queued pilot who
   docks, jumps, changes ship, logs off or dies `[I]` (falsified by nothing
   client-side: it is our own consistency), sending `OnMatchMakingFailure`
   with the matching code where one exists.
4. When enough teams are queued (`numberOfFleets` ×
   `minimumQueueSizeMultiplier`, `[I]`), the matchmaker re-validates each
   pilot; a pilot who fails gets `OnMatchMakingFailure(errors)` and the
   others stay queued `[A]` 5.

### 5.2 Match found → arena

For each matched pilot, in this order `[I]` (the client imposes no order
between the three pushes; the move must come last because the window
closes on `OnMatchFound`):

1. `OnMatchFound()` and `OnProvingGroundsMatchFound()`.
2. **Consume the filament** (one per pilot, `stacksize < 1` is
   `INVALID_KEY_AMOUNT`) `[A]` 3.
3. **Move the ship, server-initiated.** No client call is pending, so this is
   the path the server already uses to move pilots who did not ask in that
   moment: `jump_drive::depart` (the abyss's solo entry, a conduit's
   passengers) `[V]` (`abyss.rs` `join`, `jump_drive.rs:373-376`). The
   destination is a free system in the abyssal id range (`abyss::free_system`),
   so the client treats it as abyssal space (`IsAbyssalSpaceSystem`) `[V]`.
   The origin is recorded as the pilot's logoff spot, as the abyss does `[V]`.
4. On arrival: `OnAbyssalLocationEntered({content_id, nebula_graphic_id,
   difficulty_tier, location_id})` `[A]` 7, then
   `OnAddExpiryDurationForAbyssalPvpContent(end_time)` with
   `end_time = start + 15 min` (the client derives the start from it) `[V]`.

### 5.3 The arena room

`[V-data]` (`out/content/dungeons/`, the abyssal archetype the abyss uses):
of the abyssal-archetype one-room dungeons,

- **the PvE rooms** (the ones `abyss::is_pve_room` picks) each place a
  "Proving Conduit" (`typeAbyssPvPGate`) **statically**, which the server
  strips today;
- **three faction-less "PVP Dungeon" rooms** have no area-of-effect locator,
  several entry locators, two exit locators, a ring of boundary objects
  ("Boundary Pylon"/"Boundary Nexus") and a centre object, and **no**
  conduit;
- **nine faction-tagged rooms** carry the "Arena Ring" structure
  (`typeAbyssPVPArenaStructure`), several entry locators and one exit locator.

The arena ring's model is driven by its slim item `[V]`
(`clientControllerRegistry.UpdatePVPArena` :289-297): `eventComplete == 1`
(end state), else `kill == 1` (an explosion), else `startTime` and `endTime`
(filetimes) set the countdown ring. Missing slim attributes read as `None`
(`docs/re/npc-behaviors.md`), so a ring without them renders with a zero
countdown rather than failing `[I]`.

Which rooms to use, and where each team spawns, is `[A]` 6.

**The PvE objective chain.** `OnAbyssalLocationEntered` starts the client's
abyss objective chain (destroy the NPCs, loot the cache) every time `[V]`
(`abyssal_service.py:83`). Without that push, `active_content_id` is never
set and `OnAbyssalContentFinished` cannot clear the PvP timer early; the
timer still deletes itself at its end (`'DeleteWhenFinished'` in its
`TimerData`, `crimewatchTimers.py`) `[V]`. Hence `[A]` 7.

### 5.4 The match ends

The client has no rules for this; it only shows the timer, the ring and the
conduit. Everything here is server design, `[A]` 8 unless marked:

- **Decided**: the last team with a ship in the arena wins. Detection is a
  kill hook (`kill_hooks::PLAYER`, maintainer line): a ship lost in an arena
  counts against its team.
- **The loser's capsule** goes home, ejected to its origin, not killed
  `[A]` 9.
- **Timer runs out** with two or more teams standing: a **draw** for those
  teams (`draws` exists in the statistics, `[V]` for the field), and then,
  as in the abyss PvE pocket, the pocket collapses `[A]` 10.
- **The way out**: when the match is decided the server places the
  **Proving Conduit** at the room's exit locator; activating it calls
  `AbyssalPVPEndGateActivation(itemID)` inside `abyssToKnownJump` `[V]` for
  the call. The server checks the caller is in that arena and the gate is
  its conduit (as `abyss::end_gate` does), sends
  `OnAbyssalContentFinished(content_id, 14)`, and moves the pilot to the
  origin `[I]` (the label says abyss → known space). Falsified by TQ
  material showing a different exit.
- **Results** (§5.5) are written when the match is decided or drawn.
- **Bookkeeping**: pockets close when empty, as the abyss does; a pilot who
  logs off in an arena comes back at the origin (the logoff spot).

**Why the conduit sits in every PvE room** `[I]`: the older abyssal PvP,
where a pilot left a PvE run through a conduit into an arena, placed it
there; the dungeon data still carries it, and the client's call for it is
now `pvpFilamentMgr`'s end gate. Falsified by a client path that opens a
PvP queue or arena from a PvE room's conduit (none exists in this build:
the conduit's only menu action is the end-gate call `[V]`).

### 5.5 Results: statistics and leaderboard

Per (event, pilot): `wins`, `losses`, `draws`, and `rank` (1-based, 0 =
unranked), ordered by wins `[I]` (the leaderboard row shows only wins,
`leaderboard.py:67`; the backend message `LeaderboardEntry` has rank and wins
only, `eveProto/.../provinggrounds_pb2.py` `[V]`, the match result enum is
LOSS/WIN/DRAW `[V]`). The tiebreak is `[A]` 11.

- **`GetLeaderboard` answer**: the top 100 entries, then **the caller's own
  entry appended last** when the caller is outside the top 100; the client
  pops a last entry whose rank is above 100 after reading the caller's rank
  from it (`pvpFilamentSvc.py:50-62`) `[V]` for the client side, `[I]` for
  the server contract.
- **`GetCharacterStatistics` answer**: the caller's four numbers; a pilot
  with no match gets `{rank: 0, wins: 0, losses: 0, draws: 0}` `[I]` (the
  handler reads `['rank']` unconditionally).
- **Push after a match**: the client refetches the leaderboard by itself
  when a new statistics push moves the caller's rank into or out of the top
  100 (`:82-98`) `[V]`; so after a match the server pushes
  `OnPVPFilamentsCharacterStatistics` to each participant online `[I]`.

Rewards (`rewards`, `provingGroundLootTableID`) are an owner question (§8,
`[A]` 12).

---

## 6. What the server has today

| piece | today |
|---|---|
| `pvpFilamentMgr` | not registered; every call fails with `NoSuchService` (`service.rs`) `[V]`. The filament window cannot even decide whether an event is active |
| the PvP gate | `abyss::PVP_GATE` and `abyss::is_stray` strip it from PvE rooms; `site.rs` applies the filter `[V]` |
| the arena rooms | in the dungeon bundle (`out/content/dungeons/`), excluded from PvE by `abyss::is_pve_room` `[V]` |
| abyssal pocket machinery | `abyss.rs`: free abyssal system, room instancing, server-initiated depart and return, logoff-spot origin, pushes `Entered`/`Expiry`/`Finished`, the end-gate check, the collapse kill `[V]`. Most of it is private (`fn`); PG needs a few made `pub` or moved to a shared `pocket` module (`[A]` 13) |
| `UserError` with an error list | `abyss::abyss_error` builds the shape `[V]` |
| `datetime.date` | whitelisted, never sent `[V]` |
| static tables | `pvpfilamentmatchtypes` and `pvpfilamenteventdates` are **not** in the bundle (`findings/content-C1-map.tsv`: none, P3) `[V]`; `fsd.cfsd_rows` and `cfsd_struct_list` can read cFSD tables once columns are confirmed `[V]` (`tools/fsd.py:1611`, `:1760`) |
| status | `docs/status/progress.tsv`: "Proving Grounds", `missing`, issues #63-#67 |

---

## 7. Build plan

Order: PG-1 and PG-2 can start now (no abyss dependency: they serve data and
windows). PG-3 needs PG-2. PG-4 needs PG-3 and the abyss pocket work (project
order: the Proving Grounds come after the Abyss). PG-5 needs PG-4. PG-6 is
optional. Every server piece adds or updates its `progress.tsv` row
(`built-unproved`). Paths below are under `server/crates/eve-server/src/`
unless they start with `tools/` or `content/`. The registration lines
(`service.rs` `DOMAINS`, `lib.rs` `mod`, `tick.rs` `STAGES`, `world_ext.rs`
`SLOTS`, `content_tables.rs` `TABLES`, `kill_hooks.rs` `PLAYER`,
`session_hooks.rs` `ON_LOGOFF`, `slim_extra.rs` `SOURCES`, `slash.rs`
`COMMANDS`) are maintainer lines: each piece exports the item and names the
line in its PR.

### PG-1. Content: the two tables

- **Owns**: `tools/content_proving.py`, `tools/test_content_proving.py`,
  `server/crates/eve-server/src/proving_data.rs` (+ `proving_data/tests.rs`),
  the `proving/` rows of `docs/content/schema.md`, the `pvpfilamenteventdates`
  row of `findings/pyd-schemas.tsv` if it is corrected.
- **Work**: a content section `proving` writing
  `out/content/proving/match_types.tsv` (one row per match type, every §3.1
  match-type field, empty for an absent optional),
  `match_rewards.tsv` (match type, type, amount) and `event_dates.tsv`
  (schedule id, match type, start, end, region, QA). First settle the
  event-dates layout (§3.3): find the `QA` byte and the real `matchTypeID`
  offset from the loader `.pyd` (`content.py`'s schema guard re-derives it).
  A loader `proving_data.rs` with a `TableSpec`.
- **Tests**: Python: invented records through the pure derivation
  (as `content_dbuffs.py`); a check that every event date's match type
  exists. Rust: header byte-compare and invented rows round-trip.
- **[A]s**: none to ship; the `[I]` on the layout is settled by the decode
  (every `matchTypeID` keys into the match types, `QA` is 0/1).

### PG-2. The catalogue and the read-only calls

- **Owns**: `proving.rs`, `proving/tests.rs`, `service/proving.rs`,
  `content/pve/proving-schedule.tsv` (new, authored), a `/pg` slash
  command's handler in `proving.rs`.
- **Work**: state (a `world_ext` slot): the schedule, the per-event
  statistics. The event info builder (§3.1, a `KeyVal`, dates as
  `datetime.date` via an `ObjectForm::Call` helper local to the module).
  Calls 3-8 of §2.1 and the two data pushes (§2.2, §5.5), with empty
  statistics until PG-5. The active rule of §3.2 on our own schedule, with
  downtime at 11:00 UTC. `GetActiveEvents` never empty (`[A]` 2). GM:
  `/pg events`, `/pg on <matchTypeID> [days]`, `/pg off <scheduleID>`.
- **Tests** (invented match types and dates only): active/over/most recent/
  next across downtime; `QA` always active; a second region invisible;
  `GetActiveEvents` non-empty with no scheduled event; the leaderboard
  push's shape (list, `character_id`/`rank`/`wins`), the caller appended
  when outside the top 100; statistics never `None`; `GetEventID` keys of a
  push present in `GetAllEvents`; the `datetime.date` encoding bytes.
- **[A]s**: 1 (schedule), 2 (the never-empty rule).
- **Client check**: an fci scenario opening the Neocom window and a
  filament's window (`tools/client-ci/scenarios/fci_proving_window`):
  "inactive"/event shown, no traceback.

### PG-3. The queue

- **Owns**: `proving/queue.rs` (+ tests in `proving/tests.rs` or
  `proving/queue/tests.rs`), the queue arms in `service/proving.rs`.
- **Work**: `JoinPVPQueue`/`LeavePVPQueue`; the validation of §4 over
  `abyss::Facts`-style facts plus key, fleet and fit rules (one function per
  rule family, each returning `(code, args)` pairs in the client's order);
  `PVPJumpError` builder; a queue per event; a matchmaker tick stage forming
  matches of `numberOfFleets` teams; `OnMatchFound`,
  `OnProvingGroundsMatchFound`, `OnMatchMakingFailure`; dropping a pilot who
  docks, jumps, changes ship, dies or logs off (`ON_LOGOFF` maintainer
  line). Until PG-4 lands, a found match only pushes and logs.
- **Tests**: every S code's trigger and its exact `args` tuple; the C codes
  re-checked server-side; `ALREADY_IN_QUEUE`; `EVENT_NOT_ACTIVE`; leave is
  idempotent; a match forms only at the threshold; a pilot invalid at match
  time gets a failure and the others stay queued; logoff empties the queue.
- **[A]s**: 3 (key consumption), 4 (fleet rules), 5 (failure handling),
  13's seam if fit checks reuse `fitting.rs` facts.

### PG-4. The arena

- **Owns**: `proving/arena.rs` (+ tests), the end-gate arm in
  `service/proving.rs`, the arena ring's slim extras in `proving/arena.rs`;
  if `[A]` 13 moves pocket code, the moved module (claimed with the abyss
  area's owner).
- **Work**: on a match: allocate a free abyssal system, instance an arena
  room (§5.3), place teams on distinct entry locators, `depart` each pilot
  with the origin as logoff spot, the arrival pushes (§5.2.4), the ring's
  `startTime`/`endTime`. The 15-minute clock. The kill hook deciding the
  match (`kill_hooks::PLAYER` line), the loser's capsule home, `kill`/
  `eventComplete` on the ring, the conduit at the exit locator when decided,
  `AbyssalPVPEndGateActivation` (§5.4), `OnAbyssalContentFinished` with
  timer type 14, the draw and collapse at the end, pocket cleanup, logoff
  in the arena.
- **Tests**: a match of two invented pilots reaches two entry spots in one
  abyssal system; the expiry push's `end_time`; a kill of the last ship of a
  team decides it and places the conduit; the end gate refuses a stranger's
  call and a gate that is not the arena's; the timer end with both alive is a
  draw; the finished push carries 14; a logoff in the arena returns to the
  origin at the next login.
- **[A]s**: 6, 7, 8, 9, 10.
- **Client check**: `fci_proving_duel` (two clients, 1v1): queue, match,
  arena, kill, conduit home. This is the first `client-proved` evidence.

### PG-5. Results and rewards

- **Owns**: `proving/results.rs` (+ tests).
- **Work**: write wins/losses/draws per event and pilot, persisted
  (`store` through the module's own table, no new delta kind unless the
  maintainer adds one); rank by `[A]` 11; push statistics to participants;
  rewards once the owner rules (`[A]` 12).
- **Tests**: rank order and ties; the top-100-plus-self list; statistics
  after a win, loss and draw; a restart keeps them.
- **[A]s**: 11, 12.

### PG-6 (optional). Bots in the queue

- **Owns**: a bot role in `server/crates/eve-bot/src/` (the `ROLES` line is
  the maintainer's).
- **Work**: bots that own a filament and a legal ship queue during an event,
  so players find a match at any hour. Bots use only filaments and hulls
  they obtained the normal way (no filaments from nowhere).
- **[A]s**: whether bots queue at all is an owner question.

---

## 8. The assumptions, each with its falsifier

| # | assumption | falsifier / who settles |
|---|---|---|
| 1 | The schedule is ours: an authored file plus GM commands, the shipped rows only a catalogue of formats | owner decision (the client accepts any dates) |
| 2 | `GetActiveEvents` is never empty: with no real event, it carries one `QA` event (hidden from the list, so the window shows "inactive" or the last results) | a client run opening the Neocom window with that answer; a traceback falsifies |
| 3 | The filament is consumed at match found, not at queue join; a failed or left queue keeps it | owner decision; no client evidence either way |
| 4 | Formats with `fleetSize > 1` are queued by the fleet boss for the whole fleet, which must be exactly `fleetSize`, in range, and hold one filament each (codes 11, 23-25, 28, 29, 31); `fleetSize == 1` needs no fleet | TQ material, or a client label text that says otherwise |
| 5 | A pilot who fails at match time is dropped with `OnMatchMakingFailure`; their team (if any) is dropped with `FLEET_SIZE_CHANGED`; others stay queued | none from the client; design |
| 6 | Arenas: the three faction-less "PVP Dungeon" rooms first; the ring rooms when the ring's slim works; teams on distinct entry locators | a client run where the arena looks wrong, or TQ footage |
| 7 | Send `OnAbyssalLocationEntered` in the arena and accept the PvE objective panel it starts | a client run: if the panel misleads, drop the push and let the timer expire by itself |
| 8 | Win = last team with a ship in the arena | TQ rules text (`rulesHintID`'s message) read from the localisation |
| 9 | The losing capsule is sent home, not killed | owner decision; the rules message |
| 10 | At the timer's end every ship still inside dies, as in the PvE pocket; the standing teams score a draw | the rules message; owner |
| 11 | Rank by wins, then fewer losses, then earliest to reach that score | the rules or rewards message |
| 12 | Rewards: none until the owner rules; `provingGroundLootTableID` has no table in the client | owner decision (the "nothing from nowhere" rule weighs against item rewards out of thin air) |
| 13 | PG reuses the abyss's pocket code by making a few functions `pub` or moving them to a shared module, rather than copying them | the abyss area's owner, at review |

`[I]`s worth a client run: the `datetime.date` encoding (§3.1), the units of
`estimatedQueueTime` (§2.1), the conduit as the arena's exit (§5.4), and the
`pvpfilamenteventdates` layout (§3.3, settled by PG-1's decode).

---

## 9. How this was read

- The `pvpFilaments` package, whole: the client service, the activation
  window, controller and validator, the event window, controller,
  leaderboard and statistics panels, `error.py`, `common/constants.py`,
  `common/util.py`, `common/fsd_loaders.py`. Real lines with `tools/dis.py`.
- Every reference to `pvpFilamentMgr`, `pvpFilamentSvc`,
  `groupPVPfilamentKeys`, `typeAbyssPvPGate`, `typeAbyssPVPArenaStructure`,
  `AbyssalPvpExpiration`, `OnMatchFound`, `OnMatchMakingFailure` and
  `OnProvingGroundsMatchFound` in the client tree.
- `findings/remote-services.txt`, `client-calls.tsv`, `push-events.tsv`,
  `user-errors.tsv`, `pyd-schemas.tsv`, `corpus-inventory.tsv`,
  `content-C1-map.tsv`.
- The dungeon bundle, for room counts and object kinds only.
- `abyss.rs`, `site.rs`, `pyutil.rs`, `eve-rowset/src/whitelist.rs`,
  `kill_hooks.rs`, `jump_drive.rs` for what exists.
- EvEJS's `pvpFilamentMgr` service, for the method list and the
  return/push split only (hint, agrees).
