//! **The Proving Grounds queue** (issue #64, PG-1c): who is waiting for a
//! match, and why a join is refused.
//!
//! The wire arms are `service/proving.rs` (`JoinPVPQueue`, `LeavePVPQueue`);
//! the rules are here. A pilot who joins waits until a match forms
//! ([`match_round`], with [`super::matchmaker`]) or they leave, dock, jump,
//! change ship, change fleet, lose the key, log off or the event ends.
//!
//! ## What the client does
//!
//! - `JoinPVPQueue(itemID, typeID)` answers `estimatedQueueTime`, a number;
//!   below 1 the window says "unknown" **[V]**
//!   `pvpFilamentController._JoinQueue`. This server always answers **0**.
//! - A `UserError('PVPJumpError', {'errors': [(code, args), ...]})` puts the
//!   list in the window; any other `UserError` is re-raised **[V]**
//!   `pvpFilamentController.JoinQueue`.
//! - `LeavePVPQueue(itemID, typeID)` is called by the leave button and by
//!   closing the window while queued; `LeaveQueue` does not catch an error,
//!   so Leave **never raises** **[V]** `pvpFilamentController.LeaveQueue`,
//!   `Close`. Docking marks the window finished and does *not* call it
//!   **[V]** (`docs/re/proving-grounds.md` §5.1), so [`sweep`] drops a pilot
//!   who docked.
//! - The client checks the base list itself, in this order **[V]**
//!   `clientPVPfilamentValidator.perform_base_validation_checks`: ship found
//!   (21), docked (16), cloaked (15), invulnerable (17), self-destructing
//!   (18), tethered (19), in warp (14), in an abyss system (1), session
//!   change in progress (10), PvP timer (8). [`join_errors`] re-runs them in
//!   that order (a modified client skips them), then adds the server's.
//!
//! ## The server's checks, after the client's
//!
//! **[I]** The order after the client's list is ours (`_validate_key` exists
//! in the client but is not called). It follows the abyss validator, which
//! runs the fleet check before the key (`abyss/common/validator`
//! `perform_deploy_validation_checks`, ~:294): a format with more than one
//! pilot per fleet needs a fleet (23); then the key: not found (6), not in
//! the ship (5), not a filament or not the type asked for (2), not the
//! pilot's (3), empty (11); then no active event for the filament (33), then
//! already queued (32). Falsified by a client label text or TQ material that
//! shows another order.
//!
//! The queue is in memory only and empties on a restart, which is harmless:
//! a restart drops every session, and a pilot must join again.
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **A join never spends the key**: the filament is consumed at the match,
//!   one per pilot, with no refund (`docs/re/proving-grounds.md` §8 `[A]` 3,
//!   owner Q1, 2026-11-24). Falsified by the owner ruling that a join costs
//!   the filament or that a match that never opens refunds it.
//! - **A filament must be in the pilot's ship** ([`KEY_NOT_IN_SHIP`]): the
//!   window is opened from the ship's cargo. Falsified by a client run where
//!   a filament in a hangar opens the window and the join is wanted.
//! - **A fleet format needs the pilot in a fleet** ([`FLEET_NEEDED`]) of at
//!   most `fleet_size` pilots ([`FLEET_TOO_BIG`], the size as the argument);
//!   a fleet of fewer waits for the rest to queue. Falsified by TQ material,
//!   or a client label text saying otherwise.
//! - **The hull must be in the match type's `ship_restrictions` typelist**
//!   ([`INVALID_SHIP_TYPE`], the pilot as the argument), at the join and again
//!   at the match; a type or group over the match type's cap in a team
//!   fails the team at the match (`matchmaker::composition_errors`). Fit
//!   rules (meta levels, banned and limited modules) are PG-2c. Falsified by
//!   a client label text that names other arguments.
//! - **A match-time failure leaves only the failing pilot's team**: the
//!   failing pilot is told why, their teammates [`FLEET_SIZE_CHANGED`] (31);
//!   every other team stays queued. Falsified by TQ material.
//! - **A pilot who moves (another system or ship) is dropped with
//!   [`DROP_MOVED`]**, a docked one with [`DROP_DOCKED`] and one whose event
//!   ended with [`DROP_ENDED`]. Falsified by a client label text that names a better code.
//!
//! ## Wiring
//!
//! [`SLOT`] (`world_ext::SLOTS`), [`tick`] (`tick::STAGES`) and [`on_logoff`]
//! (`session_hooks::ON_LOGOFF`) are coordinator lines. Everything else here
//! takes the [`Queue`] or the `Registry` explicitly.

