//! **Service modules in an Upwell structure** (#41, lane UPWELL-FIT): fit,
//! online, offline and unfit at the controls, and the services, the upkeep
//! and the hit points that follow from what is online. The wire arms are
//! `service/inventory.rs` (`invbroker` `Add`/`MultiAdd` on the structure's
//! inventory, `dogmaIM` `SetModuleOnline`/`TakeModuleOffline` and
//! `ItemGetInfo`) and `service/structures.rs` (`structureSettings`, the
//! directory and `/upwell fit|services`); the controls themselves are
//! [`crate::structure_dock::control`]'s.
//!
//! # State
//!
//! - **Fitted** is an item in the store: `location == structure`, `flag` in
//!   164..=171 (the client's `flagServiceSlot0..7`), owned by the structure's
//!   corporation, a singleton **[V]** (`structureControl.CheckCanDisableServiceModule`
//!   wants `session.corpid == module.ownerID`, so a fitted module is
//!   corp-owned).
//! - **Online** is a delta row, kind [`KIND`] (`delta_kinds::KINDS`):
//!   `structure module -> "1"`. A row whose module is no longer fitted
//!   there is ignored by every reader and purged on the next write for that
//!   structure. [`forget`] drops a structure's rows when it is gone.
//! - Nothing here is held in memory: a restart reads both back.
//!
//! # What the client asks, in order **[V]** (`clientDogmaLocation`,
//! `structureControl`)
//!
//! 1. the pilot takes the controls (`session.shipid == structureid`);
//! 2. the client's own `TryFit` is `Add(itemID, source, qty=1, flag=slot)` on
//!    the structure's inventory: [`fit`], which **never onlines**;
//! 3. the client onlines it itself, locally first, and then calls
//!    `SetModuleOnline(structureID, moduleID)`: [`set_online`]. On any
//!    `UserError` but `EffectAlreadyActive2` it stops its own effect, so a
//!    refusal here is the truth the client shows;
//! 4. it unfits with `Add(moduleID, structureID, qty=None, flag=4)`:
//!    [`unfit`].
//!
//! # The two world switches (`/world freeclone`, `/world freeupkeep`)
//!
//! Both default **on**, which is what the server did before there were modules
//! (every online structure had a clone bay and full power). Off, the clone bay
//! needs an online Standup Cloning Center and a structure with no online
//! module goes to low power ([`has_online_module`], `structure_space::env_for`).
//!
//! # What is not here
//!
//! The quantum core is #42 part a's ([`crate::structure_bays`]). The fuel is
//! part b's ([`crate::structure_fuel`]): [`pay_online`] charges the online
//! cost there with `/world fuelburn` on and is free with it off, and the
//! hourly burn takes a module offline through [`force_offline`].
//!
//! # [A]s (falsifier in each)
//!
//! - **A1** the fit refusals are checked in this order: the controls, the
//!   role, the item, the module kind, the hull, the slot, the per-type cap.
//!   Falsifier: a TQ refusal for a doubly wrong module shows the other key.
//! - **A2** a module is fitted from the pilot's own hangar in the structure
//!   (flag 4) only. Falsifier: TQ fits from a corporation division.
//! - **A4** an unfit module goes to the pilot's hangar, owned by the pilot.
//!   Falsifier: TQ keeps it corporation-owned.
//! - **A6** the CPU and powergrid of online modules are held to the hull's
//!   48 and 11 as a ship's are. Falsifier: TQ onlines over budget.
//! - **A7** a service whose feature this server does not have yet (market,
//!   jump bridge, cynosural, loyalty store...) is refused online
//!   ([`supported`]). Falsifier: the owner wants them online anyway (D5).
//! - **A8** `JUMP_CLONE` comes with `MEDICAL` (the client maps no module to
//!   it, F17). Falsifier: a TQ clone bay without jump clones.
//! - **A9** `OFFICES` is not a hull service yet. Falsifier: a TQ lobby
//!   difference.
//! - **A20** a module onlines only in an operational structure (not offline,
//!   not disabled: state 102 and the hull reinforce are out). Falsifier: a TQ
//!   module onlined in a coreless structure.
//! - **A21** a damaged hull may online modules. Falsifier: a TQ refusal while
//!   damaged.
//! - **A22** a module carrying attribute 1970 as 1 cannot be onlined in
//!   high security space. Falsifier: a TQ reactor onlined in high-sec; also
//!   the attribute's meaning, which is [I].
//! - [`hp_multiplier`]: the highest 2744 over the online modules; with none
//!   online and `freeupkeep` on, the old [`FULL_POWER_HP_MULTIPLIER`]
//!   (today's 1:1); with none and `freeupkeep` off, the hull's own 2743, else
//!   1.0 **[I]**. Falsifier: the client's Show Info HP disagreeing with our
//!   bars after a known volley.
//! - A control or role refusal is `CrpAccessDenied` with a text of ours for
//!   `reason`, and a missing control a `CustomNotify`. Falsifier: CCP's text.

use crate::item::{Edit, Item, Slot};
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use crate::structure::{self, Structure};
use crate::structure_combat::{ATTR_HULL_HP_MULTIPLIER, ATTR_SERVICE_HP_MULTIPLIER};
use crate::structure_profile::{self as sp, *};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

pub use crate::structure_combat::FULL_POWER_HP_MULTIPLIER;

/// The delta kind of online modules: `structure module -> "1"`.
pub const KIND: &str = "strsvc";

/// `flagServiceSlot0` and `flagServiceSlot7` **[V]**
/// (`inventorycommon/const.py:198-205`).
pub const FIRST_SLOT: i64 = 164;
pub const LAST_SLOT: i64 = 171;

