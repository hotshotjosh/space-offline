//! `structureAssetSafety` (issue #49): the wire half of
//! [`crate::asset_safety`]. Shapes are **[V]** from the client's consumers
//! (`assetSafetySvc.py`, `assetSafetyEntry.py`, `assetSafetyDeliverWindow.py`):
//!
//! - `GetItemsInSafetyForCharacter()` / `ForCorp()`: a list of dicts (the
//!   Assets window and the corp Accounts tab subscript each), keys
//!   `assetWrapID`, `wrapName`, `solarSystemID`, `ejectTime` (FILETIME),
//!   `daysUntilCanDeliver`, `daysUntilAutoMove` (days), `defaultDestinationInfo`
//!   (a dict with `itemID` and `typeID`) and `structureTypeID` **[V]** (build
//!   3707965).
//! - `GetStructuresICanDeliverTo(solarSystemID, structureTypeID)`: one flat
//!   list of destinations (dicts with `itemID`, `typeID` and `solarSystemID`;
//!   the window adds `name`) **[V]**. A FOB's type adds Zarzakh's stations.
//! - `GetDefaultAssetSafetyDestination(solarSystemID, structureTypeID)`:
//!   a dict `solarSystemID`, `itemID`, `typeID` (only the system is read **[V]**).
//! - `GetWrapNames(wrapIDs)`: `{wrapID: name}`.
//! - `MovePersonalAssetsToSafety(solarSystemID, structureID)`,
//!   `MoveCorpAssetsToSafety(solarSystemID, structureID)`,
//!   `MoveSafetyWrapToStructure(wrapID, solarSystemID, destinationID=)` and
//!   `MoveEjectTimeGM(wrapID, days)`: the answer is discarded.
//! - Pushes `OnAssetSafetyCreated(ownerID, solarSystemID, locationID)` and
//!   `OnAssetSafetyDelivered(ownerID)`; the client drops its cached lists on
//!   each (`assetSafetySvc.OnAssetSafetyCreated`, `OnAssetSafetyDelivered`).
//!
//! Refusals are `CustomNotify` (**[A]**, falsifier: an empty or garbled
//! dialog).

use super::super::args::{Args, as_int};
use super::*;
use crate::asset_safety as safety;
use crate::pyutil::{dict_kv, pydict};

/// Insert the wrap's item (type 60) in `system`, owned by `owner`, and answer
/// its id. The one door to the store's private allocator that
/// [`crate::asset_safety`] needs, as `/upwell spawn` has for the structure.
pub(crate) fn insert_wrap_item(reg: &mut Registry, owner: i64, system: i64) -> i64 {
    let id = reg.store.next_id();
    reg.store.insert(crate::item::Item {
        item_id: id,
        type_id: safety::TYPE_WRAP,
        owner_id: owner,
        location_id: system,
        flag_id: 0,
        quantity: 1,
        group_id: safety::GROUP_WRAP,
        category_id: safety::CATEGORY_WRAP,
        singleton: true,
    });
    id
}

/// The pilots who hear about `owner`'s wraps: the owner, or a corporation's
/// online members.
fn audience(reg: &Registry, owner: i64) -> Vec<i64> {
    if reg.corporations().get(owner).is_some() {
        corp_audience(reg, owner)
    } else if reg.sessions().online_characters().contains(&owner) {
        vec![owner]
    } else {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Created {
    to: Vec<i64>,
    owner: i64,
    system: i64,
    location: i64,
}

impl Push for Created {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAssetSafetyCreated",
            vec![
                PyValue::Int(self.owner),
                PyValue::Int(self.system),
                PyValue::Int(self.location),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Delivered {
    to: Vec<i64>,
    owner: i64,
}

impl Push for Delivered {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnAssetSafetyDelivered", vec![PyValue::Int(self.owner)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Tell the owner's pilots a wrap was made.
pub(crate) fn tell_created(reg: &mut Registry, w: &safety::Wrap) {
    let to = audience(reg, w.owner);
    if !to.is_empty() {
        reg.schedule(
            std::time::Duration::ZERO,
            Created {
                to,
                owner: w.owner,
                system: w.system,
                location: w.origin,
            },
        );
    }
}

/// Tell the owner's pilots a wrap was delivered.
pub(crate) fn tell_delivered(reg: &mut Registry, owner: i64) {
    let to = audience(reg, owner);
    if !to.is_empty() {
        reg.schedule(std::time::Duration::ZERO, Delivered { to, owner });
    }
}

fn refused(r: safety::Refusal) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(r.text()))]),
    ))
}

