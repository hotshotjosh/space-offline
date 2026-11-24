//! The Upwell structure directory, profiles, settings and guests (lane U2,
//! issue #62; spec `handback/r-upwell` §3, §7). The model is
//! [`crate::structure`] (the structure) and [`crate::structure_profile`]
//! (profiles, settings, the access rule); this is the wire.
//!
//! Four services: `structureDirectory` (what a pilot can see and dock at),
//! `structureProfiles` (the profile window), `structureSettings` (what a
//! pilot may do at one structure) and `structureGuests` (who is docked).
//! `structureDirectory` also keeps the **war HQ** branch it had before
//! there were structures: `GetValidWarHQs` and `GetStructureInfo` for an HQ
//! item answer from [`super::war`] first.
//!
//! **No session, Destiny or inventory change** lives here: a docked pilot,
//! the ball and the hangar are U3/U4. [`GetGuests`](dispatch) therefore
//! answers from [`Registry::characters_at_station`], which is empty until a
//! pilot can be docked in a structure.
//!
//! Shapes are [V] from the client's consumers unless marked:
//! - `GetStructureInfo(id)`: an attribute object (`.typeID`, `.itemName`,
//!   `.solarSystemID`, `.x/.y/.z`, ...), `None` for a non-structure.
//! - `GetMyCharacterStructures` / `GetMyCorporationStructures`:
//!   `{structureID: dict}` with string keys (`structureEntryController`
//!   wraps each in a `KeyVal` and reads `info['services']`); the corp one
//!   carries the reinforcement keys (`reinforce_weekday`, `reinforce_hour`,
//!   `next_reinforce_weekday`, `next_reinforce_hour`, `next_reinforce_apply`).
//! - `CheckMyDockingAccessToStructures(ids)`: the ids the pilot may dock at
//!   (a list; the callers test `id in result` and `len(result)`), spec §11
//!   left it open and the two consumers
//!   (`parameter_location`, `freelance_projects/job`) settle it.
//! - `SaveProfileSettings(profileID, data)`: `data` is a list of
//!   `(settingID, value, groupID)` tuples (`structureSettingsWnd
//!   .SaveProfile:84`), the group `0` for a boolean setting.
//! - `GetStructureMapData(system)`: a `CRowset` of 12 columns, the header
//!   of `eveCfg.GetLocationsLocalBySystem`, because `mapSvc
//!   .GetSolarsystemItems` does `local.InsertNew(structure)` on each row.
//!
//! **[A]**, each with its falsifier:
//! - Errors a pilot or a profile edit is refused with are `CustomNotify`
//!   (`notify` = the text). Falsified by an empty or garbled dialog.
//! - `services` lists docking and fitting only, online with the hull;
//!   module services are U6. Falsified by the browser hiding a structure
//!   that has none.
//! - `wars` is `[]` and `upkeepState` is Full when unset; `fuelExpires` and
//!   `liquidOzoneQty` are [`crate::structure_fuel`]'s (#42 part b): `None` and
//!   0 for a structure that burns nothing and holds no ozone.
//! - A pilot sees, in the map data and the "all structures" list, the
//!   structures they may dock at; the browser's corp tab shows every
//!   structure of the pilot's own corporation. Falsified by a player not
//!   seeing a structure they can dock at, or seeing one they cannot.
//! - Managing profiles, a structure's description and the reinforce default
//!   needs a director, the CEO or the station-manager role (`sov_hub::
//!   may_launch`, the same rule as launching a hub).
//! - `/upwell spawn` puts the structure at the caller's ship, online, with
//!   the owner's default profile.

use super::args::{Args, as_float, as_int};
use super::*;
use crate::notify::Push;
use crate::structure::{self, Structure};
use crate::structure_profile as sp;
use std::collections::BTreeMap;

pub(crate) const SERVICES: &[&str] = &[
    "structureDirectory",
    "structureProfiles",
    "structureSettings",
    "structureGuests",
    "structureDeployment",
    // U4: docking in a structure, and its hangar's look.
    "structureDocking",
    "structureHangarViewMgr",
    // U6C: taking and releasing the controls of a structure.
    "structureControl",
    // L4 (#47): the reinforcement window a corporation sets.
    "structureVulnerability",
];

mod deploy;
mod dock;
pub(crate) mod safety;
mod vulnerability;

// -- small helpers -----------------------------------------------------------

/// A refusal the player reads (`CustomNotify`) **[A]**.
fn notify(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(msg.to_owned()))]),
    ))
}

/// The refusal of a setting a pilot does not hold: the client's own error
/// for that setting (`SETTING_ACCESS_ERRORS_BY_SETTING`) **[V name, I use]**.
fn access_denied(setting: i64) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(sp::access_error(setting), vec![]))
}

fn refuse(r: sp::Refusal) -> ServiceError {
    notify(match r {
        sp::Refusal::NoSuchProfile => "That structure profile does not exist.",
        sp::Refusal::BadName => "A profile needs a name.",
        sp::Refusal::TooMany => "Your corporation has too many structure profiles.",
        sp::Refusal::IsDefault => {
            "The default profile cannot be deleted. Make another one the default first."
        }
    })
}

/// A dict `{key: value}` with any keys (the wire stores `(value, key)`).
fn dict(items: Vec<(PyValue, PyValue)>) -> PyValue {
    PyValue::Dict(items.into_iter().map(|(k, v)| (v, k)).collect())
}

/// The player corporation the pilot is in.
fn my_corp(reg: &Registry, actor: Option<i64>) -> Option<i64> {
    Some(reg.corporations.membership(actor?)?.corporation_id)
}

/// The corporation `actor` may manage structures for: a director, the CEO
/// or a station manager.
fn manager_corp(reg: &Registry, actor: Option<i64>) -> Result<i64, ServiceError> {
    let corp = my_corp(reg, actor)
        .ok_or_else(|| notify("You need to be in a player corporation to manage structures."))?;
    if !super::sov_hub::may_launch(reg, actor.unwrap_or_default()) {
        return Err(notify(
            "You need the station manager or director role to do that.",
        ));
    }
    Ok(corp)
}

/// Every character that could hear a push about `corp`.
pub(crate) fn corp_audience(reg: &Registry, corp: i64) -> Vec<i64> {
    let online = reg.sessions.online_characters();
    reg.corporations
        .members_of(corp)
        .into_iter()
        .filter(|c| online.contains(c))
        .collect()
}

/// The pilots in `system` right now.
pub(crate) fn system_audience(reg: &Registry, system: i64) -> Vec<i64> {
    reg.sessions
        .online_characters()
        .into_iter()
        .filter(|c| reg.session_system(*c) == Some(system))
        .collect()
}

// -- pushes -----------------------------------------------------------------

/// `OnProfileSettingsChange(profileIDs)`: the browser reloads the profile it
/// has open.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProfileSettingsChange {
    pub to: Vec<i64>,
    pub profiles: Vec<i64>,
}

impl Push for ProfileSettingsChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnProfileSettingsChange",
            vec![PyValue::List(
                self.profiles.iter().map(|p| PyValue::Int(*p)).collect(),
            )],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnCorporationStructuresUpdated()`: the corp tab reloads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CorpStructuresUpdated {
    pub to: Vec<i64>,
}

impl Push for CorpStructuresUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnCorporationStructuresUpdated", vec![])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnDockingAccessChangedForCurrentSolarSystem()`: the dockable list of the
/// system is stale.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DockingAccessChanged {
    pub to: Vec<i64>,
}

impl Push for DockingAccessChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnDockingAccessChangedForCurrentSolarSystem", vec![])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Tell everyone who could notice that `corp`'s structures `changed`
/// (their profile or its settings) changed: the corp, and the pilots in the
/// systems of those structures.
fn announce(reg: &mut Registry, corp: i64, profiles: &[i64], changed: &[Structure]) {
    let to = corp_audience(reg, corp);
    if !profiles.is_empty() {
        reg.push(ProfileSettingsChange {
            to: to.clone(),
            profiles: profiles.to_vec(),
        });
    }
    reg.push(CorpStructuresUpdated { to });
    let mut systems: Vec<i64> = changed.iter().map(|s| s.system).collect();
    systems.sort_unstable();
    systems.dedup();
    let mut audience: Vec<i64> = Vec::new();
    for system in systems {
        for c in system_audience(reg, system) {
            if !audience.contains(&c) {
                audience.push(c);
            }
        }
    }
    if !audience.is_empty() {
        reg.push(DockingAccessChanged { to: audience });
    }
}