/// `categoryStructureModule` **[V]**: a Standup service module's category.
pub const CATEGORY_STRUCTURE_MODULE: i64 = 66;
/// `effectServiceSlot` **[V]** (F4): the effect a service module fits by.
pub const EFFECT_SERVICE_SLOT: i64 = 6306;
/// `serviceSlots`, the hull's count of service slots **[V]** (F4).
pub const ATTR_SERVICE_SLOTS: i64 = 2056;
/// `cpuOutput` and `powerOutput` of a hull, `cpu` and `power` of a module.
pub const ATTR_CPU_OUTPUT: i64 = crate::dogma::attr::CPU_OUTPUT;
pub const ATTR_POWER_OUTPUT: i64 = crate::dogma::attr::POWER_OUTPUT;
pub const ATTR_CPU: i64 = crate::dogma::attr::CPU;
pub const ATTR_POWER: i64 = crate::dogma::attr::POWER;
/// **[I]** attribute 1970 on a module: the module cannot be online in high
/// security space (the reactor class). A22.
pub const ATTR_HIGH_SEC_BAN: i64 = 1970;
/// The hull attributes the client's fitting window and structure UI read and
/// that [`hull_attributes`] adds when the hull row has them **[V]** (F15,
/// F13, F4): `powerOutput` 11, `cpuOutput` 48, 1132 `upgradeCapacity`
/// (calibration), 1137 `rigSlots`, 1547 `rigSize`, 2056 `serviceSlots`,
/// 2339 the fuel-use bonus, 3101 the core type.
pub const HULL_ATTRIBUTES: [i64; 8] = [11, 48, 1132, 1137, 1547, 2056, 2339, 3101];

// -- the client's module table (services.py `_MODULES_TO_SERVICES`) ------------

/// `typeStandupMarketHub` **[V]** (`inventorycommon/const.py:1758-1778`).
pub const TYPE_STANDUP_MARKET_HUB: i64 = 35_892;
pub const TYPE_STANDUP_CLONING_CENTER: i64 = 35_894;
pub const TYPE_STANDUP_REPROCESSING_FACILITY: i64 = 35_899;
pub const TYPE_STANDUP_RESEARCH_LAB: i64 = 35_891;
pub const TYPE_STANDUP_INVENTION_LAB: i64 = 35_886;
pub const TYPE_STANDUP_MANUFACTURING_PLANT: i64 = 35_878;
pub const TYPE_STANDUP_CAPITAL_SHIPYARD: i64 = 35_881;
pub const TYPE_STANDUP_SUPERCAPITAL_SHIPYARD: i64 = 35_877;
pub const TYPE_STANDUP_HYASYODA_RESEARCH_LAB: i64 = 45_550;
pub const TYPE_STANDUP_HYBRID_REACTOR: i64 = 45_538;
pub const TYPE_STANDUP_COMPOSITE_REACTOR: i64 = 45_537;
pub const TYPE_STANDUP_BIOCHEMICAL_REACTOR: i64 = 45_539;
pub const TYPE_STANDUP_MOON_DRILL: i64 = 45_009;
pub const TYPE_STANDUP_STARGATE_ENGINE: i64 = 35_913;
pub const TYPE_STANDUP_CYNOSURAL_GENERATOR: i64 = 35_912;
pub const TYPE_STANDUP_CYNOSURAL_JAMMER: i64 = 35_914;
pub const TYPE_STANDUP_LOYALTY_STORE: i64 = 78_330;
pub const TYPE_STANDUP_METENOX_MOON_DRILL: i64 = 82_941;

/// `typeUpwellSmallStargate`, `typeUpwellCynosuralSystemJammer`,
/// `typeUpwellCynosuralBeacon` and `typeUpwellAutoMoonMiner` **[V]**
/// (`const.py:1828-1831`): the hulls in the client's
/// `STRUCTURES_WITHOUT_ONLINE_SERVICES` (`services.py:73`).
pub const TYPE_UPWELL_SMALL_STARGATE: i64 = 35_841;
pub const TYPE_UPWELL_CYNOSURAL_JAMMER: i64 = 37_534;
pub const TYPE_UPWELL_CYNOSURAL_BEACON: i64 = 35_840;
pub const TYPE_UPWELL_AUTO_MOON_MINER: i64 = 81_826;
pub const WITHOUT_ONLINE_SERVICES: [i64; 4] = [
    TYPE_UPWELL_SMALL_STARGATE,
    TYPE_UPWELL_CYNOSURAL_JAMMER,
    TYPE_UPWELL_CYNOSURAL_BEACON,
    TYPE_UPWELL_AUTO_MOON_MINER,
];

