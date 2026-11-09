# Dynamic items (mutaplasmids): the two calls, the attribute contract, the static table

What this is: everything the client expects of a mutated ("dynamic") item, so that the server lanes
build against one contract. Client build 3670638; the decompiled tree is 3662168. The build delta (`findings/build-delta-3662168-3670638.tsv`)
changed `inventorycommon/const` (line 246; the one cited here is the flag constant `flagShipHangar`, section 2.2,
which this doc assumes did not move [A]; falsifier: that constant in `out/pyc` of 3670638) and the binary
`dynamicItemAttributesLoader.pyd` (line 82, `code-differs`), but its table schema `dynamicitemattributes` is
`identical` (line 343), so the layout and counts below hold for both builds. Every other cited module is unchanged.

Marks: **[V]** read in the client (`C:`) or in our server (`S:`), or counted in the 3670638 static table or the content
bundle (**[V-data]**, a count or a set equality, never a value); **[V-batch]** read in the hotbatch tree
(`eve-offline-lanes/hotbatch`, HEAD 1a57a9d8), whose code landed with MUTA-1 (#69) and MUTA-3 (#71)
(MUTA-DATA, #222, landed earlier, ca26120a) (state in section 6); **[I]** inferred; **[A]** assumed, with what
would falsify it. `C:` = `out/research/_w/src/`, `S:` = `server/crates/eve-server/src/`,
`T:` = `tools/client-ci/scenarios/`. No value from the client's table is written here, only counts.

Related: `docs/re/dogma.md` (attribute tables), `docs/content/schema.md` (`mutators/ranges.tsv`,
`mutators/mappings.tsv`), `findings/pyd-schemas.tsv` (the table layout, rows 449-456).

## 1. In one screen

| question | answer |
|---|---|
| remote calls | **2**, both on `dynamicItemService` through `sm.RemoteSvc`: `GetDynamicItemInfo(itemID)` and `CreateDynamicItem(mutatorItemID, sourceItemID)` [V `C:eve/client/script/dogma/dynamicItemSvc.py:20-25, 41-42`]. |
| what a mutated item is | a **new item** (own id, singleton) of the mutator's `resultingType`, a type flagged `isDynamicType` [V `C:evetypes/__init__.py:255`]. The 97 result types are exactly the 97 `isDynamicType` types, and a result type is never a source, so nothing is mutated twice [V-data]. |
| where its attributes come from | the client asks the server once per item id per session and caches the answer for good (`dynamicItemCache`, `dynamicItemSvc.py:15-25`) [V]. Its dogma base values are only `info.attributes` (`C:eve/common/script/dogma/baseDogmaLocation.py:1875-1877`), so the answer carries the **whole** attribute set. |
| the roll | `value = base * (modifier * (max - min) + min)`, `modifier` in [0, 1], `base` the **source type's** value (`C:dynamicitemattributes/__init__.py:9-14`) [V]. A uniform roll is [A]. |
| market | a dynamic type has no market group [V-data], so the client offers no sell (`C:inventoryrestrictions/common/checks.py:42-43`) [V]. |
| repackage | never: `CanRepackageType` is False for a dynamic type (`C:repackaging/__init__.py:64-66`) [V]. |
| nothing from nowhere | no NPC trade, NPC loot or exploration-loot row of the content bundle names a mutaplasmid [V-data]. The only supply is the abyss caches (#5). |

## 2. The two calls

### 2.1 `GetDynamicItemInfo(itemID)`

- Caller: `dynamicItemSvc.GetDynamicItem` for any item whose type is dynamic
  (`dynamicItemSvc.py:20-25`) [V]. The answer is cached by item id and never refreshed, so a row must
  never change after it is minted [I].
- Fields the callers read [V]:
  - `.attributes` (dict `attributeID -> value`): `GetDynamicItemAttributes` (`:27-30`), which feeds client
    dogma (`baseDogmaLocation.py:1875-1877`, `C:dogma/items/baseDogmaItem.py:38-39`), Show Info
    (`C:.../infosvc.py:3212-3219`, replaces the type's dict wholesale), the fitting window
    (`fittingDogmaLocation.py:758-767`), fitting warnings (`C:.../shipfitting/fittingWarnings.py:241-248, 354`,
    which reads a non-rolled source attribute from it), compare and filters (`neocom/compare.py:56`,
    `inventory/filterSvc.py:135`, `comparison/comparableType.py:47`) and the drone bandwidth check
    (`fittingScreen/dronePickerMenu.py:129`).
  - `.modifiers` (dict `attributeID -> modifier`): one tooltip row per key (`tooltip/dynamicItem.py:44-48`).
  - `.mutatorTypeID`: `GetMutatorAttributes(mutatorTypeID)` (a KeyError on an unknown mutator) in
    `dynamicItemSvc.py:35`, `tooltip/dynamicItem.py:55`, `infosvc.py:2930`; the Show Info header icon
    (`info/infoContentBase.py:780-807`).
  - `.sourceTypeID`: the base of each rolled attribute (`tooltip/dynamicItem.py:54`, `infosvc.py:2929`), the
    header icon, compare's market group (`compare.py:187, 248`).
- Wire shape: a `utillib.KeyVal` (every caller uses attribute access) [V]; fields in order `itemID` Int,
  `typeID` Int, `sourceTypeID` Int, `mutatorTypeID` Int, `attributes` Dict(Int -> Real), `modifiers`
  Dict(Int -> Real). The first two are extra and harmless [I]. Every value a Real and every key an Int [I:
  the rule the godma rows follow].
- Server today [V-batch `S:service/dynamic_item.rs:29-50`, `S:dynamic_item.rs:150-174`]: any owner may ask
  (contracts show other pilots' escrowed items, `C:.../neocom/contracts/contractentry.py:591`); a plain item
  gets a `CustomNotify` "not a mutated item"; an item with no row answers a fallback (the first mutator
  that makes the type, its first published source, no rolls) and logs `dynitem: no row for item N`, never
  `None`, because a cached `None` makes every caller raise `AttributeError` [I].

### 2.2 `CreateDynamicItem(mutatorItemID, sourceItemID) -> itemID`

- Caller: the crafting window, `CraftingController.ExecuteCrafting`
  (`C:.../dynamicItem/craftingWindow.py:231-253`). It re-raises a `UserError` to the player, turns any
  other exception into `UI/DynamicItem/UnknownErrorBody`, and on success calls `GetMutatedAttributes(itemID)`
  at once, i.e. `GetDynamicItemInfo`, reading `attributes[a]` for each mutator attribute [V].
- The window opens from the mutaplasmid's context menu: `OfferCraftDynamicItem` = `IsMutator(typeID)`, owned
  by me, `IsInPilotLocation` (`C:menucheckers/itemCheckers.py:274-281`) [V].
- What the client lets the player pick, and so what the server must re-check (a stale window or a second
  client can send anything):
  - both items owned by the caller and not in a fitting flag (`_IsItemReachable`, `craftingWindow.py:317-322`) [V];
  - both `IsInPilotLocation` (`itemCheckers.py:1082-1106`, `baseCheckers.py:549-559`): the docked station or
    structure, or the active ship with a flag other than `flagShipHangar` (90, `C:inventorycommon/const.py:141`),
    or a container in the docked hangar, or a corp office [V];
  - the mutator type is in the table and the source type is in its `applicableTypes`, filtered to published
    types (`dynamicitemattributes/loader.py`, `GetApplicableTypes`) [V].
- Result: drawn by the client at the **mutator's** location and flag, owner the mutator's owner, stack size 1,
  `singleton=True` (`craftingWindow.py:324-326`) [V]; so the server creates it there [I]. Whether the mutator
  and the source are consumed is a server choice [I]: the client does not decide it, it only ignores their
  inventory changes while crafting (`craftingWindow.py:155-157, 174-176`) and drops both rows from its own
  view when crafting starts (`:338-353`), which fits consumption of both.
- Push order: while `_isCrafting` the controller ignores inventory changes of the two inputs (`:155-157, :174-176`); a push about the result with a
  location key after `_resultItemID` is set calls `Reset()` and hides the result. The reply leaves before the
  call's pushes in one flush, and `_resultItemID` is set only after the `GetDynamicItemInfo` round trip, so
  the pushes are handled first **[A]**. Falsifier: a scenario in which the result is no longer shown 3 s later.
- Refusals: `CustomNotify` with a `notify` text, the shape `crateService` uses **[A]**. Falsifier: an empty or
  garbled dialog, or a non-UserError traceback in the client log.
- Server today: `CreateDynamicItem` refuses with "Mutaplasmids cannot be applied yet." [V-batch
  `S:service/dynamic_item.rs:51`]. MUTA-2 (#70) replaces it.

## 3. The attribute contract

`attributes(item)` is built in four layers, each overwriting the last [V-batch `S:dynamic_item.rs:122-148`, which
follows the rule below]:

1. every `typedogma` row of the **source type** [I: the client reads non-rolled source attributes from the
   answer, `fittingWarnings.py:354`];
2. every `typedogma` row of the **dynamic type**: of the 97 result types, 47 carry only skill-requirement rows
   (skill ids and levels, or no row) and **50 carry more** (for example tech level, meta level, hit points, charge size,
   fitting-restriction rows) [V-data]. **[A]** that this layer wins over the source's. Falsifier: Show Info of
   a TQ module made from a meta source showing the source's tech level, meta level or skill level;
3. the four `types` columns of the dynamic type: mass (4), capacity (38), volume (161), radius (162) **[A]**
   (the client adds them itself for a normal type, `get_type_attributes`). Falsifier: Show Info of a mutated
   item showing the source's mass;
4. each **rolled** attribute `a`, modifier `m`: `base * (m * (max - min) + min)`, `base` = the source type's
   own value of `a`, else the attribute's default (`GetTypeAttribute` default) [V formula,
   `dynamicitemattributes/__init__.py:9-14`; the same window in `craftingWindow.py:121-131`,
   `tooltip/dynamicItem.py:52-60`, `infosvc.py:2930-2937`].

So **from the roll**: exactly the attributes that have a range row for the mutator (2-10 of them). **From the
source type**: every other attribute, including non-rolled ones such as charge size. **From the dynamic
type**: the dynamic type's own `typedogma` rows (skills, and for 50 of the 97 types tech and meta level, hit
points and the like) and the four size columns (layers 2 and 3, both [A]).

Stored: the **modifiers**, not the values **[A]**, so a range change in a new client build moves existing
items with it. Falsifier: a TQ patch note saying existing mutated items keep their numbers after a range
change; then store the values too. Server row: delta kind `dynitem`, key `[item_id]`, value
`[source_type, mutator_type, creator, created_at, rolls]`, `rolls` = `attribute:modifier` pairs joined by `;`,
ascending [V-batch `S:dynamic_item.rs:12-17`].

Effects stay the dynamic type's own `typedogma` effect rows **[A]**. Falsifier: a mutated module whose result
type lists fewer effects than its source (it would not cycle).

### 3.1 How the server uses the roll once an item is fitted (MUTA-3, #71)

All of this is [V-batch] from the MUTA-3 round-2 diff (`handback/muta3-71/coordinator-muta3-71-r2.diff`, applied
on the hotbatch tree), and has landed.

- A fitted mutated module's base values are the roll, not its type row: `dogma::Overrides` (ascending
  `(attribute, f64)`), `Eval::base` returns the override first, `all_of` adds its keys. The sheet cache key
  includes the override list.
- A module's **required skills come from the static (dynamic) type row**, as the client does
  (`C:.../slimDogmaItem.py:35`), never from the roll: a roll that names a skill gets no "modules requiring X"
  bonus from it (test `required_skill_bonus_ignores_a_skill_named_only_in_the_roll`).
- `dynamic_item::overrides` caches per item id; `dynamic_item::put` removes the item's cached entry after
  storing, so a row rewritten in place is read fresh (test `put_drops_the_cached_overrides`). The cache itself
  rests on rows being immutable after minting, ids not reused **[A]**; falsifier: a path that edits a row
  without going through `put`.
- Fit checks (group, type restriction, volume) are meant to read the roll for the module's own type id through
  `fitting::plan_fit_for`, but that function is reached only by the **inventory caller diff**
  (`handback/muta3-71/inventory-caller-muta3-71-r2.diff`), which is separate and not yet applied; without it the
  fit check still reads the type row. The load and power check reads the sheet. `module_info` serves the
  sheet's attribute list for a dynamic item so duration and capacitor need reach the HUD.
- **Not yet routed (follow-up MUTA-3b):** these readers still go to the dynamic type's row, not the roll:
  dbuff `attribute_changes` / `attribute_changes_inputs` (`dbuff.rs`, `system_effect.rs`: a warfare link's or
  system effect's inputs miss the roll); `modules.rs` `RELOAD_TIME` and `repeat_allowed`, and the no-sheet
  online fallback (cpu and power from the type); `overheat::module_hp`, the `ewar.rs` type fallbacks,
  `tackle.rs`, `mining.rs` and `salvage.rs`. Whether a result type reaches the mining and salvage readers is not
  checked here **[A]**; falsifier: a mutated mining laser or salvager (category 7 results) among the 97 that
  uses the type row's cycle time.

## 4. The static table

- Name `dynamicitemattributes`, file `res:/staticdata/dynamicitemattributes.fsdbinary`, a cFSD table [V]. Its
  layout is `findings/pyd-schemas.tsv:449-456`: a record (stride 32) with `attributeIDs` (dict of str to
  `attributeRangeDefinition`: `max` f32, `min` f32, `highIsGood` u8 optional) and `inputOutputMapping` (list
  of `inputOutputMappingDefinition`: `applicableTypes` list of i32, `resultingType` i32).
- Loader accessors [V `C:dynamicitemattributes/loader.py:17-63`]: `GetData`, `IsMutator`, `GetMutator`,
  `GetApplicableTypes` (published only), `GetMutatorAttributes`, `GetResultType(mutator, source)`,
  `GetResultTypes`, `GetAllMutatorTypes`, `GetMutatorTypesByResultTypes`.
- Counts (3670638) [V-data]: **441** mutators (all group 1964, all published), one `inputOutputMapping` each,
  **2-10** ranges each, **2308** ranges in all, **1103** source types (76 unpublished),
  **97** result types (89 category 7 modules, 8 category 18 drones), **6** ranges with `min > max` (a reversed
  window: `modifier = 0` gives `base * min`, the larger factor).
- `highIsGood` is **optional** and set on only **12 of the 2308** ranges (6 true, 6 false; 12 distinct
  mutators) [V-data]. This is the data, not an extraction loss: the decoder reads the presence bit of every
  range (2308 presence words read, none with a stray bit) and 2296 have it clear. The client expects it: it
  reads `getattr(range, 'highIsGood', None)` (`tooltip/dynamicItem.py:58`, `craftingWindow.py:133-136`) and
  `MutatedAttribute.highIsGood` falls back to the **attribute's own** `highIsGood`
  (`C:.../shared/info/attribute.py:118-122`). So a server must never need the range's flag: the colour of a
  rolled attribute comes from `attrmeta` (`typedata` `attrmeta`, `highIsGood` column), and the range's flag
  only overrides it for the 12.
- Not in the content bundle before #222; now the `mutators` section writes `mutators/ranges.tsv` (key
  mutator type, attribute) and `mutators/mappings.tsv` (docs/content/schema.md) and `S:mutator_data.rs`
  reads them (`Mutators::get`, `result_for`, `mutators_making`; `is_dynamic_type` and `value` are free functions of the module).
  A source with two results under one mutator is refused as a content problem.

## 5. Item rules

| rule | fact |
|---|---|
| creation | `Edit::CreateAssembled` mints a singleton of a dynamic type; plain `Create` and `CreatePackaged` refuse a dynamic type, so loot, market and spawn paths cannot mint one without a row (`S:item.rs`, tests `create_refuses_a_dynamic_type` and `spawn_refuses_a_dynamic_type`; the client side is
`craftingWindow.py:324-326`) [V-batch]. GM mint: `/dynamicitem <source type> <mutator type> [roll]`. |
| stacking | a singleton: our `Merge` refuses singletons [V-batch `S:item.rs:1875`]. |
| repackaging | never (`C:repackaging/__init__.py:64-66`); the server refuses with "a mutated item cannot be unassembled" and skips a dynamic type in multi-select repackage [V-batch `S:item.rs:2774`, test
`a_dynamic_item_does_not_repackage`]. |
| market | no market group, so no sell and `cannotTradeOnMarket` on the quickbar (`C:.../market/quote.py:615-619`, `checks.py:42-43`) [V]. The mutaplasmids themselves have a market group and trade normally [V-data]. |
| contracts | supported: the contract tooltip calls `AddDynamicItemAttributes(itemID, typeID)` (`contractentry.py:591`) [V]. Our contracts escrow by **moving** the item (`S:contracts.rs:289-292`) and list `itemID` (`:459`) [V], so the `dynitem` row follows the id; `GetDynamicItemInfo` therefore needs no ownership check [I]. |
| destruction | the row is swept at rollover when its item is gone from the store; an item in contract escrow or a wreck still exists, so keeps its row [V-batch `S:dynamic_item.rs:263-280`]. The mutator and the source are consumed by a successful mutation (no refund) [I]. |
| reprocessing | the dynamic type has its own `typematerials` rows [V-data], so the existing reprocess path works by type [I]. |
| NPC loot | meta group 15 is not in `LOOT_META_GROUPS` (`S:econ_exchange.rs:170`) and a dynamic type has no price row [V-data]. |
| price | 13 of the 97 result types have a nonzero `base_price` in `types/market.tsv`, the other 84 have 0 [V-data]. What a base price feeds for these items (insurance, any NPC buy) is not traced **[A]**; falsifier: a nonzero price showing in an NPC buy or an insurance quote. `baseDogmaLocation.py:1863-1868` is `FindNumUnitsToRepairModule` (paste units for a module), not a station repair cost, and is not evidence about price. |

## 6. Build split

| lane | issue | what | state in the batch tree |
|---|---|---|---|
| MUTA-0 | #68 | this doc | this change |
| MUTA-DATA | #222 | `tools/content_mutators.py`, `S:mutator_data.rs`, bundle format 1.7 | done |
| MUTA-1 | #69 | `S:dynamic_item.rs` (row, `attributes`, `info_value`, `/dynamicitem`, rollover sweep), `S:service/dynamic_item.rs` (`GetDynamicItemInfo`; `CreateDynamicItem` refuses), `Edit::CreateAssembled`, repackage refusal, `T:fci_dynamic_info.py` | done |
| MUTA-3 | #71 | fitted mutated modules read their rolls: `dogma::Overrides`, `dynamic_item::fitted`, `fitting::plan_fit_for` (via the separate inventory caller diff), `module_info`, `T:fci_mutaplasmid_fit.py` | in review (round 2 written); scenario not yet run |
| MUTA-3b | to be filed | the readers still on the type row (section 3.1) | draft |
| MUTA-2 | #70 | `dynamic_item::create` and `roll`, the `CreateDynamicItem` arm, the refusal texts, `T:fci_mutaplasmid.py`. Must land **after** MUTA-3: before it a fitted mutated module would read the dynamic type's row, which has none of CPU, power or duration, so a player-made item would fit for free | open |
| MUTA-DRONE | #224 | mutated drones (8 of the 441 mutaplasmids; category 18 results) in space: `S:drones.rs` reads drone attributes by type. Until it lands `create` refuses a drone result | open |
| MUTA-BOT | #223 | bots roll the mutaplasmids they hold, fly a good roll in place of its source, sell the rest by contract | open |
| supply | #5 | the abyss caches must drop mutaplasmids by tier; nothing else does | open |

The batch tree is `eve-offline-lanes/hotbatch`; everything marked [V-batch] landed with it, and this doc landed
in the same batch as MUTA-1. Order: data, then MUTA-1, MUTA-3, MUTA-2, then drones and bots. `dynamic_item.rs` is claimed sequentially.

Planned for MUTA-2 [A until built]: `roll(world_seed, mutator_id, source_id, attribute_id)` uniform in [0, 1)
from the world seed and the two input ids, so a replayed call rolls the same and nothing predicts it without
the seed. The uniform shape is [A]; falsifier: a TQ distribution that is not uniform (then change `roll`, no
client change is needed, since the client only sees the stored modifiers).

## 7. Open questions

1. Drones: refuse a drone result in `CreateDynamicItem` until #224, or allow them at their source's stats
   (recommended: refuse).
2. A corp office hangar as a mutation location is not modelled [A]; falsifier: a corp member mutating from an
   office hangar on TQ.
