//! **In-space compression** (PROG-MINE): `inSpaceCompressionMgr`, and the
//! slim attribute that makes a ship a compression facility. Lane note
//! `docs/lanes/prog-mine.md`.
//!
//! What the client does, read from its own code **[V]**
//! (`itemcompression/client/*`, `menucheckers/itemCheckers.py`):
//!
//! - **The facility is a ship.** `GetFacilitiesInRange` walks the ballpark's
//!   slim items: the pilot's own ship (distance 0), and every other ball
//!   whose slim has `compression_facility_typelists` and whose `charID` is
//!   a fleet member. That attribute is a dict `{typeListID:
//!   activationRange}` (metres, `distance > activationRange` skips it);
//!   [`MAX_FACILITY_ACTIVATION_RANGE`] 250 km is the client's ceiling.
//! - **The call.** `sm.RemoteSvc('inSpaceCompressionMgr')
//!   .CompressItemInSpace(itemID, facilityBallID)` answers `None` for a
//!   refusal (the client then says `FailureToCompress`) or the six-tuple
//!   `(sourceItemID, sourceTypeID, sourceQuantity, outputItemID,
//!   outputTypeID, outputQuantity)`.
//! - **Enabled by default**: `globalConfig.IsMaterialCompressionEnabled`
//!   defaults to `True` and this server never sends the config.
//! - **A compressor module** is group 4174 **[V-data]**, effect 8364
//!   (`effectIndustrialItemCompression`), attribute 3255
//!   (`attributeCompressibleItemsTypeList`) names the typelist it takes
//!   (330 moon ore, 331 ice, 332 gas, 333 mercoxit, 334 asteroid ore) and
//!   attribute 54 (`maxRange`) is the facility range (44 km medium, 83 km
//!   large, 144 km capital) **[V-data]**; `canFitShipType1` (1302) names the
//!   hull (Porpoise, Orca, Rorqual) and the fitting validator enforces it.
//! - **The structure path** (`structureCompressionMgr`, `CompressItemInStructure`,
//!   `DecompressGasInStructure`, `GetMyGasDecompressionEfficiency`) is
//!   answered below. The client offers Compress in any structure with no
//!   service check, so the server is the only gate: every call refuses
//!   unless the structure offers the reprocessing service (service 4) to the
//!   pilot. The service comes from an online Standup Reprocessing Facility
//!   module (`structure_services`, #41), never from the hull, so a structure
//!   without one refuses every call and the gas window's efficiency reads
//!   `(0.0, 0.0)`. A refusal
//!   **raises** a `UserError`: the gas decompression window unpacks the
//!   reply with no `None` check **[V]**
//!   (`itemcompression/client/ui/decompression_window`), while the compress
//!   call answers `None` harmlessly.
//!
//! What is **[I]** (inferred from the data, no server code to read): a unit
//! of compressed ore replaces **one** unit of ore -- `types/compress.tsv`'s
//! compressed twin refines to exactly what the raw ore does per portion
//! (`types/materials.tsv`, both portion 100) and is a hundredth of the
//! volume per unit **[V-data]** (`moon.rs` header). So compression is
//! 1:1 in count and 100:1 in volume. Falsified by a client whose compressed
//! stack's refine yield is not the source's. The *ratio* of the volumes is
//! also what makes compression worth a trip: a Venture's 5,000 m3 ore hold
//! holds 100 times the ore once compressed.
//!
//! **[A]** the facility must have the compressor **fitted and online**
//! (a module that is not online does nothing); that the facility and the
//! pilot share a system and, unless it is the pilot's own ship, a fleet;
//! that the pilot is within the facility's range at the call. The client
//! checks the range itself, so the server's check only stops a modified
//! client. Falsified by a retail capture of a refusal the client does not
//! pre-empt.
//!
//! **Wiring (coordinator rows):** `lib.rs` `pub mod compression;`;
//! `service.rs` `DOMAINS` row `(compression::SERVICES,
//! compression::dispatch)`; `slim_extra::SOURCES` row
//! `crate::compression::slim_extras`.

use crate::item::{Edit, Slot};
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::slim_extra::SlimExtras;
use eve_marshal::PyValue;

/// The one service this file answers.
pub const SERVICES: &[&str] = &["inSpaceCompressionMgr", "structureCompressionMgr"];

/// `itemcompression.STRUCTURE_COMPRESSIBLE_TYPE_LIST_ID` **[V]**
/// (`itemcompression/__init__`).
pub const STRUCTURE_TYPELIST: i64 = 336;
/// `groupCompressedGas` **[V]** (`inventorycommon/const`).
pub const GROUP_COMPRESSED_GAS: i64 = 4_168;

/// `itemcompression.MAX_FACILITY_ACTIVATION_RANGE` **[V]**, metres.
pub const MAX_FACILITY_ACTIVATION_RANGE: f64 = 250_000.0;
/// `itemcompression.SLIM_ATTRIBUTE_COMPRESSION_FACILITY_TYPELISTS` **[V]**.
pub const SLIM_KEY: &str = "compression_facility_typelists";
/// The compressor modules' group **[V-data]**.
pub const GROUP_COMPRESSOR: i64 = 4174;
/// `effectIndustrialItemCompression` **[V]** (`dogma/const.py`).
pub const EFFECT_COMPRESSION: i64 = 8364;
/// `attributeCompressibleItemsTypeList` **[V]** (`dogma/const.py`).
pub const ATTR_TYPELIST: i64 = 3255;
/// `maxRange` **[V]**.
pub const ATTR_RANGE: i64 = 54;
/// The holds a pilot's ore may sit in **[V]**
/// (`compress_in_space_controller.HARVESTABLE_ORE_HOLDS`): cargo, general
/// mining hold, asteroid (ore) hold, ice hold, gas hold. Flag ids:
/// `flagCargo` 5, `flagGeneralMiningHold` 134, `flagSpecializedAsteroidHold`
/// 137, `flagSpecializedIceHold` 181, `flagSpecializedGasHold` 135.
pub const ORE_HOLD_FLAGS: [i64; 5] = [5, 134, 137, 181, 135];
/// Volume of a compressed unit over a raw unit **[V-data]** (a hundredth).
pub const VOLUME_RATIO: f64 = 0.01;

/// One compressor fitted to a ship: the typelist it takes and its range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facility {
    pub module_id: i64,
    pub type_list: i64,
    pub range: f64,
}

/// **The compressors fitted and online on `ship_id`**, from the type data
/// (`Installed`), none when the content has no type data.
pub fn facilities_on(w: &Registry, ship_id: i64) -> Vec<Facility> {
    let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
        return Vec::new();
    };
    crate::modules::slim_modules(w.store(), ship_id)
        .into_iter()
        .filter(|m| w.modules().is_online(m.item_id))
        .filter_map(|m| {
            let row = td.table().get(m.type_id)?;
            if !row.has_effect(EFFECT_COMPRESSION) {
                return None;
            }
            let list = row.attribute(ATTR_TYPELIST)? as i64;
            let range = row
                .attribute(ATTR_RANGE)
                .unwrap_or(0.0)
                .min(MAX_FACILITY_ACTIVATION_RANGE);
            Some(Facility {
                module_id: m.item_id,
                type_list: list,
                range,
            })
        })
        .collect()
}

