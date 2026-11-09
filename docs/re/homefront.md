# Homefront operations: what the objective graphs read

The key list for issue #206 (lane HF-0): every context key, message and
compared value the client's Homefront objective graphs use, so that the
server lanes (HF-2, HF-3) know what `OnDungeonValues`, `OnDungeonMessage` and
the `dungeon_values` of `GetCurrentDungeonForCharacter` must carry. Read off
the pinned client's static data (build 3670638) with the variant decode in
[`node-graphs.md`](node-graphs.md), run in `tools/analysis.sh`, and its
decompiled Python. The code cited is from build 3662168 and the data from
3670638; that is sound because the re-pin to 3670638 changed data, not the
modules cited here.

**Evidence markers.** `[V]` read from the client's code or data, with the
place. `[V-data]` a count or a name read from a static table. `[I]` inferred,
with what would falsify it. `[A]` an assumption, with its falsifier; an `[A]`
value never goes into a test. **Names and counts only**: keys, message names
and state names are code identifiers the server must send; no id, number or
text from the client is here. Graphs are labelled G1..G9 in ascending id
order, by their sites' activity tags.

---

## 1. Where the graphs come from

- **[V-data]** 24 dungeons have the Homefront archetype; all 24 have an `objectivesID`;
  they use 9 distinct `dungeonobjectives` rows, and each row's
  `clientObjectives.nodeGraphID` is a distinct graph (9 graphs), every one
  tagged `client`, `dungeon`, 25 to 56 nodes. Each row also names an
  objective chain (`objectiveChainID`); no row has `blackboardParameters` or
  `overrides`.
- **[V]** `ActiveDungeon.start` sets the row's `blackboardParameters` into the
  context, starts the objective chain on that context, then starts the graph
  on the same context (`eve/client/script/parklife/dungeonTracking.py:234-241`).
  The context is seeded with `dungeon_id` and `dungeon_room_id` (`:147-150`);
  `OnDungeonValues` writes each key (dotted = object path, `:163-166`);
  `OnDungeonMessage` sends `dungeon_<key>` (`:158-161`); `OnDungeonCompleted`
  sets `dungeon_completed` (`:126-128`).
- **[V-data]** All 9 graphs call **one shared subgraph S** (82 nodes, tags
  `client`, `dungeon`, no subgraph of its own) through a `Subgraph` node. S
  runs the lobby, warping and the state-driven conversations; each graph adds
  its own objective start and its conversation tables.

## 2. The shared subgraph S

| what | names (count of nodes) |
|---|---|
| keys read | `dungeon_state` 14, `lobby_room_id` 5, `dungeon_room_id` 4, `always_show_conversations` 3, `enter_conversations` 3, `persistent_dungeon_states` 3, `conversation_dialog` 2, `complete_remove_objective_delay` 1, `event_conversations` 1, `lobby_objectives` 1 |
| keys written | none |
| keys watched (`Blackboard Changed`) | `dungeon_state`, `conversation_dialog` |
| compared values | `dungeon_state` against `Failure` 4, `Success` 3; `lobby_objectives` against `True` 1; `always_show_conversations` against a number 3 |
| operators | `in` 7, `notEqualTo` 3, `notIn` 3; 7 compares `flipped` |
| messages listened | none |
| messages sent | `start_objective` (`lobby`, `warping`), `stop_objective` (`lobby` 2, `warping` 2), `stop_group` (`in_dungeon` 2, `in_lobby` 1), `start_objectives` 2 and `start_lobby_objectives` 1 (value wired) |
| conversation lookups | `Get Value` with the literal keys `Lobby`, `Default`, `Complete`, and three wired keys |
| other nodes | `Show Conversation` 8, `Warp Started`, `Warp Finished`, `Warping` validation 2 |

## 3. Per graph

Every graph **[V-data]**: listens for `start_objectives` (1 listener); reads
`dungeon_id` or `system_faction_id` 4 times against 4 numeric literals (which
site or which faction it is in, choosing the conversation agents); writes
`enter_conversations` and `event_conversations` (4 each: one per site or
faction, each a map from a state name to a conversation) and `lobby_room_id`;
preloads 4 conversation agents; and calls S. The tables list what is
**beyond** that.