/// The corp's structures that sit on `profiles`.
fn on_profiles(reg: &Registry, corp: i64, profiles: &[i64]) -> Vec<Structure> {
    structure::of_corp(reg.deltas(), corp)
        .into_iter()
        .filter(|s| profiles.contains(&s.profile))
        .collect()
}

// -- the structure as the client reads it ------------------------------------

fn services_value(map: &BTreeMap<i64, i64>) -> PyValue {
    dict(
        map.iter()
            .map(|(k, v)| (PyValue::Int(*k), PyValue::Int(*v)))
            .collect(),
    )
}

fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn upkeep_code(s: &Structure) -> i64 {
    s.upkeep.unwrap_or(structure::Upkeep::Full).code()
}

/// `GetStructureInfo`: an attribute object.
fn info_value(reg: &Registry, s: &Structure) -> PyValue {
    keyval(vec![
        ("structureID", PyValue::Int(s.id)),
        ("typeID", PyValue::Int(s.type_id)),
        ("ownerID", PyValue::Int(s.owner_corp)),
        ("solarSystemID", PyValue::Int(s.system)),
        ("itemName", PyValue::Unicode(s.name.clone())),
        ("inSpace", PyValue::Bool(true)),
        ("wars", PyValue::List(vec![])),
        ("upkeepState", PyValue::Int(upkeep_code(s))),
        ("services", services_value(&sp::services_of(reg, s))),
        ("state", PyValue::Int(s.state.code())),
        // #42 part b: `structureInfo.fuelExpires`, as in the directory rows.
        ("fuelExpires", crate::structure_fuel::expires(reg, s)),
        ("x", PyValue::Real(s.pos[0])),
        ("y", PyValue::Real(s.pos[1])),
        ("z", PyValue::Real(s.pos[2])),
    ])
}

/// One entry of `GetMyCharacterStructures` / `GetMyCorporationStructures`:
/// a dict with string keys. `corp` adds the corp tab's keys; they are
/// harmless in the other, and the controller reads both with `.get`.
fn entry_value(reg: &Registry, s: &Structure) -> PyValue {
    let r = &s.reinforce;
    pydict(vec![
        ("structureID", PyValue::Int(s.id)),
        ("typeID", PyValue::Int(s.type_id)),
        ("ownerID", PyValue::Int(s.owner_corp)),
        ("solarSystemID", PyValue::Int(s.system)),
        ("profileID", PyValue::Int(s.profile)),
        ("state", PyValue::Int(s.state.code())),
        ("services", services_value(&sp::services_of(reg, s))),
        ("upkeepState", PyValue::Int(upkeep_code(s))),
        ("wars", PyValue::List(vec![])),
        ("timerEnd", opt_int(s.timer.end)),
        // #42 part b: the client's own expiry arithmetic (`None` when nothing
        // burns), sent with the burn off too.
        ("fuelExpires", crate::structure_fuel::expires(reg, s)),
        ("unanchoring", opt_int(s.unanchoring)),
        (
            "liquidOzoneQty",
            PyValue::Int(crate::structure_fuel::ozone_units(reg, s)),
        ),
        ("reinforce_weekday", opt_int(r.weekday)),
        ("reinforce_hour", opt_int(r.hour)),
        ("next_reinforce_weekday", opt_int(r.next_weekday)),
        ("next_reinforce_hour", opt_int(r.next_hour)),
        ("next_reinforce_apply", opt_int(r.apply_at)),
    ])
}

fn entries(reg: &Registry, list: Vec<Structure>) -> PyValue {
    dict(
        list.iter()
            .map(|s| (PyValue::Int(s.id), entry_value(reg, s)))
            .collect(),
    )
}

/// The structures `ch` may dock at, online or not, in id order.
fn visible_to(reg: &Registry, ch: i64) -> Vec<Structure> {
    structure::all(reg.deltas())
        .into_iter()
        .filter(|s| sp::has_access(reg, s, ch, sp::SETTING_HOUSING_CAN_DOCK))
        .collect()
}

/// The structure market and system lists need: `map` data columns, the
/// header of `GetLocationsLocalBySystem` **[V]**.
fn map_rowset(list: &[Structure], group_of: impl Fn(&Structure) -> i64) -> PyValue {
    let columns: Vec<Column> = [
        ("groupID", DbType::I4),
        ("typeID", DbType::I4),
        ("itemID", DbType::I8),
        ("itemName", DbType::WStr),
        ("locationID", DbType::I4),
        ("orbitID", DbType::I4),
        ("connector", DbType::Bool),
        ("x", DbType::R5),
        ("y", DbType::R5),
        ("z", DbType::R5),
        ("celestialIndex", DbType::I4),
        ("orbitIndex", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the map columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);
    let i4 = |n: i64| DbValue::Int(n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)));
    for s in list {
        rowset
            .push_values(vec![
                i4(group_of(s)),
                i4(s.type_id),
                DbValue::Int(s.id),
                DbValue::Object(PyValue::Unicode(s.name.clone())),
                i4(s.system),
                // The structure orbits nothing: `orbitID` NULL would be
                // truer, but a solar system row of this table always has one.
                i4(s.system),
                DbValue::Bool(false),
                DbValue::Real(s.pos[0]),
                DbValue::Real(s.pos[1]),
                DbValue::Real(s.pos[2]),
                DbValue::Null,
                DbValue::Null,
            ])
            .expect("the map row matches its descriptor");
    }
    rowset.to_pyvalue()
}

// -- dispatch ----------------------------------------------------------------

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "structureDirectory" => directory(reg, req),
        "structureProfiles" => profiles(reg, req),
        "structureSettings" => settings(reg, req),
        "structureGuests" => guests(reg, req),
        "structureDeployment" => deploy::dispatch(reg, req),
        "structureVulnerability" => vulnerability::dispatch(reg, req),
        "structureDocking" | "structureHangarViewMgr" | "structureControl" => {
            dock::dispatch(reg, req)
        }
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