use crate::abyss::Refusal;
use crate::notify::Push;
use crate::proving::matchmaker::{self, Team};
use crate::proving::{Catalogue, Event, MatchType};
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "proving_queue",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Queue::default())
}

/// `PVPJumpError`'s class constants **[V]** `pvpFilaments/error.py`.
pub mod err {
    pub const IN_ABYSS_SYSTEM: i64 = 1;
    pub const INVALID_KEY_TYPE: i64 = 2;
    pub const INVALID_OWNER: i64 = 3;
    pub const INVALID_SHIP_TYPE: i64 = 4;
    pub const ITEM_NOT_REACHABLE: i64 = 5;
    pub const KEY_NOT_FOUND: i64 = 6;
    pub const PVP_TIMER_ACTIVE: i64 = 8;
    pub const SESSION_CHANGE_IN_PROGRESS: i64 = 10;
    pub const INVALID_KEY_AMOUNT: i64 = 11;
    pub const SHIP_IN_WARP: i64 = 14;
    pub const SHIP_IS_CLOAKED: i64 = 15;
    pub const SHIP_IS_DOCKED: i64 = 16;
    pub const SHIP_IS_INVULNERABLE: i64 = 17;
    pub const SHIP_IS_SELF_DESTRUCTING: i64 = 18;
    pub const SHIP_IS_TETHERED: i64 = 19;
    pub const SHIP_NOT_FOUND: i64 = 21;
    pub const UNKNOWN_ERROR: i64 = 22;
    pub const SHIP_NOT_IN_FLEET: i64 = 23;
    pub const TOO_MANY_IN_FLEET: i64 = 24;
    pub const FLEET_SIZE_CHANGED: i64 = 31;
    pub const ALREADY_IN_QUEUE: i64 = 32;
    pub const EVENT_NOT_ACTIVE: i64 = 33;
}

/// **[A]** A pilot who docked is dropped with `SHIP_IS_DOCKED` (16), the
/// code the client's own check uses. Falsified by a client label text that
/// names a better code for a queued pilot who docked.
pub const DROP_DOCKED: i64 = err::SHIP_IS_DOCKED;

/// **[A]** A pilot who changed system or ship, or lost the key, is dropped
/// with `UNKNOWN_ERROR` (22). Falsified by a client label text that names a
/// better code.
pub const DROP_MOVED: i64 = err::UNKNOWN_ERROR;

/// **[A]** A pilot whose event ended or changed is dropped with
/// `EVENT_NOT_ACTIVE` (33). Falsified by a client label text that names a
/// better code.
pub const DROP_ENDED: i64 = err::EVENT_NOT_ACTIVE;

/// **[A]** A filament outside the pilot's ship is refused with
/// `ITEM_NOT_REACHABLE` (5): the window is opened from the ship's cargo.
/// Falsified by a client run where a filament in a hangar opens the window
/// and the join is wanted.
pub const KEY_NOT_IN_SHIP: i64 = err::ITEM_NOT_REACHABLE;

/// **[A]** A format with more than one pilot per fleet refuses a pilot
/// outside a fleet with `SHIP_NOT_IN_FLEET` (23), nothing more (no size or
/// role check until teams are formed, PG-2). Falsified by TQ material, or a
/// client label text saying otherwise.
pub const FLEET_NEEDED: i64 = err::SHIP_NOT_IN_FLEET;