/// A module type to the services it gives: the shape both the client's table
/// and a test's invented one take.
pub type ServiceTable<'a> = &'a [(i64, &'a [i64])];

/// The client's `_MODULES_TO_SERVICES`, 18 rows **[V]**
/// (`structures/services.py:231-248`, `GetServiceIDsFromModuleType`). The
/// Cloning Center gives `MEDICAL` only; nothing maps a module to
/// `JUMP_CLONE` (F17).
pub const MODULE_SERVICES: ServiceTable<'static> = &[
    (TYPE_STANDUP_MARKET_HUB, &[SERVICE_MARKET]),
    (TYPE_STANDUP_CLONING_CENTER, &[SERVICE_MEDICAL]),
    (TYPE_STANDUP_REPROCESSING_FACILITY, &[SERVICE_REPROCESSING]),
    (
        TYPE_STANDUP_RESEARCH_LAB,
        &[
            SERVICE_LABORATORY_COPYING,
            SERVICE_LABORATORY_RESEARCH_MATERIAL,
            SERVICE_LABORATORY_RESEARCH_TIME,
        ],
    ),
    (TYPE_STANDUP_INVENTION_LAB, &[SERVICE_LABORATORY_INVENTION]),
    (
        TYPE_STANDUP_MANUFACTURING_PLANT,
        &[SERVICE_MANUFACTURING_BASIC],
    ),
    (
        TYPE_STANDUP_CAPITAL_SHIPYARD,
        &[SERVICE_MANUFACTURING_CAPITAL],
    ),
    (
        TYPE_STANDUP_SUPERCAPITAL_SHIPYARD,
        &[
            SERVICE_MANUFACTURING_SUPERCAPITAL,
            SERVICE_MANUFACTURING_CAPITAL,
        ],
    ),
    (
        TYPE_STANDUP_HYASYODA_RESEARCH_LAB,
        &[
            SERVICE_LABORATORY_COPYING,
            SERVICE_LABORATORY_RESEARCH_MATERIAL,
            SERVICE_LABORATORY_RESEARCH_TIME,
        ],
    ),
    (TYPE_STANDUP_HYBRID_REACTOR, &[SERVICE_REACTIONS_HYBRID]),
    (
        TYPE_STANDUP_COMPOSITE_REACTOR,
        &[SERVICE_REACTIONS_COMPOSITE],
    ),
    (
        TYPE_STANDUP_BIOCHEMICAL_REACTOR,
        &[SERVICE_REACTIONS_BIOCHEMICAL],
    ),
    (TYPE_STANDUP_MOON_DRILL, &[SERVICE_MOONMINING]),
    (TYPE_STANDUP_STARGATE_ENGINE, &[SERVICE_JUMP_BRIDGE]),
    (TYPE_STANDUP_CYNOSURAL_GENERATOR, &[SERVICE_CYNO_BEACON]),
    (TYPE_STANDUP_CYNOSURAL_JAMMER, &[SERVICE_CYNO_JAMMER]),
    (TYPE_STANDUP_LOYALTY_STORE, &[SERVICE_LOYALTY_STORE]),
    (TYPE_STANDUP_METENOX_MOON_DRILL, &[SERVICE_AUTOMOONMINING]),
];

/// The services a module of `type_id` gives under `table`.
pub fn module_services<'a>(table: ServiceTable<'a>, type_id: i64) -> &'a [i64] {
    table
        .iter()
        .find(|(t, _)| *t == type_id)
        .map_or(&[], |(_, s)| *s)
}

/// `structures.INDUSTRY_SERVICES` **[V]** (`services.py:47`): the meta
/// services and the manufacturing, laboratory and reaction ones.
pub const INDUSTRY_SERVICES: [i64; 14] = [
    SERVICE_INDUSTRY,
    SERVICE_MANUFACTURING,
    SERVICE_MANUFACTURING_BASIC,
    SERVICE_MANUFACTURING_CAPITAL,
    SERVICE_MANUFACTURING_SUPERCAPITAL,
    SERVICE_LABORATORY,
    SERVICE_LABORATORY_RESEARCH_TIME,
    SERVICE_LABORATORY_RESEARCH_MATERIAL,
    SERVICE_LABORATORY_COPYING,
    SERVICE_LABORATORY_INVENTION,
    SERVICE_REACTIONS,
    SERVICE_REACTIONS_COMPOSITE,
    SERVICE_REACTIONS_BIOCHEMICAL,
    SERVICE_REACTIONS_HYBRID,
];

/// Whether this server has the feature behind `service`, so a module may go
/// online for it (D5, **[A]** A7): the cloning bay, reprocessing, moon
/// drilling and industry. Market, jump bridge, cynosural, loyalty store and
/// the automatic moon miner wait for their features.
pub fn supported(service: i64) -> bool {
    matches!(
        service,
        SERVICE_MEDICAL | SERVICE_REPROCESSING | SERVICE_MOONMINING
    ) || INDUSTRY_SERVICES.contains(&service)
}

/// The hull services, online with the hull: docking, fitting, repair and
/// insurance. `OFFICES` is left out **[A]** (A9).
pub const HULL_SERVICES: [i64; 4] = [
    SERVICE_DOCKING,
    SERVICE_FITTING,
    SERVICE_REPAIR,
    SERVICE_INSURANCE,
];

// -- state -----------------------------------------------------------------

fn row_key(structure: i64, module: i64) -> Vec<String> {
    vec![structure.to_string(), module.to_string()]
}

/// Whether `slot` is one of a structure's service slot flags.
pub fn is_slot(flag: i64) -> bool {
    (FIRST_SLOT..=LAST_SLOT).contains(&flag)
}

/// One module fitted to a structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fitted {
    pub item_id: i64,
    pub type_id: i64,
    pub flag_id: i64,
    pub online: bool,
}

/// What is fitted to `s`, in slot order: the corporation's singletons at its
/// service slots, each with whether its row says online.
pub fn fitted(reg: &Registry, s: &Structure) -> Vec<Fitted> {
    let rows = reg.deltas().of(KIND);
    let mut out: Vec<Fitted> = reg
        .store()
        .contents_of(s.id)
        .filter(|i| is_slot(i.flag_id) && i.owner_id == s.owner_corp && i.singleton)
        .map(|i| Fitted {
            item_id: i.item_id,
            type_id: i.type_id,
            flag_id: i.flag_id,
            online: rows.contains_key(&row_key(s.id, i.item_id)),
        })
        .collect();
    out.sort_by_key(|f| (f.flag_id, f.item_id));
    out
}

/// Whether any module of `s` is online: the client's
/// `HasOnlineServiceModule` (F12), which drives the upkeep.
pub fn has_online_module(reg: &Registry, s: &Structure) -> bool {
    // No online row for this structure: nothing to scan its hangar for.
    let id = s.id.to_string();
    if !reg.deltas().of(KIND).keys().any(|k| k.first() == Some(&id)) {
        return false;
    }
    fitted(reg, s).iter().any(|f| f.online)
}

