//! **Upwell structures in space**: the ball, the slim, `Anchor`, `Unanchor`,
//! `CancelUnanchor`, `Rename` and the clock that carries a structure through
//! its states (lane U3, issue #63; spec `handback/r-upwell` §2, §5, §7).
//!
//! [`crate::structure`] is the model and owns the delta row; this is what
//! puts a row into a solar system. A structure is a body that never moves,
//! like a sov hub ([`crate::sov_hub::ball`]): `Mode::Stop`, no mass,
//! `FLAGS_STATIC_BALL`, the type's radius, its owner corp and alliance. It is
//! a ball in **every** park of its system (the type is always global in
//! the client: `structureProximityTracker` tracks it from `OnBallAdded` and
//! hides a structure out of range itself **[I]**), so [`balls_in`] is one
//! line in `destiny::scene` and the push is the hub's `HubAdded`.
//!
//! **What the client reads off the slim** (`spaceObject.structure.Structure`
//! **[V]**): `state`, `timer` `(start, end, paused)`, `upkeepState`,
//! `repairing`, `deployTimes`, `docked`, `allianceID`, `modules` and
//! `dunRotation` `(yawDegrees, 0, 0)`; `corpID` and `allianceID` load the
//! logo. They are read as plain attributes, so every one is always sent, a
//! `None` included ([`extras_of`]). A change of state rides
//! `OnSlimItemChange` ([`crate::sov_hub::HubSlimChange`]); the construction
//! animation (`BuildStructure`) is the `timer` of `State::Anchoring`.
//!
//! **The calls** (`structureDeployment`, `service::structures::deploy`):
//! - `Anchor(itemID, x, z, yaw, profileID, name, bio, reinforceWeekday,
//!   reinforceHour, extraConfig)` **[V]** (`structureDeployment.
//!   ConfirmDeployment`): the item leaves the pilot's cargo and *is* the
//!   structure from then on (the row key is its item id). `x` and `z` are
//!   an offset from the ship (the client adds the ship's position when it
//!   draws the placement, `deploymentEntity._GetPositionAndBalls`) **[V]**.
//! - `Unanchor(id)`, `CancelUnanchor(id)`, `RenameStructure(id, name)`.
//!
//! **Lockable, and shootable only while vulnerable** (#44/#45, L1): any pilot
//! in the structure's system may lock it ([`crate::structure_combat::
//! is_lockable`]); it takes damage only while `vulnerable` (the shooting lane,
//! L2). Its bars are [`crate::structure_combat::damage_state`], in the scene,
//! in `AddBalls2` and in an `OnDamageStateChange` whenever its damage changes.
//!
//! **Anchoring time** is a world setting (`Rules::upwell_anchor_secs`, `/world
//! anchortime`, default an hour) read when a structure is anchored and on
//! every tick. The **upkeep** is always "full": [`env`] passes
//! `has_service_module = true`, as the directory lane did, until the upkeep
//! lane (U6) fits real modules.
//!
//! **[A] assumptions** (falsifier in each):
//! - A structure's `y` is the ship's `y` (spec A3). Falsifier: the structure
//!   appears off the placement the client drew.
//! - `yaw` is radians and `dunRotation` degrees (spec §2). Falsifier: the
//!   model faces the wrong way.
//! - The structure is a ball to every park of its system (spec A5).
//!   Falsifier: off-grid brackets for a pilot without access.
//! - A finished unanchoring delivers the item to the corp's CEO, in the
//!   hangar of the first station of the system (else the CEO's own station).
//!   Falsifier: the owner expects it in a corp hangar, or in the cargo of the
//!   pilot who gave the order. If nowhere fits, it stays in space.
//! - Anchoring finishes in state 102 and waits for the deed (U6) before
//!   `ShieldVulnerable`; it is dockable from there (spec §2). Falsifier: the
//!   Dock button stays hidden.
//! - The placement check ignores warp-in points (a planet's and a
//!   stargate's), which `FindDeploymentConflict` also tests. Falsifier: a
//!   structure placed where the client's own check says no.

use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::item::{Edit, Slot};
use crate::notify::Push;
use crate::service::Registry;
use crate::sov_hub::{HubAdded, HubSlimChange};
use crate::structure::{self, Event, Structure};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The three home hulls, by type id (`structure::size_of_type` knows the
/// rest). Named so a test never writes the number.
pub const ASTRAHUS: i64 = 35_832;
pub const RAITARU: i64 = 35_825;
pub const ATHANOR: i64 = 35_835;

/// The radius of a structure whose type the table lacks (an Astrahus's).
pub const DEFAULT_RADIUS: f64 = 45_000.0;

/// Where the item of an anchored structure is in the store: its system,
/// flag 0, as a wreck or a launched drone is.
pub const FLAG_IN_SPACE: i64 = crate::combat::FLAG_IN_SPACE;

/// Where `Anchor` may take the item from: the same holds a hub launch may.
pub const ANCHOR_FLAGS: &[i64] = crate::sov_hub::LAUNCH_FLAGS;

// -- the tick environment ------------------------------------------------------

/// The environment every structure is stepped in: the directory lane's
/// (a service module always fitted) with the world's anchoring time.
pub fn env(reg: &Registry, now: i64) -> structure::Env {
    structure::Env {
        anchoring_secs: reg.config().rules.upwell_anchor_secs,
        ..crate::service::structures::env(now)
    }
}

/// [`env`] for one structure: what its own type and the guns on it say. Under
/// fire ([`crate::structure_combat::under_fire`]), `CanHaveAutoRepair` (5770)
/// and `CanHaveArmorPhases` (5771), each true unless the type itself says 0.
///
/// The reinforce lengths follow the hull's size and the system
/// ([`reinforce_secs`]).
pub fn env_for(reg: &Registry, s: &Structure, now: i64) -> structure::Env {
    let flag = |attr: i64| {
        crate::typedata::own_attribute_in(reg.content(), s.type_id, attr).is_none_or(|v| v != 0.0)
    };
    let (armor_secs, hull_secs) = reinforce_secs(reg, s);
    structure::Env {
        reinforce_armor_secs: armor_secs,
        reinforce_hull_secs: hull_secs,
        under_fire: crate::structure_combat::under_fire(reg, s, now),
        auto_repair: flag(crate::structure_combat::ATTR_CAN_AUTO_REPAIR),
        armor_phases: flag(crate::structure_combat::ATTR_CAN_ARMOR_PHASES),
        ..env(reg, now)
    }
}

/// `structures.REINFORCE_DURATION_ARMOR_MEDIUM_WORMHOLE` **[V]**
/// (`structures/const.py:151-172`).
pub const REINFORCE_ARMOR_MEDIUM_WORMHOLE: i64 = 216_000;
/// `REINFORCE_DURATION_ARMOR_MEDIUM_BY_SECURITY` for null and low security.
pub const REINFORCE_ARMOR_MEDIUM_LOW: i64 = 302_400;
/// The same for high security (and the client's safe class).
pub const REINFORCE_ARMOR_MEDIUM_HIGH: i64 = 388_800;
/// `REINFORCE_DURATION_HULL_WORMHOLE`.
pub const REINFORCE_HULL_WORMHOLE: i64 = 129_600;
/// `REINFORCE_DURATION_HULL_BY_SECURITY` for null and low security.
pub const REINFORCE_HULL_LOW: i64 = 216_000;
/// The same for high security (and the client's safe class).
pub const REINFORCE_HULL_HIGH: i64 = 388_800;

/// The window the client sends: a weekday 0..6 with an hour, or the client's
/// own `NO_REINFORCEMENT_WEEKDAY` (255) with an hour, which is all its deploy
/// and window screens ever send **[V]** (`structures/reinforce.py:79-86`,
/// `deploymentCont.py:526-528`, `reinforcementTimeWnd.py:37,166-169`).
/// `structure::check_reinforce_time` alone refuses 255.
pub fn check_window(weekday: i64, hour: i64) -> Result<(), &'static str> {
    if weekday == structure::NO_REINFORCEMENT_WEEKDAY {
        structure::check_reinforce_hour(hour)
    } else {
        structure::check_reinforce_time(weekday, hour)
    }
}