fn directory(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let me = req.actor;
    match req.method {
        // The war HQ stand-ins (C8): the wizard's HQ list, and the info of
        // the one a war names. A real structure is answered below; anything
        // else asked of `GetStructureInfo` (Show Info's owner button,
        // `infoContentBase.GetOwnerIDToShow`) is `None`, which every caller
        // tests for truth.
        "GetValidWarHQs" => super::war::structure_directory(reg, req).unwrap_or(Ok(PyValue::None)),
        "GetStructureInfo" => {
            let Some(id) = Args::new(req).int(0) else {
                return Ok(PyValue::None);
            };
            if let Some(s) = structure::get(reg.deltas(), id) {
                return Ok(info_value(reg, &s));
            }
            super::war::structure_directory(reg, req).unwrap_or(Ok(PyValue::None))
        }
        // An iterable of ids the pilot can dock at in the system.
        "GetMyDockableStructures" => {
            let (Some(me), Some(system)) = (me, Args::new(req).int(0)) else {
                return Ok(PyValue::List(vec![]));
            };
            Ok(PyValue::List(
                structure::in_system(reg.deltas(), system)
                    .iter()
                    .filter(|s| sp::may_dock(reg, s, me))
                    .map(|s| PyValue::Int(s.id))
                    .collect(),
            ))
        }
        "GetStructureMapData" => {
            let (Some(me), Some(system)) = (me, Args::new(req).int(0)) else {
                return Ok(map_rowset(&[], |_| 0));
            };
            let list: Vec<Structure> = visible_to(reg, me)
                .into_iter()
                .filter(|s| s.system == system)
                .collect();
            Ok(map_rowset(&list, |s| {
                crate::structure_profile::group_of(s.type_id)
            }))
        }
        // `{structureID: info}`, iterated with `.iteritems()` by
        // `GetMarketDockableLocationsInRegion` [V]: `None` raised
        // AttributeError and the multibuy window never opened.
        "GetMyCharacterStructures" => Ok(match me {
            Some(me) => entries(reg, visible_to(reg, me)),
            None => PyValue::Dict(vec![]),
        }),
        // `structureBrowserController.GetMyStructures` and
        // `openFunctions:223` call `.get` on it [V].
        "GetMyCorporationStructures" => Ok(match my_corp(reg, me) {
            Some(corp) => entries(reg, structure::of_corp(reg.deltas(), corp)),
            None => PyValue::Dict(vec![]),
        }),
        // The ids from `ids` the pilot may dock at **[V consumers]**.
        "CheckMyDockingAccessToStructures" => {
            let Some(me) = me else {
                return Ok(PyValue::List(vec![]));
            };
            let wanted = args::as_id_or_ids(Args::new(req).get(0));
            Ok(PyValue::List(
                structure::all(reg.deltas())
                    .iter()
                    .filter(|s| wanted.contains(&s.id))
                    .filter(|s| sp::has_access(reg, s, me, sp::SETTING_HOUSING_CAN_DOCK))
                    .map(|s| PyValue::Int(s.id))
                    .collect(),
            ))
        }
        // `structureEntryMyCorp.SetDescription` and the info panel read it;
        // the text is the structure's bio.
        "GetStructureDescription" => {
            let id = Args::new(req).int(0);
            Ok(id
                .and_then(|id| structure::get(reg.deltas(), id))
                .map_or(PyValue::None, |s| PyValue::Unicode(s.bio)))
        }
        "SetStructureDescription" => {
            let corp = manager_corp(reg, me)?;
            let (Some(id), Some(desc)) = (Args::new(req).int(0), Args::new(req).text(1)) else {
                return Err(Args::new(req).bad("(structureID, description)"));
            };
            let Some(mut s) = structure::get(reg.deltas(), id).filter(|s| s.owner_corp == corp)
            else {
                return Err(notify("That is not one of your corporation's structures."));
            };
            s.bio = desc.chars().take(sp::MAX_DESC_CHARS).collect();
            structure::put(reg.deltas_mut(), &s);
            Ok(PyValue::None)
        }
        // The capital "Jump to" menu (`menusvc:2160`) and
        // `capitalnavigation:125` iterate it [V]; and
        // `mapFilterCynosuralFields:26,41` does `set().union()` [V]. The
        // client asks for `GetMyAccessibleOnlineCynoBeaconStructures`
        // (`structureDirectory.py:79`); the shorter name was this arm's
        // spelling before there was a structure to ask about.
        "GetAccessibleOnlineCynoBeaconStructures"
        | "GetMyAccessibleOnlineCynoBeaconStructures"
        | "GetSolarSystemsWithBeacons" => Ok(PyValue::List(vec![])),
        // Jump bridges, as a **3-tuple** -- the arity is the whole
        // requirement, and `None` is an `UNPACK_SEQUENCE` failure.
        // `mapView.LoadAllJumpBridges:1632` unpacks
        // `(jumpBridgesGates, hasAccessTo, hasNoAccessTo)`, and a failure
        // cost the entire map (a loading wheel that never stops).
        // `evemap/map2d/fakeData/fakeDataUtil.GetJumpBridgeData` is CCP's own
        // reference: element 0 is pairs of `KeyVal(solarSystemID,
        // structureID)`, element 1 is tested with `in`, element 2 is never
        // read. Lists, because our marshal writer has no set form.
        // `mapSvc.GetJumpBridgesWithMyAccess` is `@Memoize(5)`, so a wrong
        // answer outlives the call that made it.
        "GetJumpBridgesWithMyAccess" => Ok(crate::jump_bridge::with_my_access(reg, me)),
        // The gate-deployment window's per-system list (ANSI-1, #85).
        "GetNearbyJumpBridges" => Ok(crate::jump_bridge::nearby(reg, me)),
        _ => Err(reg.no_method(req)),
    }
}

fn profiles(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let me = req.actor;
    match req.method {
        // `allProfilesController.BuildProfileDict:30-31` iterates it [V]:
        // `KeyVal(profileID, name, description, isDefault)`.
        "GetProfiles" => {
            let Some(corp) = my_corp(reg, me) else {
                return Ok(PyValue::List(vec![]));
            };
            sp::ensure_default(reg, corp);
            Ok(PyValue::List(
                sp::profiles_of(reg.deltas(), corp)
                    .into_iter()
                    .map(|p| {
                        keyval(vec![
                            ("profileID", PyValue::Int(p.id)),
                            ("name", PyValue::Unicode(p.name)),
                            ("description", PyValue::Unicode(p.desc)),
                            ("isDefault", PyValue::Bool(p.default)),
                        ])
                    })
                    .collect(),
            ))
        }
        // `{settingID: [KeyVal(groupID, value)]}`
        // (`StructureProfileController.LoadGroups`) [V].
        "GetProfileSettings" => {
            let corp = my_corp(reg, me);
            let Some(p) = Args::new(req)
                .int(0)
                .and_then(|id| sp::profile(reg.deltas(), id))
                .filter(|p| Some(p.corp) == corp)
            else {
                return Ok(PyValue::Dict(vec![]));
            };
            Ok(dict(
                sp::settings_by_setting(reg.deltas(), p.id)
                    .into_iter()
                    .map(|(setting, rows)| {
                        (
                            PyValue::Int(setting),
                            PyValue::List(
                                rows.into_iter()
                                    .map(|(group, value)| {
                                        keyval(vec![
                                            ("groupID", PyValue::Int(group)),
                                            ("value", PyValue::Real(value)),
                                        ])
                                    })
                                    .collect(),
                            ),
                        )
                    })
                    .collect(),
            ))
        }
        "SaveProfileSettings" => {
            let corp = manager_corp(reg, me)?;
            let (Some(id), Some(PyValue::List(data) | PyValue::Tuple(data))) =
                (Args::new(req).int(0), Args::new(req).get(1))
            else {
                return Err(Args::new(req).bad("(profileID, [(settingID, value, groupID)])"));
            };
            sp::profile(reg.deltas(), id)
                .filter(|p| p.corp == corp)
                .ok_or(refuse(sp::Refusal::NoSuchProfile))?;
            let rows: Vec<(i64, i64, f64)> = data
                .iter()
                .filter_map(|row| {
                    let (PyValue::Tuple(t) | PyValue::List(t)) = row else {
                        return None;
                    };
                    Some((as_int(t.first())?, as_int(t.get(2))?, as_float(t.get(1))?))
                })
                // The client drops a row whose group it cannot read; so do
                // we, rather than store a door nobody can walk through.
                .filter(|(_, group, _)| {
                    *group == sp::NO_GROUP || crate::owner_groups::group(reg, *group).is_some()
                })
                .collect();
            sp::save_settings(reg.deltas_mut(), id, &rows);
            reg.mark_dirty();
            let changed = on_profiles(reg, corp, &[id]);
            announce(reg, corp, &[id], &changed);
            Ok(PyValue::None)
        }
        "CreateProfile" => {
            let corp = manager_corp(reg, me)?;
            let name = Args::new(req).text(0).unwrap_or_default();
            let desc = Args::new(req).text(1).unwrap_or_default();
            let id = sp::create(reg, corp, &name, &desc).map_err(refuse)?;
            reg.mark_dirty();
            Ok(PyValue::Int(id))
        }
        "DuplicateProfile" => {
            let corp = manager_corp(reg, me)?;
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(profileID,)"));
            };
            let new = sp::duplicate(reg, corp, id).map_err(refuse)?;
            reg.mark_dirty();
            Ok(PyValue::Int(new))
        }
        "UpdateProfile" => {
            let corp = manager_corp(reg, me)?;
            let (Some(id), Some(name)) = (Args::new(req).int(0), Args::new(req).text(1)) else {
                return Err(Args::new(req).bad("(profileID, name, description)"));
            };
            let desc = Args::new(req).text(2).unwrap_or_default();
            sp::update(reg, corp, id, &name, &desc).map_err(refuse)?;
            reg.mark_dirty();
            let to = corp_audience(reg, corp);
            reg.push(ProfileSettingsChange {
                to,
                profiles: vec![id],
            });
            Ok(PyValue::None)
        }
        "DeleteProfile" => {
            let corp = manager_corp(reg, me)?;
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(profileID,)"));
            };
            let moved = sp::delete(reg, corp, id).map_err(refuse)?;
            reg.mark_dirty();
            let changed: Vec<Structure> = moved
                .iter()
                .filter_map(|m| structure::get(reg.deltas(), *m))
                .collect();
            announce(reg, corp, &[id], &changed);
            Ok(PyValue::None)
        }
        "SetDefaultProfile" => {
            let corp = manager_corp(reg, me)?;
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(profileID,)"));
            };
            sp::set_default(reg, corp, id).map_err(refuse)?;
            reg.mark_dirty();
            let to = corp_audience(reg, corp);
            reg.push(CorpStructuresUpdated { to });
            Ok(PyValue::None)
        }
        "ChangeProfiles" => {
            let corp = manager_corp(reg, me)?;
            let (Some(id), wanted) = (
                Args::new(req).int(1),
                args::as_id_or_ids(Args::new(req).get(0)),
            ) else {
                return Err(Args::new(req).bad("([structureID], profileID)"));
            };
            let moved = sp::assign(reg, corp, &wanted, id).map_err(refuse)?;
            reg.mark_dirty();
            let changed: Vec<Structure> = moved
                .iter()
                .filter_map(|m| structure::get(reg.deltas(), *m))
                .collect();
            if !changed.is_empty() {
                announce(reg, corp, &[id], &changed);
            }
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

fn settings(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(Args::new(req).bad("no character"));
    };
    let target = Args::new(req)
        .int(0)
        .and_then(|id| structure::get(reg.deltas(), id));
    match req.method {
        // `bool`s, memoised for two seconds by the client.
        "CharacterHasService" => {
            let service = Args::new(req).int(1);
            Ok(PyValue::Bool(target.zip(service).is_some_and(
                |(s, svc)| sp::services_for(reg, &s, me).contains_key(&svc),
            )))
        }
        "CharacterHasSetting" => {
            let setting = Args::new(req).int(1);
            Ok(PyValue::Bool(
                target
                    .zip(setting)
                    .is_some_and(|(s, set)| sp::has_access(reg, &s, me, set)),
            ))
        }
        // `{serviceID: SERVICE_STATE}` (`structureServices`); an unknown or
        // unreachable structure has no services for this pilot.
        "CharacterGetServices" => Ok(match target {
            Some(s) => services_value(&sp::services_for(reg, &s, me)),
            None => PyValue::Dict(vec![]),
        }),
        // `None` or a raise (`marketMenu`).
        "CharacterCheckService" => {
            let Some(s) = target else {
                return Err(notify("That structure does not exist."));
            };
            let Some(svc) = Args::new(req).int(1) else {
                return Err(Args::new(req).bad("(structureID, serviceID)"));
            };
            if sp::services_for(reg, &s, me).contains_key(&svc) {
                Ok(PyValue::None)
            } else {
                Err(access_denied(sp::setting_of_service(svc).unwrap_or(0)))
            }
        }
        _ => Err(reg.no_method(req)),
    }
}