/// A structure is gone, or unanchored: its online rows go.
pub fn forget(reg: &mut Registry, structure: i64) {
    let stale: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| k.first().is_some_and(|s| *s == structure.to_string()))
        .cloned()
        .collect();
    if stale.is_empty() {
        return;
    }
    for k in stale {
        reg.deltas_mut().del(KIND, &k);
    }
    reg.mark_dirty();
}

/// A structure is going away (unanchored or destroyed): every module in its
/// service slots goes to the hangar flag, offline, keeping its owner, so
/// the evacuation that follows (asset safety, the harbour move) delivers it
/// at a flag every address has; the slot flags mean nothing outside the
/// structure.
pub fn release_all(reg: &mut Registry, s: &Structure) {
    let moves: Vec<Edit> = reg
        .store()
        .contents_of(s.id)
        .filter(|i| is_slot(i.flag_id))
        .map(|i| Edit::Move {
            item_id: i.item_id,
            to: Slot::new(i.owner_id, s.id, crate::model::FLAG_HANGAR),
        })
        .collect();
    if !moves.is_empty()
        && let Err(e) = reg.apply_items(&moves)
    {
        crate::errln!("[strsvc] releasing the modules of {}: {e:?}", s.id);
    }
    forget(reg, s.id);
}

/// Drop the rows of `s` whose module is no longer fitted there, before a
/// write.
fn purge_stale(reg: &mut Registry, s: &Structure) {
    let live: Vec<String> = fitted(reg, s)
        .iter()
        .map(|f| f.item_id.to_string())
        .collect();
    let id = s.id.to_string();
    let stale: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| k.first() == Some(&id) && k.get(1).is_none_or(|m| !live.contains(m)))
        .cloned()
        .collect();
    for k in stale {
        reg.deltas_mut().del(KIND, &k);
    }
}

// -- the services ------------------------------------------------------------

/// `{service: state}` of a structure of `type_id` whose hull is `hull_online`
/// and which has `fitted` modules (`(type, online)`), under `table`:
///
/// - the hull services at the hull's state, unless the type is one of the
///   client's [`WITHOUT_ONLINE_SERVICES`] (then no hull service and no clone
///   bay; its modules' services stay);
/// - each module's services `ONLINE` when it and the hull are online, else
///   `OFFLINE`; `ONLINE` wins when two modules give one service;
/// - `JUMP_CLONE` follows `MEDICAL`'s state **[A]** (A8);
/// - `free_clone` (D1) adds `MEDICAL` and `JUMP_CLONE` at the hull's state.
pub fn services_with(
    table: ServiceTable<'_>,
    type_id: i64,
    hull_online: bool,
    fitted: &[(i64, bool)],
    free_clone: bool,
) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    let hull_gives = !WITHOUT_ONLINE_SERVICES.contains(&type_id);
    let hull = if hull_online {
        SERVICE_STATE_ONLINE
    } else {
        SERVICE_STATE_OFFLINE
    };
    let mut give = |svc: i64, state: i64| {
        out.entry(svc)
            .and_modify(|s| {
                if state == SERVICE_STATE_ONLINE {
                    *s = SERVICE_STATE_ONLINE;
                }
            })
            .or_insert(state);
    };
    if hull_gives {
        for svc in HULL_SERVICES {
            give(svc, hull);
        }
    }
    for (module, online) in fitted {
        let state = if *online && hull_online {
            SERVICE_STATE_ONLINE
        } else {
            SERVICE_STATE_OFFLINE
        };
        for svc in module_services(table, *module) {
            give(*svc, state);
            if *svc == SERVICE_MEDICAL {
                give(SERVICE_JUMP_CLONE, state);
            }
        }
    }
    if free_clone && hull_gives {
        give(SERVICE_MEDICAL, hull);
        give(SERVICE_JUMP_CLONE, hull);
    }
    out
}

/// [`services_with`] for a structure as it stands: its fitted modules and the
/// world's `freeclone`, under the client's own table.
pub fn services_of(reg: &Registry, s: &Structure) -> BTreeMap<i64, i64> {
    services_of_with(MODULE_SERVICES, reg, s)
}

/// [`services_of`] under another table.
pub fn services_of_with(
    table: ServiceTable<'_>,
    reg: &Registry,
    s: &Structure,
) -> BTreeMap<i64, i64> {
    let fitted: Vec<(i64, bool)> = fitted(reg, s)
        .iter()
        .map(|f| (f.type_id, f.online))
        .collect();
    services_with(
        table,
        s.type_id,
        s.is_online(),
        &fitted,
        reg.config().rules.upwell_free_clone,
    )
}

/// The multiplier on a structure's shield and armour points right now: at full
/// power the highest 2744 over its online modules; at low or no power the
/// hull's own 2743 (default 1.0) **[I]**. At full power with no module online
/// the old [`FULL_POWER_HP_MULTIPLIER`] **[A]** while `freeupkeep` is on (the
/// structure has full power for nothing, as before #41), else the hull's.
pub fn hp_multiplier(reg: &Registry, s: &Structure) -> f64 {
    let hull = crate::typedata::own_attribute_in(reg.content(), s.type_id, ATTR_HULL_HP_MULTIPLIER)
        .unwrap_or(1.0);
    if !crate::structure_combat::full_power(s) {
        return hull;
    }
    // Nothing online anywhere: no scan of the structure's hangar.
    let online = if reg.deltas().of(KIND).is_empty() {
        Vec::new()
    } else {
        fitted(reg, s)
    };
    let best = online
        .iter()
        .filter(|f| f.online)
        .filter_map(|f| {
            crate::typedata::own_attribute_in(reg.content(), f.type_id, ATTR_SERVICE_HP_MULTIPLIER)
        })
        .filter(|m| m.is_finite())
        .reduce(f64::max);
    match best {
        Some(m) => m,
        None if reg.config().rules.upwell_free_upkeep => FULL_POWER_HP_MULTIPLIER,
        None => hull,
    }
}