/// A hull outside the match type's `ship_restrictions` list is refused with
/// `INVALID_SHIP_TYPE` (4), the pilot as the argument **[V]** code and
/// argument shape (§4), **[A]** that the ship list is the one named by
/// `ship_restrictions`. Falsified by a label text that names another list.
pub const INVALID_SHIP_TYPE: i64 = err::INVALID_SHIP_TYPE;

/// A fleet larger than the format's `fleet_size` is refused with
/// `TOO_MANY_IN_FLEET` (24), the format's size as the argument **[V]** (§4).
pub const FLEET_TOO_BIG: i64 = err::TOO_MANY_IN_FLEET;

/// A teammate who failed at the match, or a pilot whose fleet changed while
/// queued, leaves with `FLEET_SIZE_CHANGED` (31) **[V]** (§4, "matchmaking
/// failure"). **[A]** for the fleet-changed use. Falsified by a label text
/// that names a better code.
pub const FLEET_SIZE_CHANGED: i64 = err::FLEET_SIZE_CHANGED;

/// The filament item group **[V]** `docs/content/schema.md`
/// (`proving/match_types.tsv`: "every filament type is group 4050").
pub const FILAMENT_GROUP: i64 = 4050;

/// What the client's base checks read about a pilot and their ship.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub ship_found: bool,
    pub docked: bool,
    pub cloaked: bool,
    pub invulnerable: bool,
    pub self_destructing: bool,
    pub tethered: bool,
    pub warping: bool,
    pub in_abyss_system: bool,
    pub session_change: bool,
    pub pvp_timer: bool,
    pub in_fleet: bool,
}

/// The item a join names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyFacts {
    pub type_id: i64,
    pub group_id: i64,
    pub owned: bool,
    /// In the pilot's ship.
    pub in_ship: bool,
    pub stack: i64,
}

/// What a join is checked against besides the pilot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JoinFacts {
    /// `None`: no such item.
    pub key: Option<KeyFacts>,
    /// `typeID` as the client sent it.
    pub asked_type: i64,
    /// The format has more than one pilot per fleet.
    pub fleet_format: bool,
    /// The pilot's fleet is larger than the format's `fleet_size`: the size.
    pub fleet_over: Option<i64>,
    /// The pilot's hull is on the match type's ship list (true when it has none).
    pub ship_allowed: bool,
    /// The filament has an active event.
    pub event_active: bool,
    pub already_queued: bool,
}

/// Every reason a join is refused, in the client's order and then ours
/// (module doc). Empty means the pilot may queue.
pub fn join_errors(char_id: i64, f: &Facts, j: &JoinFacts) -> Vec<Refusal> {
    let who = || vec![char_id];
    let mut e: Vec<Refusal> = Vec::new();
    if !f.ship_found {
        e.push((err::SHIP_NOT_FOUND, vec![]));
    }
    let base = [
        (f.docked, err::SHIP_IS_DOCKED),
        (f.cloaked, err::SHIP_IS_CLOAKED),
        (f.invulnerable, err::SHIP_IS_INVULNERABLE),
        (f.self_destructing, err::SHIP_IS_SELF_DESTRUCTING),
        (f.tethered, err::SHIP_IS_TETHERED),
        (f.warping, err::SHIP_IN_WARP),
        (f.in_abyss_system, err::IN_ABYSS_SYSTEM),
        (f.session_change, err::SESSION_CHANGE_IN_PROGRESS),
        (f.pvp_timer, err::PVP_TIMER_ACTIVE),
    ];
    for (hit, code) in base {
        if hit {
            e.push((code, who()));
        }
    }
    if j.fleet_format && !f.in_fleet {
        e.push((FLEET_NEEDED, vec![]));
    }
    if let Some(size) = j.fleet_over {
        e.push((FLEET_TOO_BIG, vec![size]));
    }
    if !j.ship_allowed {
        e.push((INVALID_SHIP_TYPE, who()));
    }
    match &j.key {
        None => e.push((err::KEY_NOT_FOUND, vec![])),
        Some(k) => {
            if !k.in_ship {
                e.push((KEY_NOT_IN_SHIP, vec![k.type_id]));
            }
            if k.group_id != FILAMENT_GROUP || k.type_id != j.asked_type {
                e.push((err::INVALID_KEY_TYPE, who()));
            }
            if !k.owned {
                e.push((err::INVALID_OWNER, who()));
            }
            if k.stack < 1 {
                e.push((err::INVALID_KEY_AMOUNT, vec![]));
            }
        }
    }
    if !j.event_active {
        e.push((err::EVENT_NOT_ACTIVE, vec![]));
    }
    if j.already_queued {
        e.push((err::ALREADY_IN_QUEUE, vec![]));
    }
    e
}