fn guests(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // `{charID: (corp, alliance, faction)}` (`structureGuests`).
        "GetGuests" => {
            let Some(id) = Args::new(req).int(0) else {
                return Ok(PyValue::Dict(vec![]));
            };
            Ok(dict(
                reg.characters_at_station(id)
                    .into_iter()
                    .map(|c| {
                        (
                            PyValue::Int(c.character_id),
                            PyValue::Tuple(vec![
                                PyValue::Int(c.corporation_id),
                                opt_int(c.alliance_id),
                                PyValue::None,
                            ]),
                        )
                    })
                    .collect(),
            ))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `corpRegistry.GetStructureReinforceDefault()` and
/// `SetStructureReinforceDefault(hour)` (`base_corporation:1110`). `None`
/// for any other method.
pub(crate) fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    match req.method {
        "GetStructureReinforceDefault" => Some(Ok(match my_corp(reg, req.actor) {
            Some(corp) => {
                let (weekday, hour) = sp::reinforce_default(reg.deltas(), corp);
                PyValue::Tuple(vec![PyValue::Int(weekday), PyValue::Int(hour)])
            }
            None => PyValue::Tuple(vec![
                PyValue::Int(structure::DEFAULT_REINFORCE_WEEKDAY),
                PyValue::Int(structure::DEFAULT_REINFORCE_HOUR),
            ]),
        })),
        "SetStructureReinforceDefault" => Some((|| {
            let corp = manager_corp(reg, req.actor)?;
            let Some(hour) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(reinforceHour,)"));
            };
            sp::set_reinforce_default_hour(reg.deltas_mut(), corp, hour).map_err(notify)?;
            reg.mark_dirty();
            Ok(PyValue::None)
        })()),
        _ => None,
    }
}

// -- /upwell -------------------------------------------------------------------

/// The tick environment every caller passes until the upkeep lane (U6)
/// exists: a service module is always fitted, or a small group's home goes
/// abandoned in about a week (spec brief).
pub(crate) fn env(now: i64) -> structure::Env {
    structure::Env {
        has_service_module: true,
        ..structure::Env::new(now)
    }
}

/// A structure type by the name a GM types, or by its id.
fn home_type(word: &str) -> Option<i64> {
    match word.to_ascii_lowercase().as_str() {
        "astrahus" => Some(crate::structure_space::ASTRAHUS),
        "raitaru" => Some(crate::structure_space::RAITARU),
        "athanor" => Some(crate::structure_space::ATHANOR),
        other => other
            .parse::<i64>()
            .ok()
            .filter(|n| structure::size_of_type(*n) != structure::Size::Undefined),
    }
}