// -- refusals ------------------------------------------------------------------

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    crate::modules::user_error(key, args)
}

fn notify(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn access_denied(reason: &str) -> ServiceError {
    raise(
        "CrpAccessDenied",
        vec![("reason", crate::pyutil::bstr(reason))],
    )
}

/// The key of a raised `UserError`, for a log line or a test.
pub fn error_key(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        return format!("{e:?}");
    };
    match &**header {
        PyValue::Tuple(h) => match h.get(1) {
            Some(PyValue::Tuple(a)) => match a.first() {
                Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                other => format!("{other:?}"),
            },
            other => format!("{other:?}"),
        },
        other => format!("{other:?}"),
    }
}

/// The structure `sid` and `actor`'s right to fit and online at it: at its
/// controls ([`crate::structure_dock::control::controlling`]), in its owner
/// corporation, with the station manager role (`may_launch`, which is the
/// client's `session.IsStationManager`, the director and the CEO included, as
/// everywhere else a structure is managed) **[V]** (F8
/// `CheckCanDisableServiceModule`: that role and `session.corpid ==
/// module.ownerID`; the fit and online calls the same **[A]** A1).
fn authorise(reg: &Registry, actor: i64, sid: i64) -> Result<Structure, ServiceError> {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return Err(notify("That structure does not exist."));
    };
    if crate::structure_dock::control::controlling(reg, actor) != Some(sid) {
        return Err(notify("You are not at the controls of that structure."));
    }
    let in_corp = reg.character(actor).map(|c| c.corporation_id) == Some(s.owner_corp);
    if !in_corp || !crate::service::sov_hub::may_launch(reg, actor) {
        return Err(access_denied(
            "You need the station manager or director role of the owning corporation.",
        ));
    }
    Ok(s)
}

// -- fit ----------------------------------------------------------------------------

/// A fit the validator accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub plan: Vec<Edit>,
    pub flag_id: i64,
}

/// The hull group of a structure type: its type row's, else
/// [`sp::group_of`]'s **[A]**.
fn hull_group(reg: &Registry, type_id: i64) -> i64 {
    crate::typedata::installed_in(reg.content())
        .table()
        .get(type_id)
        .map_or_else(|| sp::group_of(type_id), |t| t.group_id)
}

/// Whether `module` is a service module (`categoryStructureModule` with the
/// service-slot effect) and which flag it takes on `s`: `flag`, or the first
/// free slot for `flag == 0` (the autofit). The checks of a fit after the
/// controls, the role and the item (A1): the kind, the hull's
/// `canFitShipGroup`/`Type`, the slot count and freedom, and the per-type
/// (and group) cap.
fn place(
    reg: &Registry,
    s: &Structure,
    module: &Item,
    flag: i64,
) -> Result<i64, crate::fitting::Refusal> {
    use crate::fitting::Refusal;
    let wrong = Refusal::WrongKind {
        type_id: module.type_id,
        ship_type_id: s.type_id,
    };
    let content = reg.content();
    let service_module = module.category_id == CATEGORY_STRUCTURE_MODULE
        && crate::typedata::installed_in(content)
            .table()
            .get(module.type_id)
            .is_some_and(|t| t.has_effect(EFFECT_SERVICE_SLOT));
    if !service_module {
        return Err(wrong);
    }
    if !crate::fitting::hull_allows(
        |a| crate::typedata::own_attribute_in(content, module.type_id, a),
        hull_group(reg, s.type_id),
        s.type_id,
    ) {
        return Err(wrong);
    }
    let slots = crate::typedata::own_attribute_in(content, s.type_id, ATTR_SERVICE_SLOTS)
        .map_or(0, |v| v as i64);
    let others: Vec<&Item> = reg
        .store()
        .contents_of(s.id)
        .filter(|i| is_slot(i.flag_id) && i.item_id != module.item_id)
        .collect();
    let free = |f: i64| !others.iter().any(|i| i.flag_id == f);
    let flag = if flag == crate::fitting::FLAG_AUTOFIT {
        (FIRST_SLOT..FIRST_SLOT + slots)
            .find(|f| free(*f))
            .ok_or(Refusal::NoSlot)?
    } else if !is_slot(flag) {
        return Err(wrong);
    } else if flag - FIRST_SLOT >= slots {
        return Err(Refusal::NoSlot);
    } else if !free(flag) {
        return Err(Refusal::Occupied);
    } else {
        flag
    };
    crate::fitting::check_caps(module, s.type_id, &others, |a| {
        crate::typedata::attribute_in(content, module.type_id, a)
    })?;
    Ok(flag)
}

/// Fit item `item_id` into slot `flag` of structure `sid` (0 for the first
/// free one), or say why not (A1). Never onlines: the client's own
/// `SetModuleOnline` follows.
pub fn plan_fit(
    reg: &Registry,
    actor: i64,
    sid: i64,
    item_id: i64,
    flag: i64,
) -> Result<Planned, ServiceError> {
    let s = authorise(reg, actor, sid)?;
    let Some(item) = reg
        .item(item_id)
        .filter(|i| i.owner_id == actor && i.location_id == sid)
        .filter(|i| i.flag_id == crate::model::FLAG_HANGAR)
    else {
        return Err(crate::fitting::Refusal::NotAvailable.to_error());
    };
    let flag_id = place(reg, &s, item, flag).map_err(|r| r.to_error())?;
    let to = Slot::new(s.owner_corp, sid, flag_id);
    let plan = if item.singleton {
        vec![Edit::Move { item_id, to }]
    } else {
        vec![Edit::Assemble { item_id, to }]
    };
    Ok(Planned { plan, flag_id })
}