/// `[(code, (args,)), ...]`, the list in `PVPJumpError` and in
/// `OnMatchMakingFailure` **[V]** (`docs/re/proving-grounds.md` §4).
pub fn errors_value(errors: &[Refusal]) -> PyValue {
    PyValue::List(
        errors
            .iter()
            .map(|(code, args)| {
                PyValue::Tuple(vec![
                    PyValue::Int(*code),
                    PyValue::Tuple(args.iter().map(|a| PyValue::Int(*a)).collect()),
                ])
            })
            .collect(),
    )
}

/// `UserError('PVPJumpError', {'errors': [...]})`: what a refused join raises.
pub fn raise(errors: &[Refusal]) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        crate::proving::ERROR,
        crate::pyutil::pypairs(vec![("errors", errors_value(errors))]),
    ))
}

/// One waiting pilot.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub char_id: i64,
    pub item_id: i64,
    pub type_id: i64,
    pub match_type_id: i64,
    pub schedule_id: i64,
    pub ship_id: i64,
    pub system_id: i64,
    pub since: i64,
    /// The fleet the pilot queued in; 0 outside one.
    pub fleet_id: i64,
}

/// Everyone waiting, at most one entry per pilot, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    entries: Vec<Entry>,
    /// The last reason a formed match could not open was logged (once, not
    /// every tick).
    warned: bool,
}

impl Queue {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn of(&self, char_id: i64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.char_id == char_id)
    }

    /// Queue `e`; `false` (and nothing changes) when the pilot is queued.
    pub fn add(&mut self, e: Entry) -> bool {
        if self.of(e.char_id).is_some() {
            return false;
        }
        self.entries.push(e);
        true
    }

    /// Remove the pilot's entry, whatever item the call named. Idempotent:
    /// `false` when there was none.
    pub fn remove(&mut self, char_id: i64) -> bool {
        let n = self.entries.len();
        self.entries.retain(|e| e.char_id != char_id);
        self.entries.len() != n
    }

    /// Drop every entry `check` gives a reason for; the dropped, with their
    /// reasons, in queue order.
    pub fn sweep(
        &mut self,
        mut check: impl FnMut(&Entry) -> Option<Refusal>,
    ) -> Vec<(Entry, Refusal)> {
        let mut dropped = Vec::new();
        let mut kept = Vec::new();
        for e in std::mem::take(&mut self.entries) {
            match check(&e) {
                Some(r) => dropped.push((e, r)),
                None => kept.push(e),
            }
        }
        self.entries = kept;
        dropped
    }
}

/// What is true of a queued pilot now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Now {
    pub docked: bool,
    pub system_id: i64,
    pub ship_id: i64,
    /// `(match type, schedule)` of the filament's active event, if any.
    pub event: Option<(i64, i64)>,
    pub key_held: bool,
    /// The pilot's fleet now (0: none).
    pub fleet_id: i64,
    /// The event's format has more than one pilot per fleet.
    pub fleet_format: bool,
}