/// **The slim-extra source** (`slim_extra::SOURCES`): a pilot's ship with a
/// compressor fitted and online carries `compression_facility_typelists`
/// `{typeListID: range}`, the larger range where two compressors take one
/// list.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> SlimExtras {
    ids.iter()
        .filter_map(|id| {
            w.roster().by_ship(*id)?;
            let mut lists: Vec<(i64, f64)> = Vec::new();
            for f in facilities_on(w, *id) {
                match lists.iter_mut().find(|(l, _)| *l == f.type_list) {
                    Some(e) => e.1 = e.1.max(f.range),
                    None => lists.push((f.type_list, f.range)),
                }
            }
            if lists.is_empty() {
                return None;
            }
            lists.sort_by_key(|(l, _)| *l);
            Some((
                *id,
                vec![(
                    SLIM_KEY,
                    PyValue::Dict(
                        lists
                            .into_iter()
                            .map(|(l, r)| (PyValue::Real(r), PyValue::Int(l)))
                            .collect(),
                    ),
                )],
            ))
        })
        .collect()
}

/// Why a compression was refused (the client sees `None` for every one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotInSpace,
    NoSuchItem,
    /// The item is not in the pilot's ship, not the pilot's, or in a flag
    /// that is not an ore hold.
    NotInHold,
    /// The type has no compressed twin.
    NotCompressible,
    NoFacility,
    /// The facility is not here, not the pilot's own and not a fleet mate.
    FacilityNotHere,
    /// No online compressor on the facility takes this type.
    NoCompressor,
    OutOfRange,
    /// The store refused the plan.
    Store,
}

/// The result tuple the client unpacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {
    pub source_item: i64,
    pub source_type: i64,
    pub source_quantity: i64,
    pub output_item: i64,
    pub output_type: i64,
    pub output_quantity: i64,
}

impl Done {
    /// The tuple; an `output_item` of 0 (a decompression that made nothing)
    /// is the client's `None`.
    pub fn to_pyvalue(self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.source_item),
            PyValue::Int(self.source_type),
            PyValue::Int(self.source_quantity),
            if self.output_item == 0 {
                PyValue::None
            } else {
                PyValue::Int(self.output_item)
            },
            PyValue::Int(self.output_type),
            PyValue::Int(self.output_quantity),
        ])
    }
}

/// **The edits that turn `source` into `quantity` of `out_type` at `slot`**:
/// destroy the source (unless `quantity` is 0, which keeps it) and grow the
/// same-type stack already at `slot` (the lowest id), else create one there.
/// Shared by the in-space and the structure paths. Compression is 1:1 in
/// count **[I]** (header).
pub fn plan_twin(
    store: &crate::item::Store,
    source: &crate::item::Item,
    out_type: i64,
    quantity: i64,
    slot: Slot,
) -> Vec<Edit> {
    if quantity <= 0 {
        return Vec::new();
    }
    let stack = store
        .at(slot)
        .filter(|i| i.type_id == out_type && !i.singleton && i.item_id != source.item_id)
        .map(|i| i.item_id)
        .min();
    vec![
        Edit::Destroy {
            item_id: source.item_id,
        },
        match stack {
            Some(item_id) => Edit::Grow { item_id, quantity },
            None => Edit::Create {
                type_id: out_type,
                quantity,
                to: slot,
            },
        },
    ]
}

/// **Compress `item_id` in `actor`'s ship at the facility ball `ball`.**
pub fn compress(w: &mut Registry, actor: i64, item_id: i64, ball: i64) -> Result<Done, Refusal> {
    let me = w.roster().get(actor).copied().ok_or(Refusal::NotInSpace)?;
    let it = w.store().get(item_id).ok_or(Refusal::NoSuchItem)?.clone();
    if it.owner_id != actor || it.location_id != me.ship_id || it.singleton {
        return Err(Refusal::NotInHold);
    }
    if !ORE_HOLD_FLAGS.contains(&it.flag_id) {
        return Err(Refusal::NotInHold);
    }
    let twin = w
        .content()
        .try_get::<crate::reprocess::Table>()
        .and_then(|t| t.compressed_type(it.type_id))
        .ok_or(Refusal::NotCompressible)?;
    let fac = w
        .roster()
        .by_ship(ball)
        .copied()
        .ok_or(Refusal::NoFacility)?;
    let same_fleet = || {
        w.fleets().fleet_of(actor).is_some_and(|f| {
            f.members.contains_key(&actor) && f.members.contains_key(&fac.character_id)
        })
    };
    if fac.solar_system_id != me.solar_system_id || (fac.ship_id != me.ship_id && !same_fleet()) {
        return Err(Refusal::FacilityNotHere);
    }
    let lists = w.content().try_get::<crate::dungeon_data::Table>();
    let ok = facilities_on(w, ball).into_iter().filter(|f| {
        lists
            .and_then(|t| t.typelists.get(&f.type_list))
            .is_some_and(|l| l.contains(it.type_id, it.group_id, it.category_id))
    });
    let best = ok
        .map(|f| f.range)
        .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r))))
        .ok_or(Refusal::NoCompressor)?;
    if fac.ship_id != me.ship_id {
        w.advance_character(actor);
        w.advance_character(fac.character_id);
        let (a, b) = (
            w.roster()
                .get(actor)
                .map_or([0.0; 3], |p| p.motion.position),
            w.roster()
                .get(fac.character_id)
                .map_or([0.0; 3], |p| p.motion.position),
        );
        if crate::space_state::distance(a, b) > best {
            return Err(Refusal::OutOfRange);
        }
    }
    let slot = Slot::new(actor, me.ship_id, it.flag_id);
    let quantity = it.quantity;
    let edits = plan_twin(w.store(), &it, twin, quantity, slot);
    w.apply_items(&edits).map_err(|_| Refusal::Store)?;
    let output_item = w
        .store()
        .at(slot)
        .filter(|i| i.type_id == twin && !i.singleton)
        .map(|i| i.item_id)
        .min()
        .unwrap_or(0);
    Ok(Done {
        source_item: item_id,
        source_type: it.type_id,
        source_quantity: quantity,
        output_item,
        output_type: twin,
        output_quantity: quantity,
    })
}

// ---------------------------------------------------------------------------
// In a structure (`structureCompressionMgr`)
// ---------------------------------------------------------------------------

/// Why a structure call was refused. Every one is a raised `UserError`
/// ([`StructureRefusal::key`]); none is ever the client's `None`, because
/// the gas window unpacks the reply without a check **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureRefusal {
    /// The pilot is not docked in a structure.
    NotDocked,
    /// The structure has no online reprocessing service.
    ServiceUnavailable,
    /// The structure offers it but its profile does not grant it to the pilot.
    ServiceDenied,
    /// The item is not the pilot's, or not in the structure hangar or the
    /// active ship's cargo or ore holds.
    NotCompressable,
}

