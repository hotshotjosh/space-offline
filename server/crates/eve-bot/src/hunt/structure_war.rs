//! **Bots attack and defend Upwell structures** (STR-BOT, #213).
//!
//! A submodule of [`crate::hunt`] (the hunter's own file), so no new `mod` line
//! in the hot `lib.rs`; what other files need is re-exported through `hunt`.
//!
//! What a bot may use (nothing from nowhere, no GM in organic play): the
//! structure's slim, global in its system (`structure_space::extras_of`: `state`,
//! `timer`, the owner corp and alliance), the bars `OnDamageStateChange` tells
//! for it, the war list in the hall, and the verbs it already has: `WarpToItem`,
//! `Lock`, `Fire`, `Orbit`, drones, `Unlock`.
//!
//! 1. **May it** ([`may_attack`]): never the bot's own corporation or
//!    alliance; otherwise only at war, or on a GM's `/bots siege` order
//!    (**[A]** policy, owner decision 7).
//! 2. **What now** ([`decide`], pure): a vulnerable state is approached and
//!    engaged; a reinforced one is left and returned to [`crate::sov_bot::LEAD_SECS`]
//!    before its timer ends; anything else (invulnerable, no timer) is left.
//! 3. **The attack** ([`attack_turn`]): a hunter on a GM order flies to the
//!    ordered system and engages the structure it finds there; a hunter of a
//!    gang whose corporation is at war remembers the war structures it sees
//!    and, once per [`crate::sov_bot::PLAN_EVERY`], sorties to one within
//!    [`crate::sov_bot::VIEW_JUMPS`] when its window is open.
//! 4. **The defence** ([`defend_turn`]): a ratter whose corporation owns a
//!    structure in its system, with the structure's bars below full and a
//!    hostile pilot near it, calls for help and, if bold and armed, warps to it
//!    and fights the nearest hostile as `ess_guard` does.
//!
//! **State** is [`Mind`] (`Memory::structure_war`).

use super::{Pilot, fight, guard_with};
use crate::brain::{self, Act, Ball, Memory, Observation, ParkView, Place};
use crate::explore_roam::Nerve;
use crate::roam_gang::GangAct;
use crate::sov_bot::{LEAD_SECS, PLAN_EVERY, SOV_FLOOR, VIEW_JUMPS};
use crate::threat::{Alert, Call, Cause};
use eve_server::group_contacts::Affiliation;
use eve_server::structure::{SECOND, State};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How far from the structure's surface the bots warp in and fight, metres
/// **[A]**. Falsified by guns out of range at 20 km (the bots sit idle on the
/// grid), or by a bot that cannot hold range under fire (move closer or farther).
pub const SIEGE_RANGE: f64 = 20_000.0;
/// A bot within this surface distance of the structure fights without warping
/// again, metres **[A]**. Falsified by a bot that warps back and forth.
pub const NEAR: f64 = 2.0 * SIEGE_RANGE;
/// Warps toward a structure are at least this far apart **[A]**. Falsified by
/// a bot that gives up before its warp lands (too short) or idles at the
/// start (too long).
pub const APPROACH_EVERY: Duration = Duration::from_secs(20);
/// A war structure found out of its window is looked at again after this many
/// seconds **[A]** when it shows no timer. Falsified by sorties that arrive
/// to a closed structure (too short) or windows missed (too long).
pub const RECHECK_SECS: i64 = 600;
/// A sortie ends after this long **[A]**: the flight, the lock and the fight.
/// Falsified by sorties cut off with the structure still shooting back (it
/// does not) or hours lost on one that cannot be hurt.
pub const SORTIE_MAX: Duration = Duration::from_secs(20 * 60);
/// How far from its own corporation's structure a hostile pilot counts as
/// attacking it, metres **[A]**. Falsified by defenders that ignore a shooter
/// beyond it (too short) or answer passers-by (too long).
pub const DEFEND_NEAR: f64 = 150_000.0;
/// The nerve at which a ratter fights for the structure **[A]**: the ESS
/// guard's own line ([`crate::ess_guard::ENGAGE_NERVE`]).
pub const ENGAGE_NERVE: f64 = crate::ess_guard::ENGAGE_NERVE;
/// Most structures remembered.
pub const SEEN_MAX: usize = 32;
/// The wait of a bot that holds for a window or for an order's end.
const HOLD_WAIT: Duration = Duration::from_secs(5);