/// Why `e` must leave the queue, if it must **[A]** (module doc). Docked
/// first (16, the pilot's id), then a changed system or ship or a lost key
/// (22), then an event that ended or changed (33), then a fleet that changed
/// in a fleet format (31).
pub fn drop_reason(e: &Entry, now: &Now) -> Option<Refusal> {
    if now.docked {
        return Some((DROP_DOCKED, vec![e.char_id]));
    }
    if now.system_id != e.system_id || now.ship_id != e.ship_id || !now.key_held {
        return Some((DROP_MOVED, vec![]));
    }
    if now.event != Some((e.match_type_id, e.schedule_id)) {
        return Some((DROP_ENDED, vec![]));
    }
    if now.fleet_format && now.fleet_id != e.fleet_id {
        return Some((FLEET_SIZE_CHANGED, vec![]));
    }
    None
}

/// `OnMatchMakingFailure(errors)` to one pilot **[V]**
/// `pvpFilamentController.py:299`: shown, and the pilot is out of the queue.
#[derive(Debug)]
pub struct MatchFailure {
    pub to: i64,
    pub errors: Vec<Refusal>,
}

impl Push for MatchFailure {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnMatchMakingFailure", vec![errors_value(&self.errors)])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The world side
// ---------------------------------------------------------------------------

fn cat(reg: &Registry) -> &Catalogue {
    reg.content().get::<Catalogue>()
}

/// What the client's base checks read, from the world.
pub fn facts(reg: &Registry, char_id: i64, ft: i64) -> Facts {
    let here = reg.roster().get(char_id).copied();
    let ship = reg.character(char_id).map_or(0, |c| c.ship_id);
    let system = here.map_or(0, |p| p.solar_system_id);
    let motion = here.map(|p| p.motion);
    Facts {
        ship_found: ship != 0 && reg.store().get(ship).is_some(),
        // Out of the roster and not between gates: in a station.
        docked: here.is_none() && !reg.jumps().contains(char_id),
        cloaked: crate::cloak::is_cloaked(reg, ship),
        invulnerable: crate::invuln::character_protected(reg, char_id, ft),
        self_destructing: crate::self_destruct::is_running(reg, ship),
        // [I] the server has no structure tether yet (`Motion::tether` is
        // follow/orbit, not a structure's tether); falsified by a
        // structure-tether lane.
        tethered: false,
        warping: motion.is_some_and(|m| crate::dungeon_tracking::in_warp(&m)),
        // [I] the 32xxxxxx range stands in for the client's ABYSS_REGIONS
        // systems (celestial.rs).
        in_abyss_system: crate::celestial::VOID_SYSTEMS.contains(&system),
        session_change: reg.jumps().contains(char_id),
        pvp_timer: reg
            .combat()
            .crimewatch()
            .expiry(char_id, crate::crimewatch::Timer::Pvp, ft)
            .is_some(),
        in_fleet: reg.fleets().fleet_of(char_id).is_some(),
    }
}

fn key_facts(reg: &Registry, char_id: i64, item_id: i64) -> Option<KeyFacts> {
    let item = reg.store().get(item_id)?;
    let ship = reg.character(char_id).map(|c| c.ship_id);
    Some(KeyFacts {
        type_id: item.type_id,
        group_id: item.group_id,
        owned: item.owner_id == char_id,
        in_ship: ship == Some(item.location_id),
        stack: item.quantity,
    })
}

fn event_of(reg: &Registry, ft: i64, filament: i64) -> Option<Event> {
    cat(reg).by_filament(ft, filament)
}

/// Whether `ship_id`'s type is on the match type's `ship_restrictions` list
/// (**[I]** true when the match type has none, or the bundle lacks the list,
/// as `abyss::ship_allowed` does). Falsified by a client run that refuses a
/// hull this accepts.
fn ship_ok(reg: &Registry, mt: &MatchType, ship_id: i64) -> bool {
    let Some(list_id) = mt.ship_restrictions else {
        return true;
    };
    let Some(list) = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.typelists.get(&list_id))
    else {
        return true;
    };
    let Some(item) = reg.store().get(ship_id) else {
        return false;
    };
    list.contains(item.type_id, item.group_id, item.category_id)
}

