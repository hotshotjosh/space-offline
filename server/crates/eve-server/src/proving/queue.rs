//! **The Proving Grounds queue** (issue #64, PG-1c): who is waiting for a
//! match, and why a join is refused.
//!
//! The wire arms are `service/proving.rs` (`JoinPVPQueue`, `LeavePVPQueue`);
//! the rules are here. There is no matchmaker yet (PG-2): a pilot who joins
//! waits until they leave, dock, jump, change ship, lose the key, log off or
//! the event ends.
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
//!   not at the queue (`docs/re/proving-grounds.md` §8 `[A]` 3). Falsified by
//!   the owner ruling that a join costs the filament.
//! - **A filament must be in the pilot's ship** ([`KEY_NOT_IN_SHIP`]): the
//!   window is opened from the ship's cargo. Falsified by a client run where
//!   a filament in a hangar opens the window and the join is wanted.
//! - **A fleet format needs the pilot in a fleet** ([`FLEET_NEEDED`]), nothing more:
//!   no size or role check until teams are formed (PG-2). Falsified by TQ
//!   material, or a client label text saying otherwise.
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
use crate::proving::{Catalogue, Event};
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
}

/// Everyone waiting, at most one entry per pilot, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    entries: Vec<Entry>,
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
}

/// Why `e` must leave the queue, if it must **[A]** (module doc). Docked
/// first (16, the pilot's id), then a changed system or ship or a lost key
/// (22), then an event that ended or changed (33).
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
    let j = JoinFacts {
        key: key_facts(reg, char_id, item_id),
        asked_type: type_id,
        fleet_format: event.as_ref().is_some_and(|e| e.match_type.fleet_size > 1),
        event_active: event.is_some(),
        already_queued: q.of(char_id).is_some(),
    };
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
    });
    Ok(0)
}

/// What is true of `e`'s pilot now.
pub fn now_of(reg: &Registry, e: &Entry, ft: i64) -> Now {
    let here = reg.roster().get(e.char_id);
    Now {
        docked: here.is_none() && !reg.jumps().contains(e.char_id),
        system_id: here.map_or(0, |p| p.solar_system_id),
        ship_id: reg.character(e.char_id).map_or(0, |c| c.ship_id),
        event: event_of(reg, ft, e.type_id).map(|ev| (ev.match_type.id, ev.schedule.id)),
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

/// `tick::STAGES` row (coordinator): [`sweep`] over [`SLOT`]'s state. One
/// comparison while nobody waits.
pub fn tick(reg: &mut Registry, _: Instant, now: i64) {
    if reg.ext::<Queue>().is_empty() {
        return;
    }
    let mut q = std::mem::take(reg.ext_mut::<Queue>());
    sweep(reg, &mut q, now);
    *reg.ext_mut::<Queue>() = q;
}

/// `session_hooks::ON_LOGOFF` row (coordinator): a pilot who logged off
/// leaves the queue. Nobody is told: there is no client to tell.
pub fn on_logoff(reg: &mut Registry, who: i64) {
    reg.ext_mut::<Queue>().remove(who);
}

#[cfg(test)]
mod tests;