| graph | sites | activity tags | own reads | own writes | sends | compares | state names in its conversation maps |
|---|---|---|---|---|---|---|---|
| G1 | 4 | `activity_logistics`, `activity_combat` | - | - | `start_objective` `protect_targets` | - | `Default`, `Lobby`, `Success`, `Failure` |
| G2 | 4 | `activity_hacking`, `activity_combat` | - | - | `start_objective` `destroy_targets`, `hack` | - | `Default`, `Lobby`, `Success`, `Failure`, `Vulnerable`, `Invulnerable` |
| G3 | 4 | `activity_mining`, `activity_combat` | `ore_quantity_current` 3 (watched, validate on start), `goal_value` 1 | `current_value` (from a subtraction of the two) | `start_objective` `mine` | `ore_quantity_current` `notEqualTo` the string `None` | `Default`, `Lobby`, `Success`, `Failure` |
| G4 | 4 | `activity_logistics`, `activity_combat` | `dungeon_state` 4 (watched), `charge_objective`, `dungeon_room_id`, `lobby_room_id` | `charge_objective` 4, `charge_target` | `start_objective` `siege` and one wired, `stop_objective` `siege` | `dungeon_state` vs `Failure`, `Success`, `Siege` (`notIn` 2, `notEqualTo` 1) | `Default`, `Lobby`, `Success`, `Failure`, `Siege` |
| G5 | 1 | `activity_mining`, `activity_combat` | `current_difficulty` 5 and `dungeon_state` 5 (both watched), `dungeon_room_id` 2, `lobby_room_id` 2 | `persistent_dungeon_states`, `asteroids`, `system_faction_id` | `start_objective` `escape`, `mine`, `waves`; `show_objective` `difficulty_easy`, `difficulty_medium`, `difficulty_hard`, `drifter`, `mine`; `hide_objective` `mine`; `start_group`/`hide_group` `difficulty` | `current_difficulty` vs `Easy`, `Medium`, `Hard`, `Drifter`; `dungeon_state` vs `Wave`, `Success`, `SuccessEscape`, `Failure` | `Default`, `Lobby`, `Complete`, `SuccessEscape`, `FailureDrifter`, `FailureTimeout` |
| G6 | 4 | `activity_hauling`, `activity_combat` | - | - | `start_objective` `item_delivery` | - | `Default`, `Lobby`, `Success`, `Failure` |
| G7 | 1 | `activity_combat` | - | `system_faction_id` | `start_objective` `stop_smugglers`, `incompetence` | - | `Default`, `Lobby`, `Complete`, `Success`, `FailureCivilians`, `FailureSmugglers` |
| G8 | 1 | `activity_salvaging`, `activity_combat` | `dungeon_state` 2 | `system_faction_id`, `target` | `start_objective` `salvage` | `dungeon_state` vs `Success`, `Failure` (`notIn`) | `Default`, `Lobby`, `Complete`, `Success`, `Failure`, `Hacked` |
| G9 | 1 | `activity_combat` | - | `system_faction_id` | `start_objective` `drain` | - | `Default`, `Lobby`, `Complete`, `Success`, `Failure` |

G5, G7, G8 and G9 learn their faction themselves: a `Get Solar System` getter,
then `Get Value` of `factionID`, written to `system_faction_id`. G4, G5 and G8
also use `Get Closest Object`; G4, G5 use the `Warping` validation.

## 4. The objective chains on the same context

The chain is what draws the objectives. It is built on the same context as
the graph (`dungeonTracking.py:255-258`) **[V]**, declares the keys it reads
as `contextParameters`, and maps them into each objective's inputs
(`inputParametersMap`) **[V-data]**. Every chain has the objectives `lobby`
and `warping` (started and stopped by S) plus the ones its graph starts. The
names below are the keys and their declared `parameterType`:

