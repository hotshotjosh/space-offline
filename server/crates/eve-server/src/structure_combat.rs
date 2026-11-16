//! **Upwell structure combat, the core** (issues #44 STR-HP and #45 STR-LOCK;
//! spec `parity-structure-combat.md` L1): hit points, the per-second damage
//! limit, the "under fire" window the repair pause reads, the bars, and what a
//! pilot may lock; and (#46, L2) the volley: [`hit`], what a gun, a missile
//! or a drone does to a structure.
//!
//! **A structure is not a roster entry.** It has a row in [`crate::structure`]
//! and a ball in [`crate::structure_space`]; it has no pilot, no ship record in
//! [`crate::combat`] and no modules. So its hit points, resonances and caps
//! are read from its type ([`layers`]), its bars from its row's fractions
//! ([`damage_state`]) and its recent damage from the [`Fire`] world slot.
//!
//! **Marks.** **[V]** read in the client (`C:` = `out/research/_w/src`),
//! **[V-data]** the local content bundle (read at run time, never copied into
//! a test), **[I]** inferred, **[A]** assumed with its falsifier:
//! - **[V]** the damage-limit attributes 2034/2035/2036
//!   (`attribute{Shield,Armor,Structure}DamageLimit`), the pause thresholds
//!   3354-3356, 5770 `CanHaveAutoRepair`, 5771 `CanHaveArmorPhases`
//!   (`C:dogma/const.py`); `OfferLockTarget` has no structure exclusion, so the
//!   client asks to lock one.
//! - **[V]** 2744 is `attributeServiceModuleFullPowerStateHitpointMultiplier`
//!   (`C:dogma/const.py:880`). **[I]** from that name: it applies in the
//!   full-power state only.
//! - **[I]** the bars are fractions, so a Full to Low change keeps the
//!   fractions and shrinks the points. Falsifier: a client Show Info on a
//!   structure that just lost power, showing the same absolute shield.
//! - **[I]** 2743 is the hull's own HP multiplier (every structure hull
//!   carries 1.0 [V-data]); the limit is on damage *taken* per second.
//! - **[A]** [`FULL_POWER_HP_MULTIPLIER`], [`UNDER_FIRE_WINDOW_SECS`], damage
//!   over the limit is dropped and not carried, and range is to the
//!   structure's surface: falsifiers on each below and in the spec's register.

use crate::damage::{BlueTime, Damage};
use crate::destiny::DamageState;
use crate::service::Registry;
use crate::structure::{self, State, Structure};
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, VecDeque};

/// `attributeShieldDamageLimit` **[V]**.
pub const ATTR_SHIELD_LIMIT: i64 = 2034;
/// `attributeArmorDamageLimit` **[V]**.
pub const ATTR_ARMOR_LIMIT: i64 = 2035;
/// `attributeStructureDamageLimit` **[V]**.
pub const ATTR_HULL_LIMIT: i64 = 2036;
/// The hull's own HP multiplier **[I]**.
pub const ATTR_HULL_HP_MULTIPLIER: i64 = 2743;
/// `attributeServiceModuleFullPowerStateHitpointMultiplier`, on the module
/// types **[V]**.
pub const ATTR_SERVICE_HP_MULTIPLIER: i64 = 2744;
/// `attributePause{Shield,Armor,Hull}RepairDpsThreshold` **[V]**.
pub const ATTR_PAUSE_REPAIR_DPS: [i64; 3] = [3354, 3355, 3356];
/// `attributeStructureCanHaveAutoRepair` **[V]**.
pub const ATTR_CAN_AUTO_REPAIR: i64 = 5770;
/// `attributeStructureCanHaveArmorPhases` **[V]**.
pub const ATTR_CAN_ARMOR_PHASES: i64 = 5771;
/// `signatureRadius`.
pub const ATTR_SIGNATURE_RADIUS: i64 = 552;
/// `shieldCapacity`, `armorHP`, `hp`.
pub const ATTR_SHIELD_CAPACITY: i64 = 263;
pub const ATTR_ARMOR_HP: i64 = 265;
pub const ATTR_HULL_HP: i64 = 9;

/// **[A]** what a full-power structure's shield and armour are multiplied by,
/// while no module item exists (#41): the 2744 every Standup module type
/// carries. Falsifier: a module type whose 2744 differs, or the client's Show
/// Info HP of a full-power hull disagreeing with our bars after a known
/// volley. #41 replaces it with the online module's own value.
pub const FULL_POWER_HP_MULTIPLIER: f64 = 4.0;

/// **[A]** the rolling window "under fire" averages over. Falsifier: a client
/// capture of a structure's repair timer pausing or resuming at another delay
/// after the last hit.
pub const UNDER_FIRE_WINDOW_SECS: i64 = 10;

/// One structure type's points, resonances and caps, per layer in the order
/// shield, armour, hull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layers {
    /// Full points of each layer.
    pub hp: [f64; 3],
    /// Per layer, in the order em, explosive, kinetic, thermal.
    pub resonance: [[f64; 4]; 3],
    /// Points per second a layer may take; `f64::INFINITY` when the type has
    /// no limit.
    pub limit: [f64; 3],
    /// Points per second on the current layer above which the repair pauses;
    /// `f64::INFINITY` for a type without one (it never pauses).
    pub pause_dps: [f64; 3],
}

/// `type_id`'s [`Layers`] from the content. `full_power` is a structure with
/// its service modules online (the multiplier is [`FULL_POWER_HP_MULTIPLIER`]
/// [A]); else the hull's own 2743 (default 1.0) **[I]**. The caller passes
/// [`full_power`]`(s)`.
pub fn layers(content: &crate::content::Content, type_id: i64, full_power: bool) -> Layers {
    use crate::damage::attr;
    use crate::typedata::{attribute_in, own_attribute_in};
    let get = |a: i64| attribute_in(content, type_id, a).unwrap_or(0.0);
    let m = if full_power {
        FULL_POWER_HP_MULTIPLIER
    } else {
        own_attribute_in(content, type_id, ATTR_HULL_HP_MULTIPLIER).unwrap_or(1.0)
    };
    let res = |ids: [i64; 4]| ids.map(|a| attribute_in(content, type_id, a).unwrap_or(1.0));
    let cap = |a: i64| own_attribute_in(content, type_id, a).unwrap_or(f64::INFINITY);
    Layers {
        hp: [
            get(ATTR_SHIELD_CAPACITY) * m,
            get(ATTR_ARMOR_HP) * m,
            get(ATTR_HULL_HP),
        ],
        resonance: [
            res(attr::SHIELD_RESONANCE),
            res(attr::ARMOR_RESONANCE),
            res(attr::HULL_RESONANCE),
        ],
        limit: [
            cap(ATTR_SHIELD_LIMIT),
            cap(ATTR_ARMOR_LIMIT),
            cap(ATTR_HULL_LIMIT),
        ],
        pause_dps: ATTR_PAUSE_REPAIR_DPS.map(cap),
    }
}

