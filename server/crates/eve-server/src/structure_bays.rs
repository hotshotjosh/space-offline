//! **A structure's deed bay and fuel bay take items** (STR-CORE, issue #42
//! part a; spec `structures-spec.md` P2).
//!
//! A pilot at a structure's controls (`structure_dock::control`) sees the
//! fuel bay (flag 172) and the deed bay (flag 180) of its inventory. Until
//! now an `Add` into either was read as a move to the hangar, so nothing could
//! enter a bay and a structure anchored the normal way sat in state 102
//! (`OnliningVulnerable`) for ever, waiting for the deed
//! ([`crate::structure::Structure::set_deed`]). This module is the hook that
//! takes those calls ([`inventory_call`], one line in `service::inventory`),
//! and the few rules around it.
//!
//! # What it does
//!
//! - **Deed bay**: takes exactly one unit of the hull's own quantum core (the
//!   type its dogma row names in attribute 3101) and nothing else. Putting the
//!   core in runs `set_deed(true)`, so the state machine leaves 102. The core
//!   cannot be taken out again.
//! - **Fuel bay**: takes fuel blocks (group 1136) and Liquid Ozone, or the
//!   lava colony reagent for the Metenox. No burn happens here (STR-FUEL).
//! - **Who**: the pilot at the controls, holding the director, CEO or station
//!   manager role of the owning corporation. Bay contents are the
//!   corporation's. Taking fuel out puts it in the pilot's own hangar.
//! - **Leaving**: bay and slot items go to asset safety at the hangar flag
//!   ([`flag_after_wrap`]); rigs are destroyed ([`dies_with_hull`]).
//! - **The dogma row** carries the hull's own core attribute 3101 through
//!   `structure_services::hull_attributes` (#41 owns that list).
//!
//! # The core rule
//!
//! Whether a core is *required* is owner decision D4. The setting is
//! [`crate::rules::Rules::structure_core_required`] (`/world corerule`),
//! default on. With it off, [`waive_cores`] finishes onlining a structure
//! that waits in state 102 without a core. A hull that names no core type
//! needs none either way.
//!
//! # [A]s (falsifier in each)
//!
//! - **D4: cores are required, and `corerule` is the switch.** Falsifier: the
//!   owner decides D4 differently.
//! - Bay contents are owned by the structure's corporation; falsifier: a TQ
//!   fuel stack showing the pilot as owner.
//! - The role gate (director, CEO or station manager of the owning corp) is
//!   ours; the client checks the role only for modules. Falsifier: a TQ pilot
//!   with control rights but no role filling the bay.
//! - Fuel taken out becomes the pilot's, in his structure hangar. Falsifier:
//!   TQ keeps it corp-owned in a corporation hangar.
//! - A rig does not survive leaving its hull (destroyed on unanchoring, as on
//!   destruction). Falsifier: TQ asset safety holding rigs.
//! - Everything a structure holds in a bay or slot goes to asset safety at
//!   flag 4 when it unanchors, and is destroyed with it when it is destroyed
//!   (owner decisions D3 and D7). Falsifier: the owner wants the core consumed
//!   on unanchoring, or the wreck to hold them.
//! - The fuel bay has no volume limit yet. Falsifier: a TQ refusal for
//!   overfilling it.
//! - The refusal reason of `CrpAccessDenied` is a plain sentence, not a
//!   client label. Falsifier: the dialog reads wrong.
//!
//! The error keys are the client's own (`findings/user-errors.tsv`); where
//! TQ raises each is inferred from its name **[I]**.

use crate::item::{Edit, Slot};
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::structure::{self, Structure};
use crate::structure_dock::control::{self, FLAG_DEED, FLAG_FUEL};
use eve_marshal::PyValue;

#[cfg(test)]
mod tests;

/// `groupFuelBlock` **[V]** (`inventorycommon/const`).
pub const GROUP_FUEL_BLOCK: i64 = 1_136;
/// `typeLiquidOzone` **[V]**: also taken by the fuel bay of a non-Metenox.
pub const TYPE_LIQUID_OZONE: i64 = 16_273;
/// `typeColonyReagentLava` **[V]**: the Metenox's fuel-bay content.
pub const TYPE_COLONY_REAGENT_LAVA: i64 = 81_143;
/// `attributeStructureRequiresDeedType` **[V]** (`dogma/const`): the core
/// type the hull needs in its deed bay.
pub const ATTR_REQUIRES_DEED_TYPE: i64 = 3_101;