/// What a join (or the match's re-check) asks about the key, the event, the
/// fleet and the hull of `char_id`.
fn join_facts(
    reg: &Registry,
    event: Option<&Event>,
    char_id: i64,
    item_id: i64,
    type_id: i64,
    already_queued: bool,
) -> JoinFacts {
    let fleet_size = event.map_or(1, |e| e.match_type.fleet_size);
    let ship = reg.character(char_id).map_or(0, |c| c.ship_id);
    let members = reg
        .fleets()
        .fleet_of(char_id)
        .map_or(0, |f| i64::try_from(f.members.len()).unwrap_or(i64::MAX));
    JoinFacts {
        key: key_facts(reg, char_id, item_id),
        asked_type: type_id,
        fleet_format: fleet_size > 1,
        fleet_over: (fleet_size > 1 && members > fleet_size).then_some(fleet_size),
        ship_allowed: event.is_none_or(|e| ship_ok(reg, &e.match_type, ship)),
        event_active: event.is_some(),
        already_queued,
    }
}

fn fleet_id_of(reg: &Registry, char_id: i64) -> i64 {
    reg.fleets().fleet_of(char_id).map_or(0, |f| f.fleet_id)
}

/// `JoinPVPQueue(itemID, typeID)` for `char_id`: the refusals, or `Ok(0)`,
/// the estimate "unknown". **Never spends the key** **[A]**.
pub fn join(
    reg: &Registry,
    q: &mut Queue,
    char_id: i64,
    item_id: i64,
    type_id: i64,
    ft: i64,
) -> Result<i64, Vec<Refusal>> {
    let f = facts(reg, char_id, ft);
    let event = event_of(reg, ft, type_id);
    let j = join_facts(
        reg,
        event.as_ref(),
        char_id,
        item_id,
        type_id,
        q.of(char_id).is_some(),
    );
    let errors = join_errors(char_id, &f, &j);
    if !errors.is_empty() {
        return Err(errors);
    }
    let (Some(e), Some(p)) = (event, reg.roster().get(char_id).copied()) else {
        return Err(vec![(err::UNKNOWN_ERROR, vec![])]);
    };
    q.add(Entry {
        char_id,
        item_id,
        type_id,
        match_type_id: e.match_type.id,
        schedule_id: e.schedule.id,
        ship_id: reg.character(char_id).map_or(0, |c| c.ship_id),
        system_id: p.solar_system_id,
        since: ft,
        fleet_id: fleet_id_of(reg, char_id),
    });
    Ok(0)
}

/// What is true of `e`'s pilot now.
pub fn now_of(reg: &Registry, e: &Entry, ft: i64) -> Now {
    let here = reg.roster().get(e.char_id);
    let event = event_of(reg, ft, e.type_id);
    Now {
        docked: here.is_none() && !reg.jumps().contains(e.char_id),
        system_id: here.map_or(0, |p| p.solar_system_id),
        ship_id: reg.character(e.char_id).map_or(0, |c| c.ship_id),
        fleet_format: event
            .as_ref()
            .is_some_and(|ev| ev.match_type.fleet_size > 1),
        event: event.map(|ev| (ev.match_type.id, ev.schedule.id)),
        fleet_id: fleet_id_of(reg, e.char_id),
        key_held: key_facts(reg, e.char_id, e.item_id)
            .is_some_and(|k| k.in_ship && k.owned && k.stack >= 1 && k.type_id == e.type_id),
    }
}