impl StructureRefusal {
    /// The client's own message key (`findings/user-errors.tsv`, no raise
    /// site in the client, so the server raises it): the three keys are
    /// **[V]**. Which one answers which refusal is **[A]**: a structure that
    /// runs no service, or none the pilot is docked in, says
    /// `StructureServiceUnavailable`; one that runs it but whose profile
    /// denies the pilot says `StructureReprocessingDenied`. Falsifier: a
    /// retail refusal text that differs (the window reads only the key).
    pub fn key(self) -> &'static str {
        match self {
            StructureRefusal::NotDocked | StructureRefusal::ServiceUnavailable => {
                "StructureServiceUnavailable"
            }
            StructureRefusal::ServiceDenied => "StructureReprocessingDenied",
            StructureRefusal::NotCompressable => "NotCompressableItem",
        }
    }

    fn raise(self) -> ServiceError {
        crate::modules::user_error(self.key(), Vec::new())
    }
}

/// **The gate**: the structure `actor` is docked in, when it offers the
/// reprocessing service (service 4) to them. A structure with no reprocessing
/// module offers none, so every call stops here **[I]**: the answer is
/// [`crate::structure_services::services_of`]'s, #41's real service map. The two keys: `StructureReprocessingDenied` when the
/// structure runs the service but the profile does not grant it to the
/// pilot, else `StructureServiceUnavailable` **[I which when]**.
pub fn structure_gate(reg: &Registry, actor: i64) -> Result<i64, StructureRefusal> {
    let id = crate::structure_dock::docked_in(reg, actor).ok_or(StructureRefusal::NotDocked)?;
    if crate::structure_dock::service_refusal(
        reg,
        actor,
        crate::structure_profile::SERVICE_REPROCESSING,
    )
    .is_none()
    {
        return Ok(id);
    }
    let runs = crate::structure::get(reg.deltas(), id).is_some_and(|s| {
        crate::structure_services::services_of(reg, &s)
            .get(&crate::structure_profile::SERVICE_REPROCESSING)
            == Some(&crate::structure_profile::SERVICE_STATE_ONLINE)
    });
    Err(if runs {
        StructureRefusal::ServiceDenied
    } else {
        StructureRefusal::ServiceUnavailable
    })
}

/// The item `item_id`, when it is `actor`'s and sits directly in the
/// structure's hangar or, when `ship_ok`, in the cargo or an ore hold of
/// their active ship. The client's menu rules **[V]**
/// (`menucheckers/itemCheckers`): `OfferCompressInStructure` takes the
/// personal hangar, the active ship or a corp office; `OfferDecompressGas`
/// takes the personal hangar or a corp office, never the ship. We narrow the
/// ship to its cargo and ore holds **[I]** (the compress path's own
/// `ORE_HOLD_FLAGS`; falsified by a client that offers compress on an item
/// in another bay and expects it served). A corp office is not served
/// **[A]**: no corp office exists in a structure here; falsified by one.
fn item_here(
    w: &Registry,
    actor: i64,
    structure: i64,
    item_id: i64,
    ship_ok: bool,
) -> Result<crate::item::Item, StructureRefusal> {
    let it = w
        .store()
        .get(item_id)
        .filter(|i| i.owner_id == actor && !i.singleton)
        .ok_or(StructureRefusal::NotCompressable)?
        .clone();
    let ship = w
        .character(actor)
        .and_then(|c| crate::fitting::active_ship_of(w.store(), c))
        .map(|(id, _)| id);
    let in_hangar = it.location_id == structure && it.flag_id == crate::model::FLAG_HANGAR;
    let in_ship = ship_ok && Some(it.location_id) == ship && ORE_HOLD_FLAGS.contains(&it.flag_id);
    if in_hangar || in_ship {
        Ok(it)
    } else {
        Err(StructureRefusal::NotCompressable)
    }
}

/// **What compressing `item_id` here would do**, past the gate: the item and
/// its twin. The type must be in `typelist` (336 in play) and have a twin.
pub fn plan_compress(
    w: &Registry,
    actor: i64,
    structure: i64,
    item_id: i64,
    typelist: i64,
) -> Result<(crate::item::Item, i64), StructureRefusal> {
    let it = item_here(w, actor, structure, item_id, true)?;
    let listed = w
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.typelists.get(&typelist))
        .is_some_and(|l| l.contains(it.type_id, it.group_id, it.category_id));
    let twin = w
        .content()
        .try_get::<crate::reprocess::Table>()
        .and_then(|t| t.compressed_type(it.type_id))
        .filter(|_| listed)
        .ok_or(StructureRefusal::NotCompressable)?;
    Ok((it, twin))
}

/// **What decompressing `item_id` here would do**, past the gate: the item
/// (in the pilot's structure hangar only, never the ship **[V]**) and the one
/// material it unpacks to. The client's own three checks
/// **[V]** (`itemcompression/gasDecompression`): the type is in group
/// [`GROUP_COMPRESSED_GAS`], has exactly one material, and that material's
/// quantity and the portion size are both 1.
pub fn plan_decompress(
    w: &Registry,
    actor: i64,
    structure: i64,
    item_id: i64,
) -> Result<(crate::item::Item, i64), StructureRefusal> {
    let it = item_here(w, actor, structure, item_id, false)?;
    let table = w
        .content()
        .try_get::<crate::reprocess::Table>()
        .ok_or(StructureRefusal::NotCompressable)?;
    match table.materials(it.type_id) {
        [(out, 1)]
            if it.group_id == GROUP_COMPRESSED_GAS && table.portion_size(it.type_id) == 1 =>
        {
            Ok((it, *out))
        }
        _ => Err(StructureRefusal::NotCompressable),
    }
}

/// Whole units out of `quantity` at the two shares: summed and capped at 1
/// **[A]** (falsified by a window that shows the pair but pays another
/// total), the fraction dropped **[A]** (the client shows the fractional
/// value; falsified by a retail stack that rounds up).
pub fn decompress_output(quantity: i64, structure: f64, character: f64) -> i64 {
    let total = (structure.clamp(0.0, 1.0) + character.clamp(0.0, 1.0)).min(1.0);
    ((quantity.max(0) as f64) * total).floor() as i64
}

/// `level x bonus / 100` **[A]**: the skill's 3260 is a percentage per
/// level, so the share is a fraction. Falsifier: a trained pilot whose
/// window shows another share.
pub fn character_share(level: i64, bonus_per_level: f64) -> f64 {
    (level.max(0) as f64 * bonus_per_level / 100.0).clamp(0.0, 1.0)
}

/// The pilot's share of the gas decompression: the skill is the type of
/// category skill that carries attribute 3260, found by attribute and not by
/// a written id.
pub fn character_gas_share(w: &Registry, actor: i64) -> f64 {
    let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
        return 0.0;
    };
    let attr = crate::reprocess::structure_attrs::GAS_DECOMPRESSION_EFFICIENCY_BONUS;
    td.table()
        .types()
        .filter(|r| r.category_id == crate::typedata::CATEGORY_SKILL)
        .filter_map(|r| Some((r.type_id, r.attribute(attr)?)))
        .map(|(skill, bonus)| {
            character_share(crate::drone_skills::level_of(w, actor, skill), bonus)
        })
        .fold(0.0, f64::max)
}