/// One structure ball, from its slim (the driver fills it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub id: i64,
    pub type_id: i64,
    pub position: [f64; 3],
    pub radius: f64,
    pub owner_corp: i64,
    pub owner_alliance: Option<i64>,
    /// `structures.STATE_*` code.
    pub state: i64,
    /// `(start, end, paused)` FILETIME ticks.
    pub timer: Option<(i64, i64, Option<i64>)>,
    /// The hall says the bot's side is at war with the owner's.
    pub at_war: bool,
    /// The owner is a blue: not hostile to the bot by the hall's books. Never
    /// attacked, order or not.
    pub blue: bool,
}

/// The structures in the bot's system, and who the bot is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub me_corp: i64,
    pub me_alliance: Option<i64>,
    /// FILETIME now.
    pub now_ft: i64,
    pub structures: Vec<View>,
}

impl Scene {
    /// A structure of the bot's own corporation is in the system.
    pub fn has_own(&self) -> bool {
        self.me_corp > 0 && self.structures.iter().any(|v| v.owner_corp == self.me_corp)
    }
}

/// What to do about one structure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SiegeAct {
    /// Warp to it (the id).
    Approach(i64),
    /// Lock it and shoot with everything.
    Engage(i64),
    /// Not before this FILETIME (the window's lead): wait or go.
    Hold(i64),
    /// Nothing to do here.
    Leave,
}

/// **May the bot attack** (pure): never its own corporation or alliance, nor
/// a blue; otherwise at war, or on a GM's order **[A]** (owner decision 7).
pub fn may_attack(
    me_corp: i64,
    me_alliance: Option<i64>,
    owner_corp: i64,
    owner_alliance: Option<i64>,
    blue: bool,
    at_war: bool,
    ordered: bool,
) -> bool {
    if blue || owner_corp <= 0 || owner_corp == me_corp {
        return false;
    }
    if me_alliance.is_some() && me_alliance == owner_alliance {
        return false;
    }
    at_war || ordered
}

/// **Is the siege legal where the bot is** (pure): a war target anywhere; an
/// ordered one without war only on PvP ground (low sec, null sec, wormhole),
/// where CONCORD does not answer.
pub fn siege_legal(at_war: bool, ordered: bool, ground: bool) -> bool {
    at_war || !ordered || ground
}

/// **Is the state one the client may shoot at** (pure): the vulnerable states
/// of `structures.STATE_VULNERABILITY`.
pub fn attackable(state: i64) -> bool {
    State::from_code(state).and_then(State::vulnerability) == Some(true)
}

/// **What now** for one structure (pure). `near`: the bot is within [`NEAR`]
/// of its surface.
pub fn decide(s: &Scene, v: &View, ordered: bool, near: bool) -> SiegeAct {
    if !may_attack(
        s.me_corp,
        s.me_alliance,
        v.owner_corp,
        v.owner_alliance,
        v.blue,
        v.at_war,
        ordered,
    ) {
        return SiegeAct::Leave;
    }
    if attackable(v.state) {
        return if near {
            SiegeAct::Engage(v.id)
        } else {
            SiegeAct::Approach(v.id)
        };
    }
    match v.timer {
        Some((_, end, None)) if end > s.now_ft => {
            let back = end - LEAD_SECS * SECOND;
            if back > s.now_ft {
                SiegeAct::Hold(back)
            } else if near {
                SiegeAct::Hold(end)
            } else {
                SiegeAct::Approach(v.id)
            }
        }
        _ => SiegeAct::Leave,
    }
}

/// A structure the bot may go back to, and from when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub system: i64,
    /// FILETIME: attackable now when not after this.
    pub ready_at: i64,
}

/// What a war structure in view is worth remembering as (pure): `None` when
/// the bot may not attack it by the war rule.
pub fn seen_row(s: &Scene, v: &View, system: i64) -> Option<Seen> {
    if !may_attack(
        s.me_corp,
        s.me_alliance,
        v.owner_corp,
        v.owner_alliance,
        v.blue,
        v.at_war,
        false,
    ) {
        return None;
    }
    let ready_at = if attackable(v.state) {
        0
    } else {
        match v.timer {
            Some((_, end, None)) if end > s.now_ft => end - LEAD_SECS * SECOND,
            _ => s.now_ft + RECHECK_SECS * SECOND,
        }
    };
    Some(Seen { system, ready_at })
}