/// Whether the structure is in its full-power state: Low and Abandoned are
/// not; an unset upkeep counts as Full **[I]**: the client switches an
/// unrecognised upkeep to Full on its next update (`C:structures/structure.py:609-610`).
fn full_power(s: &Structure) -> bool {
    !matches!(
        s.upkeep,
        Some(structure::Upkeep::Low | structure::Upkeep::Abandoned)
    )
}

/// The layer a shot lands on first, by state: ShieldVulnerable 0,
/// ArmorVulnerable 1, HullVulnerable 2 **[V]** (`STATE_SHIELD_ARMOR_HULL`
/// forces the others empty). `None` unless the structure is vulnerable
/// (`vulnerable == Some(true)`): a reinforced, invulnerable or unanchored
/// structure takes no damage and has no layer under fire.
pub fn current_layer(s: &Structure) -> Option<usize> {
    if s.vulnerable != Some(true) {
        return None;
    }
    match s.state {
        State::ShieldVulnerable => Some(0),
        State::ArmorVulnerable => Some(1),
        State::HullVulnerable => Some(2),
        _ => None,
    }
}

/// What a volley did to one structure.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The new `(shield, armour, hull)` fractions.
    pub fractions: (f64, f64, f64),
    /// Points taken off each layer.
    pub dealt: [f64; 3],
    /// Points the per-second limit threw away.
    pub dropped: f64,
}

/// Pure: `damage` (pre-resist; nothing at all unless [`current_layer`] is
/// `Some`) on `s`, given what each layer already took in
/// this whole second (`taken_this_second`). Per layer from [`current_layer`]:
/// points = sum(damage_i * resonance_i); clamped to `limit - taken_this_second`
/// **[I]** (the limit is on damage taken); the clamped excess is dropped, not
/// carried **[A]** (falsifier: a client log or capture showing capped damage
/// carried into the next second); what empties a layer overflows into the next
/// one, as the share of the volley left (by the ratio of points) **[V]**
/// (the first armour loss is what moves the state on).
pub fn apply(l: &Layers, s: &Structure, damage: Damage, taken_this_second: [f64; 3]) -> Applied {
    let fr = s.damage.unwrap_or((1.0, 1.0, 1.0));
    let mut frac = [fr.0, fr.1, fr.2];
    let mut dealt = [0.0; 3];
    let mut dropped = 0.0;
    // Not vulnerable (reinforced, invulnerable, unanchored): nothing lands.
    let Some(first) = current_layer(s) else {
        return Applied {
            fractions: (frac[0], frac[1], frac[2]),
            dealt,
            dropped,
        };
    };
    let mut left = [damage.em, damage.explosive, damage.kinetic, damage.thermal];
    for i in first..3 {
        let r = l.resonance[i];
        let points: f64 = left.iter().zip(r).map(|(d, r)| d * r).sum();
        if points <= 0.0 {
            break;
        }
        let room = (l.limit[i] - taken_this_second[i]).max(0.0);
        let allowed = points.min(room);
        dropped += points - allowed;
        let remaining = frac[i].clamp(0.0, 1.0) * l.hp[i];
        if allowed <= remaining {
            dealt[i] += allowed;
            frac[i] = if l.hp[i] > 0.0 {
                (remaining - allowed) / l.hp[i]
            } else {
                0.0
            };
            break;
        }
        // The layer empties; the rest of the (capped) volley goes on.
        dealt[i] += remaining;
        frac[i] = 0.0;
        let ratio = (allowed - remaining) / points;
        for d in &mut left {
            *d *= ratio;
        }
    }
    Applied {
        fractions: (frac[0], frac[1], frac[2]),
        dealt,
        dropped,
    }
}

// -- the world slot: who shot what, recently -------------------------------------

/// Not persisted: a restart forgets who was shooting, which only means the
/// repair resumes.
#[derive(Debug, Default)]
pub struct Fire {
    by_structure: BTreeMap<i64, Recent>,
}

#[derive(Debug, Default, Clone)]
struct Recent {
    /// The whole second `taken` belongs to.
    second: i64,
    taken: [f64; 3],
    /// `(when, layer, points)`, oldest first.
    window: VecDeque<(BlueTime, usize, f64)>,
    /// The roster key of the last volley that landed, for the 184
    /// notification's `charID` (consumed when it is sent).
    last_shooter: Option<i64>,
}

/// The [`Fire`] world slot (`world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "structure_combat",
    make: |_| Box::new(Fire::default()) as Boxed,
};

fn second_of(at: BlueTime) -> i64 {
    at.div_euclid(structure::SECOND)
}

impl Fire {
    /// What each layer of `id` already took in the whole second `at` is in.
    pub fn taken_this_second(&self, id: i64, at: BlueTime) -> [f64; 3] {
        match self.by_structure.get(&id) {
            Some(r) if r.second == second_of(at) => r.taken,
            _ => [0.0; 3],
        }
    }

    /// Note what a volley dealt (after the limit) at `at`.
    pub fn record(&mut self, id: i64, at: BlueTime, dealt: [f64; 3]) {
        let r = self.by_structure.entry(id).or_default();
        if r.second != second_of(at) {
            r.second = second_of(at);
            r.taken = [0.0; 3];
        }
        let horizon = at - UNDER_FIRE_WINDOW_SECS * structure::SECOND;
        while r.window.front().is_some_and(|(t, _, _)| *t <= horizon) {
            r.window.pop_front();
        }
        for (layer, points) in dealt.into_iter().enumerate() {
            if points > 0.0 {
                r.taken[layer] += points;
                r.window.push_back((at, layer, points));
            }
        }
    }

    /// Note who fired the volley that just landed.
    pub fn note_shooter(&mut self, id: i64, shooter: i64) {
        self.by_structure.entry(id).or_default().last_shooter = Some(shooter);
    }

    /// The last shooter noted for `id`, forgotten once read (the next
    /// window's first aggression names its own). `None` for damage nobody
    /// fired (a GM command) -- but see the note: a stale shooter from an
    /// earlier volley in the same window can still be here [A].
    pub fn take_last_shooter(&mut self, id: i64) -> Option<i64> {
        self.by_structure.get_mut(&id)?.last_shooter.take()
    }

    /// Rolling points per second on `layer` over the window ending at `now`.
    pub fn dps(&self, id: i64, layer: usize, now: BlueTime) -> f64 {
        let Some(r) = self.by_structure.get(&id) else {
            return 0.0;
        };
        let horizon = now - UNDER_FIRE_WINDOW_SECS * structure::SECOND;
        let sum: f64 = r
            .window
            .iter()
            .filter(|(t, l, _)| *t > horizon && *l == layer)
            .map(|(_, _, p)| p)
            .sum();
        sum / UNDER_FIRE_WINDOW_SECS as f64
    }

    /// Forget a structure (it left space).
    pub fn forget(&mut self, id: i64) {
        self.by_structure.remove(&id);
    }
}