/// [`plan_fit`], applied; the slot it took. The services map gains the
/// module's services **offline**, so the guests are told.
pub fn fit(
    reg: &mut Registry,
    actor: i64,
    sid: i64,
    item_id: i64,
    flag: i64,
) -> Result<i64, ServiceError> {
    let planned = plan_fit(reg, actor, sid, item_id, flag)?;
    reg.apply_items(&planned.plan)
        .map_err(|e| notify(&format!("The module could not be fitted ({e:?}).")))?;
    on_services_changed(reg, sid);
    Ok(planned.flag_id)
}

/// Whether `item_id` is a module `actor` could autofit to `sid` (the client's
/// "Fit to Active Ship" sends flag 0): a service module in the pilot's hangar
/// of the structure.
pub fn is_service_module(reg: &Registry, item_id: i64) -> bool {
    reg.item(item_id).is_some_and(|i| {
        i.category_id == CATEGORY_STRUCTURE_MODULE
            && crate::typedata::installed_in(reg.content())
                .table()
                .get(i.type_id)
                .is_some_and(|t| t.has_effect(EFFECT_SERVICE_SLOT))
    })
}

/// The module `module_id`, if it is fitted to `s`.
fn fitted_one(reg: &Registry, s: &Structure, module_id: i64) -> Result<Fitted, ServiceError> {
    fitted(reg, s)
        .into_iter()
        .find(|f| f.item_id == module_id)
        .ok_or_else(|| notify("That module is not fitted to this structure."))
}

// -- online and offline ---------------------------------------------------------

/// What onlining costs: #42 part b's [`crate::structure_fuel::pay_online`],
/// the module's dogma-modified 2110 from the fuel bay with `/world fuelburn`
/// on (free with it off, today's behaviour), refused with
/// `ServiceRequiresFuel` when the bay is short.
fn pay_online(
    reg: &mut Registry,
    s: &Structure,
    module: &Fitted,
    now: i64,
) -> Result<(), ServiceError> {
    crate::structure_fuel::pay_online(reg, s, module, now)
}

/// The server takes `module_id` offline itself (#42 part b: the fuel ran
/// out): no controls and no role are asked, [`change_online`] does the rest,
/// the services push included.
pub fn force_offline(
    reg: &mut Registry,
    sid: i64,
    module_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return Err(notify("That structure does not exist."));
    };
    change_online(MODULE_SERVICES, reg, &s, module_id, false, now)
}

/// The online checks that follow the control and the module (A20, A6, A7):
/// operational, a type with online services, every service supported, a moon
/// drill by a beacon, no high-sec ban, the budget.
fn may_online(
    table: ServiceTable<'_>,
    reg: &Registry,
    s: &Structure,
    module: &Fitted,
) -> Result<(), ServiceError> {
    if !s.state.is_operational() {
        return Err(raise("StructureCannotActivateModuleNotFullyOnline", vec![]));
    }
    let services = module_services(table, module.type_id);
    if services.is_empty() || !services.iter().all(|svc| supported(*svc)) {
        return Err(notify("This service is not available on this server yet."));
    }
    if services.contains(&SERVICE_MOONMINING)
        && crate::moon_beacon::moon_in_range(reg, s.system, s.pos).is_none()
    {
        return Err(raise(
            "CannotOnlineModuleMustBeNearMoonMiningBeacon",
            vec![],
        ));
    }
    let content = reg.content();
    let banned = crate::typedata::own_attribute_in(content, module.type_id, ATTR_HIGH_SEC_BAN)
        .is_some_and(|v| v == 1.0);
    let high = matches!(
        content.get::<crate::celestial::Universe>().band(s.system),
        Some(crate::celestial::Band::High)
    );
    if banned && high {
        return Err(notify(
            "This service cannot be onlined in high security space.",
        ));
    }
    let attr = |t: i64, a: i64| crate::typedata::attribute_in(content, t, a).unwrap_or(0.0);
    let others: Vec<Fitted> = fitted(reg, s)
        .into_iter()
        .filter(|f| f.online && f.item_id != module.item_id)
        .collect();
    let over = |key: &str, load: f64, need: f64, total: f64| {
        raise(
            key,
            vec![
                (
                    "moduleType",
                    crate::modules::type_arg_for(key, "moduleType", module.type_id),
                ),
                ("remaining", PyValue::Real(total - load)),
                ("require", PyValue::Real(need)),
                ("total", PyValue::Real(total)),
            ],
        )
    };
    for (key, need_attr, out_attr) in [
        ("NotEnoughCpu", ATTR_CPU, ATTR_CPU_OUTPUT),
        ("NotEnoughPower", ATTR_POWER, ATTR_POWER_OUTPUT),
    ] {
        let need = attr(module.type_id, need_attr);
        let Some(total) = crate::typedata::own_attribute_in(content, s.type_id, out_attr) else {
            continue;
        };
        let load: f64 = others.iter().map(|f| attr(f.type_id, need_attr)).sum();
        if need != 0.0 && load + need > total + 1e-6 {
            return Err(over(key, load, need, total));
        }
    }
    Ok(())
}

/// `dogmaIM.SetModuleOnline(structureID, moduleID)` and
/// `TakeModuleOffline`, from the pilot at the controls (A1, A20): the
/// refusals in order, then the row, the effect push and the services push.
/// `Ok(None)` is the reply the client wants; `EffectAlreadyActive2` is the
/// one error it ignores.
pub fn set_online(
    reg: &mut Registry,
    actor: i64,
    sid: i64,
    module_id: i64,
    on: bool,
    now: i64,
) -> Result<PyValue, ServiceError> {
    set_online_with(MODULE_SERVICES, reg, actor, sid, module_id, on, now)
}