/// **Which remembered structure to go to** (pure): ready now, within
/// [`VIEW_JUMPS`] of the bot (`jumps(system)`), the nearest, then the sooner
/// ready, then the lower id. Returns `(structure, system)`.
pub fn plan(
    seen: &BTreeMap<i64, Seen>,
    now_ft: i64,
    jumps: impl Fn(i64) -> Option<u32>,
) -> Option<(i64, i64)> {
    seen.iter()
        .filter(|(_, s)| s.ready_at <= now_ft)
        .filter_map(|(id, s)| {
            Some((
                jumps(s.system).filter(|j| *j <= VIEW_JUMPS)?,
                s.ready_at,
                *id,
                s.system,
            ))
        })
        .min()
        .map(|(_, _, id, system)| (id, system))
}

/// A war sortie under way.
#[derive(Debug, Clone, PartialEq)]
pub struct Sortie {
    pub structure: i64,
    pub system: i64,
    pub since: Instant,
}

/// What the bot remembers (`Memory::structure_war`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The GM's siege order: the system to attack in.
    pub order: Option<i64>,
    /// War structures seen, by id.
    pub seen: BTreeMap<i64, Seen>,
    pub sortie: Option<Sortie>,
    pub next_plan: Option<Instant>,
    /// The structure the bot has locked and shoots.
    pub engaged: Option<i64>,
    pub warped_at: Option<Instant>,
    /// A distress is out for the structure under attack.
    pub called: bool,
    /// The own structures' last bars read, by id, and when they last fell.
    pub bars: BTreeMap<i64, Track>,
}

/// One own structure's bars as last read, and when a read last showed a bar
/// lower than the one before.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub bars: [f64; 3],
    pub fell_at: Option<Instant>,
}

/// How long a fall in a structure's bars counts as "under attack" **[A]**.
/// Falsified by defenders that stand down while the structure is still being
/// shot (too short) or answer a fight that ended long ago (too long).
pub const HURT_WINDOW: Duration = Duration::from_secs(60);

/// **Track a structure's bars** (pure): a bar lower than the previous read is
/// a fall; the first read never is, nor is a structure that merely sits below
/// full.
pub fn track(prev: Option<Track>, bars: [f64; 3], now: Instant) -> Track {
    let fell_at = match prev {
        Some(p) if bars.iter().zip(p.bars).any(|(n, o)| *n < o) => Some(now),
        Some(p) => p.fell_at,
        None => None,
    };
    Track { bars, fell_at }
}

/// Whether the structure is under attack now: a fall within [`HURT_WINDOW`].
pub fn is_hurt(t: &Track, now: Instant) -> bool {
    t.fell_at
        .is_some_and(|at| now.saturating_duration_since(at) <= HURT_WINDOW)
}

impl Mind {
    /// The driver's word from `/bots siege` (`Some(system)`) or `/bots
    /// unsiege` (`None`).
    pub fn set_order(&mut self, word: Option<i64>) {
        self.order = word;
        if word.is_none() {
            self.sortie = None;
        }
    }
}

/// What a defender does (pure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Defence {
    Ignore,
    /// Call for help and carry on.
    Distress,
    /// Call for help and fight.
    Fight,
}

/// **The defender's verdict** (pure): the structure's bars fell lately
/// (`hurt`, [`track`]), a hostile pilot is near it (`intruders`), and the bot
/// is bold and armed. It fights only a pilot its side is at war with
/// (`at_war`: how many of the intruders); any other hostile is only called in.
pub fn defence(hurt: bool, intruders: usize, at_war: usize, nerve: f64, armed: bool) -> Defence {
    if !hurt || intruders == 0 {
        Defence::Ignore
    } else if nerve >= ENGAGE_NERVE && armed && at_war > 0 {
        Defence::Fight
    } else {
        Defence::Distress
    }
}

fn is_hunter(m: &Memory) -> bool {
    std::ptr::eq(m.policy.role, &super::ROLE)
        || std::ptr::eq(m.policy.role, &crate::roam_gang::ROLE)
}

/// The pilot the escort's fight is shown for a structure.
fn as_prey(v: &View) -> Pilot {
    Pilot {
        ball: Ball {
            id: v.id,
            position: v.position,
            radius: v.radius,
        },
        char_id: 0,
        type_id: Some(v.type_id),
        who: Affiliation {
            character_id: 0,
            corporation_id: v.owner_corp,
            alliance_id: v.owner_alliance,
        },
        hostile: true,
        relation: 0.0,
        war: true,
    }
}

/// Remember the war structures in view, and forget the ones gone.
fn remember(m: &mut Memory, o: &Observation, system: i64) {
    let s = &o.structures;
    let mind = &mut m.structure_war;
    mind.seen
        .retain(|id, row| row.system != system || s.structures.iter().any(|v| v.id == *id));
    for v in &s.structures {
        match seen_row(s, v, system) {
            Some(row) => {
                mind.seen.insert(v.id, row);
            }
            None => {
                mind.seen.remove(&v.id);
            }
        }
    }
    while mind.seen.len() > SEEN_MAX {
        mind.seen.pop_first();
    }
}