/// Drop every queued pilot who must go (see [`drop_reason`]) and tell them
/// with `OnMatchMakingFailure`. Returns the pilots dropped.
pub fn sweep(reg: &mut Registry, q: &mut Queue, ft: i64) -> Vec<i64> {
    let dropped = q.sweep(|e| drop_reason(e, &now_of(reg, e, ft)));
    let mut gone = Vec::new();
    for (e, reason) in dropped {
        gone.push(e.char_id);
        reg.schedule(
            Duration::ZERO,
            MatchFailure {
                to: e.char_id,
                errors: vec![reason],
            },
        );
    }
    gone
}

/// The queue slot's `JoinPVPQueue`: [`join`] over [`SLOT`]'s state.
pub fn join_slot(
    reg: &mut Registry,
    char_id: i64,
    item_id: i64,
    type_id: i64,
    ft: i64,
) -> Result<i64, Vec<Refusal>> {
    let mut q = std::mem::take(reg.ext_mut::<Queue>());
    let r = join(reg, &mut q, char_id, item_id, type_id, ft);
    *reg.ext_mut::<Queue>() = q;
    r
}

/// The queue slot's `LeavePVPQueue`; `true` when the pilot was queued.
pub fn leave_slot(reg: &mut Registry, char_id: i64) -> bool {
    reg.ext_mut::<Queue>().remove(char_id)
}

/// `OnMatchFound()` **[V]** `pvpFilamentController.py:294`: the window
/// closes. No arguments.
#[derive(Debug)]
pub struct MatchFound {
    pub to: i64,
}