/// `containerHangar`, which an aliased hangar inventory is bound by.
const CONTAINER_HANGAR: i64 = 10_004;

/// `flagRigSlot0..2` **[V]**.
const RIG_SLOTS: std::ops::RangeInclusive<i64> = 92..=94;

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn type_arg(type_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(crate::modules::UE_TYPEID),
        PyValue::Int(type_id),
    ])
}

fn denied(reason: &str) -> ServiceError {
    raise(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn notify(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// The hull's own core type: attribute 3101 **[V]**. `None` when the hull
/// does not carry it or it is 0 (the hull needs no core).
pub fn core_type(content: &crate::content::Content, structure_type: i64) -> Option<i64> {
    crate::typedata::own_attribute_in(content, structure_type, ATTR_REQUIRES_DEED_TYPE)
        .filter(|v| *v != 0.0)
        .map(|v| v as i64)
}

/// Whether the Metenox's fuel rule applies to `structure_type`.
fn is_metenox(structure_type: i64) -> bool {
    structure_type == structure::TYPE_UPWELL_AUTO_MOON_MINER
}

/// Whether `flag` is one of the two bays this module owns.
pub fn is_owned_bay(flag: i64) -> bool {
    flag == FLAG_FUEL || flag == FLAG_DEED
}

/// May an item of `item_type` / `item_group` go into `flag` of a structure of
/// `structure_type`? Only the content rule: who, the source and the bay's
/// state are the call's.
pub fn may_hold(
    content: &crate::content::Content,
    structure_type: i64,
    flag: i64,
    item_type: i64,
    item_group: i64,
) -> Result<(), ServiceError> {
    match flag {
        FLAG_DEED => {
            if core_type(content, structure_type) == Some(item_type) {
                Ok(())
            } else {
                Err(raise("CannotPutNonDeedInDeedSlot", vec![]))
            }
        }
        FLAG_FUEL => {
            let ok = item_group == GROUP_FUEL_BLOCK
                || if is_metenox(structure_type) {
                    item_type == TYPE_COLONY_REAGENT_LAVA
                } else {
                    item_type == TYPE_LIQUID_OZONE
                };
            if ok {
                Ok(())
            } else {
                Err(raise(
                    "CannotPlaceTypeInFuelBay",
                    vec![("type", type_arg(item_type))],
                ))
            }
        }
        _ => Ok(()),
    }
}

/// Where an item held in `flag` of a structure goes when the structure is
/// unanchored: bay and slot items to the hangar (asset safety keeps the flag,
/// and a wrap holding a flag-172 stack would be nonsense).
pub fn flag_after_wrap(flag: i64) -> i64 {
    if control::is_bay_flag(flag) {
        crate::model::FLAG_HANGAR
    } else {
        flag
    }
}

/// Whether an item in `flag` at a structure is destroyed with it: its bays
/// and slots, and its rigs ([`RIG_SLOTS`]).
pub fn dies_with_hull(flag: i64) -> bool {
    control::is_bay_flag(flag) || RIG_SLOTS.contains(&flag)
}

/// Whether the rigs are destroyed when the structure leaves space (unanchor).
fn is_rig(flag: i64) -> bool {
    RIG_SLOTS.contains(&flag)
}

/// A structure leaving space (unanchoring): its rigs are destroyed before
/// anything is wrapped, so asset safety never receives one.
pub fn destroy_rigs(reg: &mut Registry, structure_id: i64) {
    let rigs: Vec<Edit> = reg
        .store()
        .contents_of(structure_id)
        .filter(|i| is_rig(i.flag_id))
        .map(|i| Edit::Destroy { item_id: i.item_id })
        .collect();
    if rigs.is_empty() {
        return;
    }
    if let Err(e) = reg.apply_items(&rigs) {
        crate::errln!("[structure] the rigs of {structure_id} were refused: {e:?}");
    }
}

/// Whether `s` waits in state 102 for a core it does not need: the world's
/// rule says cores are not required, or its hull names none.
fn core_waived(reg: &Registry, s: &Structure) -> bool {
    !reg.config().rules.structure_core_required || core_type(reg.content(), s.type_id).is_none()
}

/// The tick's pre-pass: a structure in state 102 with no core in its bay that
/// needs none gets its deed, so the state machine finishes onlining it. A
/// no-op when nothing waits.
pub fn waive_cores(reg: &mut Registry, now: i64) {
    let waiting: Vec<i64> = structure::all(reg.deltas())
        .iter()
        .filter(|s| s.state == structure::State::OnliningVulnerable && !s.deed_present)
        .filter(|s| core_waived(reg, s))
        .map(|s| s.id)
        .collect();
    for id in waiting {
        let Some(mut s) = structure::get(reg.deltas(), id) else {
            continue;
        };
        let e = crate::structure_space::env_for(reg, &s, now);
        s.set_deed(true, &e);
        let events = s.take_events();
        structure::put(reg.deltas_mut(), &s);
        reg.mark_dirty();
        crate::structure_space::react_at(reg, id, &events, now);
    }
}

/// `actor` may fill and empty the bays of `s`: a director, the CEO or a
/// station manager of the owning corporation **[A]**.
fn may_use(reg: &Registry, actor: i64, s: &Structure) -> Result<(), ServiceError> {
    let in_owner = reg
        .corporations()
        .membership(actor)
        .is_some_and(|m| m.corporation_id == s.owner_corp);
    if in_owner && crate::service::sov_hub::may_launch(reg, actor) {
        Ok(())
    } else {
        Err(denied(
            "You need the Director or Station Manager role in the corporation that owns this structure.",
        ))
    }
}

fn bound_of(bind: Option<&PyValue>) -> Option<i64> {
    match bind? {
        PyValue::Tuple(parts) | PyValue::List(parts) => match parts.first()? {
            PyValue::Int(id) => Some(*id),
            _ => None,
        },
        PyValue::Int(id) => Some(*id),
        _ => None,
    }
}

/// The calls on a bound inventory this module answers: an `Add` or `MultiAdd`
/// into the controlled structure's fuel or deed bay, and an `Add` of items
/// out of those bays into the hangar. `None` for everything else, which
/// carries on in `service::inventory` unchanged (a pilot not at the controls
/// included).
pub(crate) fn inventory_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if req.service != "invbroker::inventory" || !matches!(req.method, "Add" | "MultiAdd") {
        return None;
    }
    let actor = req.actor?;
    let sid = control::controlling(reg, actor)?;
    let bound = bound_of(req.bind_params)?;
    let a = Args::new(req);
    let ids: Vec<i64> = if req.method == "Add" {
        a.int(0).into_iter().collect()
    } else {
        a.ids(0).unwrap_or_default()
    };
    if ids.is_empty() {
        return None;
    }
    let qty = a.int_kw("qty").filter(|_| req.method == "Add");
    let structure_bound = bound == sid
        || bound == crate::structure_dock::CONTAINER_STRUCTURE
        || bound == CONTAINER_HANGAR;
    let flag = a.int_kw("flag");
    if let Some(flag) = flag.filter(|f| is_owned_bay(*f))
        && (bound == sid || bound == crate::structure_dock::CONTAINER_STRUCTURE)
    {
        let s = structure::get(reg.deltas(), sid)?;
        return Some(put_all(reg, actor, &s, &ids, qty, flag));
    }
    let in_bay = |reg: &Registry, id: i64| {
        reg.item(id)
            .is_some_and(|i| i.location_id == sid && is_owned_bay(i.flag_id))
    };
    if structure_bound
        && flag.is_none_or(|f| f == crate::model::FLAG_HANGAR)
        && ids.iter().all(|id| in_bay(reg, *id))
    {
        let s = structure::get(reg.deltas(), sid)?;
        return Some(take_all(reg, actor, &s, &ids, qty));
    }
    None
}

fn put_all(
    reg: &mut Registry,
    actor: i64,
    s: &Structure,
    ids: &[i64],
    qty: Option<i64>,
    flag: i64,
) -> Result<PyValue, ServiceError> {
    may_use(reg, actor, s)?;
    let mut first = None;
    let mut done = false;
    for &id in ids {
        match put(reg, actor, s, id, qty, flag) {
            Ok(()) => done = true,
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    match first {
        Some(e) if !done || ids.len() == 1 => Err(e),
        _ => Ok(PyValue::None),
    }
}

/// The units of `item` that `actor` may put into a bay of `s`: his own, in
/// his hangar here or in his active ship docked here.
fn source_ok(reg: &Registry, actor: i64, sid: i64, item: &crate::item::Item) -> bool {
    if item.owner_id != actor {
        return false;
    }
    if item.location_id == sid && item.flag_id == crate::model::FLAG_HANGAR {
        return true;
    }
    reg.character(actor).is_some_and(|c| {
        c.ship_id == item.location_id && !crate::item::is_ship_fitting_flag(item.flag_id)
    })
}

/// Put `item_id` (`qty` units of it, or the whole stack) into `flag`.
pub fn put(
    reg: &mut Registry,
    actor: i64,
    s: &Structure,
    item_id: i64,
    qty: Option<i64>,
    flag: i64,
) -> Result<(), ServiceError> {
    let Some(item) = reg.item(item_id).cloned() else {
        return Err(notify("That item is not in your hangar."));
    };
    if !source_ok(reg, actor, s.id, &item) {
        return Err(notify("That item is not in your hangar or your ship here."));
    }
    may_hold(reg.content(), s.type_id, flag, item.type_id, item.group_id)?;
    let have = item.quantity.max(1);
    let units = if flag == FLAG_DEED {
        1
    } else {
        qty.unwrap_or(have).clamp(1, have)
    };
    let to = Slot::new(s.owner_corp, s.id, flag);
    let in_bay: Vec<crate::item::Item> = reg
        .store()
        .contents_of(s.id)
        .filter(|i| i.flag_id == flag)
        .cloned()
        .collect();
    if flag == FLAG_DEED && !in_bay.is_empty() {
        return Err(raise("CannotPutNonDeedInDeedSlot", vec![]));
    }
    let same = in_bay
        .iter()
        .find(|i| i.type_id == item.type_id && !i.singleton)
        .map(|i| i.item_id);
    let mut edits = Vec::new();
    match (same, units >= have) {
        // A merge needs one address, so the units are added to the bay's
        // stack and taken off (or consumed whole from) the source.
        (Some(into), whole) => {
            edits.push(if whole {
                Edit::Destroy { item_id }
            } else {
                Edit::Consume {
                    item_id,
                    quantity: units,
                }
            });
            edits.push(Edit::Grow {
                item_id: into,
                quantity: units,
            });
        }
        (None, true) => edits.push(Edit::Move { item_id, to }),
        (None, false) => edits.push(Edit::Split {
            item_id,
            quantity: units,
            to,
        }),
    }
    reg.apply_items(&edits)
        .map_err(|e| notify(&format!("The bay refused it: {e}")))?;
    if flag == FLAG_DEED {
        let mut row = structure::get(reg.deltas(), s.id).unwrap_or_else(|| s.clone());
        let now = crate::pyutil::filetime_now();
        let e = crate::structure_space::env_for(reg, &row, now);
        row.set_deed(true, &e);
        let events = row.take_events();
        structure::put(reg.deltas_mut(), &row);
        reg.mark_dirty();
        crate::structure_space::react(reg, s.id, &events);
    }
    Ok(())
}

fn take_all(
    reg: &mut Registry,
    actor: i64,
    s: &Structure,
    ids: &[i64],
    qty: Option<i64>,
) -> Result<PyValue, ServiceError> {
    may_use(reg, actor, s)?;
    for &id in ids {
        take(reg, actor, s, id, qty)?;
    }
    Ok(PyValue::None)
}

/// Take `item_id` out of the fuel bay into the pilot's own hangar. The deed
/// bay's core stays.
pub fn take(
    reg: &mut Registry,
    actor: i64,
    s: &Structure,
    item_id: i64,
    qty: Option<i64>,
) -> Result<(), ServiceError> {
    let Some(item) = reg.item(item_id).cloned() else {
        return Err(notify("That item is not here."));
    };
    if item.location_id != s.id {
        return Err(notify("That item is not here."));
    }
    if item.flag_id == FLAG_DEED {
        return Err(raise("CannotRemoveDeedOnceFitted", vec![]));
    }
    let to = Slot::new(actor, s.id, crate::model::FLAG_HANGAR);
    let have = item.quantity.max(1);
    let units = qty.unwrap_or(have).clamp(1, have);
    let edit = if units >= have {
        Edit::Move { item_id, to }
    } else {
        Edit::Split {
            item_id,
            quantity: units,
            to,
        }
    };
    reg.apply_items(&[edit])
        .map(|_| ())
        .map_err(|e| notify(&format!("The hangar refused it: {e}")))
}