/// A wait in seconds as the client's day count: whole days as an integer, else
/// a real.
fn days(secs: i64) -> PyValue {
    if secs % 86_400 == 0 {
        PyValue::Int(secs / 86_400)
    } else {
        PyValue::Real(secs as f64 / 86_400.0)
    }
}

fn station_info(id: i64, type_id: i64) -> PyValue {
    pydict(vec![
        ("itemID", PyValue::Int(id)),
        ("typeID", PyValue::Int(type_id)),
    ])
}

fn row(reg: &Registry, w: &safety::Wrap, near: (i64, i64)) -> PyValue {
    let rules = &reg.config().rules;
    let manual = days(rules.safety_manual_secs.max(1));
    let auto = days(rules.safety_auto_secs);
    pydict(vec![
        ("assetWrapID", PyValue::Int(w.id)),
        ("wrapName", PyValue::Unicode(w.name.clone())),
        ("solarSystemID", PyValue::Int(w.system)),
        ("ejectTime", PyValue::Long(w.created)),
        // The client divides by this one (`UpdateProgress`): never 0.
        ("daysUntilCanDeliver", manual),
        ("daysUntilAutoMove", auto),
        ("defaultDestinationInfo", station_info(near.0, near.1)),
        // [A] 0 is a safe type for a wrap saved before the column existed
        // (falsifier: a client traceback naming `structureTypeID`).
        ("structureTypeID", PyValue::Int(w.origin_type)),
    ])
}