/// `ShouldRepairPause` **[I]**: the rolling dps on the current layer is above
/// that layer's pause threshold. A type without a threshold never pauses.
pub fn under_fire(w: &Registry, s: &Structure, now: BlueTime) -> bool {
    let Some(layer) = current_layer(s) else {
        return false;
    };
    let threshold =
        crate::typedata::own_attribute_in(w.content(), s.type_id, ATTR_PAUSE_REPAIR_DPS[layer])
            .unwrap_or(f64::INFINITY);
    w.ext::<Fire>().dps(s.id, layer, now) > threshold
}

// -- the bars --------------------------------------------------------------------

/// The bars: `DamageState { shield: Some(s), shield_tau_ms: 0.0, armour, hull }`
/// (no passive regeneration: a frozen bar **[I]**); an unset damage reads full.
pub fn damage_state(s: &Structure) -> DamageState {
    match s.damage {
        None => DamageState::FULL,
        Some((sh, ar, hu)) => DamageState {
            shield: Some(sh),
            shield_tau_ms: 0.0,
            armour: ar,
            hull: hu,
        },
    }
}

/// [`damage_state`] of structure `id`, if it is one.
pub fn damage_state_of(w: &Registry, id: i64) -> Option<DamageState> {
    get_structure(w, id).map(|s| damage_state(&s))
}

/// The structure `id`, parsed once (a missing row answers before any parse).
fn get_structure(w: &Registry, id: i64) -> Option<Structure> {
    structure::get(w.deltas(), id)
}

// -- what can be locked ----------------------------------------------------------

/// A structure in space in `system` that is not unanchored: lockable **[V]**
/// (`OfferLockTarget` has no structure exclusion).
pub fn is_lockable(w: &Registry, id: i64, system: i64) -> bool {
    get_structure(w, id).is_some_and(|s| s.system == system && s.state != State::Unanchored)
}

/// Cheap: is `id` a structure row at all? A key probe, no parse.
pub fn is_target(w: &Registry, id: i64) -> bool {
    w.deltas()
        .of(structure::KIND)
        .contains_key(&structure::row_key(id))
}

/// What the lock arithmetic needs of a structure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedTarget {
    pub system: i64,
    pub position: [f64; 3],
    pub radius: f64,
    pub signature: f64,
}

/// A structure in space as a fixed target. The signature is the type's own
/// 552, read as the client's `GetScanSpeed` reads it (`own_attribute_in`;
/// absent: the ball radius, the client's fallback) **[V]**
/// (`targeting::acquisition_delay` documents the rule).
pub fn fixed_target(w: &Registry, id: i64) -> Option<FixedTarget> {
    let s = get_structure(w, id)?;
    if s.state == State::Unanchored {
        return None;
    }
    let radius = crate::structure_space::radius(w.content(), s.type_id);
    Some(FixedTarget {
        system: s.system,
        position: s.pos,
        radius,
        signature: crate::typedata::own_attribute_in(w.content(), s.type_id, ATTR_SIGNATURE_RADIUS)
            .unwrap_or(radius),
    })
}

// -- the volley (#46) -------------------------------------------------------------

/// One volley at a structure, from any weapon (spec `parity-structure-combat.md`
/// L2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Volley {
    /// Roster key of the pilot (a drone's owner, for drones).
    pub shooter: i64,
    /// The ball the log line names (the drone, for drones).
    pub attacker_ship: i64,
    pub weapon_type: i64,
    pub target: i64,
    /// Pre-resist, application already applied.
    pub damage: Damage,
    /// 0 is a miss.
    pub hit_quality: u8,
    pub banked: bool,
    pub at: BlueTime,
}

/// **[I]** surface distance from `from` to structure `id`: the centre gap
/// minus the structure's radius (a radius of tens of kilometres makes the
/// centre gap meaningless), never below 0. Falsifier: guns that miss a
/// structure the client's own range circle covers. `None` if `id` is not a
/// fixed target.
pub fn surface_gap(w: &Registry, from: [f64; 3], id: i64) -> Option<f64> {
    let t = fixed_target(w, id)?;
    Some((crate::space_state::distance(from, t.position) - t.radius).max(0.0))
}

/// **[I]** the point of structure `id`'s surface nearest `from`: the centre
/// moved toward `from` by the radius, so `distance(from, point)` is
/// [`surface_gap`]. `from` itself when it is inside the radius. What an NPC
/// steers at and measures its range to. Falsifier: a besieger that stops
/// short of, or inside, the gap its guns reach. `None` if `id` is not a fixed
/// target.
pub fn surface_point(w: &Registry, from: [f64; 3], id: i64) -> Option<[f64; 3]> {
    Some(surface_point_of(&fixed_target(w, id)?, from))
}

/// [`surface_point`] of a [`FixedTarget`] already read.
pub fn surface_point_of(t: &FixedTarget, from: [f64; 3]) -> [f64; 3] {
    let d = crate::space_state::distance(from, t.position);
    if d <= t.radius {
        return from;
    }
    let k = t.radius / d;
    [
        t.position[0] + (from[0] - t.position[0]) * k,
        t.position[1] + (from[1] - t.position[1]) * k,
        t.position[2] + (from[2] - t.position[2]) * k,
    ]
}

/// What a structure keeps when its hull is gone, until death exists (#48):
/// a fraction that is not zero, so it stays shootable and nothing divides by
/// it.
const HULL_FLOOR: f64 = f64::EPSILON;

/// **A volley lands on a structure.**
/// 1. a missing structure, or one not in the shooter's system: nothing;
/// 2. aggression, hit or miss ([`crate::crimewatch::aggression_on_structure`]);
/// 3. not vulnerable, or a miss: no damage, and the shooter's line is a miss
///    **[A]** (falsifier: a retail log on a reinforced structure showing a hit
///    for 0, or nothing);
/// 4. otherwise [`apply`] under the caps, the bars and the state, the repair
///    paused by the fire (`under_fire` is true for this update), the tell;
/// 5. one combat line to a character shooter;
/// 6. the killmail tally;
/// 7. a hull at 0 in the vulnerable hull state is [`destroy`]ed; in any
///    other state it is clamped to [`f64::EPSILON`].
pub fn hit(w: &mut Registry, v: Volley, now: BlueTime) {
    let Some(mut s) = get_structure(w, v.target) else {
        return;
    };
    if w.session_system(v.shooter) != Some(s.system) {
        return;
    }
    crate::crimewatch::aggression_on_structure(w, v.shooter, &s, v.at, now);
    let first_layer = current_layer(&s);
    let landing = v.hit_quality != 0 && first_layer.is_some();
    let mut dealt = [0.0; 3];
    let mut dead = false;
    if landing {
        let l = layers(w.content(), s.type_id, full_power(&s));
        let taken = w.ext::<Fire>().taken_this_second(s.id, v.at);
        let a = apply(&l, &s, v.damage, taken);
        dealt = a.dealt;
        w.ext_mut::<Fire>().record(s.id, v.at, dealt);
        let e = structure::Env {
            under_fire: true,
            ..crate::structure_space::env_for(w, &s, now)
        };
        w.ext_mut::<Fire>().note_shooter(s.id, v.shooter);
        let (sh, ar, hu) = a.fractions;
        s.set_damage(sh, ar, hu.max(HULL_FLOOR), &e);
        crate::structure_space::settle(&mut s, &e);
        // Hull gone by a volley that began on the hull layer: it dies (after
        // the tally below). A volley that ran through the shield and the
        // armour first reinforces instead and the hull is held at the
        // floor, so a reinforce reset cannot kill.
        if hu <= 0.0 && first_layer == Some(2) {
            dead = true;
        } else {
            let events = s.take_events();
            structure::put(w.deltas_mut(), &s);
            w.mark_dirty();
            crate::structure_space::react(w, s.id, &events);
        }
    }
    let total: f64 = dealt.iter().sum();
    if w.character(v.shooter).is_some() {
        let shot = crate::damage::Shot {
            at: v.at,
            attacker_ship_id: v.attacker_ship,
            weapon_item_id: 0,
            weapon_type_id: v.weapon_type,
            target_ship_id: v.target,
            damage: v.damage,
            hit_quality: if landing { v.hit_quality } else { 0 },
        };
        let mut message = shot.attacker_message(total);
        message.is_banked = v.banked;
        message.target_owner_id = Some(s.owner_corp);
        w.schedule(
            std::time::Duration::ZERO,
            crate::combat::CombatMessage {
                to: vec![v.shooter],
                message,
            },
        );
    }
    crate::killmail::on_structure_volley(w, s.id, v.shooter, v.weapon_type, v.at, total);
    if dead {
        destroy(w, &s, Some(v.shooter), now);
    }
}

