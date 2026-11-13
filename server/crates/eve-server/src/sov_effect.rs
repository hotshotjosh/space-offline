//! The Stability Generators' system effects (SOV-UPG-4, #80): an Online upgrade's
//! `dbuff:` tokens (see `sov_upgrade`) are held as continuous dbuffs on every
//! character ship in space in the hub's system, source = the hub's item id.
//!
//! Facts:
//! - [V] a generator is a set of continuous dbuffs, its collections named by
//!   `buffBarConst.DBUFFS_BY_SLOT` (`buffBarConst.py:432-445`), shown on the buff bar
//!   in the generator's own slot with no timer (`buffButtons.py:265-276`).
//! - [V] the client carries no magnitude: the values are the server's, in
//!   `content/sov/upgrades.tsv`.
//! - [I] TQ keys the effect by solar system with no owner field
//!   (`eve/dogma/modifier/system_wide/api/events_pb2.py`).
//!
//! Assumptions, each with its falsifier:
//! - **[A1]** every character ship in the system is held, whoever holds sov.
//!   Falsifier: a TQ pilot of a hostile alliance in a generator system has no
//!   `SovSystemModifier*` icon.
//! - **[A2]** the magnitudes in the content table. Falsifier: a TQ buff-bar tooltip
//!   in a generator system, or an owner balance ruling.
//! - **[A3]** NPCs and drones are not held (no dogma sheet). Falsifier: testers
//!   finding NPCs in a generator system unchanged when TQ NPCs get the buff.
//! - **[A4]** only an Online generator of a claimed hub acts. Falsifier: a TQ
//!   generator at Low power whose icon stays.

use crate::damage::BlueTime;
use crate::dbuff;
use crate::service::Registry;
use crate::sov_upgrade;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::time::Instant;

#[cfg(test)]
mod tests;

/// What the stage holds and what it last read of the sov rows.
#[derive(Debug, Default)]
pub struct Held {
    /// Character -> (ship, hub) it holds the dbuffs on.
    by_character: BTreeMap<i64, (i64, i64)>,
    /// System -> (hub, dbuffs) for every claimed hub with an Online effect,
    /// rebuilt when the sov rows or the table change ([`Held::refresh`]).
    systems: BTreeMap<i64, (i64, Vec<(i64, f64)>)>,
    /// The revisions and table the `systems` map was read at.
    read_at: Option<(u64, u64, u64, usize)>,
}

impl Held {
    /// Re-read `systems` when a claim, a hub or an upgrade row changed, or the
    /// table was replaced; true when it did (values may have changed).
    fn refresh(&mut self, d: &crate::delta::Deltas, t: &sov_upgrade::UpgradeTable) -> bool {
        let at = (
            d.kind_revision(crate::sov::KIND),
            d.kind_revision(crate::sov_hub::KIND),
            d.kind_revision(sov_upgrade::KIND),
            std::ptr::from_ref(t) as usize,
        );
        if self.read_at == Some(at) {
            return false;
        }
        self.read_at = Some(at);
        self.systems.clear();
        if sov_upgrade::any_effects(t) {
            for h in crate::sov_hub::all(d) {
                if let Some(found) = sov_upgrade::online_effects(d, t, h.system_id) {
                    self.systems.insert(h.system_id, found);
                }
            }
        }
        true
    }
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "sov_effect",
    make: |_| Box::new(Held::default()) as Boxed,
};

/// The world clock's stage: hold what each pilot in a generator system should
/// hold, release what they no longer should. A docked pilot is not in the roster,
/// so is released here; a lost hold (death, restart) is held again. The cost per
/// tick is three revision reads, one map lookup per occupied system and, for a
/// pilot in a generator system, one `holds` check; the sov rows are re-read only
/// when they change.
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    let mut held = std::mem::take(w.ext_mut::<Held>());
    let fresh = held.refresh(w.deltas(), sov_upgrade::table(w.content()));
    *w.ext_mut::<Held>() = held;
    if w.ext::<Held>().systems.is_empty() && w.ext::<Held>().by_character.is_empty() {
        return;
    }
    // Who should hold what: character -> (ship, hub, system).
    let mut want: BTreeMap<i64, (i64, i64, i64)> = BTreeMap::new();
    for (system, ids) in w.roster().systems() {
        let Some((hub, _)) = w.ext::<Held>().systems.get(&system) else {
            continue;
        };
        for id in ids {
            if w.character(*id).is_none() {
                continue;
            }
            let Some(p) = w.roster().get(*id) else {
                continue;
            };
            want.insert(*id, (p.ship_id, *hub, system));
        }
    }
    let held: Vec<(i64, (i64, i64))> = w
        .ext::<Held>()
        .by_character
        .iter()
        .map(|(c, h)| (*c, *h))
        .collect();
    for (character, (ship, hub)) in held {
        if want
            .get(&character)
            .is_some_and(|(s, h, _)| (*s, *h) == (ship, hub))
        {
            continue;
        }
        dbuff::release(w, ship, hub, now);
        w.ext_mut::<Held>().by_character.remove(&character);
    }
    for (character, (ship, hub, system)) in want {
        // After a re-read `hold` runs for everyone (a no-op for unchanged
        // values); otherwise only when the hold is missing, which restores what
        // a death or a restart cleared.
        let recorded = w.ext::<Held>().by_character.get(&character) == Some(&(ship, hub));
        if recorded && !fresh && dbuff::holds(w, ship, hub) {
            continue;
        }
        let buffs = w.ext::<Held>().systems[&system].1.clone();
        dbuff::hold(w, ship, hub, &buffs, now);
        w.ext_mut::<Held>()
            .by_character
            .insert(character, (ship, hub));
    }
}