/// The acts that let go of a structure: the lock, and the target.
fn release(m: &mut Memory) -> Option<Vec<Act>> {
    let id = m.structure_war.engaged.take()?;
    m.target = None;
    m.hunt.prey = None;
    Some(vec![Act::Unlock(id)])
}

/// **The attack turn** (module header, 3): `None` leaves the turn to the role.
pub fn attack_turn(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !is_hunter(m) {
        return None;
    }
    let (Some(park), Place::InSpace { system, .. }) = (&o.park, o.place) else {
        return None;
    };
    remember(m, o, system);
    // Survival first: a hostile pilot on the grid, or a hurt ship, is the
    // hunter's own turn.
    if o.hunt.pilots.iter().any(|p| {
        p.hostile
            && o.park
                .as_ref()
                .is_some_and(|k| k.ego.surface_to(&p.ball) <= super::NEAR)
    }) || super::retreat(o.health, o.hunt.temper, f64::INFINITY).is_some()
    {
        return release(m);
    }
    // A sortie that ran out ends.
    if m.structure_war
        .sortie
        .as_ref()
        .is_some_and(|s| o.now.saturating_duration_since(s.since) >= SORTIE_MAX)
    {
        m.structure_war.sortie = None;
    }
    // Organic: a gang at war plans a sortie, no oftener than PLAN_EVERY.
    if m.structure_war.order.is_none()
        && m.structure_war.sortie.is_none()
        && m.policy.gang.is_some()
        && o.hunt.war_on
        && m.homing.is_none()
        && m.structure_war
            .next_plan
            .is_none_or(|t| o.now.saturating_duration_since(t) >= PLAN_EVERY)
    {
        m.structure_war.next_plan = Some(o.now);
        let map = m.policy.travel.clone();
        let jumps = |to: i64| {
            if to == system {
                Some(0)
            } else {
                map.as_ref()?.jumps(system, to, SOV_FLOOR)
            }
        };
        if let Some((structure, to)) = plan(&m.structure_war.seen, o.structures.now_ft, jumps) {
            crate::counter::log_decision(
                "structure",
                &m.name,
                "sortie",
                structure,
                &format!("structure={structure} system={to}"),
                true,
            );
            m.structure_war.sortie = Some(Sortie {
                structure,
                system: to,
                since: o.now,
            });
        }
    }
    let ordered = m.structure_war.order.is_some();
    let target_system = match (m.structure_war.order, &m.structure_war.sortie) {
        (Some(s), _) => s,
        (None, Some(s)) => s.system,
        (None, None) => return release(m),
    };
    // Hurt, unarmed or going home: the role's own turn.
    if m.homing.is_some() || o.guns.is_empty() {
        return release(m);
    }
    if system != target_system {
        if m.gang.fly_failed >= 3 {
            m.structure_war.sortie = None;
            return if ordered {
                Some(vec![Act::Wait(HOLD_WAIT)])
            } else {
                None
            };
        }
        return Some(vec![Act::Gang(GangAct::Fly {
            system: target_system,
        })]);
    }
    let scene = &o.structures;
    let ground = m.policy.pvp.as_ref().is_some_and(|d| d.is_ground(system));
    let allowed = |v: &&View| {
        may_attack(
            scene.me_corp,
            scene.me_alliance,
            v.owner_corp,
            v.owner_alliance,
            v.blue,
            v.at_war,
            ordered,
        )
    };
    let view = match &m.structure_war.sortie {
        Some(s) if !ordered => scene.structures.iter().find(|v| v.id == s.structure),
        _ => scene
            .structures
            .iter()
            .filter(allowed)
            .filter(|v| siege_legal(v.at_war, ordered, ground))
            .min_by_key(|v| (!attackable(v.state), v.id)),
    };
    if view.is_none()
        && scene
            .structures
            .iter()
            .filter(allowed)
            .any(|v| !siege_legal(v.at_war, ordered, ground))
    {
        // High sec, no war: CONCORD would answer. Hold.
        crate::counter::log_decision(
            "structure",
            &m.name,
            "refuse-highsec",
            system,
            &format!("system={system}"),
            false,
        );
        let mut acts = release(m).unwrap_or_default();
        acts.push(Act::Wait(HOLD_WAIT));
        return Some(acts);
    }
    let Some(view) = view.copied() else {
        // Gone, or none to attack: the sortie is over, an order waits.
        let acts = release(m);
        if ordered {
            return Some(acts.unwrap_or_else(|| vec![Act::Wait(HOLD_WAIT)]));
        }
        m.structure_war.sortie = None;
        return acts;
    };
    let near = park.ego.surface_to(&Ball {
        id: view.id,
        position: view.position,
        radius: view.radius,
    }) <= NEAR;
    match decide(scene, &view, ordered, near) {
        SiegeAct::Engage(id) => {
            m.structure_war.engaged = Some(id);
            crate::counter::log_decision(
                "structure",
                &m.name,
                "engage",
                id,
                &format!("structure={id} state={}", view.state),
                true,
            );
            let acts = fight(m, o, park, &as_prey(&view), Vec::new());
            Some(guard_with(acts, o, Some(id), &[]))
        }
        SiegeAct::Approach(id) => {
            if park.ego_warping
                || m.structure_war
                    .warped_at
                    .is_some_and(|t| o.now.saturating_duration_since(t) < APPROACH_EVERY)
            {
                return Some(vec![Act::Wait(brain::SHORT_WAIT)]);
            }
            m.structure_war.warped_at = Some(o.now);
            Some(vec![Act::WarpToItem {
                id,
                range: SIEGE_RANGE,
            }])
        }
        SiegeAct::Hold(_) if ordered => {
            let mut acts = release(m).unwrap_or_default();
            acts.push(Act::Wait(HOLD_WAIT));
            Some(acts)
        }
        // Organic: leave it; `remember` took its window, and the planner
        // brings the gang back when it opens.
        SiegeAct::Hold(_) | SiegeAct::Leave => {
            m.structure_war.sortie = None;
            release(m)
        }
    }
}