| graph | chain objectives (besides `lobby`, `warping`) | context parameters |
|---|---|---|
| G1 | `protect_targets` | `protect_target` unionList, `timer` unionDict |
| G2 | `destroy_targets`, `hack` | `timer` unionDict, `destroy_target` unionList, `personal_only` bool, `node_a_timer`/`node_b_timer`/`node_c_timer` unionDict, `node_a_target`/`node_b_target`/`node_c_target` object |
| G3 | `mine` | `timer` unionDict, `asteroid_target` unionDict, `current_value` float, `goal_value` float |
| G4 | `charge_amarr`, `charge_caldari`, `charge_gallente`, `charge_minmatar`, `siege` | `timer` unionDict, `destroy_target` unionList, `charge` float, `capacitor_need` float, `charge_target` unionDict, `siege_timer` unionDict |
| G5 | `mine`, `waves`, `escape`, `difficulty_easy`, `difficulty_medium`, `difficulty_hard`, `drifter` | `wave_timer` unionDict, `wave_number` float, `asteroids` unionDict, `wave_number_max` float, `asteroids_depleted` float, `asteroids_depleted_max` float, `cooldown_timer`/`escape_timer`/`initial_timer` unionDict |
| G6 | `item_delivery` | `timer` unionDict, `items_delivered` float, `required_items` float, `container_target` object |
| G7 | `stop_smugglers`, `incompetence` | `timer` unionDict, `friendly_fire_current`/`escaped_current`/`friendly_fire_max`/`escaped_max` float |
| G8 | `salvage` | `timer` unionDict, `salvaged_current`/`salvaged_goal` float, `target` unionDict |
| G9 | `drain` | `timer` unionDict, `max_charge`/`min_charge`/`charge_requirement` float, `current_a`..`current_e` float, `target_a`..`target_e` unionDict, `drain_display_percentage` bool |

The `parameterType` names are the chain table's strings with their `Type`
suffix dropped (`unionDictType` and so on).

## 5. What the server must supply

A key is **external** when a graph (with S) reads it and no node of that
closure writes it, and the client does not seed it. **[I]** a grep of the
decompiled client finds no other writer of these keys, so they come from the
server's dungeon values; a client module writing one would falsify it.

| key | read by | values the graphs test it against |
|---|---|---|
| `dungeon_state` | S (all 9), G4, G5, G8 | the state names below |
| `always_show_conversations` | S | a number |
| `lobby_objectives` | S | `True` |
| `conversation_dialog` | S (watched) | - |
| `complete_remove_objective_delay` | S | - (a delay) |
| `persistent_dungeon_states` | S (all but G5, which writes it) | - (read as a collection [I]) |
| `current_difficulty` | G5 | `Easy`, `Medium`, `Hard`, `Drifter` |
| `ore_quantity_current`, `goal_value` | G3 | `ore_quantity_current` not the string `None` |
| the chain's context parameters (§4) | the chains | - |

**[I] `dungeon_state` names** (the union of what is compared and what keys a
conversation map): `Default`, `Lobby`, `Complete`, `Success`, `Failure`,
`Vulnerable`, `Invulnerable`, `Siege`, `Wave`, `Hacked`, `SuccessEscape`,
`FailureDrifter`, `FailureTimeout`, `FailureCivilians`, `FailureSmugglers`.
Comparisons cast one side to the other's type
(`nodegraph/common/util.py:87-121`), so a string is the safe form [I].
EvEJS's hint list also names `Cooldown`; no Homefront graph tests or maps it
(G5's chain has a `cooldown_timer`).

**Messages.** No Homefront graph listens for a `dungeon_`-prefixed message:
the only listened key is `start_objectives`, which S sends itself **[V-data]**.
So the server drives these sites through dungeon values, not
`OnDungeonMessage` [I: a listener added by a later build would falsify it].

**`[A]`s carried** (none in a test):

| `[A]` | falsifier |
|---|---|
| the server sends `dungeon_state` as one of the names above, as a string | a TQ capture of `OnDungeonValues` for a Homefront site |
| the chain parameters (timers, targets, counts) are sent as dungeon values under the names in §4 | a TQ capture, or the objective-chain code reading them from elsewhere |
