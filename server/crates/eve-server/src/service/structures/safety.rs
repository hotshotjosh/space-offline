//! `structureAssetSafety` (issue #49): the wire half of
//! [`crate::asset_safety`]. Shapes are **[V]** from the client's consumers
//! (`assetSafetySvc.py`, `assetSafetyEntry.py`, `assetSafetyDeliverWindow.py`):
//!
//! - `GetItemsInSafetyForCharacter()` / `ForCorp()`: a list of dicts (the
//!   Assets window and the corp Accounts tab subscript each), keys
//!   `assetWrapID`, `wrapName`, `solarSystemID`, `ejectTime` (FILETIME),
//!   `daysUntilCanDeliverConst`, `daysUntilAutoMoveConst` (days) and
//!   `nearestNPCStationInfo` (a dict with `itemID` and `typeID`).
//! - `GetStructuresICanDeliverTo(solarSystemID)`: a pair of the destinations
//!   (dicts with `itemID` and `typeID`; the window adds `name`) and the
//!   nearest NPC station (same two keys).
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

fn station_info(reg: &Registry, system: i64) -> PyValue {
    let (id, type_id) =
        safety::nearest_station(reg, system).map_or((0, 0), |r| (r.station_id, r.type_id));
    pydict(vec![
        ("itemID", PyValue::Int(id)),
        ("typeID", PyValue::Int(type_id)),
    ])
}

fn row(reg: &Registry, w: &safety::Wrap) -> PyValue {
    let rules = &reg.config().rules;
    pydict(vec![
        ("assetWrapID", PyValue::Int(w.id)),
        ("wrapName", PyValue::Unicode(w.name.clone())),
        ("solarSystemID", PyValue::Int(w.system)),
        ("ejectTime", PyValue::Long(w.created)),
        // The client divides by this one (`UpdateProgress`): never 0.
        (
            "daysUntilCanDeliverConst",
            days(rules.safety_manual_secs.max(1)),
        ),
        ("daysUntilAutoMoveConst", days(rules.safety_auto_secs)),
        ("nearestNPCStationInfo", station_info(reg, w.system)),
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
            // The client subscripts the station's info and looks the id up:
            // a wrap with no NPC station to name is left off the list.
            .filter(|w| safety::nearest_station(reg, w.system).is_some())
            .map(|w| row(reg, w))
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
                return Err(a.bad("GetStructuresICanDeliverTo(solarSystemID)"));
            };
            let places = safety::destinations(reg, actor, system)
                .into_iter()
                .map(|(id, type_id)| {
                    pydict(vec![
                        ("itemID", PyValue::Int(id)),
                        ("typeID", PyValue::Int(type_id)),
                    ])
                })
                .collect();
            Ok(PyValue::Tuple(vec![
                PyValue::List(places),
                station_info(reg, system),
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