/// **The defence turn** (module header, 4): `None` leaves the turn to the
/// role.
pub fn defend_turn(m: &mut Memory, o: &Observation, park: &ParkView) -> Option<Vec<Act>> {
    let Place::InSpace { .. } = o.place else {
        return None;
    };
    let scene = &o.structures;
    if scene.me_corp <= 0 {
        return None;
    }
    // The own structure under attack: its bars fell lately, a hostile near it.
    for v in scene
        .structures
        .iter()
        .filter(|v| v.owner_corp == scene.me_corp)
    {
        if let Some(h) = o.logi.bars.get(&v.id) {
            let prev = m.structure_war.bars.get(&v.id).copied();
            let t = track(prev, [h.shield, h.armour, h.hull], o.now);
            m.structure_war.bars.insert(v.id, t);
        }
    }
    let verdict = scene
        .structures
        .iter()
        .filter(|v| v.owner_corp == scene.me_corp)
        .find_map(|v| {
            let hurt = m
                .structure_war
                .bars
                .get(&v.id)
                .is_some_and(|t| is_hurt(t, o.now));
            let intruders: Vec<&Pilot> = o
                .hunt
                .pilots
                .iter()
                .filter(|p| p.hostile && dist(p.ball.position, v.position) <= DEFEND_NEAR)
                .collect();
            let nerve = Nerve {
                risk: o.hunt.temper.risk,
                aggression: o.hunt.temper.aggression,
            }
            .value();
            let at_war = intruders.iter().filter(|p| p.war).count();
            let d = defence(hurt, intruders.len(), at_war, nerve, !o.guns.is_empty());
            (d != Defence::Ignore).then_some((d, *v, intruders))
        });
    let Some((verdict, view, intruders)) = verdict else {
        m.structure_war.called = false;
        if m.structure_war.warped_at.is_some() {
            m.structure_war.warped_at = None;
            m.target = None;
            m.hunt.prey = None;
        }
        return None;
    };
    let mut acts = Vec::new();
    if !m.structure_war.called {
        m.structure_war.called = true;
        crate::counter::log_decision(
            "structure",
            &m.name,
            "defend",
            view.id,
            &format!("structure={} verdict={verdict:?}", view.id),
            true,
        );
        acts.extend(call(m, o));
    }
    if verdict == Defence::Distress {
        return Some(acts).filter(|a| !a.is_empty());
    }
    // Bold and armed: on the grid, the nearest hostile; else warp in.
    let nearest = intruders
        .iter()
        .filter(|p| p.war)
        .min_by(|a, b| {
            park.ego
                .surface_to(&a.ball)
                .total_cmp(&park.ego.surface_to(&b.ball))
        })
        .map(|p| (*p).clone());
    match nearest {
        Some(p) if dist(park.ego.position, p.ball.position) <= DEFEND_NEAR => {
            acts.extend(fight(m, o, park, &p, Vec::new()));
            Some(guard_with(acts, o, Some(p.ball.id), &[]))
        }
        _ => {
            if park.ego_warping
                || m.structure_war
                    .warped_at
                    .is_some_and(|t| o.now.saturating_duration_since(t) < APPROACH_EVERY)
            {
                acts.push(Act::Wait(brain::SHORT_WAIT));
            } else {
                m.structure_war.warped_at = Some(o.now);
                acts.push(Act::WarpToItem {
                    id: view.id,
                    range: SIEGE_RANGE,
                });
            }
            Some(acts)
        }
    }
}

