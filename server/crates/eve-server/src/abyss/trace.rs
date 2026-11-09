//! **The run's trace** (issue #62, ABYSS-5): the ball the filament leaves in
//! the pilot's own space.
//!
//! ## What the client says **[V]**
//!
//! `typeKnownToAbyssalBeacon` 47458 and `typeKnownToAbyssalSuspectBeacon`
//! 47833 are both "Abyssal Trace", group 1991 `groupAbyssalTraces`
//! (`inventorycommon/const.py:1099,1860-1861`); the co-op entrance gate is in
//! the same group. The client draws a 1991 ball as a `WarpGate`
//! (`spaceObject/repository.py:174`), lists it on the overview and scans it.
//! `effects.JumpOutAbyssal` (`Jump.py:658-715`) reads `ballIDs[1]`: a ball of
//! one of the two trace types is torn (`effects.AbyssalSpaceTear`) and the
//! jumper gets `JumpTransitionAbyssal`; any other target is the gate's tunnel.
//! The suspect type is the key's mark: it is the one for a key above the
//! system's limit ([`super::Facts::suspicious`]).
//!
//! ## What stands where **[I]**
//!
//! A solo run's trace stands at the origin, placed when the pilot goes in
//! ([`place_at`] from `join`, before the effect that names it). A fleet run's
//! stands where the entrance gate stood, placed when the gate goes (the last
//! seat, or its life over with someone inside). Like the entrance, it is an
//! instance with no dungeon holding one object, and it is made again when its
//! system warms up ([`rewarm`]). It goes with the run ([`vacate`]).
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **The trace's lifetime is the run's.** Falsified by a TQ trace that stays
//!   after the last pilot is home, or goes sooner.
//! - **It stands exactly where the pilot stood.** Falsified by a TQ trace at an
//!   offset.

use super::{Run, next_site, site_of};
use crate::damage::BlueTime;
use crate::service::Registry;
use crate::site::{self, Sites};

/// `typeKnownToAbyssalBeacon` **[V]**: the trace of a key within the limit.
pub const TRACE: i64 = 47_458;
/// `typeKnownToAbyssalSuspectBeacon` **[V]**: the trace of a suspicious key.
pub const TRACE_SUSPECT: i64 = 47_833;

/// The trace in space: its instance, its ball, and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub site_id: i64,
    /// The ball's item id (changes when the instance is made again).
    pub ball: i64,
    pub system: i64,
    pub at: [f64; 3],
}

/// The type of a run's trace.
pub fn kind(suspect: bool) -> i64 {
    if suspect { TRACE_SUSPECT } else { TRACE }
}

/// Put the instance and its ball into space. `false` when it could not be
/// made.
fn make(reg: &mut Registry, t: &mut Trace, suspect: bool, ft: BlueTime) -> bool {
    if reg.ext::<Sites>().get(t.site_id).is_some() {
        return true;
    }
    if !site::instantiate(reg, &site_of(t.system, t.site_id, 0, ft)) {
        return false;
    }
    match site::add_object(reg, t.site_id, kind(suspect), t.at) {
        Some(b) => {
            t.ball = b.item_id;
            true
        }
        None => {
            site::vacate(reg, t.site_id);
            false
        }
    }
}

/// **Place the run's trace** at `at` in `system`, once. A trace that could
/// not be made leaves the run without one (the departure then keeps the jump
/// drive's effect).
pub(super) fn place_at(reg: &mut Registry, run: &mut Run, system: i64, at: [f64; 3], ft: BlueTime) {
    if run.trace.is_some() {
        return;
    }
    let mut t = Trace {
        site_id: next_site(reg),
        ball: 0,
        system,
        at,
    };
    if make(reg, &mut t, run.suspect, ft) {
        run.trace = Some(t);
    }
}

/// **Make the trace again** when its instance has gone (the site stage drops
/// an instance whose system went cold) and someone is in its system to see it.
pub(super) fn rewarm(reg: &mut Registry, run: &mut Run, ft: BlueTime) {
    let suspect = run.suspect;
    let Some(t) = run.trace.as_mut() else {
        return;
    };
    if reg.ext::<Sites>().get(t.site_id).is_some() || reg.roster().in_system(t.system).is_empty() {
        return;
    }
    if !make(reg, t, suspect, ft) {
        crate::errln!("[abyss] run {}: the trace could not be re-made", run.id);
    }
}

/// **Take the trace out of space** (the run closes).
pub(super) fn vacate(reg: &mut Registry, run: &mut Run) {
    if let Some(t) = run.trace.take() {
        site::vacate(reg, t.site_id);
    }
}

#[cfg(test)]
mod tests;