/// `/upwell spawn <type> <name...>` and `/upwell list`: a GM puts a
/// finished, online Upwell structure at their ship, owned by their
/// corporation. For tests and for a small group's home before the build chain
/// exists (spec §10.1).
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    const USAGE: &str = "/upwell spawn <astrahus|raitaru|athanor|typeID> <name> | /upwell give <type> | /upwell list | /upwell damage <id> <shield> <armor> <hull> | /upwell state <id> <code|name> | /upwell timer <id> <secs> | /upwell fit <id> <typeID> | /upwell services <id>";
    let Some(actor) = actor else {
        return Ok(bstr("no character"));
    };
    match args.first().copied() {
        Some("list") => {
            let lines: Vec<String> = structure::all(reg.deltas())
                .iter()
                .map(|s| {
                    format!(
                        "{} {} type {} corp {} system {} state {}",
                        s.id,
                        s.name,
                        s.type_id,
                        s.owner_corp,
                        s.system,
                        s.state.code()
                    )
                })
                .collect();
            Ok(bstr(&if lines.is_empty() {
                "no structures".to_owned()
            } else {
                lines.join("\n")
            }))
        }
        // #41: a GM fits one service module and onlines it (skipping the
        // controls and the role), and reads what a structure offers.
        Some("fit") => {
            let (Some(id), Some(type_id), 3) = (
                args.get(1).and_then(|w| w.parse::<i64>().ok()),
                args.get(2).and_then(|w| w.parse::<i64>().ok()),
                args.len(),
            ) else {
                return Ok(bstr(USAGE));
            };
            Ok(bstr(&crate::structure_services::gm_fit(
                reg,
                actor,
                id,
                type_id,
                crate::pyutil::filetime_now(),
            )))
        }
        Some("services") => {
            let Some(id) = args.get(1).and_then(|w| w.parse::<i64>().ok()) else {
                return Ok(bstr(USAGE));
            };
            Ok(bstr(&crate::structure_services::services_text(reg, id)))
        }
        Some("spawn") => {
            let Some(kind) = args.get(1) else {
                return Ok(bstr(USAGE));
            };
            let Some(type_id) = home_type(kind) else {
                return Ok(bstr(&format!("not a structure type: {kind}")));
            };
            let name = args[2..].join(" ");
            if !structure::valid_name(&name) {
                return Ok(bstr("give it a name of at least three characters"));
            }
            let Some(corp) = my_corp(reg, Some(actor)) else {
                return Ok(bstr(
                    "join or found a player corporation first (/devcorp on a dev server)",
                ));
            };
            let Some(system) = reg.session_system(actor) else {
                return Ok(bstr("you are not anywhere"));
            };
            reg.advance_character(actor);
            let pos = reg
                .roster()
                .get(actor)
                .map(|p| p.motion.position)
                .or_else(|| {
                    reg.character(actor)
                        .and_then(|c| c.station_id)
                        .map(|st| reg.station_position(st))
                })
                .unwrap_or([0.0; 3]);
            let profile = sp::ensure_default(reg, corp);
            let now = crate::pyutil::filetime_now();
            let e = env(now);
            let id = reg.store.next_id();
            let mut s =
                Structure::anchored(id, type_id, corp, system, pos, 0.0, &name, profile, &e);
            s.set_deed(true, &e);
            s.set_state(structure::State::ShieldVulnerable, &e);
            for _ in 0..8 {
                s.update(&e);
            }
            s.take_events();
            reg.store.insert(crate::item::Item {
                item_id: id,
                type_id,
                owner_id: corp,
                location_id: system,
                flag_id: 0,
                quantity: 1,
                group_id: sp::group_of(type_id),
                category_id: sp::CATEGORY_STRUCTURE,
                singleton: true,
            });
            structure::put(reg.deltas_mut(), &s);
            reg.mark_dirty();
            // U3: it is a ball in its system from now on.
            crate::structure_space::announce_added(reg, &s);
            announce(reg, corp, &[], &[s]);
            Ok(bstr(&format!("structure {id} \"{name}\" is online")))
        }
        // U3: the deployable itself, into the caller's cargo, to anchor with
        // the client's own Deploy menu (a small group's home before a build
        // chain exists, spec §10.1).
        Some("give") => {
            let Some(kind) = args.get(1) else {
                return Ok(bstr(USAGE));
            };
            let Some(type_id) = home_type(kind) else {
                return Ok(bstr(&format!("not a structure type: {kind}")));
            };
            let ship = reg.character(actor).map(|c| c.ship_id).unwrap_or_default();
            if reg.item(ship).is_none() && reg.character(actor).is_none() {
                return Ok(bstr("no ship to put it in"));
            }
            let to = crate::item::Slot::new(actor, ship, crate::sov_hub::FLAG_CARGO);
            match reg.apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity: 1,
                to,
            }]) {
                Ok(_) => Ok(bstr(&format!(
                    "a structure ({type_id}) is in your cargo; Deploy it from there"
                ))),
                Err(e) => Ok(bstr(&format!(
                    "could not create type {type_id} ({e}); the content bundle may lack the structure types"
                ))),
            }
        }
        // L1 (#44): a GM sets any structure's damage fractions (0..=1); the
        // state machine runs as it would after a volley.
        Some("damage") => {
            let id = args.get(1).and_then(|w| w.parse::<i64>().ok());
            let frac: Vec<f64> = args
                .iter()
                .skip(2)
                .filter_map(|w| w.parse::<f64>().ok())
                .collect();
            let (Some(id), [sh, ar, hu], 5) = (id, frac.as_slice(), args.len()) else {
                return Ok(bstr(USAGE));
            };
            let Some(mut s) = structure::get(reg.deltas(), id) else {
                return Ok(bstr(&format!("no structure {id}")));
            };
            let e = crate::structure_space::env_for(reg, &s, crate::pyutil::filetime_now());
            s.set_damage(*sh, *ar, *hu, &e);
            crate::structure_space::settle(&mut s, &e);
            let events = s.take_events();
            structure::put(reg.deltas_mut(), &s);
            reg.mark_dirty();
            crate::structure_space::react(reg, id, &events);
            let (a, b, c) = s.damage.unwrap_or((1.0, 1.0, 1.0));
            Ok(bstr(&format!(
                "structure {id} is in state {} with shield {a:.3} armor {b:.3} hull {c:.3}",
                s.state.code()
            )))
        }
        // L1 (#44): a GM forces any structure's state.
        Some("state") => {
            let (Some(id), Some(word)) =
                (args.get(1).and_then(|w| w.parse::<i64>().ok()), args.get(2))
            else {
                return Ok(bstr(USAGE));
            };
            let Some(state) = crate::structure_combat::state_by_name(word) else {
                return Ok(bstr(&format!("not a state: {word}")));
            };
            // Unanchored runs `finish_unanchor` at once (the ball and the row
            // go); a real unanchor has its countdown, so the GM does not skip it.
            if matches!(
                state,
                structure::State::Unanchored | structure::State::Unknown
            ) {
                return Ok(bstr(&format!(
                    "refused: {} is not a state to force; unanchor the structure with its \
                     own Unanchor button (the countdown) instead",
                    crate::structure_combat::snake_name(state)
                )));
            }
            let Some(mut s) = structure::get(reg.deltas(), id) else {
                return Ok(bstr(&format!("no structure {id}")));
            };
            let from = s.state;
            let e = crate::structure_space::env_for(reg, &s, crate::pyutil::filetime_now());
            s.set_state(state, &e);
            crate::structure_space::settle(&mut s, &e);
            let events = s.take_events();
            structure::put(reg.deltas_mut(), &s);
            reg.mark_dirty();
            crate::structure_space::react(reg, id, &events);
            Ok(bstr(&format!(
                "structure {id}: {} -> {}",
                crate::structure_combat::snake_name(from),
                crate::structure_combat::snake_name(s.state)
            )))
        }
        // L4 (#47): a GM sets the running timer to end `secs` from now (its
        // start kept, a pause cleared); the next tick moves the state on when
        // it has run out.
        Some("timer") => {
            let (Some(id), Some(secs), 3) = (
                args.get(1).and_then(|w| w.parse::<i64>().ok()),
                args.get(2).and_then(|w| w.parse::<i64>().ok()),
                args.len(),
            ) else {
                return Ok(bstr(USAGE));
            };
            let Some(mut s) = structure::get(reg.deltas(), id) else {
                return Ok(bstr(&format!("no structure {id}")));
            };
            if !s.timer.is_running() {
                return Ok(bstr(&format!(
                    "structure {id} has no running timer (state {})",
                    s.state.code()
                )));
            }
            let now = crate::pyutil::filetime_now();
            s.timer.end = Some(now.saturating_add(secs.saturating_mul(structure::SECOND)));
            s.timer.paused = None;
            let events = vec![structure::Event::TimerChanged];
            structure::put(reg.deltas_mut(), &s);
            reg.mark_dirty();
            crate::structure_space::react_at(reg, id, &events, now);
            Ok(bstr(&format!(
                "structure {id}: the {} timer ends in {secs} s",
                crate::structure_combat::snake_name(s.state)
            )))
        }
        _ => Ok(bstr(USAGE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Character;
    use crate::owner_groups as og;

    // Characters and corporations are invented small numbers.
    const BOSS: i64 = 7;
    const PAL: i64 = 8;
    const FOE: i64 = 9;
    const CORP: i64 = 11;
    const PAL_CORP: i64 = 12;
    const FOE_CORP: i64 = 13;
    const SYSTEM: i64 = 31;

    fn world() -> Registry {
        let ch = |id: i64, corp: i64| Character {
            character_id: id,
            corporation_id: corp,
            solar_system_id: Some(SYSTEM),
            ship_id: 1_000 + id,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(BOSS, CORP), ch(PAL, PAL_CORP), ch(FOE, FOE_CORP)],
            ..SandboxConfig::default()
        });
        // The boss is a director; the others are plain members.
        reg.corporations_mut()
            .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        reg.corporations_mut().enrol_for_test(PAL, PAL_CORP, 0);
        reg.corporations_mut().enrol_for_test(FOE, FOE_CORP, 0);
        reg
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch_as_test(reg, who, service, method, args)
    }

    fn dispatch_as_test(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        };
        match service {
            "corpRegistry" => corp_registry(reg, &req).expect("a structure arm"),
            _ => dispatch(reg, &req),
        }
    }

    fn spawn(reg: &mut Registry) -> i64 {
        let out = slash(reg, Some(BOSS), &["spawn", "astrahus", "Home", "Base"]).unwrap();
        let PyValue::Bytes(b) = out else {
            panic!("{out:?}")
        };
        let msg = String::from_utf8(b).unwrap();
        assert!(msg.contains("online"), "{msg}");
        structure::all(reg.deltas())
            .last()
            .map(|s| s.id)
            .expect("spawned")
    }

    fn attr(v: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal expected, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    fn key(v: &PyValue, name: &str) -> PyValue {
        crate::pyutil::dict_get(v, name.as_bytes())
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing in {v:?}"))
    }

    fn ints(v: PyValue) -> Vec<i64> {
        let PyValue::List(l) = v else { panic!("{v:?}") };
        l.into_iter()
            .map(|i| match i {
                PyValue::Int(n) => n,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn dict_keys(v: &PyValue) -> Vec<i64> {
        let PyValue::Dict(d) = v else { panic!("{v:?}") };
        d.iter()
            .map(|(_, k)| match k {
                PyValue::Int(n) => *n,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn spawn_makes_an_online_structure_with_a_default_profile() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let s = structure::get(reg.deltas(), id).unwrap();
        assert!(s.can_dock(), "{:?}", s.state);
        assert_eq!(
            (s.owner_corp, s.system, s.name.as_str()),
            (CORP, SYSTEM, "Home Base")
        );
        assert!(
            crate::structure_profile::profile(reg.deltas(), s.profile)
                .unwrap()
                .default
        );
        assert!(reg.store().get(id).is_some(), "a real item");
        assert!(id >= crate::item::MIN_PLAYER_ITEM);
    }

    #[test]
    fn a_settled_home_does_not_go_abandoned_with_the_tick_env() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let now = crate::pyutil::filetime_now();
        let later = now + 30 * structure::DAY;
        let out = structure::tick(reg.deltas_mut(), |_| env(later));
        let _ = out;
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_ne!(s.upkeep, Some(structure::Upkeep::Abandoned));
        assert!(s.abandon_at.is_none());
    }

    #[test]
    fn the_damage_and_state_commands_move_a_structure_and_refuse_nonsense() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let say = |reg: &mut Registry, args: &[&str]| {
            let PyValue::Bytes(b) = slash(reg, Some(BOSS), args).unwrap() else {
                panic!()
            };
            String::from_utf8(b).unwrap()
        };
        let sid = id.to_string();
        let out = say(&mut reg, &["damage", &sid, "0.5", "1", "1"]);
        assert!(out.contains("0.500"), "{out}");
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_eq!(s.damage, Some((0.5, 1.0, 1.0)));
        let out = say(&mut reg, &["state", &sid, "hull_vulnerable"]);
        assert!(
            out.contains("shield_vulnerable -> hull_vulnerable"),
            "{out}"
        );
        assert_eq!(
            structure::get(reg.deltas(), id).unwrap().state,
            structure::State::HullVulnerable
        );
        assert!(say(&mut reg, &["damage", &sid, "0.5"]).contains("/upwell"));
        assert!(say(&mut reg, &["damage", "1", "0", "0", "0"]).contains("no structure"));
        assert!(say(&mut reg, &["state", &sid, "nonsense"]).contains("not a state"));
        // Unanchored (and unknown) are refused: the row and its ball stay.
        for word in ["unanchored", "unknown", "1"] {
            let out = say(&mut reg, &["state", &sid, word]);
            assert!(out.contains("refused"), "{word}: {out}");
        }
        assert_eq!(
            structure::get(reg.deltas(), id).unwrap().state,
            structure::State::HullVulnerable,
            "untouched"
        );
    }

    #[test]
    fn the_spawn_command_refuses_nonsense_and_a_pilot_with_no_corp() {
        let mut reg = world();
        let say = |reg: &mut Registry, who, args: &[&str]| {
            let PyValue::Bytes(b) = slash(reg, Some(who), args).unwrap() else {
                panic!()
            };
            String::from_utf8(b).unwrap()
        };
        assert!(say(&mut reg, BOSS, &["spawn"]).contains("/upwell"));
        assert!(say(&mut reg, BOSS, &["spawn", "rifter", "Home"]).contains("not a structure"));
        assert!(say(&mut reg, BOSS, &["spawn", "astrahus", "ab"]).contains("name"));
        assert_eq!(say(&mut reg, BOSS, &["list"]), "no structures");
        assert!(structure::all(reg.deltas()).is_empty());
    }

    #[test]
    fn get_structure_info_has_the_attributes_and_keeps_the_war_hq_branch() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let info = call(
            &mut reg,
            PAL,
            "structureDirectory",
            "GetStructureInfo",
            &[PyValue::Int(id)],
        )
        .unwrap();
        for name in [
            "structureID",
            "typeID",
            "ownerID",
            "solarSystemID",
            "itemName",
            "inSpace",
            "wars",
            "upkeepState",
            "services",
            "state",
            "x",
            "y",
            "z",
        ] {
            let _ = attr(&info, name);
        }
        assert_eq!(attr(&info, "ownerID"), PyValue::Int(CORP));
        assert_eq!(
            attr(&info, "itemName"),
            PyValue::Unicode("Home Base".into())
        );
        // Not a structure: None, as before there were structures.
        for not in [PyValue::Int(5), PyValue::Int(id + 1), PyValue::None] {
            assert_eq!(
                call(
                    &mut reg,
                    PAL,
                    "structureDirectory",
                    "GetStructureInfo",
                    &[not]
                )
                .unwrap(),
                PyValue::None
            );
        }
        // The war HQ of a corporation still answers.
        let hq = crate::war::Hq {
            owner: CORP,
            item_id: crate::war::HQ_ITEM_BASE + 1,
            station_id: 3,
            system_id: SYSTEM,
        };
        reg.deltas_mut().put(
            crate::war::KIND_HQ,
            vec![hq.owner.to_string()],
            vec![
                hq.item_id.to_string(),
                hq.station_id.to_string(),
                hq.system_id.to_string(),
            ],
        );
        let got = call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "GetStructureInfo",
            &[PyValue::Int(hq.item_id)],
        )
        .unwrap();
        assert_eq!(attr(&got, "structureID"), PyValue::Int(hq.item_id));
        // And the structure answers GetValidWarHQs the way it did.
        assert!(matches!(
            call(
                &mut reg,
                BOSS,
                "structureDirectory",
                "GetValidWarHQs",
                &[PyValue::Int(CORP)]
            )
            .unwrap(),
            PyValue::List(_)
        ));
    }

    #[test]
    fn corp_entries_carry_the_browsers_keys_and_services_are_a_dict() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let mine = call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "GetMyCorporationStructures",
            &[],
        )
        .unwrap();
        assert_eq!(dict_keys(&mine), vec![id]);
        let PyValue::Dict(d) = &mine else { panic!() };
        let entry = &d[0].0;
        for name in [
            "structureID",
            "typeID",
            "ownerID",
            "solarSystemID",
            "profileID",
            "state",
            "services",
            "upkeepState",
            "timerEnd",
            "fuelExpires",
            "unanchoring",
            "liquidOzoneQty",
            "reinforce_weekday",
            "reinforce_hour",
            "next_reinforce_weekday",
            "next_reinforce_hour",
            "next_reinforce_apply",
            "wars",
        ] {
            let _ = key(entry, name);
        }
        // A plain dict, not a KeyVal: the controller subscripts `services`.
        assert!(matches!(key(entry, "services"), PyValue::Dict(_)));
        assert_eq!(
            key(entry, "reinforce_hour"),
            PyValue::Int(structure::DEFAULT_REINFORCE_HOUR)
        );
        // Another corporation sees none of it in its own tab.
        let theirs = call(
            &mut reg,
            PAL,
            "structureDirectory",
            "GetMyCorporationStructures",
            &[],
        )
        .unwrap();
        assert_eq!(dict_keys(&theirs), Vec::<i64>::new());
    }

    /// The access matrix: owner, a friend let in by a profile group, a
    /// stranger, and an excluded member, over the four calls that carry it.
    #[test]
    fn the_access_matrix_runs_through_every_call() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let profile = structure::get(reg.deltas(), id).unwrap().profile;
        let ask = |reg: &mut Registry, who: i64| {
            (
                ints(
                    call(
                        reg,
                        who,
                        "structureDirectory",
                        "GetMyDockableStructures",
                        &[PyValue::Int(SYSTEM)],
                    )
                    .unwrap(),
                ),
                dict_keys(
                    &call(
                        reg,
                        who,
                        "structureDirectory",
                        "GetMyCharacterStructures",
                        &[],
                    )
                    .unwrap(),
                ),
                ints(
                    call(
                        reg,
                        who,
                        "structureDirectory",
                        "CheckMyDockingAccessToStructures",
                        &[PyValue::List(vec![PyValue::Int(id), PyValue::Int(id + 7)])],
                    )
                    .unwrap(),
                ),
                call(
                    reg,
                    who,
                    "structureSettings",
                    "CharacterHasSetting",
                    &[
                        PyValue::Int(id),
                        PyValue::Int(crate::structure_profile::SETTING_HOUSING_CAN_DOCK),
                    ],
                )
                .unwrap(),
            )
        };
        let yes = (vec![id], vec![id], vec![id], PyValue::Bool(true));
        let no = (vec![], vec![], vec![], PyValue::Bool(false));
        assert_eq!(ask(&mut reg, BOSS), yes);
        assert_eq!(ask(&mut reg, PAL), no);
        assert_eq!(ask(&mut reg, FOE), no);

        // A group of the pal's corporation, with the foe excluded.
        let g = og::create_for(
            &mut reg,
            CORP,
            "friends",
            "",
            &[(PAL_CORP, og::MEMBER), (FOE, og::EXCLUDED)],
        )
        .group_id;
        let data = PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(crate::structure_profile::SETTING_HOUSING_CAN_DOCK),
            PyValue::Real(0.0),
            PyValue::Int(g),
        ])]);
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "SaveProfileSettings",
            &[PyValue::Int(profile), data],
        )
        .unwrap();
        assert_eq!(ask(&mut reg, PAL), yes);
        assert_eq!(ask(&mut reg, FOE), no);
        assert_eq!(ask(&mut reg, BOSS), yes, "the owner keeps the door");
    }

    #[test]
    fn an_offline_structure_is_listed_but_not_dockable() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.state = structure::State::AnchorVulnerable;
        structure::put(reg.deltas_mut(), &s);
        let dock = call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "GetMyDockableStructures",
            &[PyValue::Int(SYSTEM)],
        )
        .unwrap();
        assert_eq!(ints(dock), Vec::<i64>::new());
        let all = call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "GetMyCharacterStructures",
            &[],
        )
        .unwrap();
        assert_eq!(dict_keys(&all), vec![id]);
        let info = call(
            &mut reg,
            BOSS,
            "structureSettings",
            "CharacterGetServices",
            &[PyValue::Int(id)],
        )
        .unwrap();
        assert_eq!(info, PyValue::Dict(vec![]));
    }

    #[test]
    fn profiles_over_the_wire_from_the_window() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let list = call(&mut reg, BOSS, "structureProfiles", "GetProfiles", &[]).unwrap();
        let PyValue::List(l) = &list else { panic!() };
        assert_eq!(l.len(), 1);
        for name in ["profileID", "name", "description", "isDefault"] {
            let _ = attr(&l[0], name);
        }
        assert_eq!(attr(&l[0], "isDefault"), PyValue::Bool(true));
        let default = as_int(Some(&attr(&l[0], "profileID"))).unwrap();
        // Its settings: `{settingID: [KeyVal(groupID, value)]}`.
        let s = call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "GetProfileSettings",
            &[PyValue::Int(default)],
        )
        .unwrap();
        let PyValue::Dict(d) = &s else { panic!() };
        assert_eq!(d.len(), 1);
        let PyValue::List(groups) = &d[0].0 else {
            panic!()
        };
        let _ = attr(&groups[0], "groupID");
        let _ = attr(&groups[0], "value");

        // Create, duplicate, update, assign, default, delete.
        let a = as_int(Some(
            &call(
                &mut reg,
                BOSS,
                "structureProfiles",
                "CreateProfile",
                &[
                    PyValue::Unicode("Allies".into()),
                    PyValue::Unicode("d".into()),
                ],
            )
            .unwrap(),
        ))
        .unwrap();
        let b = as_int(Some(
            &call(
                &mut reg,
                BOSS,
                "structureProfiles",
                "DuplicateProfile",
                &[PyValue::Int(default)],
            )
            .unwrap(),
        ))
        .unwrap();
        assert!(a != b && a != default);
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "UpdateProfile",
            &[
                PyValue::Int(a),
                PyValue::Unicode("Friends".into()),
                PyValue::Unicode(String::new()),
            ],
        )
        .unwrap();
        let _ = reg.take_pending();
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "ChangeProfiles",
            &[PyValue::List(vec![PyValue::Int(id)]), PyValue::Int(a)],
        )
        .unwrap();
        assert_eq!(structure::get(reg.deltas(), id).unwrap().profile, a);
        let names: Vec<&str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert!(names.contains(&"OnProfileSettingsChange"), "{names:?}");
        assert!(
            names.contains(&"OnCorporationStructuresUpdated"),
            "{names:?}"
        );
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "SetDefaultProfile",
            &[PyValue::Int(a)],
        )
        .unwrap();
        assert!(
            call(
                &mut reg,
                BOSS,
                "structureProfiles",
                "DeleteProfile",
                &[PyValue::Int(a)]
            )
            .is_err(),
            "the default stays"
        );
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "DeleteProfile",
            &[PyValue::Int(default)],
        )
        .unwrap();
        let after = call(&mut reg, BOSS, "structureProfiles", "GetProfiles", &[]).unwrap();
        let PyValue::List(l) = after else { panic!() };
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn only_a_manager_edits_profiles_and_never_another_corps() {
        let mut reg = world();
        let _ = spawn(&mut reg);
        let list = call(&mut reg, BOSS, "structureProfiles", "GetProfiles", &[]).unwrap();
        let PyValue::List(l) = &list else { panic!() };
        let mine = as_int(Some(&attr(&l[0], "profileID"))).unwrap();
        // A plain member may read the list but not write.
        assert!(
            call(
                &mut reg,
                PAL,
                "structureProfiles",
                "CreateProfile",
                &[
                    PyValue::Unicode("x".into()),
                    PyValue::Unicode(String::new())
                ]
            )
            .is_err()
        );
        // The foe's own director cannot touch the boss's profile.
        reg.corporations_mut()
            .enrol_for_test(FOE, FOE_CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        assert!(
            call(
                &mut reg,
                FOE,
                "structureProfiles",
                "DeleteProfile",
                &[PyValue::Int(mine)]
            )
            .is_err()
        );
        assert!(
            call(
                &mut reg,
                FOE,
                "structureProfiles",
                "SaveProfileSettings",
                &[PyValue::Int(mine), PyValue::List(vec![])]
            )
            .is_err()
        );
        // Reading it is empty for the foe.
        assert_eq!(
            call(
                &mut reg,
                FOE,
                "structureProfiles",
                "GetProfileSettings",
                &[PyValue::Int(mine)]
            )
            .unwrap(),
            PyValue::Dict(vec![])
        );
        // Nothing of the boss's changed.
        assert_eq!(
            crate::structure_profile::settings(reg.deltas(), mine).len(),
            1
        );
    }

    #[test]
    fn save_drops_a_group_that_does_not_exist() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let profile = structure::get(reg.deltas(), id).unwrap().profile;
        let data = PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(crate::structure_profile::SETTING_HOUSING_CAN_DOCK),
            PyValue::Int(0),
            PyValue::Int(777),
        ])]);
        call(
            &mut reg,
            BOSS,
            "structureProfiles",
            "SaveProfileSettings",
            &[PyValue::Int(profile), data],
        )
        .unwrap();
        assert!(crate::structure_profile::settings(reg.deltas(), profile).is_empty());
    }

    #[test]
    fn settings_services_and_checks() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let svc = call(
            &mut reg,
            BOSS,
            "structureSettings",
            "CharacterGetServices",
            &[PyValue::Int(id)],
        )
        .unwrap();
        assert_eq!(
            dict_keys(&svc),
            vec![
                crate::structure_profile::SERVICE_DOCKING,
                crate::structure_profile::SERVICE_FITTING,
                crate::structure_profile::SERVICE_MEDICAL,
                crate::structure_profile::SERVICE_REPAIR,
                crate::structure_profile::SERVICE_INSURANCE,
                crate::structure_profile::SERVICE_JUMP_CLONE
            ]
        );
        assert_eq!(
            call(
                &mut reg,
                BOSS,
                "structureSettings",
                "CharacterHasService",
                &[
                    PyValue::Int(id),
                    PyValue::Int(crate::structure_profile::SERVICE_DOCKING)
                ]
            )
            .unwrap(),
            PyValue::Bool(true)
        );
        assert_eq!(
            call(
                &mut reg,
                PAL,
                "structureSettings",
                "CharacterHasService",
                &[
                    PyValue::Int(id),
                    PyValue::Int(crate::structure_profile::SERVICE_DOCKING)
                ]
            )
            .unwrap(),
            PyValue::Bool(false)
        );
        assert_eq!(
            call(
                &mut reg,
                BOSS,
                "structureSettings",
                "CharacterCheckService",
                &[
                    PyValue::Int(id),
                    PyValue::Int(crate::structure_profile::SERVICE_DOCKING)
                ]
            )
            .unwrap(),
            PyValue::None
        );
        assert!(
            call(
                &mut reg,
                PAL,
                "structureSettings",
                "CharacterCheckService",
                &[
                    PyValue::Int(id),
                    PyValue::Int(crate::structure_profile::SERVICE_DOCKING)
                ]
            )
            .is_err()
        );
        // An unknown structure has nothing.
        assert_eq!(
            call(
                &mut reg,
                BOSS,
                "structureSettings",
                "CharacterGetServices",
                &[PyValue::Int(5)]
            )
            .unwrap(),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn guests_are_a_dict_and_empty_until_someone_docks() {
        let mut reg = world();
        let id = spawn(&mut reg);
        assert_eq!(
            call(
                &mut reg,
                BOSS,
                "structureGuests",
                "GetGuests",
                &[PyValue::Int(id)]
            )
            .unwrap(),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn the_map_data_is_a_rowset_of_the_clients_columns() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let rs = call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "GetStructureMapData",
            &[PyValue::Int(SYSTEM)],
        )
        .unwrap();
        // `CRowset(descriptor, rows)`: one row for the owner, none for a stranger.
        let rows = |v: &PyValue| -> usize {
            let PyValue::ObjectEx { header, .. } = v else {
                panic!("{v:?}")
            };
            let PyValue::Tuple(h) = &**header else {
                panic!()
            };
            let PyValue::Tuple(args) = &h[1] else {
                panic!()
            };
            let PyValue::List(rows) = &args[1] else {
                panic!()
            };
            rows.len()
        };
        assert_eq!(rows(&rs), 1);
        assert!(format!("{rs:?}").contains(&format!("Int({id})")));
        let other = call(
            &mut reg,
            FOE,
            "structureDirectory",
            "GetStructureMapData",
            &[PyValue::Int(SYSTEM)],
        )
        .unwrap();
        assert_eq!(rows(&other), 0, "a stranger sees no row");
    }

    #[test]
    fn reinforce_default_round_trips_and_needs_a_manager() {
        let mut reg = world();
        let got = call(
            &mut reg,
            BOSS,
            "corpRegistry",
            "GetStructureReinforceDefault",
            &[],
        )
        .unwrap();
        assert_eq!(
            got,
            PyValue::Tuple(vec![
                PyValue::Int(structure::DEFAULT_REINFORCE_WEEKDAY),
                PyValue::Int(structure::DEFAULT_REINFORCE_HOUR)
            ])
        );
        call(
            &mut reg,
            BOSS,
            "corpRegistry",
            "SetStructureReinforceDefault",
            &[PyValue::Int(3)],
        )
        .unwrap();
        let got = call(
            &mut reg,
            BOSS,
            "corpRegistry",
            "GetStructureReinforceDefault",
            &[],
        )
        .unwrap();
        assert_eq!(
            got,
            PyValue::Tuple(vec![
                PyValue::Int(structure::DEFAULT_REINFORCE_WEEKDAY),
                PyValue::Int(3)
            ])
        );
        assert!(
            call(
                &mut reg,
                PAL,
                "corpRegistry",
                "SetStructureReinforceDefault",
                &[PyValue::Int(3)]
            )
            .is_err()
        );
        assert!(
            call(
                &mut reg,
                BOSS,
                "corpRegistry",
                "SetStructureReinforceDefault",
                &[PyValue::Int(99)]
            )
            .is_err()
        );
    }

    #[test]
    fn description_is_read_by_anyone_and_set_by_a_manager() {
        let mut reg = world();
        let id = spawn(&mut reg);
        call(
            &mut reg,
            BOSS,
            "structureDirectory",
            "SetStructureDescription",
            &[PyValue::Int(id), PyValue::Unicode("our home".into())],
        )
        .unwrap();
        assert_eq!(
            call(
                &mut reg,
                FOE,
                "structureDirectory",
                "GetStructureDescription",
                &[PyValue::Int(id)]
            )
            .unwrap(),
            PyValue::Unicode("our home".into())
        );
        assert!(
            call(
                &mut reg,
                FOE,
                "structureDirectory",
                "SetStructureDescription",
                &[PyValue::Int(id), PyValue::Unicode("mine".into())]
            )
            .is_err()
        );
    }

    #[test]
    fn the_stubs_that_were_here_still_answer() {
        let mut reg = world();
        for m in [
            "GetAccessibleOnlineCynoBeaconStructures",
            "GetMyAccessibleOnlineCynoBeaconStructures",
            "GetSolarSystemsWithBeacons",
        ] {
            assert_eq!(
                call(&mut reg, BOSS, "structureDirectory", m, &[]).unwrap(),
                PyValue::List(vec![]),
                "{m}"
            );
        }
        assert!(matches!(
            call(&mut reg, BOSS, "structureDirectory", "GetJumpBridgesWithMyAccess", &[]).unwrap(),
            PyValue::Tuple(t) if t.len() == 3
        ));
        assert_eq!(
            call(
                &mut reg,
                BOSS,
                "structureDirectory",
                "GetMyCharacterStructures",
                &[]
            )
            .unwrap(),
            PyValue::Dict(vec![])
        );
        // A pilot in no player corporation sees an empty profile list.
        let mut solo = Registry::new(SandboxConfig::default());
        let who = solo.config().characters[0].character_id;
        assert_eq!(
            call(&mut solo, who, "structureProfiles", "GetProfiles", &[]).unwrap(),
            PyValue::List(vec![])
        );
        assert!(matches!(
            call(&mut reg, BOSS, "structureDirectory", "NoSuchThing", &[]),
            Err(ServiceError::NoSuchMethod { .. })
        ));
    }

    #[test]
    fn every_service_here_is_routed() {
        for s in SERVICES {
            let mut reg = world();
            let r = dispatch(
                &mut reg,
                &Request {
                    service: s,
                    method: "NoSuchMethodHere",
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(BOSS),
                },
            );
            assert!(
                matches!(r, Err(ServiceError::NoSuchMethod { .. })),
                "{s}: {r:?}"
            );
        }
    }

    /// `CheckMyDockingAccessToStructures` reads a bare id, a list or a tuple,
    /// and skips a `Bool` entry (pins the shared id reader).
    #[test]
    fn docking_access_reads_a_bare_id_a_list_and_a_tuple() {
        let mut reg = world();
        let id = spawn(&mut reg);
        let mut ask = |arg: PyValue| {
            ints(
                call(
                    &mut reg,
                    BOSS,
                    "structureDirectory",
                    "CheckMyDockingAccessToStructures",
                    &[arg],
                )
                .unwrap(),
            )
        };
        assert_eq!(ask(PyValue::Int(id)), vec![id]);
        assert_eq!(ask(PyValue::Tuple(vec![PyValue::Int(id)])), vec![id]);
        assert_eq!(
            ask(PyValue::List(vec![PyValue::Bool(true), PyValue::Int(id)])),
            vec![id]
        );
        assert_eq!(ask(PyValue::Real(id as f64)), Vec::<i64>::new());
    }
}