/// [`set_online`] under another module table.
pub fn set_online_with(
    table: ServiceTable<'_>,
    reg: &mut Registry,
    actor: i64,
    sid: i64,
    module_id: i64,
    on: bool,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let s = authorise(reg, actor, sid)?;
    change_online(table, reg, &s, module_id, on, now)
}

/// The online change itself, with the controls and the role already settled
/// (or, for the GM's `/upwell fit`, waived).
fn change_online(
    table: ServiceTable<'_>,
    reg: &mut Registry,
    s: &Structure,
    module_id: i64,
    on: bool,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let module = fitted_one(reg, s, module_id)?;
    if module.online == on {
        return if on {
            Err(raise(
                "EffectAlreadyActive2",
                vec![("modulename", crate::modules::type_arg(module.type_id))],
            ))
        } else {
            Ok(PyValue::None)
        };
    }
    if on {
        may_online(table, reg, s, &module)?;
        pay_online(reg, s, &module, now)?;
    }
    purge_stale(reg, s);
    if on {
        reg.deltas_mut()
            .put(KIND, row_key(s.id, module_id), vec!["1".to_owned()]);
    } else {
        reg.deltas_mut().del(KIND, &row_key(s.id, module_id));
    }
    reg.mark_dirty();
    push_online(reg, s.id, module_id, on, now);
    // The line `fci_upwell_modules` looks for.
    crate::errln!(
        "[strsvc] {} module {} {}",
        s.id,
        module_id,
        if on { "online" } else { "offline" }
    );
    on_services_changed(reg, s.id);
    Ok(PyValue::None)
}

/// The online effect (16) to the pilot at the controls, as a ship's module
/// gets it **[V]** (`OnGodmaShipEffect`, with the structure as the ship).
fn push_online(reg: &mut Registry, sid: i64, module_id: i64, on: bool, now: i64) {
    if let Some(pilot) = crate::structure_dock::control::pilot_of(reg, sid) {
        reg.push(crate::modules::online_push(pilot, sid, module_id, on, now));
    }
}

/// Unfit `module_id` from `sid`: the controls, the role, fitted here; taken
/// offline first, then to the pilot's hangar in the structure **[A]** (A4).
pub fn unfit(
    reg: &mut Registry,
    actor: i64,
    sid: i64,
    module_id: i64,
    now: i64,
) -> Result<(), ServiceError> {
    let s = authorise(reg, actor, sid)?;
    let module = fitted_one(reg, &s, module_id)?;
    purge_stale(reg, &s);
    if module.online {
        reg.deltas_mut().del(KIND, &row_key(sid, module_id));
        reg.mark_dirty();
        push_online(reg, sid, module_id, false, now);
    }
    let to = Slot::new(actor, sid, crate::model::FLAG_HANGAR);
    reg.apply_items(&[Edit::Move {
        item_id: module_id,
        to,
    }])
    .map_err(|e| notify(&format!("The module could not be unfitted ({e:?}).")))?;
    // #57: a job whose service has no module left is cancelled.
    crate::structure_industry::on_module_removed(reg, sid, module.type_id, now);
    on_services_changed(reg, sid);
    Ok(())
}

/// A service of the structure changed (a module fitted, onlined, offlined,
/// unfitted, or a `/world` switch flipped): `OnStructureServiceChanged(structureID)`
/// to the pilots docked in it **[V]** (`structureServices.pyc:43`, one
/// positional), and the corporation tab of every online member of the owner
/// corporation reloads (`OnCorporationStructuresUpdated`; the corp's own
/// audience, not only its managers).
pub fn on_services_changed(reg: &mut Registry, sid: i64) {
    let to: Vec<i64> = reg
        .characters_at_station(sid)
        .iter()
        .map(|c| c.character_id)
        .collect();
    if !to.is_empty() {
        reg.push(ServiceChanged { to, id: sid });
    }
    if let Some(s) = structure::get(reg.deltas(), sid) {
        let corp = crate::service::structures::corp_audience(reg, s.owner_corp);
        if !corp.is_empty() {
            reg.push(crate::service::structures::CorpStructuresUpdated { to: corp });
        }
    }
    // One line each, where #53 (moon drills) learns that a module changed.
    // #42 part b's fuel burn needs no hook: it reads the online rows on its
    // own clock and starts a module's burn at the next hour
    // (`structure_fuel::pay_online`).
    // #57: industry jobs pause when their service goes offline and resume
    // when it is back.
    crate::structure_industry::on_services_changed(reg, sid, crate::pyutil::filetime_now());
}

/// `OnStructureServiceChanged(structureID)`: the client refetches the
/// structure's services (`structureServices.OnStructureServiceChanged`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ServiceChanged {
    pub to: Vec<i64>,
    pub id: i64,
}

impl Push for ServiceChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnStructureServiceChanged", vec![PyValue::Int(self.id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

// -- what the client reads -------------------------------------------------------

/// `module`'s `ItemGetInfo` row with the structure as its ship: the item's
/// row, the type's attributes plus `isOnline` (2), and the online effect (16)
/// as an active effect when it is online (the shape of
/// [`crate::modules::module_info`], whose pilot and ship are the structure's
/// here).
fn module_row(reg: &Registry, s: &Structure, f: &Fitted, pilot: i64) -> Option<PyValue> {
    let item = reg.item(f.item_id)?;
    let mut attributes: Vec<(PyValue, PyValue)> = crate::typedata::installed_in(reg.content())
        .table()
        .get(f.type_id)
        .map(|t| t.attributes.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|(id, v)| (PyValue::Real(*v), PyValue::Int(*id)))
        .collect();
    attributes.push((
        PyValue::Real(if f.online { 1.0 } else { 0.0 }),
        PyValue::Int(crate::modules::ATTR_IS_ONLINE),
    ));
    let mut active: Vec<(PyValue, PyValue)> = Vec::new();
    if f.online {
        active.push((
            PyValue::List(vec![
                PyValue::Int(f.item_id),
                PyValue::Int(pilot),
                PyValue::Int(s.id),
                PyValue::None,
                PyValue::None,
                PyValue::List(Vec::new()),
                PyValue::Int(crate::modules::EFFECT_ONLINE),
                PyValue::None,
                PyValue::Int(-1),
                PyValue::Int(-1),
            ]),
            PyValue::Int(crate::modules::EFFECT_ONLINE),
        ));
    }
    Some(crate::pyutil::keyval(vec![
        ("itemID", PyValue::Int(f.item_id)),
        ("invItem", reg.item_row_value(item)),
        ("attributes", PyValue::Dict(attributes)),
        ("activeEffects", PyValue::Dict(active)),
        ("time", PyValue::None),
        ("wallclockTime", PyValue::None),
    ]))
}