/// The structure's own share: the best online reprocessing module's base
/// efficiency (attribute 3262) plus the hull's bonus (3261), clamped to
/// `[0, 1]`. Both attributes are fractions **[V-data]** (the extract's rows
/// are fractions below 1, not percentages). That the two **add** is **[A]**;
/// falsifier: a window for a module on a hull that shows another total
/// (the max, or the module alone). No online reprocessing module, no share.
pub fn structure_gas_share(w: &Registry, structure: i64) -> f64 {
    let Some(s) = crate::structure::get(w.deltas(), structure) else {
        return 0.0;
    };
    let content = w.content();
    let base = crate::structure_services::fitted(w, &s)
        .iter()
        .filter(|f| f.online && s.is_online())
        .filter(|f| {
            crate::structure_services::module_services(
                crate::structure_services::MODULE_SERVICES,
                f.type_id,
            )
            .contains(&crate::structure_profile::SERVICE_REPROCESSING)
        })
        .filter_map(|f| {
            crate::typedata::own_attribute_in(
                content,
                f.type_id,
                crate::reprocess::structure_attrs::GAS_DECOMPRESSION_BASE_EFFICIENCY,
            )
        })
        .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))));
    let Some(base) = base else {
        return 0.0;
    };
    let hull = crate::typedata::own_attribute_in(
        content,
        s.type_id,
        crate::reprocess::structure_attrs::STRUCTURE_GAS_DECOMPRESSION_BONUS,
    )
    .unwrap_or(0.0);
    (base + hull).clamp(0.0, 1.0)
}

/// `CompressItemInStructure(itemID)`.
pub fn compress_in_structure(
    w: &mut Registry,
    actor: i64,
    item_id: i64,
) -> Result<Done, StructureRefusal> {
    compress_in_structure_with(w, actor, item_id, STRUCTURE_TYPELIST)
}

/// [`compress_in_structure`] under another typelist (the tests' invented one).
fn compress_in_structure_with(
    w: &mut Registry,
    actor: i64,
    item_id: i64,
    typelist: i64,
) -> Result<Done, StructureRefusal> {
    let structure = structure_gate(w, actor)?;
    let (it, twin) = plan_compress(w, actor, structure, item_id, typelist)?;
    let slot = Slot::new(actor, it.location_id, it.flag_id);
    let edits = plan_twin(w.store(), &it, twin, it.quantity, slot);
    w.apply_items(&edits)
        .map_err(|_| StructureRefusal::NotCompressable)?;
    Ok(Done {
        source_item: item_id,
        source_type: it.type_id,
        source_quantity: it.quantity,
        output_item: stack_at(w, slot, twin),
        output_type: twin,
        output_quantity: it.quantity,
    })
}

/// `DecompressGasInStructure(itemID)`: a refusal is an `Err`, never `None`.
pub fn decompress_in_structure(
    w: &mut Registry,
    actor: i64,
    item_id: i64,
) -> Result<Done, StructureRefusal> {
    let structure = structure_gate(w, actor)?;
    let (it, out) = plan_decompress(w, actor, structure, item_id)?;
    let quantity = decompress_output(
        it.quantity,
        structure_gas_share(w, structure),
        character_gas_share(w, actor),
    );
    let slot = Slot::new(actor, it.location_id, it.flag_id);
    // **[A]** a share that rounds to nothing keeps the stack and answers a
    // zero output; falsified by a retail window that eats it. The answer's
    // `source_quantity` is still the stack's size although the stack is kept:
    // the client ignores it on a zero output **[I]**.
    let edits = plan_twin(w.store(), &it, out, quantity, slot);
    w.apply_items(&edits)
        .map_err(|_| StructureRefusal::NotCompressable)?;
    Ok(Done {
        source_item: item_id,
        source_type: it.type_id,
        source_quantity: it.quantity,
        output_item: if quantity > 0 {
            stack_at(w, slot, out)
        } else {
            0
        },
        output_type: out,
        output_quantity: quantity,
    })
}

fn stack_at(w: &Registry, slot: Slot, type_id: i64) -> i64 {
    w.store()
        .at(slot)
        .filter(|i| i.type_id == type_id && !i.singleton)
        .map(|i| i.item_id)
        .min()
        .unwrap_or(0)
}

/// `GetMyGasDecompressionEfficiency()`: `(structure, character)`, each in
/// `[0, 1]` (the client raises `ValueError` outside it **[V]**), and
/// `(0, 0)` where the gate would refuse.
pub fn gas_efficiency(w: &Registry, actor: i64) -> (f64, f64) {
    match structure_gate(w, actor) {
        Ok(id) => (
            structure_gas_share(w, id).clamp(0.0, 1.0),
            character_gas_share(w, actor).clamp(0.0, 1.0),
        ),
        Err(_) => (0.0, 0.0),
    }
}