/// The armour and hull reinforce lengths of `s`, seconds. The choice per
/// hull is the server's hook (`GetReinforceDurationArmor/Hull` return the
/// defaults in the client's shared class, `structures/structure.py:94-98`)
/// **[I]**: a medium's armour timer is longer than the default, in
/// wormhole space the wormhole length and elsewhere the security class's;
/// a large or bigger hull's hull timer likewise; a medium never reinforces
/// its hull, and the other sizes keep the defaults. An unknown system reads
/// as low sec **[A]**, as `crimewatch::space_of` does; falsifier: a high-sec
/// structure with a null-sec length.
pub fn reinforce_secs(reg: &Registry, s: &Structure) -> (i64, i64) {
    use crate::celestial::SecurityClass;
    // A Triglavian system counts as wormhole space: the client's
    // `REINFORCE_DURATION_*_TRIGLAVIAN` are the wormhole values
    // (`structures/const.py`) **[V]**.
    let wormhole = crate::celestial::WORMHOLE_SYSTEMS.contains(&s.system)
        || crate::pochven::in_pochven(reg, s.system);
    let high = !wormhole
        && matches!(
            reg.content()
                .get::<crate::celestial::Universe>()
                .security_class(s.system),
            Some(SecurityClass::High | SecurityClass::Safe)
        );
    let armor = match s.size() {
        structure::Size::Medium if wormhole => REINFORCE_ARMOR_MEDIUM_WORMHOLE,
        structure::Size::Medium if high => REINFORCE_ARMOR_MEDIUM_HIGH,
        structure::Size::Medium => REINFORCE_ARMOR_MEDIUM_LOW,
        _ => structure::REINFORCE_DURATION_ARMOR_DEFAULT,
    };
    let hull = match s.size() {
        structure::Size::Large | structure::Size::ExtraLarge if wormhole => REINFORCE_HULL_WORMHOLE,
        structure::Size::Large | structure::Size::ExtraLarge if high => REINFORCE_HULL_HIGH,
        structure::Size::Large | structure::Size::ExtraLarge => REINFORCE_HULL_LOW,
        _ => structure::REINFORCE_DURATION_HULL_DEFAULT,
    };
    (armor, hull)
}

// -- the ball and the slim -------------------------------------------------------

/// The type's radius: the type table's, else [`DEFAULT_RADIUS`].
pub fn radius(content: &crate::content::Content, type_id: i64) -> f64 {
    content
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(type_id))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(DEFAULT_RADIUS)
}

/// The structure's ball.
pub fn ball(reg: &Registry, s: &Structure) -> Ball {
    let alliance = crate::alliance::of_corp(reg.deltas(), s.owner_corp);
    Ball {
        ball_id: s.id,
        mode: Mode::Stop,
        radius: radius(reg.content(), s.type_id) as f32,
        position: s.pos,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(s.owner_corp).unwrap_or(-1),
        alliance_id: alliance.and_then(|a| i32::try_from(a).ok()).unwrap_or(-1),
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// The five slim attributes: the owner corp is the owner and the corp, no
/// pilot. The rest ride in [`extras_of`].
pub fn slim(s: &Structure) -> Slim {
    Slim {
        item_id: s.id,
        type_id: s.type_id,
        owner_id: s.owner_corp,
        char_id: None,
        corp_id: Some(s.owner_corp),
        response_thresholds: None,
    }
}

fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// `(start, end, paused)` or `None`, the timer as `Structure._OnSlimItem
/// Updated` unpacks it **[V]**. A running timer only.
fn timer_value(s: &Structure) -> PyValue {
    match (s.timer.start, s.timer.end) {
        (Some(start), Some(end)) => PyValue::Tuple(vec![
            PyValue::Int(start),
            PyValue::Int(end),
            opt_int(s.timer.paused),
        ]),
        _ => PyValue::None,
    }
}

/// Every key the client reads off a structure's slim, always present.
pub fn extras_of(reg: &Registry, s: &Structure) -> Vec<(&'static str, PyValue)> {
    let alliance = crate::alliance::of_corp(reg.deltas(), s.owner_corp);
    let mut out = vec![
        // michelle hints `cfg.evelocations` from this; None and the client never asks.
        ("name", PyValue::Unicode(s.name.clone())),
        ("state", PyValue::Int(s.state.code())),
        ("timer", timer_value(s)),
        (
            "upkeepState",
            s.upkeep.map_or(PyValue::None, |u| PyValue::Int(u.code())),
        ),
        (
            "repairing",
            s.repairing.map_or(PyValue::None, PyValue::Bool),
        ),
        (
            "deployTimes",
            match (s.deploy_start, s.deploy_end) {
                (Some(a), Some(b)) => PyValue::Tuple(vec![PyValue::Int(a), PyValue::Int(b)]),
                _ => PyValue::None,
            },
        ),
        // Pilots docked inside (U4).
        (
            "docked",
            PyValue::Int(crate::structure_dock::docked_count(reg, s.id) as i64),
        ),
        ("allianceID", opt_int(alliance)),
        ("modules", PyValue::List(Vec::new())),
        (
            "dunRotation",
            PyValue::Tuple(vec![
                PyValue::Real(s.yaw.to_degrees()),
                PyValue::Real(0.0),
                PyValue::Real(0.0),
            ]),
        ),
    ];
    // ANSI-1 (#85): an Ansiblex also carries its link (`jump_bridge`).
    out.extend(crate::jump_bridge::slim_keys(reg, s));
    out
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(structure::KIND).is_empty() {
        return Vec::new();
    }
    ids.iter()
        .filter_map(|id| {
            let s = structure::get(w.deltas(), *id)?;
            Some((s.id, extras_of(w, &s)))
        })
        .collect()
}

/// `system`'s structure balls and slims, for `destiny::scene` (coordinator:
/// one line there). Global, like the hub: in no `grid::local_balls`.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if w.deltas().of(structure::KIND).is_empty() {
        return Vec::new();
    }
    structure::in_system(w.deltas(), system)
        .iter()
        .filter(|s| s.state != structure::State::Unanchored)
        .map(|s| (ball(w, s), slim(s)))
        .collect()
}

/// Where the structure `ball_id` is, for `Registry::ball_target`
/// (coordinator: one line there): approach, orbit and warp to. A structure
/// is category 65, which the client gives no warp-in point **[I]**.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    if w.deltas().of(structure::KIND).is_empty() {
        return None;
    }
    let s = structure::get(w.deltas(), ball_id)?;
    Some(crate::model::Target {
        position: s.pos,
        radius: radius(w.content(), s.type_id),
        warp_in: None,
    })
}

/// `config.GetMultiLocationsEx`'s row for each structure asked about
/// (coordinator: one line in `Registry::multi_locations`). `cfg.evelocations.
/// Get` raises on a miss, and `uix.GetSlimItemName` falls through to it for
/// a structure **[V]**, so a structure with no row would break its bracket.
/// A row the caller already made (a station name for a docked pilot's
/// `station_id`, once U4 makes one) is replaced.
pub fn overlay(reg: &Registry, wanted: &[i64], rows: &mut Vec<eve_rowset::cfg::Location>) {
    if reg.deltas().of(structure::KIND).is_empty() {
        return;
    }
    for &id in wanted {
        let Some(s) = structure::get(reg.deltas(), id) else {
            continue;
        };
        let row = location_of(&s);
        match rows.iter_mut().find(|l| l.location_id == id) {
            Some(l) => *l = row,
            None => rows.push(row),
        }
    }
}

fn location_of(s: &Structure) -> eve_rowset::cfg::Location {
    eve_rowset::cfg::Location::new(s.id, s.name.clone()).at(s.system, s.pos[0], s.pos[1], s.pos[2])
}

// -- who is told -----------------------------------------------------------------