/// The distress, once per attack: a fleet's broadcast, else the hall's board
/// (as `ess_guard::call`).
fn call(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match (o.fleet, o.ship) {
        (Some(_), Some(ship)) => vec![Act::Fleet(crate::fleet::Order::Broadcast {
            name: crate::fleet::BROADCAST_NEED_BACKUP,
            item: ship.ship_id,
            type_id: None,
        })],
        _ => {
            m.threat.alert = Some(Alert {
                cause: Cause::Mate,
                foes: BTreeSet::new(),
                belt: None,
                call: Call::Pending,
                said: true,
                said_mate: true,
                docked_at: None,
                until: None,
                extended: false,
            });
            Vec::new()
        }
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids.
    const ME: i64 = 98_000_001;
    const FOE: i64 = 98_000_002;
    const ALLY_AL: i64 = 99_000_001;
    const NOW: i64 = 1_000_000 * SECOND;

    fn scene() -> Scene {
        Scene {
            me_corp: ME,
            me_alliance: Some(ALLY_AL),
            now_ft: NOW,
            structures: Vec::new(),
        }
    }

    fn view(owner: i64, state: State, timer: Option<(i64, i64, Option<i64>)>) -> View {
        View {
            id: 7_001,
            type_id: 1_000_001,
            position: [0.0; 3],
            radius: 5_000.0,
            owner_corp: owner,
            owner_alliance: None,
            state: state.code(),
            timer,
            at_war: true,
            blue: false,
        }
    }

    #[test]
    fn a_bot_never_attacks_its_own_or_allied_structure() {
        // Own corp: not at war, not even on an order.
        assert!(!may_attack(ME, None, ME, None, false, true, true));
        // Allied: same alliance, whatever the war list says.
        assert!(!may_attack(
            ME,
            Some(ALLY_AL),
            FOE,
            Some(ALLY_AL),
            false,
            true,
            true
        ));
        // A blue (the hall does not call it hostile): never, order or not.
        assert!(!may_attack(ME, None, FOE, None, true, true, true));
        assert!(may_attack(ME, None, FOE, None, false, false, true));
        let mut b = view(FOE, State::ShieldVulnerable, None);
        b.blue = true;
        assert_eq!(decide(&scene(), &b, true, true), SiegeAct::Leave);
        assert!(seen_row(&scene(), &b, 5).is_none());
        // Unknown owner.
        assert!(!may_attack(ME, None, 0, None, false, true, true));
        // And through `decide`.
        let mut v = view(ME, State::ShieldVulnerable, None);
        assert_eq!(decide(&scene(), &v, true, true), SiegeAct::Leave);
        v.owner_corp = FOE;
        v.owner_alliance = Some(ALLY_AL);
        assert_eq!(decide(&scene(), &v, true, true), SiegeAct::Leave);
    }

    #[test]
    fn a_reinforced_structure_is_left_and_returned_to_before_its_timer() {
        let end = NOW + 3_600 * SECOND;
        let v = view(FOE, State::ArmorReinforce, Some((NOW - SECOND, end, None)));
        // Far from the window: hold until the lead before it ends.
        assert_eq!(
            decide(&scene(), &v, false, true),
            SiegeAct::Hold(end - LEAD_SECS * SECOND)
        );
        // Inside the lead and far away: go.
        let mut s = scene();
        s.now_ft = end - 10 * SECOND;
        assert_eq!(decide(&s, &v, false, false), SiegeAct::Approach(v.id));
        // The planner takes it back when its time is up, not before.
        let row = seen_row(&scene(), &v, 5).unwrap();
        assert_eq!(row.ready_at, end - LEAD_SECS * SECOND);
        let seen = BTreeMap::from([(v.id, row)]);
        assert_eq!(plan(&seen, NOW, |_| Some(1)), None);
        assert_eq!(plan(&seen, row.ready_at, |_| Some(1)), Some((v.id, 5)));
    }

    #[test]
    fn a_vulnerable_war_target_structure_is_engaged() {
        let v = view(FOE, State::ShieldVulnerable, None);
        assert!(attackable(v.state));
        assert_eq!(decide(&scene(), &v, false, false), SiegeAct::Approach(v.id));
        assert_eq!(decide(&scene(), &v, false, true), SiegeAct::Engage(v.id));
        // Invulnerable with no timer: nothing to do.
        let q = view(FOE, State::FittingInvulnerable, None);
        assert_eq!(decide(&scene(), &q, false, true), SiegeAct::Leave);
        // Too far to plan for.
        let seen = BTreeMap::from([(v.id, seen_row(&scene(), &v, 5).unwrap())]);
        assert_eq!(plan(&seen, NOW, |_| Some(VIEW_JUMPS + 1)), None);
        assert_eq!(plan(&seen, NOW, |_| Some(VIEW_JUMPS)), Some((v.id, 5)));
    }

    #[test]
    fn a_siege_order_overrides_the_war_rule() {
        let mut v = view(FOE, State::ShieldVulnerable, None);
        v.at_war = false;
        assert_eq!(decide(&scene(), &v, false, true), SiegeAct::Leave);
        assert_eq!(decide(&scene(), &v, true, true), SiegeAct::Engage(v.id));
        assert!(seen_row(&scene(), &v, 5).is_none());
    }

    #[test]
    fn a_defender_answers_an_attack_on_its_corps_structure() {
        // Bars full, or nobody near it: nothing.
        assert_eq!(defence(false, 2, 2, 0.9, true), Defence::Ignore);
        assert_eq!(defence(true, 0, 0, 0.9, true), Defence::Ignore);
        // Hurt with a shooter near: a bold armed bot fights, a timid one calls.
        assert_eq!(defence(true, 1, 1, ENGAGE_NERVE, true), Defence::Fight);
        assert_eq!(
            defence(true, 1, 1, ENGAGE_NERVE - 0.1, true),
            Defence::Distress
        );
        assert_eq!(defence(true, 1, 1, 0.9, false), Defence::Distress);
        // A neutral intruder (no war) is only called in, never fought.
        assert_eq!(defence(true, 1, 0, 0.9, true), Defence::Distress);
        // Only the own corp's structure counts.
        let mut s = scene();
        s.structures.push(view(FOE, State::ShieldVulnerable, None));
        assert!(!s.has_own());
        s.structures.push(view(ME, State::ShieldVulnerable, None));
        assert!(s.has_own());
    }

    const SYS: i64 = 30_990_777;

    fn memory(order: Option<i64>) -> Memory {
        let mut policy = crate::brain::Policy::fast();
        policy.role = &super::super::ROLE;
        let mut m = Memory::new("Invented Hunter", policy);
        m.structure_war.order = order;
        m
    }

    fn pilot(id: i64, x: f64, war: bool) -> Pilot {
        Pilot {
            ball: Ball {
                id,
                position: [x, 0.0, 0.0],
                radius: 50.0,
            },
            char_id: id + 1_000,
            type_id: Some(1),
            who: Affiliation {
                character_id: id + 1_000,
                corporation_id: FOE,
                alliance_id: None,
            },
            hostile: true,
            relation: 0.0,
            war,
        }
    }

    fn obs(structures: Vec<View>) -> Observation {
        let mut o = Observation::empty(std::time::Instant::now());
        o.place = Place::InSpace {
            system: SYS,
            bound: true,
        };
        o.guns = vec![crate::brain::GunInfo {
            item_id: 2,
            type_id: 1,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(1),
            capacity: 1,
            range: 1_000.0,
            reload: Duration::ZERO,
            kind: crate::brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.park = Some(ParkView {
            ego: Ball {
                id: 1,
                position: [0.0; 3],
                radius: 50.0,
            },
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        });
        o.hunt.temper.risk = 0.9;
        o.hunt.temper.aggression = 0.9;
        o.structures = Scene {
            structures,
            ..scene()
        };
        o
    }

    #[test]
    fn a_siege_order_without_war_is_refused_in_high_sec() {
        let mut v = view(FOE, State::ShieldVulnerable, None);
        v.at_war = false;
        let o = obs(vec![v]);
        // High sec (or unknown): the order waits, nothing is locked.
        let mut m = memory(Some(SYS));
        m.policy.pvp = Some(std::sync::Arc::new(
            crate::hunt::Ground::new().with_security(SYS, 0.9),
        ));
        let acts = attack_turn(&mut m, &o).expect("a wait");
        assert!(matches!(acts[..], [Act::Wait(_)]), "{acts:?}");
        assert_eq!(m.structure_war.engaged, None);
        // Low sec: the order is carried out.
        let mut m = memory(Some(SYS));
        m.policy.pvp = Some(std::sync::Arc::new(
            crate::hunt::Ground::new().with_security(SYS, 0.2),
        ));
        assert!(attack_turn(&mut m, &o).is_some());
        assert_eq!(m.structure_war.engaged, Some(v.id));
        // High sec with a war: legal.
        let mut w = v;
        w.at_war = true;
        let mut m = memory(Some(SYS));
        m.policy.pvp = Some(std::sync::Arc::new(
            crate::hunt::Ground::new().with_security(SYS, 0.9),
        ));
        assert!(attack_turn(&mut m, &obs(vec![w])).is_some());
        assert_eq!(m.structure_war.engaged, Some(w.id));
        assert!(siege_legal(true, true, false));
        assert!(!siege_legal(false, true, false));
    }

    #[test]
    fn a_hunter_with_a_hostile_pilot_on_the_grid_keeps_its_own_turn() {
        let v = view(FOE, State::ShieldVulnerable, None);
        let mut o = obs(vec![v]);
        let mut m = memory(Some(SYS));
        m.policy.pvp = Some(std::sync::Arc::new(
            crate::hunt::Ground::new().with_security(SYS, 0.2),
        ));
        o.hunt.pilots = vec![pilot(50, 10_000.0, false)];
        assert_eq!(attack_turn(&mut m, &o), None);
        assert_eq!(m.structure_war.engaged, None);
        // Far off the grid it does not count.
        o.hunt.pilots = vec![pilot(50, 10.0 * crate::hunt::NEAR, false)];
        assert!(attack_turn(&mut m, &o).is_some());
    }

    #[test]
    fn a_fall_in_the_bars_is_hurt_for_a_window_only() {
        let t0 = std::time::Instant::now();
        // The first read, however low, is no fall.
        let a = track(None, [1.0, 0.5, 1.0], t0);
        assert!(!is_hurt(&a, t0));
        // The same read again: no fall.
        let b = track(Some(a), [1.0, 0.5, 1.0], t0 + Duration::from_secs(5));
        assert!(!is_hurt(&b, t0 + Duration::from_secs(5)));
        // A lower bar: a fall, for the window.
        let c = track(Some(b), [1.0, 0.4, 1.0], t0 + Duration::from_secs(10));
        let at = t0 + Duration::from_secs(10);
        assert!(is_hurt(&c, at));
        assert!(is_hurt(&c, at + HURT_WINDOW));
        assert!(!is_hurt(&c, at + HURT_WINDOW + Duration::from_secs(1)));
        // A repair (a higher bar) is no fall.
        let d = track(Some(a), [1.0, 0.9, 1.0], at);
        assert!(!is_hurt(&d, at));
    }

    #[test]
    fn a_defender_never_fights_a_passer_by_who_did_not_shoot() {
        let own = view(ME, State::ShieldVulnerable, None);
        let mut o = obs(vec![own]);
        o.structures.me_corp = ME;
        let mut policy = crate::brain::Policy::fast();
        policy.role = &crate::site_runner::ROLE;
        let mut m = Memory::new("Invented Ratter", policy);
        let park = o.park.clone().unwrap();
        let hit = |m: &mut Memory, o: &mut Observation, shield: f64| {
            o.logi.bars.insert(
                own.id,
                crate::brain::Health {
                    shield,
                    armour: 1.0,
                    hull: 1.0,
                },
            );
            defend_turn(m, o, &park)
        };
        assert_eq!(hit(&mut m, &mut o, 1.0), None);
        // A neutral pilot is near; the bars fall: a call, no fight.
        o.hunt.pilots = vec![pilot(60, 20_000.0, false)];
        o.now += Duration::from_secs(5);
        let acts = hit(&mut m, &mut o, 0.8);
        assert!(acts.is_none(), "{acts:?}");
        assert!(m.threat.alert.is_some(), "the distress is out");
        assert_eq!(m.target, None);
        // The same pilot at war with the bot's side: it is fought.
        o.hunt.pilots = vec![pilot(60, 20_000.0, true)];
        o.now += Duration::from_secs(5);
        let acts = hit(&mut m, &mut o, 0.6).expect("a fight");
        assert!(!acts.is_empty());
    }
}