impl Push for MatchFound {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnMatchFound", vec![])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnProvingGroundsMatchFound()` **[V]** `audioService.py:1036`: the
/// "opponent matched" sound. No arguments.
#[derive(Debug)]
pub struct ArenaSound {
    pub to: i64,
}

impl Push for ArenaSound {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnProvingGroundsMatchFound", vec![])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// What a match round asks of the arena: make it (`Err` for the log: no free
/// system, no room), and, once the keys are paid and the pilots told, send
/// the pilots in. Parameters so the round is testable on its own.
pub type Open = fn(&mut Registry, &Event, &[Team], i64) -> Result<u64, String>;
pub type Launch = fn(&mut Registry, u64, i64);

/// A pilot's errors at the match: the queue's own drop reasons, then every
/// join check run again (a modified client, a key that moved, a ship that
/// changed) with the pilot not counted as queued.
fn pilot_errors(reg: &Registry, ev: &Event, e: &Entry, ft: i64) -> Vec<Refusal> {
    if let Some(r) = drop_reason(e, &now_of(reg, e, ft)) {
        return vec![r];
    }
    let f = facts(reg, e.char_id, ft);
    let j = join_facts(reg, Some(ev), e.char_id, e.item_id, e.type_id, false);
    join_errors(e.char_id, &f, &j)
}

fn fail(reg: &mut Registry, q: &mut Queue, who: i64, errors: Vec<Refusal>) {
    q.remove(who);
    reg.schedule(Duration::ZERO, MatchFailure { to: who, errors });
}

/// The key of every pilot of `teams`, one each **[A]** (module doc): a stack
/// of one is destroyed, a larger stack is reduced by one.
fn consume_keys(reg: &mut Registry, teams: &[Team]) {
    for e in teams.iter().flat_map(|t| &t.members) {
        let stack = reg.store().get(e.item_id).map_or(0, |i| i.quantity);
        let edit = if stack <= 1 {
            crate::item::Edit::Destroy { item_id: e.item_id }
        } else {
            crate::item::Edit::Consume {
                item_id: e.item_id,
                quantity: 1,
            }
        };
        if let Err(err) = reg.apply_items(&[edit]) {
            crate::errln!(
                "[proving] key {} of {} not taken: {err:?}",
                e.item_id,
                e.char_id
            );
        }
    }
}

/// One step for one event: a failed pilot (and their team) leaves, or a match
/// forms and starts. `true` when something happened, so the caller looks again.
fn round_one(
    reg: &mut Registry,
    q: &mut Queue,
    ev: &Event,
    ft: i64,
    open: Open,
    launch: Launch,
) -> bool {
    let teams = matchmaker::teams(q.entries(), &ev.match_type, ev.schedule.id);
    let formed = matchmaker::form(&teams, &ev.match_type);
    if formed.is_empty() {
        return false;
    }
    // Every pilot is checked again; a team with a failing pilot is out, the
    // failing pilot told why and the teammates told the team broke.
    let mut broke = false;
    for t in &formed {
        let bad: Vec<(i64, Vec<Refusal>)> = t
            .members
            .iter()
            .map(|e| (e.char_id, pilot_errors(reg, ev, e, ft)))
            .filter(|(_, errors)| !errors.is_empty())
            .collect();
        if !bad.is_empty() {
            for e in &t.members {
                let errors = match bad.iter().find(|(c, _)| *c == e.char_id) {
                    Some((_, errors)) => errors.clone(),
                    None => vec![(FLEET_SIZE_CHANGED, vec![])],
                };
                fail(reg, q, e.char_id, errors);
            }
            broke = true;
            continue;
        }
        let hulls: Vec<(i64, i64, i64)> = t
            .members
            .iter()
            .filter_map(|e| {
                let ship = reg.character(e.char_id)?.ship_id;
                let i = reg.store().get(ship)?;
                Some((e.char_id, i.type_id, i.group_id))
            })
            .collect();
        let errors = matchmaker::composition_errors(&hulls, &ev.match_type);
        if !errors.is_empty() {
            for e in &t.members {
                fail(reg, q, e.char_id, errors.clone());
            }
            broke = true;
        }
    }
    if broke {
        return true;
    }
    let arena = match open(reg, ev, &formed, ft) {
        Ok(id) => id,
        Err(why) => {
            if !q.warned {
                q.warned = true;
                crate::errln!(
                    "[proving] a match of event {} is formed but cannot open: {why}",
                    ev.event_id()
                );
            }
            return false;
        }
    };
    q.warned = false;
    consume_keys(reg, &formed);
    for e in formed.iter().flat_map(|t| &t.members) {
        q.remove(e.char_id);
        reg.schedule(Duration::ZERO, MatchFound { to: e.char_id });
        reg.schedule(Duration::ZERO, ArenaSound { to: e.char_id });
    }
    launch(reg, arena, ft);
    true
}

/// **A match round**: for each waiting event, in the order of its oldest
/// waiting pilot, form the matches that can be formed ([`matchmaker`]) and
/// start them through `open` and `launch`.
pub fn match_round(reg: &mut Registry, q: &mut Queue, ft: i64, open: Open, launch: Launch) {
    let mut events: Vec<(i64, i64, i64)> = Vec::new();
    for e in q.entries() {
        let k = (e.match_type_id, e.schedule_id, e.type_id);
        if !events.contains(&k) {
            events.push(k);
        }
    }
    for (m, s, t) in events {
        let Some(ev) = event_of(reg, ft, t) else {
            continue;
        };
        if ev.match_type.id != m || ev.schedule.id != s {
            continue;
        }
        while round_one(reg, q, &ev, ft, open, launch) {}
    }
}

/// `tick::STAGES` row (coordinator): [`sweep`], then [`match_round`], over
/// [`SLOT`]'s state. One comparison while nobody waits.
pub fn tick(reg: &mut Registry, _: Instant, now: i64) {
    if reg.ext::<Queue>().is_empty() {
        return;
    }
    let mut q = std::mem::take(reg.ext_mut::<Queue>());
    sweep(reg, &mut q, now);
    match_round(
        reg,
        &mut q,
        now,
        crate::proving::arena::open,
        crate::proving::arena::launch,
    );
    *reg.ext_mut::<Queue>() = q;
}

/// `session_hooks::ON_LOGOFF` row (coordinator): a pilot who logged off
/// leaves the queue. Nobody is told: there is no client to tell.
pub fn on_logoff(reg: &mut Registry, who: i64) {
    reg.ext_mut::<Queue>().remove(who);
}

#[cfg(test)]
mod tests;