/// The characters in space in `system`: who sees a ball there.
pub(crate) fn in_space(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// Online members of `corp` and the online pilots in `system`, once each.
pub(crate) fn interested(w: &Registry, corp: i64, system: i64) -> Vec<i64> {
    let mut out = crate::service::structures::corp_audience(w, corp);
    for c in crate::service::structures::system_audience(w, system) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// `OnStructureStateChanged(solarSystemID, structureID, state)`: the
/// browser's entry for the structure follows **[V]**
/// (`structureBrowserController.OnStructureStateChanged`).
#[derive(Debug, Clone, PartialEq)]
pub struct StateChanged {
    pub to: Vec<i64>,
    pub system: i64,
    pub id: i64,
    pub state: i64,
}

impl Push for StateChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnStructureStateChanged",
            vec![
                PyValue::Int(self.system),
                PyValue::Int(self.id),
                PyValue::Int(self.state),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnStructureRemovedFromSpace(structureID)`: the directory forgets it
/// **[V]** (`structureDirectory.OnStructureRemovedFromSpace`).
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedFromSpace {
    pub to: Vec<i64>,
    pub id: i64,
}

impl Push for RemovedFromSpace {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnStructureRemovedFromSpace", vec![PyValue::Int(self.id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `AddBalls2` of a structure to every park in its system.
pub fn announce_added(reg: &mut Registry, s: &Structure) {
    let to = in_space(reg, s.system);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![ball(reg, s)],
        },
        vec![(
            slim(s),
            Some(crate::structure_combat::damage_state(s).to_pyvalue()),
        )],
    )
    .map(|a| a.with_extras(vec![(s.id, extras_of(reg, s))]));
    if let Some(a) = added {
        // Scheduled, not pushed: this runs from the clock as well as from
        // calls, and `pending` only drains after a client's next call.
        reg.schedule(std::time::Duration::ZERO, HubAdded(a));
    }
}

/// `OnSlimItemChange` with the structure's slim as it now reads.
pub fn announce_slim(reg: &mut Registry, s: &Structure) {
    let to = in_space(reg, s.system);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let push = HubSlimChange {
        to,
        stamp,
        slim: slim(s),
        extras: extras_of(reg, s),
    };
    // Scheduled, not pushed (see `announce_bars`): the timers flip a state on
    // the server's clock, and `pending` drains only after a client call
    // [V] the 2026-11-10 timers run: 5.4 s flip, nothing sent in the 30 s wait.
    reg.schedule(std::time::Duration::ZERO, push);
}

/// `OnDamageStateChange` with the structure's bars as they now read, to every
/// pilot in its system. Scheduled, not pushed: a volley and the clock both run
/// outside a call (`combat.rs` does the same for a ship).
pub fn announce_bars(reg: &mut Registry, s: &Structure) {
    let to = in_space(reg, s.system);
    let stamp = reg.next_destiny_stamp();
    if let Some(p) = crate::destiny::DamageStateChange::new(
        to,
        stamp,
        s.id,
        crate::structure_combat::damage_state(s),
    ) {
        reg.schedule(std::time::Duration::ZERO, p);
    }
}

/// `RemoveBalls` to the system.
fn announce_removed(reg: &mut Registry, system: i64, id: i64) {
    let to = in_space(reg, system);
    let stamp = reg.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, vec![id]) {
        reg.schedule(std::time::Duration::ZERO, gone);
    }
}

/// What changed for people outside the system: the corp's browser and
/// the dock lists. `dockable_changed` says the Dock button flipped.
pub(crate) fn announce_lists(reg: &mut Registry, s: &Structure, dockable_changed: bool) {
    let corp = crate::service::structures::corp_audience(reg, s.owner_corp);
    reg.push(crate::service::structures::CorpStructuresUpdated { to: corp });
    if dockable_changed {
        let near = crate::service::structures::system_audience(reg, s.system);
        if !near.is_empty() {
            reg.push(crate::service::structures::DockingAccessChanged { to: near });
        }
    }
}

// -- the clock --------------------------------------------------------------------

/// The `tick::Stage` row (coordinator): step every structure's state
/// machine, and tell people what changed. A world with no structure reads
/// one empty kind.
///
/// **`ft` is the clock** the repair pause is judged on: the tick's own blue
/// time, not the wall clock. [`crate::structure_combat::Fire::record`] must
/// be given the same clock (the shooting lane's `now`), or a volley and the
/// window that reads it disagree about when it landed.
pub fn tick(reg: &mut Registry, _: std::time::Instant, ft: crate::damage::BlueTime) {
    if reg.deltas().of(structure::KIND).is_empty() {
        return;
    }
    let now = ft;
    let fallback = env(reg, now);
    let envs: BTreeMap<i64, structure::Env> = structure::all(reg.deltas())
        .iter()
        .map(|s| (s.id, env_for(reg, s, now)))
        .collect();
    let ran = structure::tick(reg.deltas_mut(), |s| {
        envs.get(&s.id).copied().unwrap_or(fallback)
    });
    if ran.is_empty() {
        return;
    }
    reg.mark_dirty();
    for (id, events) in ran {
        react_at(reg, id, &events, now);
    }
}

/// Tell the world about the events one structure raised.
pub(crate) fn react(reg: &mut Registry, id: i64, events: &[Event]) {
    react_at(reg, id, events, crate::pyutil::filetime_now());
}

/// [`react`] on the caller's clock: the tick's own, so the notifications'
/// `timeLeft` is judged on the clock the state machine used.
pub(crate) fn react_at(reg: &mut Registry, id: i64, events: &[Event], now: i64) {
    let Some(s) = structure::get(reg.deltas(), id) else {
        return;
    };
    let mut state_to = None;
    let mut slim_dirty = false;
    let mut dockable_changed = false;
    let mut bars_dirty = false;
    for ev in events {
        match ev {
            Event::DamageChanged => bars_dirty = true,
            Event::StateChanged { from, to } => {
                state_to = Some(*to);
                slim_dirty = true;
                dockable_changed |= is_dockable(*from) != is_dockable(*to);
            }
            Event::TimerChanged
            | Event::UpkeepChanged { .. }
            | Event::RepairingChanged { .. }
            | Event::DeployTimeChanged
            | Event::UnanchoringChanged => slim_dirty = true,
            _ => {}
        }
    }
    if state_to == Some(structure::State::Unanchored) {
        finish_unanchor(reg, &s);
        return;
    }
    crate::structure_notify::on_events(reg, &s, events, now);
    if bars_dirty {
        announce_bars(reg, &s);
    }
    if slim_dirty {
        announce_slim(reg, &s);
        crate::structure_dock::announce_hangar_state(reg, &s);
    }
    if let Some(to) = state_to {
        let audience = crate::service::structures::corp_audience(reg, s.owner_corp);
        reg.push(StateChanged {
            to: audience,
            system: s.system,
            id: s.id,
            state: to.code(),
        });
    }
    if slim_dirty {
        announce_lists(reg, &s, dockable_changed);
    }
}

/// `react` for a test in another module.
#[cfg(test)]
pub(crate) fn react_for_test(reg: &mut Registry, id: i64) {
    react(
        reg,
        id,
        &[Event::StateChanged {
            from: structure::State::ShieldVulnerable,
            to: structure::State::Unanchored,
        }],
    );
}

/// Whether the Dock button is offered in `state` (`OfferDock`: not an
/// offline state **[V]**).
fn is_dockable(state: structure::State) -> bool {
    !state.is_offline()
}

// -- the calls ----------------------------------------------------------------------

/// A refusal, as the player reads it.
pub type Refused = String;

/// `Anchor`'s arguments past the item, as the client sends them.
#[derive(Debug, Clone)]
pub struct AnchorArgs {
    pub item_id: i64,
    pub x: f64,
    pub z: f64,
    pub yaw: f64,
    /// `None`: the corporation's default profile.
    pub profile: Option<i64>,
    pub name: String,
    pub bio: String,
    pub reinforce_weekday: i64,
    pub reinforce_hour: i64,
}

fn space_ball(
    item_id: i64,
    group_id: i64,
    category_id: i64,
    type_id: i64,
    pos: [f64; 3],
    radius: f64,
) -> structure::SpaceBall {
    structure::SpaceBall {
        item_id,
        type_id,
        group_id,
        category_id,
        pos,
        radius,
        kind: structure::PointKind::RealBall,
    }
}

/// Every ball in `system` the placement is checked against: the
/// celestials the park serves (`inventorycommon.const` group and category
/// by kind **[V]**) and the other structures.
fn deployment_balls(reg: &Registry, system: i64) -> Vec<structure::SpaceBall> {
    use crate::celestial::Kind;
    let mut out = Vec::new();
    for c in reg.celestials().served_in(system) {
        let (group, category) = match c.kind {
            Kind::Star => (6, 2),
            Kind::Planet => (7, 2),
            Kind::Moon => (8, 2),
            Kind::Belt => (9, 2),
            Kind::Gate => (10, 2),
            Kind::Station => (15, 3),
            Kind::Other => continue,
        };
        out.push(space_ball(
            c.id, group, category, c.type_id, c.position, c.radius,
        ));
    }
    for other in structure::in_system(reg.deltas(), system) {
        out.push(space_ball(
            other.id,
            crate::structure_profile::group_of(other.type_id),
            crate::structure_profile::CATEGORY_STRUCTURE,
            other.type_id,
            other.pos,
            radius(reg.content(), other.type_id),
        ));
    }
    out
}

/// What `Anchor` checks before it takes the item, in the order a pilot
/// meets them, then the anchoring itself. `corp` is the pilot's corporation,
/// already known to be allowed (the service checks the role). The new
/// structure.
pub fn anchor(
    reg: &mut Registry,
    actor: i64,
    corp: i64,
    a: &AnchorArgs,
) -> Result<Structure, Refused> {
    if !structure::valid_name(&a.name) {
        return Err("A structure needs a name of at least three characters.".to_owned());
    }
    if !a.x.is_finite() || !a.z.is_finite() || !a.yaw.is_finite() {
        return Err("That is not a place.".to_owned());
    }
    check_window(a.reinforce_weekday, a.reinforce_hour).map_err(str::to_owned)?;
    let ch = reg
        .character(actor)
        .cloned()
        .ok_or("You are not in space.")?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == ch.ship_id)
        .ok_or("You must be in your ship in space to anchor a structure.")?;
    if here.motion.warp_active() {
        return Err("You cannot anchor a structure while warping.".to_owned());
    }
    let item = reg
        .store()
        .get(a.item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor && i.location_id == ch.ship_id && ANCHOR_FLAGS.contains(&i.flag_id)
        })
        .ok_or("That structure is not in your ship's hold.")?;
    if structure::size_of_type(item.type_id) == structure::Size::Undefined {
        return Err("That item is not a structure that can be anchored.".to_owned());
    }
    let system = here.solar_system_id;
    // The placement: an offset from the ship, in the plane (`y` is the ship's, A3).
    let ship = here.motion.position;
    let pos = [ship[0] + a.x, ship[1], ship[2] + a.z];
    if !structure::within_deploy_range(ship, pos) {
        return Err("That is too far from your ship.".to_owned());
    }
    let subject = structure::DeployType {
        type_id: item.type_id,
        group_id: item.group_id,
        category_id: item.category_id,
        radius: radius(reg.content(), item.type_id),
    };
    let balls = deployment_balls(reg, system);
    let beacons: Vec<structure::BeaconAt> = crate::moon_beacon::beacons(reg)
        .in_system(system)
        .iter()
        .map(|b| (b.moon_id, b.pos))
        .collect();
    if let Some(c) = structure::find_deployment_conflict(&subject, pos, &balls, &beacons) {
        if c.min_distance < 0.0 {
            return Err("That must be anchored within 250 km of a moon mining beacon.".to_owned());
        }
        if c.min_distance == structure::MINIMUM_MOON_DISTANCE
            && structure::moonmining_availability(&subject, pos, &balls, &beacons) == 1
        {
            return Err("Too close to the moon and too far from its mining beacon.".to_owned());
        }
        return Err(format!(
            "A structure cannot be anchored within {} km of that.",
            (c.min_distance / 1000.0) as i64
        ));
    }
    let profile = match a.profile {
        Some(p) => crate::structure_profile::profile(reg.deltas(), p)
            .filter(|pr| pr.corp == corp)
            .map(|pr| pr.id)
            .ok_or("That access profile is not your corporation's.")?,
        None => crate::structure_profile::ensure_default(reg, corp),
    };
    // The item leaves the hold: one of a stack, or the whole item.
    let to = Slot::new(corp, system, FLAG_IN_SPACE);
    let edit = if item.quantity > 1 {
        Edit::Split {
            item_id: a.item_id,
            quantity: 1,
            to,
        }
    } else {
        Edit::Move {
            item_id: a.item_id,
            to,
        }
    };
    let changes = reg
        .apply_items(&[edit])
        .map_err(|e| format!("The anchoring failed: {e}"))?;
    let id = changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(a.item_id);
    let e = env(reg, crate::pyutil::filetime_now());
    let mut s = Structure::anchored(
        id,
        item.type_id,
        corp,
        system,
        pos,
        a.yaw,
        &a.name,
        profile,
        &e,
    );
    s.bio = a
        .bio
        .chars()
        .take(crate::structure_profile::MAX_DESC_CHARS)
        .collect();
    s.set_reinforce_times(a.reinforce_weekday, a.reinforce_hour);
    // `AnchorVulnerable` repairs its hull for 15 minutes before it moves on
    // (`update_repair`); here the world's anchor time is the whole wait, so
    // the hull is whole at once and the first update moves it to
    // `Anchoring` with its timer. [A] falsifier: the owner wants the
    // retail quarter hour in front.
    s.set_damage(1.0, 1.0, 1.0, &e);
    settle(&mut s, &e);
    s.take_events();
    structure::put(reg.deltas_mut(), &s);
    reg.mark_dirty();
    announce_added(reg, &s);
    announce_lists(reg, &s, true);
    crate::structure_notify::anchoring(reg, &s, e.now);
    crate::errln!(
        "[structure] {} anchored in {system} by {actor} for corp {corp} at {:?}",
        s.id,
        s.pos
    );
    Ok(s)
}

/// The structure `id` of `corp`, or why not.
fn owned(reg: &Registry, corp: i64, id: i64) -> Result<Structure, Refused> {
    structure::get(reg.deltas(), id)
        .filter(|s| s.owner_corp == corp)
        .ok_or_else(|| "That is not one of your corporation's structures.".to_owned())
}

/// `Unanchor`: start the countdown. A state that cancels it (anchoring, or
/// one still being built) refuses.
pub fn unanchor(reg: &mut Registry, corp: i64, id: i64) -> Result<(), Refused> {
    let mut s = owned(reg, corp, id)?;
    if s.unanchoring.is_some() {
        return Err("That structure is already being decommissioned.".to_owned());
    }
    if s.state.cancels_unanchor() == Some(true) {
        return Err("That structure cannot be decommissioned in its present state.".to_owned());
    }
    let e = env(reg, crate::pyutil::filetime_now());
    s.start_unanchoring(&e);
    settle(&mut s, &e);
    let told = s.clone();
    save_and_tell(reg, s);
    crate::structure_notify::unanchoring(reg, &told, e.now);
    Ok(())
}

/// `CancelUnanchor`.
pub fn cancel_unanchor(reg: &mut Registry, corp: i64, id: i64) -> Result<(), Refused> {
    let mut s = owned(reg, corp, id)?;
    if s.unanchoring.is_none() {
        return Err("That structure is not being decommissioned.".to_owned());
    }
    s.stop_unanchoring();
    let e = env(reg, crate::pyutil::filetime_now());
    settle(&mut s, &e);
    save_and_tell(reg, s);
    Ok(())
}

/// `RenameStructure`: the name goes in the row, the pilots who may have the
/// old one cached are sent the new `evelocations` row.
pub fn rename(reg: &mut Registry, corp: i64, id: i64, name: &str) -> Result<(), Refused> {
    let mut s = owned(reg, corp, id)?;
    let clean = crate::item_names::clean(name);
    if !structure::valid_name(&clean) {
        return Err("A structure needs a name of at least three characters.".to_owned());
    }
    s.name = clean;
    structure::put(reg.deltas_mut(), &s);
    reg.mark_dirty();
    let loc = location_of(&s);
    for to in interested(reg, s.owner_corp, s.system) {
        reg.push(crate::item_names::CfgRow {
            to,
            location: loc.clone(),
        });
    }
    announce_lists(reg, &s, false);
    Ok(())
}

/// Run a structure's updates until nothing moves (the tick's own rule).
pub(crate) fn settle(s: &mut Structure, e: &structure::Env) {
    for _ in 0..structure::SETTLE_PASSES {
        let row = s.to_row();
        s.update(e);
        if s.to_row() == row {
            break;
        }
    }
}

/// Save a structure a call changed and tell people what it raised.
fn save_and_tell(reg: &mut Registry, mut s: Structure) {
    let events = s.take_events();
    structure::put(reg.deltas_mut(), &s);
    reg.mark_dirty();
    react(reg, s.id, &events);
    // An unanchoring that started or stopped changes the slim and the
    // browser's circle even when no event named it.
    announce_slim(reg, &s);
    announce_lists(reg, &s, false);
}

/// An unanchoring that ran out: the row and the ball go, the item goes to
/// the owner (module header, [A]).
fn finish_unanchor(reg: &mut Registry, s: &Structure) {
    // The pilots inside are put out, and what is stored goes to a station
    // that can take it, while the row still says where it was (U4).
    crate::structure_dock::evacuate(reg, s);
    let audience = interested(reg, s.owner_corp, s.system);
    structure::remove(reg.deltas_mut(), s.id);
    reg.mark_dirty();
    announce_removed(reg, s.system, s.id);
    reg.push(RemovedFromSpace {
        to: audience,
        id: s.id,
    });
    announce_lists(reg, s, true);
    deliver(reg, s);
    crate::errln!("[structure] {} unanchored in {}", s.id, s.system);
}

/// The first NPC station in `system`, from the catalogue.
pub(crate) fn station_in(reg: &Registry, system: i64) -> Option<i64> {
    reg.celestials()
        .served_in(system)
        .find(|c| c.kind == crate::celestial::Kind::Station)
        .map(|c| c.id)
}

fn deliver(reg: &mut Registry, s: &Structure) {
    let Some(ceo) = reg.corporations().get(s.owner_corp).map(|c| c.ceo_id) else {
        return;
    };
    let station = crate::structure_dock::harbour(reg, s, ceo).map(|r| r.station_id);
    let Some(station) = station else {
        crate::errln!(
            "[structure] {} has nowhere to be delivered; it stays in space",
            s.id
        );
        return;
    };
    let to = Slot::new(ceo, station, crate::model::FLAG_HANGAR);
    if let Err(e) = reg.apply_items(&[Edit::Move { item_id: s.id, to }]) {
        crate::errln!("[structure] delivering {}: {e}", s.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::space_state::{Motion, Presence};
    use crate::structure::State;

    const BOSS: i64 = 7;
    const PAL: i64 = 8;
    const CORP: i64 = 11;
    const SYSTEM: i64 = 31;
    const SHIP: i64 = 1_007;
    const DEPLOYABLE: i64 = 100_700_001;
    const TEST_RADIUS: f64 = 20_000.0;
    const T0: i64 = 134_000_000_000_000_000;

    fn content() -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        let rows = format!(
            "{}type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t{TEST_RADIUS}\t0\t\t\tInvented Hull\n",
            crate::typedata::FIXTURE
        );
        let table = crate::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn world() -> Registry {
        let ch = |id: i64, ship: i64| crate::model::Character {
            character_id: id,
            corporation_id: CORP,
            solar_system_id: Some(SYSTEM),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![ch(BOSS, SHIP), ch(PAL, SHIP + 1)],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()));
        let mut items = reg.store().items().to_vec();
        items.push(Item {
            item_id: DEPLOYABLE,
            type_id: ASTRAHUS,
            owner_id: BOSS,
            location_id: SHIP,
            flag_id: crate::sov_hub::FLAG_CARGO,
            quantity: 1,
            group_id: 1657,
            category_id: 65,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_701_000,
            ..crate::store::Snapshot::default()
        });
        reg.corporations_mut()
            .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        reg.corporations_mut().enrol_for_test(PAL, CORP, 0);
        for (n, (who, ship)) in [(BOSS, SHIP), (PAL, SHIP + 1)].into_iter().enumerate() {
            reg.roster_mut().arrive(Presence::arriving(
                who,
                ship,
                SYSTEM,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
            let n = n as i64 + 1;
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        reg
    }

    fn args(item_id: i64) -> AnchorArgs {
        AnchorArgs {
            item_id,
            x: 5_000.0,
            z: 0.0,
            yaw: 1.0,
            profile: None,
            name: "Invented Home".to_owned(),
            bio: String::new(),
            reinforce_weekday: 3,
            reinforce_hour: 12,
        }
    }

    /// Everything queued, by call and by clock: what a pilot is sent.
    fn drain(reg: &mut Registry) -> Vec<Box<dyn crate::notify::Push>> {
        let mut out: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        out.extend(reg.take_due(later));
        out
    }

    fn names(reg: &mut Registry) -> Vec<&'static str> {
        drain(reg).into_iter().map(|p| p.to_push().0).collect()
    }

    #[test]
    fn anchoring_takes_the_item_makes_the_ball_and_starts_the_timer() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).expect("anchors");
        assert_eq!(s.id, DEPLOYABLE, "the item is the structure");
        assert_eq!(s.pos, [5_000.0, 0.0, 0.0]);
        let it = reg.store().get(DEPLOYABLE).unwrap();
        assert_eq!((it.location_id, it.owner_id), (SYSTEM, CORP));
        let row = structure::get(reg.deltas(), s.id).unwrap();
        assert_eq!(row.state, State::Anchoring);
        let (a, b) = (row.timer.start.unwrap(), row.timer.end.unwrap());
        assert_eq!((b - a) / structure::SECOND, 3_600, "the world's hour");
        let sent = names(&mut reg);
        assert!(sent.contains(&"DoDestinyUpdate"), "{sent:?}");
        assert!(sent.contains(&"OnCorporationStructuresUpdated"), "{sent:?}");
        // The scene carries it, the slim carries every key.
        let balls = balls_in(&reg, SYSTEM);
        assert_eq!(balls.len(), 1);
        assert_eq!(balls[0].0.radius, TEST_RADIUS as f32);
        assert_eq!(balls[0].0.mode, Mode::Stop);
        let ex = slim_extras(&reg, &[s.id]);
        let keys: Vec<&str> = ex[0].1.iter().map(|(k, _)| *k).collect();
        for k in [
            "name",
            "state",
            "timer",
            "upkeepState",
            "repairing",
            "deployTimes",
            "docked",
            "allianceID",
            "modules",
            "dunRotation",
        ] {
            assert!(keys.contains(&k), "{k}");
        }
        assert_eq!(target(&reg, s.id).unwrap().radius, TEST_RADIUS);
        assert!(target(&reg, s.id + 1).is_none());
    }

    #[test]
    fn a_damage_change_pushes_the_bars_to_the_system() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).expect("anchors");
        let _ = reg.take_pending();
        let mut row = structure::get(reg.deltas(), s.id).unwrap();
        let e = env_for(&reg, &row, T0);
        row.set_damage(0.5, 1.0, 1.0, &e);
        let events = row.take_events();
        structure::put(reg.deltas_mut(), &row);
        react(&mut reg, s.id, &events);
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let sent: Vec<_> = reg
            .take_due(later)
            .into_iter()
            .filter(|p| p.to_push().0 == "DoDestinyUpdate")
            .filter(|p| {
                let name = format!("{:?}", crate::pyutil::bstr("OnDamageStateChange"));
                format!("{:?}", p.to_push().1).contains(&name)
            })
            .collect();
        assert_eq!(sent.len(), 1, "one bars update");
        let body = format!("{:?}", sent[0].to_push().1);
        let name = format!("{:?}", crate::pyutil::bstr("OnDamageStateChange"));
        assert!(body.contains(&name), "{body}");
        assert!(body.contains(&format!("Int({})", s.id)), "{body}");
        assert!(body.contains("Real(0.5)"), "{body}");
        let mut to = sent[0].audience().unwrap();
        to.sort();
        assert_eq!(to, vec![BOSS, PAL]);
    }

    #[test]
    fn a_damaged_structure_rides_the_bootstrap_damage_dict() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).expect("anchors");
        let mut row = structure::get(reg.deltas(), s.id).unwrap();
        let e = env_for(&reg, &row, T0);
        row.set_damage(0.5, 1.0, 1.0, &e);
        structure::put(reg.deltas_mut(), &row);
        let b = crate::destiny::bootstrap_for(&mut reg, BOSS).expect("a state");
        let (_, state) = b
            .damage
            .iter()
            .find(|(id, _)| *id == s.id)
            .expect("the structure has a damage entry");
        let body = format!("{state:?}");
        assert!(body.contains("Real(0.5)"), "{body}");
        let full = crate::destiny::DamageState::FULL.to_pyvalue();
        assert_ne!(format!("{full:?}"), body, "not the full bars");
    }

    #[test]
    fn the_type_can_switch_off_auto_repair_and_armor_phases() {
        use crate::structure_combat::{ATTR_CAN_ARMOR_PHASES, ATTR_CAN_AUTO_REPAIR};
        let with_attrs = |attrs: &str| {
            let mut c = crate::content::Content::fixture();
            let rows = format!(
                "{}type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t{TEST_RADIUS}\t0\t\t{attrs}\tInvented Hull\n",
                crate::typedata::FIXTURE
            );
            let table = crate::typedata::Table::parse(&rows).expect("rows parse");
            c.insert("typedata", crate::typedata::Installed::build(table));
            Registry::new(crate::model::SandboxConfig::default())
                .with_content(std::sync::Arc::new(c))
        };
        let row = |reg: &Registry| {
            let e = env(reg, T0);
            Structure::anchored(
                DEPLOYABLE,
                ASTRAHUS,
                CORP,
                SYSTEM,
                [0.0; 3],
                0.0,
                "Invented Home",
                1,
                &e,
            )
        };
        let off = with_attrs(&format!(
            "{ATTR_CAN_AUTO_REPAIR}=0;{ATTR_CAN_ARMOR_PHASES}=0"
        ));
        let e = env_for(&off, &row(&off), T0);
        assert!(!e.auto_repair && !e.armor_phases, "the type says 0");
        let plain = with_attrs("");
        let e = env_for(&plain, &row(&plain), T0);
        assert!(e.auto_repair && e.armor_phases, "a default type has both");
    }

    #[test]
    fn announce_added_carries_a_damage_pair() {
        let mut reg = world();
        let e = env(&reg, T0);
        let mut row = Structure::anchored(
            DEPLOYABLE,
            ASTRAHUS,
            CORP,
            SYSTEM,
            [0.0; 3],
            0.0,
            "Invented Home",
            1,
            &e,
        );
        row.set_damage(0.25, 1.0, 1.0, &e);
        let _ = reg.take_pending();
        announce_added(&mut reg, &row);
        let sent = drain(&mut reg);
        let body = format!("{:?}", sent[0].to_push().1);
        assert!(
            body.contains("Real(0.25)"),
            "the shield fraction rides the slim pair: {body}"
        );
    }

    #[test]
    fn the_anchor_time_is_the_worlds_setting() {
        let mut reg = world();
        let mut r = reg.config().rules;
        r.upwell_anchor_secs = 120;
        crate::rules::set_boot(&mut reg, r).unwrap();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).unwrap();
        let t = structure::get(reg.deltas(), s.id).unwrap().timer;
        assert_eq!((t.end.unwrap() - t.start.unwrap()) / structure::SECOND, 120);
    }

    #[test]
    fn the_clock_carries_it_to_onlining_and_dockable() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).unwrap();
        assert!(!structure::get(reg.deltas(), s.id).unwrap().can_dock());
        // Later than the timer: step the model with that clock.
        let later = crate::pyutil::filetime_now() + 2 * 3_600 * structure::SECOND;
        let e = env(&reg, later);
        let ran = structure::tick(reg.deltas_mut(), |_| e);
        for (id, ev) in ran {
            react(&mut reg, id, &ev);
        }
        let row = structure::get(reg.deltas(), s.id).unwrap();
        assert_eq!(row.state, State::OnliningVulnerable);
        assert!(row.can_dock());
        let sent = names(&mut reg);
        assert!(sent.contains(&"OnStructureStateChanged"), "{sent:?}");
        assert!(sent.contains(&"OnDockingAccessChangedForCurrentSolarSystem"));
        let _ = T0;
    }

    #[test]
    fn anchor_refuses_what_it_should() {
        let mut reg = world();
        let mut a = args(DEPLOYABLE);
        a.name = "ab".to_owned();
        assert!(anchor(&mut reg, BOSS, CORP, &a).is_err());
        let mut a = args(DEPLOYABLE);
        a.x = 900_000.0;
        assert!(
            anchor(&mut reg, BOSS, CORP, &a)
                .unwrap_err()
                .contains("far")
        );
        let mut a = args(DEPLOYABLE);
        a.reinforce_hour = 99;
        assert!(anchor(&mut reg, BOSS, CORP, &a).is_err());
        let mut a = args(DEPLOYABLE);
        a.profile = Some(424_242);
        assert!(
            anchor(&mut reg, BOSS, CORP, &a)
                .unwrap_err()
                .contains("profile")
        );
        // Not in the pilot's hold.
        assert!(anchor(&mut reg, PAL, CORP, &args(DEPLOYABLE)).is_err());
        // Nothing happened to the item.
        assert_eq!(reg.store().get(DEPLOYABLE).unwrap().location_id, SHIP);
        assert!(structure::all(reg.deltas()).is_empty());
    }

    #[test]
    fn two_structures_cannot_sit_close() {
        let mut reg = world();
        anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).unwrap();
        // A second hull, in the same hold, right beside the first.
        let changes = reg
            .apply_items(&[Edit::Create {
                type_id: ASTRAHUS,
                quantity: 1,
                to: Slot::new(BOSS, SHIP, crate::sov_hub::FLAG_CARGO),
            }])
            .expect("created");
        let second = changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("an id");
        let e = anchor(&mut reg, BOSS, CORP, &args(second)).unwrap_err();
        assert!(e.contains("1000 km"), "{e}");
    }

    #[test]
    fn unanchor_cancel_and_the_end_of_it() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).unwrap();
        // While anchoring, it cannot be decommissioned.
        assert!(unanchor(&mut reg, CORP, s.id).is_err());
        let later = crate::pyutil::filetime_now() + 2 * 3_600 * structure::SECOND;
        let e = env(&reg, later);
        for (id, ev) in structure::tick(reg.deltas_mut(), |_| e) {
            react(&mut reg, id, &ev);
        }
        unanchor(&mut reg, CORP, s.id).expect("starts");
        assert!(
            structure::get(reg.deltas(), s.id)
                .unwrap()
                .unanchoring
                .is_some()
        );
        assert!(unanchor(&mut reg, CORP, s.id).is_err(), "twice");
        cancel_unanchor(&mut reg, CORP, s.id).expect("cancels");
        assert!(
            structure::get(reg.deltas(), s.id)
                .unwrap()
                .unanchoring
                .is_none()
        );
        assert!(cancel_unanchor(&mut reg, CORP, s.id).is_err());
        // Another corp's call is refused.
        assert!(unanchor(&mut reg, CORP + 1, s.id).is_err());
        // Let it run out.
        unanchor(&mut reg, CORP, s.id).unwrap();
        let end = crate::pyutil::filetime_now() + 30 * structure::DAY;
        let e = env(&reg, end);
        let _ = names(&mut reg);
        for (id, ev) in structure::tick(reg.deltas_mut(), |_| e) {
            react(&mut reg, id, &ev);
        }
        assert!(
            structure::get(reg.deltas(), s.id).is_none(),
            "the row is gone"
        );
        assert!(balls_in(&reg, SYSTEM).is_empty());
        let sent = names(&mut reg);
        assert!(sent.contains(&"OnStructureRemovedFromSpace"), "{sent:?}");
        assert!(sent.contains(&"DoDestinyUpdate"), "RemoveBalls: {sent:?}");
    }

    #[test]
    fn rename_changes_the_row_and_the_location_name() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).unwrap();
        let _ = names(&mut reg);
        assert!(rename(&mut reg, CORP, s.id, "xy").is_err());
        rename(&mut reg, CORP, s.id, "  New\nName ").unwrap();
        assert_eq!(structure::get(reg.deltas(), s.id).unwrap().name, "New Name");
        let sent = names(&mut reg);
        assert!(sent.contains(&"OnCfgDataChanged"), "{sent:?}");
        let mut rows = Vec::new();
        overlay(&reg, &[s.id, 5], &mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].location_name, "New Name");
    }

    // -- L4 (#47): reinforce lengths and the notifications -----------------------

    /// Invented systems from `Universe::fixture`: a high-sec one, a null-sec
    /// one; and an id in the wormhole range that no table lists.
    const HIGH_SYSTEM: i64 = 39_000_001;
    const NULL_SYSTEM: i64 = 39_000_003;
    const WORMHOLE_SYSTEM: i64 = 31_990_042;
    /// A large-class type id the size table lists.
    const LARGE_TYPE: i64 = 35_833;

    fn at_system(system: i64, type_id: i64) -> Structure {
        let e = structure::Env::new(T0);
        Structure::anchored(
            5_001, type_id, CORP, system, [0.0; 3], 0.0, "Invented", 1, &e,
        )
    }

    fn received(reg: &Registry, who: i64, type_id: i64) -> Vec<crate::notifications::Notification> {
        crate::notifications::of(reg.deltas(), who)
            .into_iter()
            .filter(|n| n.type_id == type_id)
            .collect()
    }

    fn get<'a>(
        n: &'a crate::notifications::Notification,
        key: &str,
    ) -> &'a crate::notifications::Value {
        &n.data
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("{key} missing in {:?}", n.data))
            .1
    }

    fn int(v: &crate::notifications::Value) -> i64 {
        let crate::notifications::Value::Int(n) = v else {
            panic!("{v:?}")
        };
        *n
    }

    #[test]
    fn the_armor_reinforce_length_follows_size_and_security() {
        let reg = world();
        let medium = |system| reinforce_secs(&reg, &at_system(system, ASTRAHUS));
        let large = |system| reinforce_secs(&reg, &at_system(system, LARGE_TYPE));
        // A medium: high sec is longer than null sec, a wormhole shorter.
        assert_eq!(medium(HIGH_SYSTEM).0, REINFORCE_ARMOR_MEDIUM_HIGH);
        assert_eq!(medium(NULL_SYSTEM).0, REINFORCE_ARMOR_MEDIUM_LOW);
        assert_eq!(medium(WORMHOLE_SYSTEM).0, REINFORCE_ARMOR_MEDIUM_WORMHOLE);
        assert!(medium(HIGH_SYSTEM).0 > medium(NULL_SYSTEM).0);
        assert!(medium(NULL_SYSTEM).0 > medium(WORMHOLE_SYSTEM).0);
        // A large hull keeps the default armour length and takes the hull one
        // from its system.
        assert_eq!(
            large(HIGH_SYSTEM).0,
            structure::REINFORCE_DURATION_ARMOR_DEFAULT
        );
        assert_eq!(large(HIGH_SYSTEM).1, REINFORCE_HULL_HIGH);
        assert_eq!(large(NULL_SYSTEM).1, REINFORCE_HULL_LOW);
        assert_eq!(large(WORMHOLE_SYSTEM).1, REINFORCE_HULL_WORMHOLE);
        // A medium never gets a system hull length.
        assert_eq!(
            medium(HIGH_SYSTEM).1,
            structure::REINFORCE_DURATION_HULL_DEFAULT
        );
        // The tick's environment carries it.
        let mut s = at_system(HIGH_SYSTEM, ASTRAHUS);
        s.system = HIGH_SYSTEM;
        let e = env_for(&reg, &s, T0);
        assert_eq!(e.reinforce_armor_secs, REINFORCE_ARMOR_MEDIUM_HIGH);
    }

    /// Put `s` in the world and run a damage change through the same path a
    /// GM command takes: set, settle, save, react.
    fn damage(reg: &mut Registry, id: i64, fractions: (f64, f64, f64)) {
        let mut s = structure::get(reg.deltas(), id).unwrap();
        let e = env_for(reg, &s, crate::pyutil::filetime_now());
        s.set_damage(fractions.0, fractions.1, fractions.2, &e);
        settle(&mut s, &e);
        let events = s.take_events();
        structure::put(reg.deltas_mut(), &s);
        react(reg, id, &events);
    }

    fn online_structure(reg: &mut Registry, system: i64, type_id: i64) -> i64 {
        let e = env(reg, crate::pyutil::filetime_now());
        let mut s = at_system(system, type_id);
        s.set_deed(true, &e);
        s.set_state(State::ShieldVulnerable, &e);
        settle(&mut s, &e);
        s.take_events();
        structure::put(reg.deltas_mut(), &s);
        s.id
    }

    #[test]
    fn lost_shields_and_lost_armor_carry_the_timer() {
        let mut reg = world();
        let id = online_structure(&mut reg, HIGH_SYSTEM, ASTRAHUS);
        let before = crate::pyutil::filetime_now();
        damage(&mut reg, id, (0.0, 0.9, 1.0));
        let after = crate::pyutil::filetime_now();
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_eq!(s.state, State::ArmorReinforce);
        let (start, end) = (s.timer.start.unwrap(), s.timer.end.unwrap());
        // The timer is at least the medium high-sec length less the exit jitter.
        assert!(
            (end - start) / structure::SECOND
                >= REINFORCE_ARMOR_MEDIUM_HIGH - structure::REINFORCE_EXIT_JITTER_MEDIUM
        );
        let got = received(&reg, BOSS, 186);
        assert_eq!(got.len(), 1, "one lost-shields notification");
        assert_eq!(int(get(&got[0], "timestamp")), end, "the slim timer's end");
        let left = int(get(&got[0], "timeLeft"));
        assert!(left <= end - before && left >= end - after, "{left}");
        assert_eq!(int(get(&got[0], "structureID")), id);
        assert!(received(&reg, BOSS, 187).is_empty());
        assert!(
            received(&reg, PAL, 186).is_empty(),
            "a member without the role"
        );
        // Armour gone on a large hull, from the armour-vulnerable state: 187.
        let big = online_structure(&mut reg, HIGH_SYSTEM, LARGE_TYPE);
        damage(&mut reg, big, (0.0, 0.9, 1.0));
        let mut s = structure::get(reg.deltas(), big).unwrap();
        assert_eq!(s.state, State::ArmorReinforce);
        let e = env_for(&reg, &s, crate::pyutil::filetime_now());
        s.set_state(State::ArmorVulnerable, &e);
        settle(&mut s, &e);
        s.take_events();
        structure::put(reg.deltas_mut(), &s);
        damage(&mut reg, big, (0.0, 0.0, 0.9));
        let s = structure::get(reg.deltas(), big).unwrap();
        assert_eq!(s.state, State::HullReinforce);
        let (start, end) = (s.timer.start.unwrap(), s.timer.end.unwrap());
        assert!(
            (end - start) / structure::SECOND
                >= REINFORCE_HULL_HIGH - structure::REINFORCE_EXIT_JITTER_DEFAULT
        );
        let got = received(&reg, BOSS, 187);
        assert_eq!(got.len(), 1);
        assert_eq!(int(get(&got[0], "timestamp")), end);
        assert_eq!(int(get(&got[0], "structureID")), big);
        assert_eq!(received(&reg, BOSS, 186).len(), 2, "one per structure");
    }

    #[test]
    fn a_triglavian_system_reads_like_wormhole_space() {
        use crate::celestial::Universe;
        // An invented map: one low-sec system of the Triglavian faction, one
        // low-sec system of no faction.
        let row = |id: i64, faction: &str| {
            format!(
                "{id}\t19070001\t29070001\t{faction}\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented"
            )
        };
        let tri = 39_070_001;
        let plain = 39_070_002;
        let u = Universe::parse(
            &[
                row(tri, &crate::pochven::FACTION_TRIGLAVIAN.to_string()),
                row(plain, ""),
            ],
            &[],
            &[],
        )
        .expect("invented rows parse");
        let mut c = content();
        c.insert("universe", u);
        let reg = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(c));
        let medium = |system| reinforce_secs(&reg, &at_system(system, ASTRAHUS));
        let large = |system| reinforce_secs(&reg, &at_system(system, LARGE_TYPE));
        assert_eq!(medium(tri).0, REINFORCE_ARMOR_MEDIUM_WORMHOLE);
        assert_eq!(large(tri).1, REINFORCE_HULL_WORMHOLE);
        assert_eq!(medium(plain).0, REINFORCE_ARMOR_MEDIUM_LOW);
        assert_eq!(large(plain).1, REINFORCE_HULL_LOW);
    }

    #[test]
    fn the_client_deploy_window_with_weekday_255_anchors() {
        let mut reg = world();
        let mut a = args(DEPLOYABLE);
        a.reinforce_weekday = structure::NO_REINFORCEMENT_WEEKDAY;
        a.reinforce_hour = 20;
        let s = anchor(&mut reg, BOSS, CORP, &a).expect("255 is the client's own day");
        let r = structure::get(reg.deltas(), s.id).unwrap().reinforce;
        assert_eq!(
            (r.weekday, r.hour),
            (Some(structure::NO_REINFORCEMENT_WEEKDAY), Some(20))
        );
        // The hour is still checked, and a day 7..254 is still refused.
        assert!(check_window(255, 24).is_err());
        assert!(check_window(7, 5).is_err() && check_window(254, 5).is_err());
        assert!(check_window(6, 23).is_ok());
    }

    #[test]
    fn the_tick_clock_reaches_the_notification() {
        let mut reg = world();
        let id = online_structure(&mut reg, HIGH_SYSTEM, ASTRAHUS);
        let mut s = structure::get(reg.deltas(), id).unwrap();
        let e = env_for(&reg, &s, T0);
        s.set_damage(0.0, 0.9, 1.0, &e);
        settle(&mut s, &e);
        let events = s.take_events();
        structure::put(reg.deltas_mut(), &s);
        // React on the clock the state machine used, far from the wall clock.
        react_at(&mut reg, id, &events, T0);
        let end = structure::get(reg.deltas(), id).unwrap().timer.end.unwrap();
        let got = received(&reg, BOSS, 186);
        assert_eq!(got.len(), 1);
        assert_eq!(int(get(&got[0], "timeLeft")), end - T0);
    }

    #[test]
    fn the_tick_clock_reaches_a_tick_driven_notification() {
        let mut reg = world();
        let id = online_structure(&mut reg, HIGH_SYSTEM, ASTRAHUS);
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.damage = Some((0.0, 0.9, 1.0));
        structure::put(reg.deltas_mut(), &s);
        tick(&mut reg, std::time::Instant::now(), T0);
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_eq!(s.state, State::ArmorReinforce);
        let got = received(&reg, BOSS, 186);
        assert_eq!(got.len(), 1);
        assert_eq!(int(get(&got[0], "timeLeft")), s.timer.end.unwrap() - T0);
    }

    /// A state the server's clock flips reaches the pilots on the clock's
    /// queue, not in `pending`, which drains only after a client's next call.
    #[test]
    fn a_clock_driven_slim_change_is_scheduled_not_pushed() {
        let mut reg = world();
        let id = online_structure(&mut reg, SYSTEM, ASTRAHUS);
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.damage = Some((0.0, 0.9, 1.0));
        structure::put(reg.deltas_mut(), &s);
        let _ = reg.take_pending_raw();
        let queued = reg.scheduled_len();
        tick(&mut reg, std::time::Instant::now(), T0);
        assert_eq!(
            structure::get(reg.deltas(), id).unwrap().state,
            State::ArmorReinforce
        );
        let pending: Vec<&str> = reg
            .take_pending_raw()
            .into_iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert!(
            !pending.contains(&"DoDestinyUpdate"),
            "the slim change waits for a call: {pending:?}"
        );
        assert!(
            reg.scheduled_len() >= queued + 2,
            "the slim change and the bars are on the clock's queue"
        );
    }

    #[test]
    fn anchoring_and_unanchoring_notify() {
        let mut reg = world();
        let s = anchor(&mut reg, BOSS, CORP, &args(DEPLOYABLE)).expect("anchors");
        let got = received(&reg, BOSS, 182);
        assert_eq!(got.len(), 1);
        assert_eq!(int(get(&got[0], "structureID")), s.id);
        let left = int(get(&got[0], "timeLeft"));
        let world_secs = reg.config().rules.upwell_anchor_secs * structure::SECOND;
        assert!(left > 0 && left <= world_secs, "{left} of {world_secs}");
        assert!(matches!(
            get(&got[0], "ownerCorpLinkData"),
            crate::notifications::Value::Tuple(t) if t.len() == 3
        ));
        assert!(received(&reg, PAL, 182).is_empty());
        // Onlining ends: 185, once.
        let mut later = crate::pyutil::filetime_now() + 2 * 3_600 * structure::SECOND;
        tick(&mut reg, std::time::Instant::now(), later);
        assert_eq!(
            structure::get(reg.deltas(), s.id).unwrap().state,
            State::OnliningVulnerable
        );
        assert!(received(&reg, BOSS, 185).is_empty(), "not yet");
        // The deed goes in; the onlining repair starts, and its timer runs out.
        let mut row = structure::get(reg.deltas(), s.id).unwrap();
        row.set_deed(true, &env(&reg, later));
        let events = row.take_events();
        structure::put(reg.deltas_mut(), &row);
        react(&mut reg, s.id, &events);
        for _ in 0..3 {
            later += 1_000 * structure::SECOND;
            tick(&mut reg, std::time::Instant::now(), later);
        }
        assert_eq!(
            structure::get(reg.deltas(), s.id).unwrap().state,
            State::ShieldVulnerable
        );
        assert_eq!(received(&reg, BOSS, 185).len(), 1);
        tick(&mut reg, std::time::Instant::now(), later + 1);
        assert_eq!(received(&reg, BOSS, 185).len(), 1, "once");
        // Decommission: 183 with the countdown; cancelling sends nothing.
        assert!(received(&reg, BOSS, 183).is_empty());
        let before = crate::pyutil::filetime_now();
        unanchor(&mut reg, CORP, s.id).expect("starts");
        let after = crate::pyutil::filetime_now();
        let got = received(&reg, BOSS, 183);
        assert_eq!(got.len(), 1);
        let row = structure::get(reg.deltas(), s.id).unwrap();
        let left = int(get(&got[0], "timeLeft"));
        let end = row.unanchoring.unwrap();
        assert!(left <= end - before && left >= end - after, "{left}");
        assert!(left > 0);
        cancel_unanchor(&mut reg, CORP, s.id).expect("cancels");
        assert_eq!(received(&reg, BOSS, 183).len(), 1);
    }

    fn wire(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, crate::service::ServiceError> {
        let req = crate::service::Request {
            service: "structureDeployment",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        };
        crate::service::structures::dispatch(reg, &req)
    }

    #[test]
    fn the_wire_calls_anchor_rename_and_unanchor_with_the_role() {
        let mut reg = world();
        let call = vec![
            PyValue::Int(DEPLOYABLE),
            PyValue::Real(5_000.0),
            PyValue::Real(0.0),
            PyValue::Real(0.5),
            PyValue::None,
            PyValue::Unicode("Invented Keep".to_owned()),
            PyValue::Unicode(String::new()),
            PyValue::Int(3),
            PyValue::Int(12),
            PyValue::None,
        ];
        // A plain member may not.
        assert!(wire(&mut reg, PAL, "Anchor", &call).is_err());
        assert!(structure::all(reg.deltas()).is_empty());
        assert_eq!(wire(&mut reg, BOSS, "Anchor", &call), Ok(PyValue::None));
        assert_eq!(structure::all(reg.deltas()).len(), 1);
        let id = [PyValue::Int(DEPLOYABLE)];
        assert!(
            wire(
                &mut reg,
                PAL,
                "RenameStructure",
                &[id[0].clone(), PyValue::Unicode("Other".into())]
            )
            .is_err()
        );
        assert_eq!(
            wire(
                &mut reg,
                BOSS,
                "RenameStructure",
                &[id[0].clone(), PyValue::Unicode("Other Name".into())]
            ),
            Ok(PyValue::None)
        );
        assert!(
            wire(&mut reg, BOSS, "Unanchor", &id).is_err(),
            "still anchoring"
        );
        assert!(wire(&mut reg, BOSS, "Nope", &id).is_err());
        assert!(wire(&mut reg, BOSS, "Anchor", &[]).is_err());
    }
}