/// The rows of `owner`'s wraps, for a caller who acts for them.
fn rows_of(reg: &Registry, actor: Option<i64>, owner: Option<i64>) -> PyValue {
    let (Some(actor), Some(owner)) = (actor, owner) else {
        return PyValue::List(vec![]);
    };
    if !safety::acts_for(reg, actor, owner) {
        return PyValue::List(vec![]);
    }
    PyValue::List(
        safety::of_owner(reg.deltas(), owner)
            .iter()
            // The client subscripts the destination's info and looks the id
            // up: a wrap with none to name is left off the list.
            .filter_map(|w| {
                safety::default_of(reg, w.system, w.origin_type).map(|d| row(reg, w, (d.0, d.1)))
            })
            .collect(),
    )
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    let now = crate::pyutil::filetime_now();
    match req.method {
        "GetItemsInSafetyForCharacter" => Ok(rows_of(reg, req.actor, req.actor)),
        "GetItemsInSafetyForCorp" => {
            let corp = req
                .actor
                .and_then(|c| reg.character(c))
                .map(|c| c.corporation_id);
            Ok(rows_of(reg, req.actor, corp))
        }
        "GetStructuresICanDeliverTo" => {
            let (Some(actor), Some(system)) = (req.actor, a.int_or_kw(0, "solarSystemID")) else {
                return Err(a.bad("GetStructuresICanDeliverTo(solarSystemID, structureTypeID)"));
            };
            let Some(ty) = a.int_or_kw(1, "structureTypeID") else {
                return Err(a.bad("GetStructuresICanDeliverTo(solarSystemID, structureTypeID)"));
            };
            let (fobs, hub) = (&safety::INSURGENCY_FOBS, safety::SOLAR_SYSTEM_ZARZAKH);
            let places = safety::places_for(reg, actor, system, ty, fobs, hub)
                .into_iter()
                .map(|(id, t, sys)| {
                    pydict(vec![
                        ("itemID", PyValue::Int(id)),
                        ("typeID", PyValue::Int(t)),
                        ("solarSystemID", PyValue::Int(sys)),
                    ])
                })
                .collect();
            Ok(PyValue::List(places))
        }
        "GetDefaultAssetSafetyDestination" => {
            let Some(system) = a.int_or_kw(0, "solarSystemID") else {
                return Err(
                    a.bad("GetDefaultAssetSafetyDestination(solarSystemID, structureTypeID)")
                );
            };
            let ty = a.int_or_kw(1, "structureTypeID").unwrap_or(0);
            let (id, type_id, sys) =
                safety::default_of(reg, system, ty).unwrap_or((0, 0, safety::SOLAR_SYSTEM_ZARZAKH));
            Ok(pydict(vec![
                ("solarSystemID", PyValue::Int(sys)),
                ("itemID", PyValue::Int(id)),
                ("typeID", PyValue::Int(type_id)),
            ]))
        }
        "GetWrapNames" => {
            let ids: Vec<i64> = a
                .need_list(0)?
                .iter()
                .filter_map(|v| as_int(Some(v)))
                .collect();
            Ok(dict_kv(ids.into_iter().filter_map(|id| {
                safety::get(reg.deltas(), id).map(|w| (PyValue::Int(id), PyValue::Unicode(w.name)))
            })))
        }
        "MovePersonalAssetsToSafety" => {
            let (Some(actor), Some(system), Some(place)) = (
                req.actor,
                a.int_or_kw(0, "solarSystemID"),
                a.int_or_kw(1, "structureID"),
            ) else {
                return Err(a.bad("MovePersonalAssetsToSafety(solarSystemID, structureID)"));
            };
            if reg.docked_station(actor) == Some(place) {
                return Err(refused(safety::Refusal::Docked));
            }
            let ship = reg.character(actor).map_or(0, |c| c.ship_id);
            safety::wrap_place(reg, actor, system, place, now, &|i| i == ship).map_err(refused)?;
            Ok(PyValue::None)
        }
        "MoveCorpAssetsToSafety" => {
            let (Some(actor), Some(system), Some(place)) = (
                req.actor,
                a.int_or_kw(0, "solarSystemID"),
                a.int_or_kw(1, "structureID"),
            ) else {
                return Err(a.bad("MoveCorpAssetsToSafety(solarSystemID, structureID)"));
            };
            let corp = reg.character(actor).map_or(0, |c| c.corporation_id);
            if !safety::acts_for(reg, actor, corp) || reg.corporations().get(corp).is_none() {
                return Err(refused(safety::Refusal::NotYours));
            }
            safety::wrap_place(reg, corp, system, place, now, &|_| false).map_err(refused)?;
            Ok(PyValue::None)
        }
        "MoveSafetyWrapToStructure" => {
            let (Some(actor), Some(wrap), Some(dest)) = (
                req.actor,
                a.int_or_kw(0, "assetWrapID"),
                a.int_or_kw(2, "destinationID"),
            ) else {
                return Err(
                    a.bad("MoveSafetyWrapToStructure(wrapID, solarSystemID, destinationID)")
                );
            };
            safety::deliver_to(reg, actor, wrap, dest, now).map_err(refused)?;
            Ok(PyValue::None)
        }
        "MoveEjectTimeGM" => {
            let (Some(actor), Some(wrap), Some(d)) = (
                req.actor,
                a.int_or_kw(0, "assetWrapID"),
                a.int_or_kw(1, "days"),
            ) else {
                return Err(a.bad("MoveEjectTimeGM(wrapID, days)"));
            };
            safety::move_eject_time(reg, actor, wrap, d).map_err(refused)?;
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests;