/// **A structure dies** (#48, spec L3.4): hull gone in the vulnerable hull
/// state. Order matters: the row must still say where it is while the pilots
/// are put out and the items moved (`structure_space::finish_unanchor`'s
/// rule).
/// 1. the managers get the 188;
/// 2. the killmail, with the corporation as victim, and the 141;
/// 3. [`crate::structure_dock::evacuate`]: docked pilots are put out at the
///    structure, hangars go to asset safety; in wormhole space and Pochven
///    (no asset safety) the hangar is rolled into the wreck first
///    ([`crate::asset_safety::drop_plan`]) and only a pilot's active ship
///    goes to the harbour. **[A]** the killmail (built by `killmail.rs`,
///    outside this lane) lists none of those hangar items; falsifier: a TQ
///    J-space structure killmail that lists hangar items;
/// 4. the hull's wreck, `abandoned` **[A]** (falsifier: a TQ loot from one
///    that flags a suspect), holding the J-space and Pochven hangar drop and
///    otherwise empty, and the structure's item destroyed;
///    what a dead structure drops (fitted modules, a quantum core) is owner
///    decision O-14 **[A]**, default none (falsifier: TQ wrecks that hold
///    loot);
/// 5. one [`crate::destiny::Kill`] to the system: the ball's
///    `RemoveBall(id, True)` is the explosion, and the wreck's ball rides
///    with it **[A]** (falsifier: the ball stays on a pilot's grid);
/// 6. locks on it are dropped (`'Exploding'`) and modules aimed at it stop;
/// 7. the row, the fire record and the directory entry go.
pub fn destroy(w: &mut Registry, s: &Structure, killer: Option<i64>, now: BlueTime) {
    use crate::item::{Edit, Slot};
    let tell_corp = crate::structure_space::interested(w, s.owner_corp, s.system);
    let d = crate::structure_notify::destroyed(w, s);
    crate::structure_notify::tell(w, s, crate::structure_notify::STRUCTURE_DESTROYED, d);
    if let Some(report) = crate::killmail::on_kill_structure(w, s, killer, now)
        .and_then(|id| crate::killmail::get(w, id))
    {
        crate::notifications::on_structure_kill(w, &report);
        crate::war::file_report(w, &report);
    }
    // The structure's own bays (fuel, deed, ammo, fighters, moon material,
    // service slots) go down with it; evacuation wraps the hangars only [I].
    // The set is the bay flags only: fitting (U6) must extend it (fitted
    // slots, rigs, the quantum core) per O-14 [A]. [A] Until then a fitted
    // item located here would be rolled into a J-space wreck by `drop_plan`,
    // where TQ always destroys rigs; U6 must extend `is_bay_flag` or filter
    // the plan by flag. Falsifier: a TQ J-space wreck that holds rigs.
    let bays: Vec<Edit> = w
        .store()
        .contents_of(s.id)
        .filter(|i| crate::structure_dock::control::is_bay_flag(i.flag_id))
        .map(|i| Edit::Destroy { item_id: i.item_id })
        .collect();
    let refused = if bays.is_empty() {
        None
    } else {
        apply_on_the_clock_to(w, &[], &bays).err()
    };
    if let Some(e) = refused {
        crate::errln!("[structure] the bays of {} were refused: {e:?}", s.id);
    }
    // The wreck first: nothing can be moved into an id the same plan mints,
    // and it is owned by the corporation, shown to nobody's item list.
    let mut wreck = None;
    if let Some(type_id) = crate::combat::wreck_type_in(w.content(), s.type_id) {
        let create = [Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(s.owner_corp, s.system, crate::combat::FLAG_IN_SPACE),
        }];
        match apply_on_the_clock_to(w, &[], &create) {
            Ok(changes) => {
                wreck = changes
                    .iter()
                    .find_map(|ch| match ch {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                    .map(|wreck_id| crate::combat::Wreck {
                        wreck_id,
                        type_id,
                        solar_system_id: s.system,
                        position: s.pos,
                        ship_id: s.id,
                        owner_id: s.owner_corp,
                        corp_id: s.owner_corp,
                        fleet_id: None,
                        abandoned: true,
                        expires_at: now.saturating_add(crate::combat::WRECK_LIFETIME),
                    });
            }
            Err(e) => crate::errln!("[structure] the wreck of {} was refused: {e:?}", s.id),
        }
    }
    // Where there is no asset safety (wormhole space, Pochven) the hangar
    // drops into the wreck, rolled like a ship's cargo; a pilot's active
    // ship is left for `evacuate`, which sends it to the harbour with them.
    if !crate::asset_safety::applies_in(w, s.system) && wreck.is_none() {
        // [A] No wreck type for this hull, or the wreck was refused: nothing
        // can drop, so `evacuate` keeps the older harbour move for the
        // hangar. Falsifier: a TQ J-space kill with no wreck that loses
        // the hangar.
        crate::errln!(
            "[structure] no wreck for {} in no-safety space: the hangar goes to the harbour",
            s.id
        );
    }
    if let Some(wreck_id) = wreck.map(|wr| wr.wreck_id)
        && !crate::asset_safety::applies_in(w, s.system)
    {
        let ships: std::collections::BTreeSet<i64> =
            w.config().characters.iter().map(|c| c.ship_id).collect();
        let chance = crate::combat::drop_chance(w, Some(s.system));
        let plan =
            crate::asset_safety::drop_plan(w, s.id, wreck_id, chance, &|i| ships.contains(&i));
        if !plan.is_empty()
            && let Err(e) = apply_on_the_clock_to(w, &[], &plan)
        {
            crate::errln!("[structure] the drop of {} was refused: {e:?}", s.id);
        }
    }
    // Evacuation pushes (the ejected pilot's mid-session `SetState` whose
    // ego is this hull, the guests' frames) onto `pending`, which only a
    // payload turn drains, while the Kill below leaves with the clock: a
    // gun or missile kill runs in the tick, so the Kill would reach the pilot
    // first and the stale `SetState` would re-add the hull (structures are
    // global balls, `grid.rs`) for good. So, as `apply_on_the_clock_to` does:
    // set aside what was there, take what evacuation pushed, put the rest
    // back and schedule evacuation's own at ZERO ahead of the Kill (the
    // clock keeps insertion order). A push with no audience stays on
    // `pending` as before: it has no one the clock could address it to.
    // [A] On a gun kill the ejected pilot therefore gets SetState and the
    // hull's RemoveBall before the undock session change (queue_session_change
    // is its own queue, drained after the tick); a normal undock sends them
    // the other way round. Falsifier: the B side of fci_upwell_kill (the
    // pilot's client must be in space, not stuck, after the hull goes).
    // Inside an open clock the outer call's pushes so far leave first
    // (`sov_hub::clock_flush`).
    crate::sov_hub::clock_flush(w);
    let before = w.take_pending_raw();
    crate::structure_dock::evacuate(w, s);
    let ours = w.take_pending_raw();
    w.restore_pending(before);
    for (after, p) in ours {
        if p.audience().is_some() {
            w.schedule(
                std::time::Duration::ZERO,
                crate::notify::Addressed { to: None, inner: p },
            );
        } else {
            w.push_later(after, crate::notify::Addressed { to: None, inner: p });
        }
    }
    // Who sees the explosion is read after the pilots were put out: they are
    // in space now and must get the batch that removes the hull (as
    // `finish_unanchor` reads its audience after `evacuate`).
    let audience = crate::structure_space::in_space(w, s.system);
    if w.store().get(s.id).is_some() {
        let corp = crate::service::structures::corp_audience(w, s.owner_corp);
        if let Err(e) = apply_on_the_clock_to(w, &corp, &[Edit::Destroy { item_id: s.id }]) {
            crate::errln!("[structure] the item of {} was refused: {e:?}", s.id);
        }
    }
    let stamp = w.next_destiny_stamp();
    let wreck_ball = wreck.map(|wr| crate::combat::wreck_parts(w, &wr));
    if let Some(k) = crate::destiny::Kill::new(audience, stamp, s.id, wreck_ball, None) {
        w.schedule(std::time::Duration::ZERO, k);
    }
    if let Some(wr) = wreck {
        crate::combat::add_wreck(w, wr);
    }
    // Locks and guns aimed at it, as `anchor::release_target` does for a unit.
    crate::anchor::release_target(w, s.id, now);
    structure::remove(w.deltas_mut(), s.id);
    w.ext_mut::<Fire>().forget(s.id);
    w.mark_dirty();
    w.push(crate::structure_space::RemovedFromSpace {
        to: tell_corp,
        id: s.id,
    });
    crate::structure_space::announce_lists(w, s, true, false);
    crate::errln!(
        "[structure] {} destroyed in {} by {killer:?}",
        s.id,
        s.system
    );
}

/// [`crate::sov_hub::apply_items_on_the_clock_to`]: the plan's pushes go to
/// `to` as scheduled events, never to whoever spoke next (a structure dies
/// inside the world tick). An empty `to` drops them. Nest-safe (#258).
fn apply_on_the_clock_to(
    w: &mut Registry,
    to: &[i64],
    edits: &[crate::item::Edit],
) -> Result<Vec<crate::item::Change>, crate::item::EditError> {
    crate::sov_hub::apply_items_on_the_clock_to(w, to, edits)
}

/// A missile lands: a [`Volley`] at [`crate::combat::HIT_QUALITY`].
#[allow(clippy::too_many_arguments)] // the missile's own fields
pub fn missile_hit(
    w: &mut Registry,
    shooter: i64,
    launcher_ship: i64,
    missile_type: i64,
    target: i64,
    damage: Damage,
    banked: bool,
    at: BlueTime,
    now: BlueTime,
) {
    hit(
        w,
        Volley {
            shooter,
            attacker_ship: launcher_ship,
            weapon_type: missile_type,
            target,
            damage,
            hit_quality: crate::combat::HIT_QUALITY,
            banked,
            at,
        },
        now,
    );
}

// -- GM ----------------------------------------------------------------------------

/// A state by the snake-case name of its variant (`hull_vulnerable`), or by
/// its code.
pub fn state_by_name(word: &str) -> Option<State> {
    if let Ok(code) = word.parse::<i64>() {
        return State::from_code(code);
    }
    let want = word.to_ascii_lowercase().replace('-', "_");
    State::ALL.into_iter().find(|s| snake_name(*s) == want)
}

/// The variant name in snake case.
pub fn snake_name(s: State) -> String {
    let dbg = format!("{s:?}");
    let mut out = String::new();
    for (i, c) in dbg.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::structure::{Env, Size, Upkeep};
    use crate::structure_space::ASTRAHUS;
    use eve_marshal::PyValue;

    // Invented numbers, none the client's.
    pub(crate) const SHIELD: f64 = 1_000.0;
    pub(crate) const ARMOR: f64 = 2_000.0;
    pub(crate) const HULL: f64 = 3_000.0;
    pub(crate) const HULL_MULT: f64 = 1.5;
    pub(crate) const LIMIT: f64 = 100.0;
    pub(crate) const PAUSE: f64 = 50.0;
    const T0: i64 = 134_000_000_000_000_000;

    fn at(secs: i64) -> i64 {
        T0 + secs * structure::SECOND
    }

    pub(crate) fn attr_text(extra: &str) -> String {
        format!(
            "{ATTR_SHIELD_CAPACITY}={SHIELD};{ATTR_ARMOR_HP}={ARMOR};{ATTR_HULL_HP}={HULL};\
             {ATTR_HULL_HP_MULTIPLIER}={HULL_MULT};{ATTR_SHIELD_LIMIT}={LIMIT};\
             {ATTR_ARMOR_LIMIT}={LIMIT};{ATTR_HULL_LIMIT}={LIMIT};\
             {}={PAUSE};{}={PAUSE};{}={PAUSE}{extra}",
            ATTR_PAUSE_REPAIR_DPS[0], ATTR_PAUSE_REPAIR_DPS[1], ATTR_PAUSE_REPAIR_DPS[2]
        )
    }

    /// The type-data row of an invented `ASTRAHUS` hull with `attrs`.
    pub(crate) fn astrahus_row(attrs: &str) -> String {
        format!(
            "type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t500.0\t0\t\t{attrs}\tInvented Hull\n"
        )
    }

    pub(crate) fn content_with(attrs: &str) -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        let rows = format!("{}{}", crate::typedata::FIXTURE, astrahus_row(attrs));
        let table = crate::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn world(attrs: &str) -> Registry {
        Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(content_with(attrs)))
    }

    fn plain() -> Layers {
        Layers {
            hp: [SHIELD, ARMOR, HULL],
            resonance: [[1.0; 4]; 3],
            limit: [f64::INFINITY; 3],
            pause_dps: [f64::INFINITY; 3],
        }
    }

    fn kinetic(n: f64) -> Damage {
        Damage {
            kinetic: n,
            ..Damage::ZERO
        }
    }

    fn shield_up(type_id: i64, e: &Env) -> Structure {
        let mut s = Structure::anchored(7, type_id, 11, 22, [0.0; 3], 0.0, "Home", 1, e);
        s.set_state(State::ShieldVulnerable, e);
        s.upkeep = Some(Upkeep::Full);
        s
    }

    #[test]
    fn layers_read_the_type_and_the_full_power_multiplier() {
        let reg = world(&attr_text(""));
        let off = layers(reg.content(), ASTRAHUS, false);
        assert_eq!(
            off.hp,
            [SHIELD * HULL_MULT, ARMOR * HULL_MULT, HULL],
            "the hull's own multiplier"
        );
        let on = layers(reg.content(), ASTRAHUS, true);
        assert_eq!(
            on.hp,
            [
                SHIELD * FULL_POWER_HP_MULTIPLIER,
                ARMOR * FULL_POWER_HP_MULTIPLIER,
                HULL
            ]
        );
        assert_eq!(on.limit, [LIMIT; 3]);
        assert_eq!(on.pause_dps, [PAUSE; 3]);
        // A type with no limit has none; no multiplier reads 1.0.
        let bare = world(&format!("{ATTR_SHIELD_CAPACITY}={SHIELD}"));
        let l = layers(bare.content(), ASTRAHUS, false);
        assert_eq!(l.hp[0], SHIELD);
        assert!(l.limit[0].is_infinite() && l.pause_dps[2].is_infinite());
    }

    #[test]
    fn the_limit_drops_what_exceeds_a_second() {
        let l = Layers {
            limit: [LIMIT; 3],
            ..plain()
        };
        let e = Env::new(at(0));
        let s = shield_up(ASTRAHUS, &e);
        let mut fire = Fire::default();
        let first = apply(&l, &s, kinetic(80.0), fire.taken_this_second(s.id, at(0)));
        assert_eq!(first.dealt[0], 80.0);
        fire.record(s.id, at(0), first.dealt);
        let second = apply(&l, &s, kinetic(80.0), fire.taken_this_second(s.id, at(0)));
        assert_eq!(second.dealt[0], LIMIT - 80.0);
        assert_eq!(second.dropped, 80.0 - (LIMIT - 80.0));
        fire.record(s.id, at(0), second.dealt);
        let next = apply(&l, &s, kinetic(80.0), fire.taken_this_second(s.id, at(1)));
        assert_eq!(next.dealt[0], 80.0, "the next second starts clean");
    }

    #[test]
    fn a_shield_volley_that_empties_the_layer_overflows_into_armour_and_reinforces() {
        let e = Env::new(at(0));
        let mut s = shield_up(ASTRAHUS, &e);
        let a = apply(&plain(), &s, kinetic(SHIELD + 500.0), [0.0; 3]);
        assert_eq!(a.dealt, [SHIELD, 500.0, 0.0]);
        assert_eq!(a.fractions, (0.0, 1.0 - 500.0 / ARMOR, 1.0));
        s.set_damage(a.fractions.0, a.fractions.1, a.fractions.2, &e);
        assert_eq!(s.state, State::ArmorReinforce);
        assert_eq!(s.damage, Some((0.0, 1.0, 1.0)), "the state forces the bars");
        assert_eq!(s.vulnerable, Some(false));
    }

    #[test]
    fn a_medium_hull_goes_hull_vulnerable_a_large_hull_reinforces() {
        let large = (1..100_000)
            .find(|t| structure::size_of_type(*t) == Size::Large)
            .expect("a large type is known");
        let e = Env::new(at(0));
        let hit = |type_id: i64| {
            let mut s = shield_up(type_id, &e);
            s.set_state(State::ArmorVulnerable, &e);
            let a = apply(&plain(), &s, kinetic(ARMOR + 500.0), [0.0; 3]);
            assert_eq!(a.dealt, [0.0, ARMOR, 500.0]);
            s.set_damage(a.fractions.0, a.fractions.1, a.fractions.2, &e);
            s.state
        };
        assert_eq!(hit(ASTRAHUS), State::HullVulnerable);
        assert_eq!(hit(large), State::HullReinforce);
    }

    #[test]
    fn resonance_scales_by_layer() {
        let mut l = plain();
        l.resonance[0] = [1.0, 1.0, 0.5, 1.0];
        let e = Env::new(at(0));
        let s = shield_up(ASTRAHUS, &e);
        let a = apply(&l, &s, kinetic(200.0), [0.0; 3]);
        assert_eq!(a.dealt[0], 100.0);
        assert_eq!(a.fractions.0, 1.0 - 100.0 / SHIELD);
    }

    #[test]
    fn the_bars_are_the_fractions_and_unset_is_full() {
        let e = Env::new(at(0));
        let mut s = shield_up(ASTRAHUS, &e);
        assert_eq!(damage_state(&s), DamageState::FULL);
        s.set_damage(0.5, 1.0, 1.0, &e);
        let d = damage_state(&s);
        assert_eq!((d.shield, d.armour, d.hull), (Some(0.5), 1.0, 1.0));
        assert_eq!(d.shield_tau_ms, 0.0, "a frozen bar");
        s.damage = None;
        assert_eq!(damage_state(&s), DamageState::FULL);
    }

    #[test]
    fn repair_pauses_while_under_fire_and_finishes_after() {
        let mut reg = world(&attr_text(""));
        let e0 = crate::structure_space::env(&reg, at(0));
        let mut s = shield_up(ASTRAHUS, &e0);
        s.set_damage(0.5, 1.0, 1.0, &e0);
        // The first update starts the repair.
        s.update(&e0);
        assert_eq!(s.repairing, Some(true));
        // A hit above the pause threshold: the repair stops.
        reg.ext_mut::<Fire>().record(
            s.id,
            at(1),
            [PAUSE * UNDER_FIRE_WINDOW_SECS as f64 * 2.0, 0.0, 0.0],
        );
        let e1 = crate::structure_space::env_for(&reg, &s, at(1));
        assert!(e1.under_fire);
        s.update(&e1);
        assert_eq!(s.repairing, Some(false));
        assert!(s.timer.is_paused());
        // Quiet for longer than the window: it resumes.
        let later = 1 + UNDER_FIRE_WINDOW_SECS + 1;
        let e2 = crate::structure_space::env_for(&reg, &s, at(later));
        assert!(!e2.under_fire);
        s.update(&e2);
        assert_eq!(s.repairing, Some(true));
        assert!(!s.timer.is_paused());
        // And past the repair time the bars are full.
        let done = later + State::ShieldVulnerable.repair_secs() + 5;
        let e3 = crate::structure_space::env_for(&reg, &s, at(done));
        s.update(&e3);
        assert_eq!(s.damage, Some((1.0, 1.0, 1.0)));
    }

    #[test]
    fn a_structure_that_is_not_vulnerable_takes_nothing() {
        let e = Env::new(at(0));
        let mut s = shield_up(ASTRAHUS, &e);
        s.set_damage(0.5, 1.0, 1.0, &e);
        assert_eq!(current_layer(&s), Some(0));
        let hit = apply(&plain(), &s, kinetic(100.0), [0.0; 3]);
        assert!(hit.dealt[0] > 0.0, "vulnerable: it lands");
        s.set_state(State::ArmorReinforce, &e);
        assert_eq!(s.vulnerable, Some(false), "reinforced");
        assert_eq!(current_layer(&s), None);
        let before = s.damage.unwrap();
        let a = apply(&plain(), &s, kinetic(1.0e6), [0.0; 3]);
        assert_eq!(a.fractions, before, "the bars do not move");
        assert_eq!(a.dealt, [0.0; 3]);
        assert_eq!(a.dropped, 0.0);
        // And a reinforced structure is never under fire.
        let mut reg = world(&attr_text(""));
        reg.ext_mut::<Fire>()
            .record(s.id, at(0), [1.0e9, 1.0e9, 1.0e9]);
        assert!(!under_fire(&reg, &s, at(0)));
    }

    #[test]
    fn a_type_with_no_pause_threshold_never_pauses() {
        let mut reg = world(&format!("{ATTR_SHIELD_CAPACITY}={SHIELD}"));
        let e = Env::new(at(0));
        let s = shield_up(ASTRAHUS, &e);
        reg.ext_mut::<Fire>().record(s.id, at(0), [1.0e9, 0.0, 0.0]);
        assert!(!under_fire(&reg, &s, at(0)));
    }

    #[test]
    fn the_window_forgets_and_the_second_resets() {
        let mut f = Fire::default();
        f.record(1, at(0), [10.0, 0.0, 0.0]);
        assert_eq!(f.taken_this_second(1, at(0)), [10.0, 0.0, 0.0]);
        assert_eq!(f.taken_this_second(1, at(1)), [0.0; 3]);
        assert_eq!(f.dps(1, 0, at(5)), 10.0 / UNDER_FIRE_WINDOW_SECS as f64);
        assert_eq!(f.dps(1, 1, at(5)), 0.0, "another layer");
        assert_eq!(f.dps(1, 0, at(UNDER_FIRE_WINDOW_SECS + 1)), 0.0);
        f.forget(1);
        assert_eq!(f.dps(1, 0, at(0)), 0.0);
    }

    #[test]
    fn states_are_found_by_snake_name_or_code() {
        assert_eq!(
            state_by_name("hull_vulnerable"),
            Some(State::HullVulnerable)
        );
        assert_eq!(
            state_by_name("Shield-Vulnerable"),
            Some(State::ShieldVulnerable)
        );
        assert_eq!(
            state_by_name(&State::ArmorReinforce.code().to_string()),
            Some(State::ArmorReinforce)
        );
        assert_eq!(state_by_name("nonsense"), None);
    }

    #[test]
    fn a_structure_is_a_fixed_target_with_its_own_signature() {
        let mut reg = world(&attr_text(&format!(";{ATTR_SIGNATURE_RADIUS}=777.0")));
        let e = Env::new(at(0));
        let s = shield_up(ASTRAHUS, &e);
        structure::put(reg.deltas_mut(), &s);
        assert!(is_lockable(&reg, s.id, s.system));
        assert!(!is_lockable(&reg, s.id, s.system + 1));
        assert!(!is_lockable(&reg, s.id + 1, s.system));
        let t = fixed_target(&reg, s.id).expect("a structure");
        assert_eq!((t.system, t.signature, t.radius), (s.system, 777.0, 500.0));
        // With no 552 of its own, the ball radius.
        let mut bare = world(&attr_text(""));
        structure::put(bare.deltas_mut(), &s);
        assert_eq!(fixed_target(&bare, s.id).unwrap().signature, 500.0);
        let _ = bare.take_pending();
    }

    // ---- the volley (#46) -----------------------------------------------------

    use crate::combat::tests::{ALICE, SYSTEM};

    /// An invented gun type: `hit` and the tally take the type as it is.
    const GUN: i64 = 990_700_001;
    /// A player corporation id nobody in the fixture belongs to.
    pub(crate) const OWNER_CORP: i64 = 98_700_001;
    /// Well under the per-second limit, so the cap does not touch it.
    const SMALL: f64 = LIMIT / 2.0;

    fn blue_now() -> BlueTime {
        crate::pyutil::filetime_now()
    }

    /// Alice in space in `SYSTEM`, a shield-vulnerable structure 5 km away
    /// in the same system (id 7), no limits other than the fixture's.
    fn shoot_world(attrs: &str) -> (Registry, i64) {
        let mut reg =
            crate::combat::tests::locked_on(&[], Some(std::sync::Arc::new(content_with(attrs))));
        let e = crate::structure_space::env(&reg, blue_now());
        let mut s = Structure::anchored(
            7,
            ASTRAHUS,
            OWNER_CORP,
            SYSTEM,
            [5_000.0, 0.0, 0.0],
            0.0,
            "Target",
            1,
            &e,
        );
        s.set_state(State::ShieldVulnerable, &e);
        s.upkeep = Some(Upkeep::Full);
        structure::put(reg.deltas_mut(), &s);
        let _ = reg.take_pending();
        (reg, s.id)
    }

    fn volley(target: i64, damage: Damage, hit_quality: u8) -> Volley {
        Volley {
            shooter: ALICE,
            attacker_ship: crate::combat::tests::ALICE_SHIP,
            weapon_type: GUN,
            target,
            damage,
            hit_quality,
            banked: false,
            at: blue_now(),
        }
    }

    fn bars(reg: &Registry, id: i64) -> (f64, f64, f64) {
        structure::get(reg.deltas(), id)
            .and_then(|s| s.damage)
            .unwrap_or((1.0, 1.0, 1.0))
    }

    fn lines(reg: &mut Registry) -> Vec<PyValue> {
        crate::combat::tests::due(reg)
            .into_iter()
            .filter(|(_, n, _)| *n == "OnDamageMessage")
            .map(|(to, _, a)| {
                assert_eq!(to, vec![ALICE], "only the shooter reads it");
                a[0].clone()
            })
            .collect()
    }

    fn field(d: &PyValue, k: &str) -> Option<PyValue> {
        crate::pyutil::dict_get(d, k.as_bytes()).cloned()
    }

    #[test]
    fn a_turret_volley_takes_shield_off_a_vulnerable_structure() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let now = blue_now();
        hit(&mut reg, volley(id, kinetic(SMALL), 3), now);
        // Resonances read 1.0 in the fixture: the points taken are the volley.
        let hp = SHIELD * FULL_POWER_HP_MULTIPLIER;
        assert_eq!(bars(&reg, id), (1.0 - SMALL / hp, 1.0, 1.0));
        assert_eq!(
            reg.ext::<Fire>().taken_this_second(id, now)[0],
            SMALL,
            "recorded for the per-second limit"
        );
    }

    /// One kinetic volley of `d` on a structure with `upkeep`; the shield bar.
    fn shield_bar_after_volley(upkeep: Upkeep, d: f64) -> (f64, f64) {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.upkeep = Some(upkeep);
        structure::put(reg.deltas_mut(), &s);
        hit(&mut reg, volley(id, kinetic(d), 3), blue_now());
        let hp = |full| layers(reg.content(), ASTRAHUS, full).hp[0];
        assert!(d < hp(false), "the volley stays inside the bare layer");
        assert_ne!(
            hp(true),
            hp(false),
            "the fixture's full-power multiplier changes the layer"
        );
        let bar = bars(&reg, id).0;
        (bar, 1.0 - d / hp(upkeep == Upkeep::Full))
    }

    #[test]
    fn a_low_power_structure_takes_the_volley_on_its_bare_shield() {
        let (bar, want) = shield_bar_after_volley(Upkeep::Low, SMALL);
        assert!((bar - want).abs() < 1e-9, "{bar} vs {want}");
    }

    #[test]
    fn an_abandoned_structure_takes_the_volley_on_its_bare_shield() {
        let (bar, want) = shield_bar_after_volley(Upkeep::Abandoned, SMALL);
        assert!((bar - want).abs() < 1e-9, "{bar} vs {want}");
    }

    #[test]
    fn a_full_power_structure_still_uses_the_full_power_layers() {
        let (bar, want) = shield_bar_after_volley(Upkeep::Full, SMALL);
        assert!((bar - want).abs() < 1e-9, "{bar} vs {want}");
    }

    #[test]
    fn full_power_counts_unset_upkeep_as_full() {
        let (reg, id) = shoot_world(&attr_text(""));
        let mut s = structure::get(reg.deltas(), id).unwrap();
        for (u, want) in [
            (None, true),
            (Some(Upkeep::Full), true),
            (Some(Upkeep::Low), false),
            (Some(Upkeep::Abandoned), false),
        ] {
            s.upkeep = u;
            assert_eq!(full_power(&s), want, "{u:?}");
        }
    }

    #[test]
    fn a_volley_over_the_limit_is_capped() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        hit(&mut reg, volley(id, kinetic(LIMIT * 3.0), 3), blue_now());
        let hp = SHIELD * FULL_POWER_HP_MULTIPLIER;
        assert_eq!(bars(&reg, id).0, 1.0 - LIMIT / hp);
    }

    #[test]
    fn a_volley_at_an_invulnerable_structure_deals_nothing_and_logs_a_miss() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let now = blue_now();
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.set_state(
            State::ArmorReinforce,
            &crate::structure_space::env(&reg, now),
        );
        structure::put(reg.deltas_mut(), &s);
        let before = bars(&reg, id);
        let _ = reg.take_pending();
        hit(&mut reg, volley(id, kinetic(SMALL), 3), now);
        assert_eq!(bars(&reg, id), before, "the bars do not move");
        let out = lines(&mut reg);
        assert_eq!(out.len(), 1);
        assert_eq!(field(&out[0], "hitQuality"), Some(PyValue::Int(0)));
        assert_eq!(field(&out[0], "damage"), Some(PyValue::Real(0.0)));
    }

    #[test]
    fn a_miss_deals_nothing_to_a_vulnerable_structure() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        hit(&mut reg, volley(id, kinetic(SMALL), 0), blue_now());
        assert_eq!(bars(&reg, id), (1.0, 1.0, 1.0));
    }

    #[test]
    fn the_shooter_gets_one_combat_line_naming_the_structure() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        hit(&mut reg, volley(id, kinetic(SMALL), 3), blue_now());
        let out = lines(&mut reg);
        assert_eq!(out.len(), 1, "one line, to the shooter");
        assert_eq!(
            field(&out[0], "attackType"),
            Some(crate::pyutil::bstr("me"))
        );
        assert_eq!(field(&out[0], "target"), Some(PyValue::Int(id)));
        assert_eq!(field(&out[0], "weapon"), Some(PyValue::Int(GUN)));
        assert_eq!(field(&out[0], "damage"), Some(PyValue::Real(SMALL)));
        assert_eq!(field(&out[0], "hitQuality"), Some(PyValue::Int(3)));
    }

    #[test]
    fn a_hit_keeps_the_repair_paused() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let now = blue_now();
        hit(&mut reg, volley(id, kinetic(SMALL), 3), now);
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_eq!(s.repairing, Some(false), "paused by the hit");
        assert!(s.timer.is_paused());
        // Quiet for longer than the window: the world tick resumes it.
        let later = now + (UNDER_FIRE_WINDOW_SECS + 2) * structure::SECOND;
        crate::structure_space::tick(&mut reg, std::time::Instant::now(), later);
        let s = structure::get(reg.deltas(), id).unwrap();
        assert_eq!(s.repairing, Some(true), "resumed after the window");
    }

    #[test]
    fn a_structure_in_another_system_is_not_hit() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let mut s = structure::get(reg.deltas(), id).unwrap();
        s.system = SYSTEM + 1;
        structure::put(reg.deltas_mut(), &s);
        let _ = reg.take_pending();
        hit(&mut reg, volley(id, kinetic(SMALL), 3), blue_now());
        assert_eq!(bars(&reg, id), (1.0, 1.0, 1.0));
        assert!(lines(&mut reg).is_empty(), "no line either");
        assert!(!crate::crimewatch::has_weapons_timer(
            &reg,
            ALICE,
            blue_now()
        ));
    }

    #[test]
    fn a_volley_at_a_structure_is_aggression() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        let now = blue_now();
        assert!(!crate::crimewatch::has_weapons_timer(&reg, ALICE, now));
        // Even a miss.
        hit(&mut reg, volley(id, kinetic(SMALL), 0), now);
        assert!(crate::crimewatch::has_weapons_timer(&reg, ALICE, now));
    }

    #[test]
    fn a_volley_is_tallied_for_the_killmail() {
        let (mut reg, id) = shoot_world(&attr_text(""));
        hit(&mut reg, volley(id, kinetic(SMALL), 3), blue_now());
        let hits = reg.ext_mut::<crate::killmail::Tally>().take(id, blue_now());
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].who, hits[0].damage), (ALICE, SMALL));
    }

    #[test]
    fn the_surface_gap_is_the_centre_gap_less_the_radius() {
        let (reg, id) = shoot_world(&attr_text(""));
        let t = fixed_target(&reg, id).unwrap();
        let from = [t.position[0] + t.radius + 1_234.0, 0.0, 0.0];
        assert_eq!(surface_gap(&reg, from, id), Some(1_234.0));
        let inside = [t.position[0], 0.0, 0.0];
        assert_eq!(surface_gap(&reg, inside, id), Some(0.0), "never negative");
        assert_eq!(surface_gap(&reg, from, id + 1), None);
    }
}

#[cfg(test)]
mod death_tests;