/// The fitted modules of `sid` as `shipInfo` entries `(row, key)` for the
/// pilot at its controls (`GetAllInfo` primes the structure as the ship).
pub fn info_entries(reg: &Registry, sid: i64) -> Vec<(PyValue, PyValue)> {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return Vec::new();
    };
    let pilot = crate::structure_dock::control::pilot_of(reg, sid).unwrap_or(0);
    fitted(reg, &s)
        .iter()
        .filter_map(|f| module_row(reg, &s, f, pilot).map(|row| (row, PyValue::Int(f.item_id))))
        .collect()
}

/// `dogmaIM.ItemGetInfo(itemID)` for a module fitted to the structure `actor`
/// has the controls of (godma meets a module fitted after the prime).
pub fn item_info(reg: &Registry, actor: i64, item_id: i64) -> Option<PyValue> {
    let sid = crate::structure_dock::control::controlling(reg, actor)?;
    let s = structure::get(reg.deltas(), sid)?;
    let f = fitted(reg, &s).into_iter().find(|f| f.item_id == item_id)?;
    module_row(reg, &s, &f, actor)
}

/// `base`, the structure type's ship-shaped attributes, plus the hull's own
/// fitting numbers the client reads at the controls ([`HULL_ATTRIBUTES`]),
/// each when the hull's row has it and `base` lacks it.
pub fn hull_attributes(content: &crate::content::Content, type_id: i64, base: PyValue) -> PyValue {
    let PyValue::Dict(mut entries) = base else {
        return base;
    };
    for attr in HULL_ATTRIBUTES {
        let has = entries
            .iter()
            .any(|(_, k)| matches!(k, PyValue::Int(id) if *id == attr));
        if has {
            continue;
        }
        if let Some(v) = crate::typedata::own_attribute_in(content, type_id, attr) {
            entries.push((PyValue::Real(v), PyValue::Int(attr)));
        }
    }
    PyValue::Dict(entries)
}

// -- the GM's tools ------------------------------------------------------------------

/// `/upwell fit <structureID> <typeID>`: one unit of `type_id` in the first
/// free slot of `sid`, onlined, skipping the controls and the role but not
/// the slot, the type, the beacon or the high-security checks. Answers the
/// line the GM reads.
pub fn gm_fit(reg: &mut Registry, actor: i64, sid: i64, type_id: i64, now: i64) -> String {
    gm_fit_with(MODULE_SERVICES, reg, actor, sid, type_id, now)
}

/// [`gm_fit`] under another module table.
pub fn gm_fit_with(
    table: ServiceTable<'_>,
    reg: &mut Registry,
    actor: i64,
    sid: i64,
    type_id: i64,
    now: i64,
) -> String {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return format!("no structure {sid}");
    };
    if reg.character(actor).map(|c| c.corporation_id) != Some(s.owner_corp) {
        return "that structure is not your corporation's".to_owned();
    }
    let Some(row) = crate::typedata::installed_in(reg.content())
        .table()
        .get(type_id)
        .cloned()
    else {
        return format!("no such type {type_id}");
    };
    let probe = Item {
        item_id: 0,
        type_id,
        owner_id: s.owner_corp,
        location_id: sid,
        flag_id: 0,
        quantity: 1,
        group_id: row.group_id,
        category_id: row.category_id,
        singleton: true,
    };
    let flag = match place(reg, &s, &probe, crate::fitting::FLAG_AUTOFIT) {
        Ok(f) => f,
        Err(r) => return format!("refused: {}", error_key(&r.to_error())),
    };
    let to = Slot::new(s.owner_corp, sid, flag);
    let id = match reg.apply_items(&[Edit::CreateAssembled { type_id, to }]) {
        Ok(changes) => changes.iter().find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        }),
        Err(e) => return format!("could not create type {type_id} ({e:?})"),
    };
    let Some(id) = id else {
        return "nothing was created".to_owned();
    };
    on_services_changed(reg, sid);
    match change_online(table, reg, &s, id, true, now) {
        Ok(_) => format!("fitted {id} in slot {flag}, online"),
        Err(e) => format!(
            "fitted {id} in slot {flag}, offline: online refused ({})",
            error_key(&e)
        ),
    }
}

/// `/upwell services <id>`: what the structure offers, by service id.
pub fn services_text(reg: &Registry, sid: i64) -> String {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return format!("no structure {sid}");
    };
    let mut lines: Vec<String> = services_of(reg, &s)
        .iter()
        .map(|(svc, st)| {
            format!(
                "service {svc} {}",
                if *st == SERVICE_STATE_ONLINE {
                    "online"
                } else {
                    "offline"
                }
            )
        })
        .collect();
    for f in fitted(reg, &s) {
        lines.push(format!(
            "slot {}: module {} type {} {}",
            f.flag_id,
            f.item_id,
            f.type_id,
            if f.online { "online" } else { "offline" }
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