fn structure_dispatch(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(StructureRefusal::NotDocked.raise());
    };
    let item = || {
        Args::new(req)
            .int(0)
            .ok_or_else(|| StructureRefusal::NotCompressable.raise())
    };
    match req.method {
        "CompressItemInStructure" => compress_in_structure(w, actor, item()?)
            .map(Done::to_pyvalue)
            .map_err(StructureRefusal::raise),
        "DecompressGasInStructure" => decompress_in_structure(w, actor, item()?)
            .map(Done::to_pyvalue)
            .map_err(StructureRefusal::raise),
        "GetMyGasDecompressionEfficiency" => {
            let (s, c) = gas_efficiency(w, actor);
            Ok(PyValue::Tuple(vec![PyValue::Real(s), PyValue::Real(c)]))
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

/// `inSpaceCompressionMgr.CompressItemInSpace(itemID, facilityBallID)`.
pub fn dispatch(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    if req.service == "structureCompressionMgr" {
        return structure_dispatch(w, req);
    }
    match req.method {
        "CompressItemInSpace" => {
            let args = Args::new(req);
            let (Some(actor), Some(item), Some(ball)) = (req.actor, args.int(0), args.int(1))
            else {
                return Ok(PyValue::None);
            };
            Ok(compress(w, actor, item, ball).map_or(PyValue::None, Done::to_pyvalue))
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::reprocess::fixture::{ORE_MIXED, ORE_MIXED_COMPRESSED};
    use std::time::Duration;

    const ALICE: i64 = 1_591_001;
    const BOB: i64 = 1_591_002;
    const CAROL: i64 = 1_591_003;
    const ALICE_SHIP: i64 = 159_100_001;
    const BOB_SHIP: i64 = 159_100_002;
    const CAROL_SHIP: i64 = 159_100_003;
    /// An invented compressor type, its fitted item on Bob's ship and
    /// Alice's, and the typelist it takes.
    const COMPRESSOR_T: i64 = 990_641;
    const BOB_MODULE: i64 = 159_100_011;
    const ALICE_MODULE: i64 = 159_100_012;
    const LIST: i64 = 990_642;
    const ORE_GROUP: i64 = 99_001;
    const HULL_T: i64 = 990_643;
    const RANGE: f64 = 83_000.0;
    const HI_SLOT_0: i64 = 27;
    const STACK: i64 = 159_100_021;
    const STACK_QUANTITY: i64 = 5_000;

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Alice, Bob and Carol in one system. Alice and Bob are in one fleet,
    /// Bob 40 km out with an online compressor; Carol is in no fleet.
    /// Alice holds a stack of the fixture's compressible ore in cargo and
    /// has a compressor fitted **offline** (so her own ship is no facility
    /// until a test onlines it).
    fn world() -> Registry {
        world_with("")
    }

    /// [`world`] with extra typedata rows.
    fn world_with(extra_rows: &str) -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let rows = format!(
            "type\t{ORE_MIXED}\t{ORE_GROUP}\t25\t1\t100\t0.35\t0.0\t0.0\t1.0\t0\t\t\tFixture Ore\n\
             type\t{ORE_MIXED_COMPRESSED}\t{ORE_GROUP}\t25\t1\t100\t0.0035\t0.0\t0.0\t1.0\t0\t\t\tFixture Compressed Ore\n\
             type\t{COMPRESSOR_T}\t4174\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12,16,{EFFECT_COMPRESSION}\t{ATTR_TYPELIST}={LIST}.0;{ATTR_RANGE}={RANGE:?}\tFixture Compressor\n\
             type\t{HULL_T}\t25\t6\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Hull\n{extra_rows}"
        );
        let table = crate::typedata::Table::parse(&format!("{}{rows}", crate::typedata::FIXTURE))
            .expect("the rows parse");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut dungeons = crate::dungeon_data::Table::fixture();
        for list in [LIST, LOCAL_LIST] {
            dungeons.typelists.insert(
                list,
                crate::dungeon_data::TypeList {
                    inc_groups: vec![ORE_GROUP],
                    ..Default::default()
                },
            );
        }
        content.insert("dungeons", dungeons);
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
            ],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        let item = |item_id, type_id, owner, location, flag, quantity, group, category| {
            crate::item::Item {
                item_id,
                type_id,
                owner_id: owner,
                location_id: location,
                flag_id: flag,
                quantity,
                group_id: group,
                category_id: category,
                singleton: quantity == 1 && category != 25,
            }
        };
        let mut items = reg.store().items().to_vec();
        items.extend([
            item(
                BOB_MODULE,
                COMPRESSOR_T,
                BOB,
                BOB_SHIP,
                HI_SLOT_0,
                1,
                4174,
                7,
            ),
            item(
                ALICE_MODULE,
                COMPRESSOR_T,
                ALICE,
                ALICE_SHIP,
                HI_SLOT_0,
                1,
                4174,
                7,
            ),
            item(
                STACK,
                ORE_MIXED,
                ALICE,
                ALICE_SHIP,
                5,
                STACK_QUANTITY,
                ORE_GROUP,
                25,
            ),
        ]);
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        for c in [ALICE, BOB, CAROL] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [(ALICE, 0.0), (BOB, 40_000.0), (CAROL, 1_000.0)] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
            p.since = crate::space_state::now_seconds();
        }
        reg.modules_mut().set_online(ALICE_MODULE, false);
        let id = reg.fleets_mut().create(ALICE).expect("created");
        reg.fleets_mut()
            .init(ALICE, id, Some(HULL_T), None, None)
            .expect("init");
        reg.fleets_mut()
            .invite(ALICE, id, BOB, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(BOB, id, Some(HULL_T), None, None)
            .expect("accepted");
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    fn ore_of(reg: &Registry, ship: i64, type_id: i64) -> i64 {
        reg.store()
            .contents_of(ship)
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    #[test]
    fn a_fleet_mates_compressor_compresses_one_for_one() {
        let mut reg = world();
        let done = compress(&mut reg, ALICE, STACK, BOB_SHIP).expect("compresses");
        assert_eq!(
            (done.source_type, done.source_quantity),
            (ORE_MIXED, STACK_QUANTITY)
        );
        assert_eq!(
            (done.output_type, done.output_quantity),
            (ORE_MIXED_COMPRESSED, STACK_QUANTITY)
        );
        assert_eq!(ore_of(&reg, ALICE_SHIP, ORE_MIXED), 0);
        assert_eq!(
            ore_of(&reg, ALICE_SHIP, ORE_MIXED_COMPRESSED),
            STACK_QUANTITY
        );
        // The output lands in the stack's own hold, a hundredth the volume.
        let out = reg.store().get(done.output_item).expect("the output stack");
        assert_eq!((out.location_id, out.flag_id), (ALICE_SHIP, 5));
        let used = reg.store().used_volume_in(reg.content(), ALICE_SHIP, 5);
        assert!(
            (used - STACK_QUANTITY as f64 * 0.0035).abs() < 1e-6,
            "{used}"
        );
    }

    #[test]
    fn a_second_compression_grows_the_stack_instead_of_adding_one() {
        let mut reg = world();
        let first = compress(&mut reg, ALICE, STACK, BOB_SHIP).expect("compresses");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: 159_100_022,
            type_id: ORE_MIXED,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: 5,
            quantity: 100,
            group_id: ORE_GROUP,
            category_id: 25,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        let second = compress(&mut reg, ALICE, 159_100_022, BOB_SHIP).expect("again");
        assert_eq!(second.output_item, first.output_item);
        assert_eq!(second.output_quantity, 100);
        assert_eq!(
            ore_of(&reg, ALICE_SHIP, ORE_MIXED_COMPRESSED),
            STACK_QUANTITY + 100
        );
    }

    #[test]
    fn the_refusals_are_the_clients_none() {
        let mut reg = world();
        // Out of range: 40 km is inside 83 km; move Bob past it.
        reg.roster_mut().get_mut(BOB).expect("Bob").motion =
            crate::space_state::Motion::arrived([9.0e6, 0.0, 0.0], [0.0; 3]);
        assert_eq!(
            compress(&mut reg, ALICE, STACK, BOB_SHIP),
            Err(Refusal::OutOfRange)
        );
        // Her own compressor is fitted but offline.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, ALICE_SHIP),
            Err(Refusal::NoCompressor)
        );
        // Carol is in no fleet and has no compressor.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, CAROL_SHIP),
            Err(Refusal::FacilityNotHere)
        );
        // Not a facility ball at all.
        assert_eq!(
            compress(&mut reg, ALICE, STACK, 42),
            Err(Refusal::NoFacility)
        );
        // Not hers, and not an ore.
        assert_eq!(
            compress(&mut reg, BOB, STACK, BOB_SHIP),
            Err(Refusal::NotInHold)
        );
        assert_eq!(
            compress(&mut reg, ALICE, ALICE_MODULE, BOB_SHIP),
            Err(Refusal::NotInHold)
        );
        assert_eq!(ore_of(&reg, ALICE_SHIP, ORE_MIXED), STACK_QUANTITY);
    }

    #[test]
    fn a_pilot_compresses_in_his_own_online_compressor_and_a_compressed_stack_does_not_compress_again()
     {
        let mut reg = world();
        reg.modules_mut().set_online(ALICE_MODULE, true);
        let done = compress(&mut reg, ALICE, STACK, ALICE_SHIP).expect("own facility");
        assert_eq!(
            compress(&mut reg, ALICE, done.output_item, ALICE_SHIP),
            Err(Refusal::NotCompressible)
        );
    }

    #[test]
    fn only_an_online_compressor_makes_a_slim_facility_with_its_range() {
        let mut reg = world();
        let lists = |reg: &Registry, ship: i64| slim_extras(reg, &[ship]);
        assert!(lists(&reg, ALICE_SHIP).is_empty(), "offline");
        assert!(lists(&reg, CAROL_SHIP).is_empty(), "no module");
        assert_eq!(
            lists(&reg, BOB_SHIP),
            vec![(
                BOB_SHIP,
                vec![(
                    SLIM_KEY,
                    PyValue::Dict(vec![(PyValue::Real(RANGE), PyValue::Int(LIST))])
                )]
            )]
        );
        reg.modules_mut().set_online(ALICE_MODULE, true);
        assert_eq!(lists(&reg, ALICE_SHIP).len(), 1);
    }

    #[test]
    fn the_service_answers_none_for_a_refusal_and_the_tuple_for_a_success() {
        let mut reg = world();
        let call = |reg: &mut Registry, item: i64, ball: i64| {
            let args = [PyValue::Int(item), PyValue::Int(ball)];
            let req = Request {
                service: "inSpaceCompressionMgr",
                method: "CompressItemInSpace",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            };
            dispatch(reg, &req).expect("answers")
        };
        assert_eq!(call(&mut reg, STACK, CAROL_SHIP), PyValue::None);
        let PyValue::Tuple(t) = call(&mut reg, STACK, BOB_SHIP) else {
            panic!("a tuple");
        };
        assert_eq!(t.len(), 6);
        assert_eq!(t[0], PyValue::Int(STACK));
        assert_eq!(t[5], PyValue::Int(STACK_QUANTITY));
    }

    #[test]
    fn the_result_is_the_six_tuple_the_client_unpacks() {
        let d = Done {
            source_item: 1,
            source_type: 2,
            source_quantity: 3,
            output_item: 4,
            output_type: 5,
            output_quantity: 6,
        };
        assert_eq!(
            d.to_pyvalue(),
            PyValue::Tuple((1..=6).map(PyValue::Int).collect())
        );
    }

    // ----- structureCompressionMgr -----

    const STRUCTURE: i64 = 1_040_000_159_001;
    const LOCAL_LIST: i64 = 900_336;
    const SKILL_T: i64 = 990_651;
    const GAS_STACK: i64 = 159_100_031;
    const HANGAR_ORE: i64 = 159_100_032;
    const GAS_C: i64 = crate::reprocess::fixture::GAS_COMPRESSED;
    const GAS_U: i64 = crate::reprocess::fixture::GAS;
    const STRUCTURE_T: i64 = 990_035_001;
    const STR_GROUP: i64 = 990_036_001;
    const OWNER_CORP: i64 = 98_000_001;
    /// The client's own Standup Reprocessing Facility type, with an invented
    /// row: the gate reads the real service map, which keys on this id.
    const REPROC_MODULE: i64 = crate::structure_services::TYPE_STANDUP_REPROCESSING_FACILITY;
    const MODULE_SHARE: f64 = 0.5;
    const HULL_SHARE: f64 = 0.25;
    const REPROC_ITEM: i64 = 159_100_040;
    const GAS_HOLD: i64 = 159_100_033;

    /// Alice docked in an invented structure with ore and compressed gas in
    /// its hangar; a typelist `LOCAL_LIST` stands for 336, and an invented
    /// skill carries attribute 3260 = 2.0.
    fn docked_world() -> Registry {
        let mut reg = world_with(&format!(
            "type\t{SKILL_T}\t255\t16\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t3260=2.0\tFixture Gas Skill\n\
             type\t{REPROC_MODULE}\t{STR_GROUP}\t66\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t3262={MODULE_SHARE:?}\tFixture Reprocessing Module\n\
             type\t{GAS_C}\t{GROUP_COMPRESSED_GAS}\t25\t1\t1\t0.15\t0.0\t0.0\t1.0\t0\t\t\tFixture Compressed Gas\n\
             type\t{GAS_U}\t4000\t25\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Gas\n\
             type\t{STRUCTURE_T}\t{STR_GROUP}\t65\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t\t3261={HULL_SHARE:?}\tFixture Structure\n"
        ));
        let system = reg
            .character(ALICE)
            .and_then(|c| c.solar_system_id)
            .expect("a system");
        let env = crate::structure::Env::new(0);
        let mut s = crate::structure::Structure::anchored(
            STRUCTURE,
            STRUCTURE_T,
            OWNER_CORP,
            system,
            [0.0; 3],
            0.0,
            "Invented Structure",
            0,
            &env,
        );
        s.set_state(crate::structure::State::ShieldVulnerable, &env);
        reg.roster_mut().leave(ALICE);
        reg.character_mut(ALICE).expect("Alice").station_id = Some(STRUCTURE);
        let mut items = reg.store().items().to_vec();
        let mk = |item_id, type_id, quantity, group_id, category_id| crate::item::Item {
            item_id,
            type_id,
            owner_id: ALICE,
            location_id: STRUCTURE,
            flag_id: 4,
            quantity,
            group_id,
            category_id,
            singleton: false,
        };
        items.push(mk(HANGAR_ORE, ORE_MIXED, 700, ORE_GROUP, 25));
        items.push(crate::item::Item {
            item_id: GAS_HOLD,
            location_id: ALICE_SHIP,
            flag_id: 5,
            quantity: 4,
            ..mk(
                GAS_HOLD,
                crate::reprocess::fixture::GAS_COMPRESSED,
                4,
                GROUP_COMPRESSED_GAS,
                25,
            )
        });
        items.push(mk(
            GAS_STACK,
            crate::reprocess::fixture::GAS_COMPRESSED,
            9,
            GROUP_COMPRESSED_GAS,
            25,
        ));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        // After the snapshot: applying one replaces the delta rows.
        crate::structure::put(reg.deltas_mut(), &s);
        reg
    }

    fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "structureCompressionMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        dispatch(reg, &req)
    }

    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(h) = &**header else {
            return format!("{e:?}");
        };
        match h.get(1) {
            Some(PyValue::Tuple(c)) => match c.first() {
                Some(PyValue::Bytes(k)) => String::from_utf8_lossy(k).into_owned(),
                _ => format!("{e:?}"),
            },
            _ => format!("{e:?}"),
        }
    }

    /// The three client keys **[V]** a refusal may carry.
    const REFUSAL_KEYS: [&str; 3] = [
        "StructureServiceUnavailable",
        "StructureReprocessingDenied",
        "NotCompressableItem",
    ];

    fn assert_refusal(r: &Result<PyValue, ServiceError>) {
        let Err(e @ ServiceError::Raise(_)) = r else {
            panic!("a raised refusal, got {r:?}");
        };
        assert!(REFUSAL_KEYS.contains(&key_of(e).as_str()), "{}", key_of(e));
    }

    #[test]
    fn compress_refuses_outside_a_structure() {
        let mut reg = world();
        let r = call(&mut reg, "CompressItemInStructure", &[PyValue::Int(STACK)]);
        assert_refusal(&r);
        assert_eq!(ore_of(&reg, ALICE_SHIP, ORE_MIXED), STACK_QUANTITY);
    }

    #[test]
    fn compress_refuses_without_the_reprocessing_service() {
        let mut reg = docked_world();
        // The hull's services never include service 4.
        assert!(structure_gate(&reg, ALICE).is_err());
        let r = call(
            &mut reg,
            "CompressItemInStructure",
            &[PyValue::Int(HANGAR_ORE)],
        );
        assert_refusal(&r);
        assert_eq!(ore_of(&reg, STRUCTURE, ORE_MIXED), 700);
    }

    #[test]
    fn decompress_never_answers_none() {
        let mut reg = docked_world();
        for item in [GAS_STACK, HANGAR_ORE, 42] {
            let r = call(&mut reg, "DecompressGasInStructure", &[PyValue::Int(item)]);
            assert_refusal(&r);
        }
        let r = call(&mut reg, "DecompressGasInStructure", &[]);
        assert_refusal(&r);
        let mut space = world();
        let r = call(
            &mut space,
            "DecompressGasInStructure",
            &[PyValue::Int(STACK)],
        );
        assert_refusal(&r);
    }

    #[test]
    fn every_refusal_raises_one_of_the_three_client_keys() {
        for r in [
            StructureRefusal::NotDocked,
            StructureRefusal::ServiceUnavailable,
            StructureRefusal::ServiceDenied,
            StructureRefusal::NotCompressable,
        ] {
            assert!(REFUSAL_KEYS.contains(&r.key()), "{r:?}");
            assert_refusal(&Err(r.raise()));
        }
    }

    #[test]
    fn decompress_takes_the_hangar_only_and_compress_may_take_the_ship() {
        let reg = docked_world();
        // Compressed gas in the active ship's cargo: the decompress menu
        // does not offer it, the compress menu would offer a listed type.
        assert_eq!(
            plan_decompress(&reg, ALICE, STRUCTURE, GAS_HOLD),
            Err(StructureRefusal::NotCompressable)
        );
        assert!(plan_decompress(&reg, ALICE, STRUCTURE, GAS_STACK).is_ok());
        assert!(plan_compress(&reg, ALICE, STRUCTURE, STACK, LOCAL_LIST).is_ok());
        let mut reg = docked_world();
        fit_reprocessing(&mut reg, true, true);
        let r = call(
            &mut reg,
            "DecompressGasInStructure",
            &[PyValue::Int(GAS_HOLD)],
        );
        assert_refusal(&r);
        assert_eq!(
            ore_of(&reg, ALICE_SHIP, crate::reprocess::fixture::GAS_COMPRESSED),
            4
        );
    }

    #[test]
    fn plan_compress_takes_a_listed_type_from_the_hangar_only() {
        let reg = docked_world();
        let (it, twin) =
            plan_compress(&reg, ALICE, STRUCTURE, HANGAR_ORE, LOCAL_LIST).expect("listed");
        assert_eq!((it.item_id, twin), (HANGAR_ORE, ORE_MIXED_COMPRESSED));
        // Outside the typelist.
        assert_eq!(
            plan_compress(&reg, ALICE, STRUCTURE, HANGAR_ORE, LOCAL_LIST + 1),
            Err(StructureRefusal::NotCompressable)
        );
        // Not Alice's ship's hold (the ship is not where the pilot is) and not his.
        assert_eq!(
            plan_compress(&reg, BOB, STRUCTURE, HANGAR_ORE, LOCAL_LIST),
            Err(StructureRefusal::NotCompressable)
        );
        // The gas has no twin.
        assert_eq!(
            plan_compress(&reg, ALICE, STRUCTURE, GAS_STACK, LOCAL_LIST),
            Err(StructureRefusal::NotCompressable)
        );
    }

    #[test]
    fn plan_decompress_wants_the_gas_group_and_one_for_one() {
        let reg = docked_world();
        let (_, out) = plan_decompress(&reg, ALICE, STRUCTURE, GAS_STACK).expect("gas");
        assert_eq!(out, crate::reprocess::fixture::GAS);
        assert_eq!(
            plan_decompress(&reg, ALICE, STRUCTURE, HANGAR_ORE),
            Err(StructureRefusal::NotCompressable)
        );
    }

    #[test]
    fn plan_twin_grows_an_existing_compressed_stack() {
        let reg = world();
        let src = reg.store().get(STACK).expect("stack").clone();
        let slot = Slot::new(ALICE, ALICE_SHIP, 5);
        let edits = plan_twin(reg.store(), &src, ORE_MIXED_COMPRESSED, 40, slot);
        assert!(matches!(edits[0], Edit::Destroy { item_id: STACK }));
        assert!(matches!(edits[1], Edit::Create { quantity: 40, .. }));
        let mut reg = reg;
        reg.apply_items(&edits).expect("applies");
        let src2 = crate::item::Item {
            item_id: 159_100_023,
            quantity: 3,
            ..src
        };
        let edits = plan_twin(reg.store(), &src2, ORE_MIXED_COMPRESSED, 3, slot);
        assert!(
            matches!(edits[1], Edit::Grow { quantity: 3, .. }),
            "{edits:?}"
        );
    }

    #[test]
    fn plan_twin_creates_the_twin_where_the_source_was() {
        let reg = world();
        let src = reg.store().get(STACK).expect("stack").clone();
        let slot = Slot::new(ALICE, ALICE_SHIP, 5);
        let edits = plan_twin(reg.store(), &src, ORE_MIXED_COMPRESSED, 5, slot);
        assert!(matches!(
            edits[1],
            Edit::Create { type_id: ORE_MIXED_COMPRESSED, to, .. } if to == slot
        ));
        // A zero output keeps the source.
        assert!(plan_twin(reg.store(), &src, ORE_MIXED_COMPRESSED, 0, slot).is_empty());
    }

    #[test]
    fn decompression_output_never_exceeds_the_input_and_grows_with_the_shares() {
        for q in [0, 1, 7, 10, 1_000] {
            let mut last = 0;
            for step in 0..=20 {
                let share = step as f64 / 10.0 - 0.5; // out of range on purpose
                let out = decompress_output(q, share, share / 2.0);
                assert!((0..=q).contains(&out), "q {q} share {share} out {out}");
                assert!(out >= last, "monotone: q {q} share {share}");
                last = out;
            }
        }
        assert_eq!(decompress_output(10, 0.0, 0.0), 0);
        assert_eq!(decompress_output(-3, 1.0, 1.0), 0);
    }

    #[test]
    fn decompression_efficiency_is_zero_zero_outside_a_structure() {
        let reg = world();
        assert_eq!(gas_efficiency(&reg, ALICE), (0.0, 0.0));
        let mut reg = docked_world();
        assert_eq!(gas_efficiency(&reg, ALICE), (0.0, 0.0));
        let v = call(&mut reg, "GetMyGasDecompressionEfficiency", &[]).expect("answers");
        assert_eq!(
            v,
            PyValue::Tuple(vec![PyValue::Real(0.0), PyValue::Real(0.0)])
        );
    }

    #[test]
    fn character_share_is_a_fraction_that_grows_with_the_skill_level() {
        let mut last = 0.0;
        for level in -1..=7 {
            let share = character_share(level, 2.0);
            assert!((0.0..=1.0).contains(&share), "level {level}");
            assert!(share >= last, "monotone at level {level}");
            last = share;
        }
        assert_eq!(character_share(0, 2.0), 0.0);
        assert!(character_share(5, 2.0) > 0.0);
        assert!(character_share(5, 1e9) <= 1.0, "capped");
        let mut reg = docked_world();
        assert_eq!(character_gas_share(&reg, ALICE), 0.0, "not trained");
        let mut last = 0.0;
        for level in 0..=5 {
            *reg.skills_mut(ALICE).expect("skills") =
                crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
                    type_id: SKILL_T,
                    level,
                    points: 0,
                    rank: 1,
                }]);
            let share = character_gas_share(&reg, ALICE);
            assert!((0.0..=1.0).contains(&share), "level {level}");
            assert!(share >= last, "monotone at level {level}");
            last = share;
        }
        assert!(last > 0.0, "a trained pilot has a share");
    }

    /// Fit the reprocessing module in the structure's first service slot and
    /// put its online row when `online`; `owner` seats Alice in the owner
    /// corporation, which uses its own services.
    fn fit_reprocessing(reg: &mut Registry, owner: bool, online: bool) {
        use crate::structure_services as ss;
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: REPROC_ITEM,
            type_id: REPROC_MODULE,
            owner_id: OWNER_CORP,
            location_id: STRUCTURE,
            flag_id: ss::FIRST_SLOT,
            quantity: 1,
            group_id: STR_GROUP,
            category_id: 66,
            singleton: true,
        });
        let deltas = reg.deltas().clone();
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 159_200_000,
            ..crate::store::Snapshot::default()
        });
        *reg.deltas_mut() = deltas;
        if online {
            reg.deltas_mut().put(
                ss::KIND,
                vec![STRUCTURE.to_string(), REPROC_ITEM.to_string()],
                vec!["1".to_owned()],
            );
        }
        if owner {
            reg.character_mut(ALICE).expect("Alice").corporation_id = OWNER_CORP;
        }
    }

    #[test]
    fn an_online_reprocessing_module_opens_the_gate_to_its_owner_only() {
        let mut reg = docked_world();
        fit_reprocessing(&mut reg, false, true);
        // The service runs but Alice's corporation holds no reprocessing setting.
        assert!(structure_gate(&reg, ALICE).is_err());
        reg.character_mut(ALICE).expect("Alice").corporation_id = OWNER_CORP;
        assert_eq!(structure_gate(&reg, ALICE), Ok(STRUCTURE));
    }

    #[test]
    fn a_fitted_but_offline_module_leaves_the_gate_shut() {
        let mut reg = docked_world();
        fit_reprocessing(&mut reg, true, false);
        assert!(structure_gate(&reg, ALICE).is_err());
        let r = call(
            &mut reg,
            "CompressItemInStructure",
            &[PyValue::Int(HANGAR_ORE)],
        );
        assert_refusal(&r);
        assert_eq!(ore_of(&reg, STRUCTURE, ORE_MIXED), 700);
    }

    #[test]
    fn compress_in_a_structure_with_the_module_online_makes_the_twin() {
        let mut reg = docked_world();
        fit_reprocessing(&mut reg, true, true);
        // The wire arm reads the real typelist 336, which the fixture world
        // lacks: the call refuses a type it does not list.
        let r = call(
            &mut reg,
            "CompressItemInStructure",
            &[PyValue::Int(HANGAR_ORE)],
        );
        assert_refusal(&r);
        let done = compress_in_structure_with(&mut reg, ALICE, HANGAR_ORE, LOCAL_LIST)
            .expect("compresses");
        assert_eq!(done.output_quantity, 700);
        assert_eq!(done.output_type, ORE_MIXED_COMPRESSED);
        assert_eq!(ore_of(&reg, STRUCTURE, ORE_MIXED), 0);
        assert_eq!(ore_of(&reg, STRUCTURE, ORE_MIXED_COMPRESSED), 700);
        // The gas has no twin: still refused.
        let r = call(
            &mut reg,
            "CompressItemInStructure",
            &[PyValue::Int(GAS_STACK)],
        );
        assert_refusal(&r);
    }

    #[test]
    fn the_structure_share_is_a_fraction_and_only_while_a_module_is_online() {
        let mut reg = docked_world();
        assert_eq!(structure_gas_share(&reg, STRUCTURE), 0.0, "no module");
        fit_reprocessing(&mut reg, true, false);
        assert_eq!(structure_gas_share(&reg, STRUCTURE), 0.0, "offline");
        reg.deltas_mut().put(
            crate::structure_services::KIND,
            vec![STRUCTURE.to_string(), REPROC_ITEM.to_string()],
            vec!["1".to_owned()],
        );
        let share = structure_gas_share(&reg, STRUCTURE);
        assert!((0.0..=1.0).contains(&share));
        assert!(share >= MODULE_SHARE.min(1.0), "the hull never takes away");
    }

    #[test]
    fn decompress_in_a_structure_is_wired_to_the_output_rule_and_conserves() {
        for level in [0, 3, 5] {
            let mut reg = docked_world();
            fit_reprocessing(&mut reg, true, true);
            *reg.skills_mut(ALICE).expect("skills") =
                crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
                    type_id: SKILL_T,
                    level,
                    points: 0,
                    rank: 1,
                }]);
            let before = ore_of(&reg, STRUCTURE, crate::reprocess::fixture::GAS_COMPRESSED);
            let (s, c) = gas_efficiency(&reg, ALICE);
            assert!((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&c));
            let v = call(
                &mut reg,
                "DecompressGasInStructure",
                &[PyValue::Int(GAS_STACK)],
            )
            .expect("decompresses");
            let PyValue::Tuple(t) = v else {
                panic!("the six-tuple");
            };
            let out = decompress_output(before, s, c);
            assert_eq!(t[5], PyValue::Int(out), "level {level}");
            assert!(out <= before);
            let left = ore_of(&reg, STRUCTURE, crate::reprocess::fixture::GAS_COMPRESSED);
            let made = ore_of(&reg, STRUCTURE, crate::reprocess::fixture::GAS);
            assert_eq!(made, out);
            // The source is consumed exactly when something came out of it.
            assert_eq!(left == 0, out > 0, "level {level}");
            assert_eq!(left, if out > 0 { 0 } else { before });
            assert_eq!(t[3] == PyValue::None, out == 0);
        }
    }

    #[test]
    fn a_zero_output_is_the_clients_none_item() {
        let d = Done {
            source_item: 1,
            source_type: 2,
            source_quantity: 3,
            output_item: 0,
            output_type: 5,
            output_quantity: 0,
        };
        let PyValue::Tuple(t) = d.to_pyvalue() else {
            panic!("a tuple");
        };
        assert_eq!(t[3], PyValue::None);
    }
}
