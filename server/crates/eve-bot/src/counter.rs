//! **Operational counters** (BP-COUNTER, issue #124, L7 of
//! `handback/r-bot-pvp/report.md`): what a bot does when the enemy's utility
//! is on it. Each rule is pure; the escort, the hunter and the gang FC ask.
//!
//! | The enemy has | The bots do | Here |
//! |---|---|---|
//! | Point, scram | kill the tackler first ([`priority`], [`W_ROLE`]) | `hunt::pick`, `roam_gang::choose_primary` |
//! | ECM | target the jammer from its `OnEwarStart` ([`priority`]) | the same |
//! | Damp | close under the cut lock range ([`orbit`]) | `escort::in_space` |
//! | Tracking disruptor | turret ships close under the cut optimal ([`orbit`]); a ship with no weapon keeps going | the same |
//! | Guidance disruptor | missile ships close in and keep the nearest prey ([`orbit`], [`nearest_first`]) | the same, `hunt::pick` |
//! | Smartbomb (`effects.EMPWave` seen) | tag the ship a bomber for [`BOMBER_TTL`] and keep the orbit above [`BOMB_SAFE`] ([`Sense`], [`orbit`]) | the same |
//! | MJD (`effects.MicroJumpDriveEngage` seen) | scram it inside [`MJD_WINDOW`] ([`mjd_scram`]) | `escort::in_space` |
//! | Bubble | while held, the interdictor or HIC is the prey ([`BUBBLE_GROUPS`]) | `hunt::pick` |
//! | Cloak | nothing here: a cloaked ball is not on the grid, and `Cause::Decloak` already summons a fighter | `threat.rs` |
//!
//! **Difficulty** (`content/bots/difficulty.tsv`): `KIT_DISCIPLINE`
//! ([`CounterKnobs`]) is how well the bots keep these rules. At level 0 it is
//! `0` and every rule is off, so today's behaviour stands; above it the
//! seen-bomber tag and the MJD answer wait [`REACT_MAX_S`] times `1 - it`.
//! The leader's half (`COUNTER_READ`) is in `doctrine_lead`.
//!
//! Every number here is **[A]** policy and none is in a test by value.
//! Falsifiers: a tackler that stays alive while the fleet shoots the
//! logistics (raise [`W_ROLE`]); a bot that flips between two tacklers
//! (it is below the stick bonus's reach: lower it); a damped bot that sits
//! out of lock range for minutes ([`DAMP_SHARE`] too high); a bomber the bot
//! cannot hit because the orbit above [`BOMB_SAFE`] is past its guns (the
//! rule then does nothing, by design); a scrambler fired at an MJD that has
//! already jumped (shorten [`MJD_WINDOW`]).

use crate::brain::{Act, Ball, EwarKind, GunKind, Memory, Observation};
use crate::ewar_aim::Foe;
use crate::hunt::Arm;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The bonus a tackler, a jammer or a bubbler gets in the prey's score **[A]**.
/// Above [`crate::hunt::W_STICK`], so a tackler that arrives takes the fire
/// from the prey already engaged.
pub const W_ROLE: f64 = 150.0;
/// Hull groups that make a bubble: interdictors and heavy interdictors
/// **[V]** (`handback/r-bot-pvp`, from typedata). Re-check at the 11-09 port.
pub const BUBBLE_GROUPS: [i64; 2] = [541, 894];
/// A damped bot orbits under this share of its (cut) lock range **[A]**.
pub const DAMP_SHARE: f64 = 0.8;
/// A turret ship under a tracking disruptor orbits at this share of its
/// range **[A]**: the cut is not read off the module, only assumed.
pub const TD_SHARE: f64 = 0.6;
/// A missile ship under a guidance disruptor, the same **[A]**.
pub const GD_SHARE: f64 = 0.6;
/// A bomber is kept this far (m) off: a large smartbomb's 5 km plus a margin
/// **[A]**; falsified by a bot bombed at this orbit.
pub const BOMB_SAFE: f64 = 7_000.0;
/// A bomber stays tagged this long after its last wave **[A]** (the report's 60 s).
pub const BOMBER_TTL: Duration = Duration::from_secs(60);
/// A drive's spool is about this long; the scram must land inside it **[A]**
/// (the report's 12 s).
pub const MJD_WINDOW: Duration = Duration::from_secs(12);
/// The slowest reaction, at the least discipline above zero **[A]**.
pub const REACT_MAX_S: f64 = 15.0;
/// The guids of the two effects the rules watch **[V]** (dogma `guid` of the
/// default effects 38 and 4921).
pub const FX_BOMB: &str = "effects.EMPWave";
pub const FX_MJD: &str = "effects.MicroJumpDriveEngage";

/// **The PvP report's line** (#136), `tools/pvp-report.py`'s `SIGNAL`:
/// `[tag] NAME: kind detail ok=1|0`. `detail` is `key=value` words or empty.
pub fn signal_line(tag: &str, bot: &str, kind: &str, detail: &str, ok: bool) -> String {
    let sep = if detail.is_empty() { "" } else { " " };
    format!("[{tag}] {bot}: {kind}{sep}{detail} ok={}", u8::from(ok))
}

std::thread_local! {
    /// What each `(tag, kind)` last logged on this thread (a bot has its own
    /// thread, named for it): the decision functions run every turn, the log
    /// line only when the decision changes.
    static LOGGED: std::cell::RefCell<BTreeMap<(String, String), String>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// Whether `key` is new for `(tag, kind)` on this thread; it is then recorded.
fn changed(tag: &str, kind: &str, key: String) -> bool {
    LOGGED.with(|l| {
        let mut l = l.borrow_mut();
        let k = (tag.to_string(), kind.to_string());
        if l.get(&k) == Some(&key) {
            false
        } else {
            l.insert(k, key);
            true
        }
    })
}

/// Print the report line to stderr, once per change of `(target, ok)` (a
/// decision, never a line per turn). Returns whether it printed.
pub fn log_decision(tag: &str, bot: &str, kind: &str, target: i64, detail: &str, ok: bool) -> bool {
    if !changed(tag, kind, format!("{target} {ok}")) {
        return false;
    }
    eprintln!("{}", signal_line(tag, bot, kind, detail, ok));
    true
}

/// The calling bot thread's name (the driver names it for the bot).
fn thread_bot() -> String {
    std::thread::current().name().unwrap_or("?").to_string()
}

/// **How well the bots keep the counters** (`KIT_DISCIPLINE`), `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CounterKnobs {
    pub discipline: f64,
}

impl CounterKnobs {
    pub const EASY: CounterKnobs = CounterKnobs { discipline: 0.4 };
    pub const NORMAL: CounterKnobs = CounterKnobs { discipline: 0.7 };
    pub const HARD: CounterKnobs = CounterKnobs { discipline: 0.9 };
    pub const BRUTAL: CounterKnobs = CounterKnobs { discipline: 1.0 };

    /// 1 easy .. 4 brutal; 2 normal; 0 is off, as the world was before.
    pub fn at(level: u8) -> CounterKnobs {
        match level {
            0 => CounterKnobs { discipline: 0.0 },
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }

    /// Whether any counter rule runs.
    pub fn on(&self) -> bool {
        self.discipline > 0.0
    }

    /// The wait before a counter to something newly seen.
    pub fn reaction(&self) -> Duration {
        Duration::from_secs_f64(REACT_MAX_S * (1.0 - self.discipline.clamp(0.0, 1.0)))
    }
}

impl Default for CounterKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// What the brain sees of the enemy's utility this turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    /// Ships tagged as bombers.
    pub bombers: Vec<i64>,
    /// Ships spooling a micro jump drive: `(ball, how long ago)`.
    pub engaging: Vec<(i64, Duration)>,
}

/// The driver's side: the `OnSpecialFX` the rules watch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sense {
    /// `ship -> (first wave seen, last wave seen)`.
    bombs: BTreeMap<i64, (Instant, Instant)>,
    /// `ship -> the engage's time`.
    engages: BTreeMap<i64, Instant>,
}

impl Sense {
    /// Fold in one `OnSpecialFX` of another ship; `guid` is its effect's.
    pub fn on_fx(&mut self, guid: &str, ship: i64, now: Instant) {
        match guid {
            FX_BOMB => {
                let e = self.bombs.entry(ship).or_insert((now, now));
                if now.saturating_duration_since(e.1) >= BOMBER_TTL {
                    e.0 = now;
                }
                e.1 = now;
            }
            FX_MJD => {
                self.engages.insert(ship, now);
            }
            _ => {}
        }
    }

    /// The turn's view: bombers past the reaction delay and not stale, drives
    /// still spooling inside [`MJD_WINDOW`]. Spent entries are dropped.
    pub fn view(&mut self, now: Instant, k: &CounterKnobs) -> Seen {
        self.bombs
            .retain(|_, (_, last)| now.saturating_duration_since(*last) < BOMBER_TTL);
        self.engages
            .retain(|_, at| now.saturating_duration_since(*at) < MJD_WINDOW);
        if !k.on() {
            return Seen::default();
        }
        let wait = k.reaction();
        Seen {
            bombers: self
                .bombs
                .iter()
                .filter(|(_, (first, _))| now.saturating_duration_since(*first) >= wait)
                .map(|(s, _)| *s)
                .collect(),
            engaging: self
                .engages
                .iter()
                .map(|(s, at)| (*s, now.saturating_duration_since(*at)))
                .filter(|(_, age)| *age >= wait)
                .collect(),
        }
    }
}

/// **Who to kill first** (pure): the ships tackling or jamming this one,
/// newest first, from the scram view. Empty when the rules are off.
pub fn priority(view: &crate::scram::View, k: &CounterKnobs) -> Vec<i64> {
    if !k.on() {
        return Vec::new();
    }
    view.ewar
        .iter()
        .filter(|(_, kind)| {
            matches!(
                kind,
                GunKind::Point | GunKind::Scram | GunKind::Ewar(EwarKind::Ecm)
            )
        })
        .map(|(s, _)| *s)
        .collect()
}

/// Whether the ship under guidance disruption keeps the nearest prey: it
/// shoots missiles and a guidance disruptor is on it.
pub fn nearest_first(view: &crate::scram::View, arm: Arm, k: &CounterKnobs) -> bool {
    k.on()
        && arm == Arm::Missiles
        && view
            .ewar
            .iter()
            .any(|(_, kind)| *kind == GunKind::Ewar(EwarKind::Guide))
}

/// Fill the hunter's scene with this turn's counters (the driver, once a turn).
///
/// #136: `held` means a refused warp **and** a hostile bubbler hull on grid.
/// The refusal alone also comes from a friendly dictor's sphere
/// (`bubble::binds` binds friends too), which is no reason to shoot anyone.
pub fn steer(o: &mut Observation, arm: Arm, k: &CounterKnobs, doc: Option<&crate::hunt::Ground>) {
    o.hunt.priority = priority(&o.scram, k);
    o.hunt.held = k.on()
        && o.warp_bubble.held
        && doc.is_some_and(|d| {
            o.hunt
                .pilots
                .iter()
                .any(|p| p.hostile && d.is_bubbler(p.type_id))
        });
    o.hunt.nearest_first = nearest_first(&o.scram, arm, k);
}

/// **The orbit** for `target` (pure but for the report line it logs, [`log_engaged`]): the escort's fight range, shortened for
/// a damp, a tracking disruptor or a guidance disruptor on this ship, and
/// held above [`BOMB_SAFE`] against a bomber when the guns reach it.
pub fn orbit(m: &Memory, o: &Observation, target: i64) -> f64 {
    let base = crate::escort::fight_range(&o.guns);
    let k = m.policy.counter;
    if k.on() {
        log_engaged(m, o, target);
    }
    let reach = o
        .guns
        .iter()
        .filter(|g| g.kind == GunKind::Weapon && g.online)
        .map(|g| g.range)
        .fold(0.0, f64::max);
    if !k.on() || reach <= 0.0 {
        return base;
    }
    let arm = m
        .policy
        .pvp
        .as_ref()
        .map_or(Arm::Unknown, |d| d.arm_of(o.ship.and_then(|s| s.type_id)));
    let on_me = |e: EwarKind| {
        o.scram
            .ewar
            .iter()
            .any(|(_, kind)| *kind == GunKind::Ewar(e))
    };
    let mut r = base;
    if on_me(EwarKind::Damp) && o.lock_range > 0.0 {
        r = r.min(o.lock_range * DAMP_SHARE);
    }
    if on_me(EwarKind::Track) && arm != Arm::Missiles {
        r = r.min(base * TD_SHARE);
    }
    if on_me(EwarKind::Guide) && arm == Arm::Missiles {
        r = r.min(base * GD_SHARE);
    }
    if o.counter.bombers.contains(&target) {
        if reach >= BOMB_SAFE {
            r = r.max(BOMB_SAFE);
        }
        log_decision(
            "counter",
            &m.name,
            "bomb-keepoff",
            target,
            &format!("target={target} range={}", r as i64),
            r >= BOMB_SAFE,
        );
    }
    r
}

/// #136: the engaged `target` against the counters, as the report reads them.
/// A tackler or jammer on this ship is named by [`priority`]: `ok=1` when the
/// target is one of them, `ok=0` when the fight is on something else. Held in
/// a bubble: `ok=1` when the target is a bubbler hull, `ok=0` when not.
fn log_engaged(m: &Memory, o: &Observation, target: i64) {
    if !o.hunt.priority.is_empty() {
        log_decision(
            "counter",
            &m.name,
            "tackler-first",
            target,
            &format!("target={target}"),
            o.hunt.priority.contains(&target),
        );
    }
    if let Some(ok) = held_verdict(m, o, target) {
        log_decision(
            "counter",
            &m.name,
            "dictor-held",
            target,
            &format!("target={target}"),
            ok,
        );
    }
}

/// #136: the "held, shoot the dictor" rule against `target` (pure): `None`
/// when the bot is not held by a foe (see [`steer`]), else whether `target`
/// is a hostile bubbler hull.
fn held_verdict(m: &Memory, o: &Observation, target: i64) -> Option<bool> {
    if !o.hunt.held {
        return None;
    }
    Some(m.policy.pvp.as_ref().is_some_and(|d| {
        o.hunt
            .pilots
            .iter()
            .find(|p| p.ball.id == target)
            .is_some_and(|p| p.hostile && d.is_bubbler(p.type_id))
    }))
}

/// **The answer to a drive spooling** (pure): a ready scrambler at the ship
/// that engaged, inside [`MJD_WINDOW`] and its reach. A lock first when it is
/// not locked. `None` when no scrambler is ready, none reaches, or the rules
/// are off.
pub fn mjd_scram(o: &Observation, k: &CounterKnobs, foes: &[Foe], ego: &Ball) -> Option<Act> {
    if !k.on() {
        return None;
    }
    let answer = mjd_answer(o, foes, ego);
    // #136: one line per drive spooling, answered (`ok=1`) or not (`ok=0`);
    // a lock already sent and not yet landed is neither. Only a bot with an
    // online scrambler is meant to answer, and only a foe's drive counts
    // (`engaging` holds fleetmates too).
    if let Some((target, ok)) = mjd_logged(o, foes, answer.as_ref().map(|a| a.0))
        && (ok || !o.pending.contains(&target))
    {
        let bot = thread_bot();
        log_decision(
            "counter",
            &bot,
            "mjd-scram",
            target,
            &format!("target={target}"),
            ok,
        );
    }
    answer.map(|a| a.1)
}

/// **What [`mjd_scram`] logs** (pure): `(target, ok)`. `None` for a bot with
/// no online scrambler, or when no drive in `engaging` is a foe's. An answer
/// is `ok`; otherwise the first foe drive that was not answered is `ok=0`.
fn mjd_logged(o: &Observation, foes: &[Foe], answered: Option<i64>) -> Option<(i64, bool)> {
    if !o.guns.iter().any(|g| g.kind == GunKind::Scram && g.online) {
        return None;
    }
    if let Some(t) = answered {
        return Some((t, true));
    }
    o.counter
        .engaging
        .iter()
        .map(|(id, _)| *id)
        .find(|id| foes.iter().any(|f| f.ball.id == *id))
        .map(|id| (id, false))
}

/// [`mjd_scram`]'s choice: the drive's ball and the act.
fn mjd_answer(o: &Observation, foes: &[Foe], ego: &Ball) -> Option<(i64, Act)> {
    let gun = o
        .guns
        .iter()
        .find(|g| g.kind == GunKind::Scram && g.online && !g.active)?;
    let (target, _) = o
        .counter
        .engaging
        .iter()
        .filter(|(_, age)| *age < MJD_WINDOW)
        .find(|(id, _)| {
            foes.iter().any(|f| {
                f.ball.id == *id && (gun.reach <= 0.0 || ego.surface_to(&f.ball) <= gun.reach)
            })
        })?;
    if o.locked.contains(target) {
        let act = Act::Fire {
            gun: gun.item_id,
            target: *target,
        };
        Some((*target, act))
    } else if o.pending.contains(target) {
        None
    } else {
        Some((*target, Act::Lock(*target)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::GunInfo;

    const ME: i64 = 7_001;
    const A: i64 = 7_101;
    const B: i64 = 7_102;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 0.0,
        }
    }

    fn gun(id: i64, kind: GunKind, range: f64) -> GunInfo {
        GunInfo {
            item_id: id,
            type_id: 1,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(10),
            capacity: 10,
            range,
            reload: Duration::from_secs(1),
            kind,
            reach: range,
        }
    }

    fn view(ewar: Vec<(i64, GunKind)>) -> crate::scram::View {
        crate::scram::View {
            ewar,
            ..Default::default()
        }
    }

    fn memory() -> Memory {
        Memory::new(
            "Counter1",
            crate::brain::Policy {
                role: crate::brain::Role::Escort,
                ..crate::brain::Policy::fast()
            },
        )
    }

    fn obs(guns: Vec<GunInfo>, ewar: Vec<(i64, GunKind)>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.guns = guns;
        o.scram = view(ewar);
        o.lock_range = 40_000.0;
        o
    }

    const DICTOR: i64 = 7_900;
    const HULL: i64 = 7_901;

    fn pilot(id: i64, type_id: i64, hostile: bool) -> crate::hunt::Pilot {
        crate::hunt::Pilot {
            ball: ball(id, 1_000.0),
            char_id: id + 1_000,
            type_id: Some(type_id),
            who: eve_server::group_contacts::Affiliation {
                character_id: id + 1_000,
                corporation_id: 9_001,
                alliance_id: None,
            },
            hostile,
            relation: 0.0,
            war: false,
        }
    }

    fn doctrine() -> crate::hunt::Ground {
        crate::hunt::Ground::new().with_fit(DICTOR, BUBBLE_GROUPS[0], 0, 0)
    }

    /// #136: held needs a refused warp and a hostile bubbler on grid; a
    /// friendly dictor's sphere (which also binds friends) is no reason.
    #[test]
    fn held_needs_a_hostile_bubbler_on_grid() {
        let k = CounterKnobs::NORMAL;
        let d = doctrine();
        let mut o = obs(vec![], vec![]);
        o.warp_bubble.held = true;
        o.hunt.pilots = vec![pilot(A, DICTOR, false), pilot(B, HULL, true)];
        steer(&mut o, Arm::Unknown, &k, Some(&d));
        assert!(!o.hunt.held, "a friendly dictor only");
        o.hunt.pilots = vec![pilot(A, DICTOR, true)];
        steer(&mut o, Arm::Unknown, &k, Some(&d));
        assert!(o.hunt.held, "a hostile bubbler");
        steer(&mut o, Arm::Unknown, &k, None);
        assert!(!o.hunt.held, "no doctrine");
        o.warp_bubble.held = false;
        steer(&mut o, Arm::Unknown, &k, Some(&d));
        assert!(!o.hunt.held, "not held");
    }

    #[test]
    fn dictor_held_is_logged_for_a_victim_of_a_hostile_bubbler_only() {
        let mut m = memory();
        m.policy.pvp = Some(std::sync::Arc::new(doctrine()));
        let k = CounterKnobs::NORMAL;
        let d = doctrine();
        let mut o = obs(vec![], vec![]);
        o.warp_bubble.held = true;
        o.hunt.pilots = vec![pilot(A, DICTOR, true), pilot(B, HULL, true)];
        steer(&mut o, Arm::Unknown, &k, Some(&d));
        assert_eq!(held_verdict(&m, &o, A), Some(true));
        assert_eq!(held_verdict(&m, &o, B), Some(false));
        o.hunt.pilots = vec![pilot(A, DICTOR, false), pilot(B, HULL, true)];
        steer(&mut o, Arm::Unknown, &k, Some(&d));
        assert_eq!(held_verdict(&m, &o, A), None, "only a friendly bubbler");
        assert_eq!(held_verdict(&m, &o, B), None);
    }

    #[test]
    fn a_tackler_and_a_jammer_come_first() {
        let v = view(vec![
            (A, GunKind::Point),
            (B, GunKind::Ewar(EwarKind::Ecm)),
            (ME, GunKind::Web),
            (ME + 1, GunKind::Ewar(EwarKind::Damp)),
        ]);
        assert_eq!(priority(&v, &CounterKnobs::NORMAL), vec![A, B]);
        assert!(
            priority(&v, &CounterKnobs::at(0)).is_empty(),
            "level 0 keeps today's behaviour"
        );
    }

    #[test]
    fn a_damped_ship_closes_under_its_lock_range() {
        let m = memory();
        let guns = vec![gun(1, GunKind::Weapon, 60_000.0)];
        let plain = orbit(&m, &obs(guns.clone(), vec![]), A);
        let damped = orbit(&m, &obs(guns, vec![(B, GunKind::Ewar(EwarKind::Damp))]), A);
        assert!(damped < plain, "{damped} {plain}");
        assert!(damped < 40_000.0);
    }

    #[test]
    fn a_disrupted_turret_ship_closes_and_a_logi_keeps_going() {
        let m = memory();
        let td = vec![(B, GunKind::Ewar(EwarKind::Track))];
        let gunner = orbit(
            &m,
            &obs(vec![gun(1, GunKind::Weapon, 20_000.0)], td.clone()),
            A,
        );
        assert!(gunner < 20_000.0);
        // No weapon: the escort's own rule stands.
        let logi = obs(vec![gun(2, GunKind::Neut, 8_000.0)], td);
        assert_eq!(orbit(&m, &logi, A), crate::escort::fight_range(&logi.guns));
    }

    #[test]
    fn a_guidance_disruptor_does_nothing_to_a_turret_ship() {
        let m = memory();
        let o = obs(
            vec![gun(1, GunKind::Weapon, 20_000.0)],
            vec![(B, GunKind::Ewar(EwarKind::Guide))],
        );
        assert_eq!(orbit(&m, &o, A), 20_000.0);
        assert!(!nearest_first(
            &o.scram,
            Arm::Turrets,
            &CounterKnobs::NORMAL
        ));
        assert!(nearest_first(
            &o.scram,
            Arm::Missiles,
            &CounterKnobs::NORMAL
        ));
        assert!(!nearest_first(
            &o.scram,
            Arm::Missiles,
            &CounterKnobs::at(0)
        ));
    }

    #[test]
    fn a_seen_bomber_is_tagged_and_kept_off() {
        let t0 = Instant::now();
        let mut s = Sense::default();
        s.on_fx(FX_BOMB, A, t0);
        // A fresh sighting waits out the reaction delay, a brutal one does not.
        let k = CounterKnobs::NORMAL;
        assert!(s.view(t0, &k).bombers.is_empty());
        let later = t0 + k.reaction();
        assert_eq!(s.view(later, &k).bombers, vec![A]);
        s.on_fx(FX_BOMB, A, later);
        assert_eq!(s.view(later, &CounterKnobs::BRUTAL).bombers, vec![A]);
        // Stale after the tag lapses.
        assert!(s.view(later + BOMBER_TTL, &k).bombers.is_empty());
        // The orbit stays above the bomb range when the guns reach it.
        let m = memory();
        let mut o = obs(vec![gun(1, GunKind::Weapon, BOMB_SAFE * 2.0)], vec![]);
        let plain = orbit(&m, &o, A);
        o.counter.bombers = vec![A];
        let safe = orbit(&m, &o, A);
        assert!(safe >= BOMB_SAFE);
        assert!(safe >= plain.min(BOMB_SAFE));
        // Guns that cannot reach past the bomb keep their own orbit.
        let mut short = obs(vec![gun(1, GunKind::Weapon, BOMB_SAFE / 2.0)], vec![]);
        short.counter.bombers = vec![A];
        assert_eq!(orbit(&m, &short, A), BOMB_SAFE / 2.0);
    }

    #[test]
    fn an_engaging_drive_is_scrammed_inside_the_window() {
        let t0 = Instant::now();
        let mut s = Sense::default();
        s.on_fx(FX_MJD, A, t0);
        let k = CounterKnobs::BRUTAL;
        let ego = ball(ME, 0.0);
        let foes = [Foe {
            ball: ball(A, 5_000.0),
            logi: false,
            arm: Arm::Unknown,
        }];
        let mut o = obs(vec![gun(9, GunKind::Scram, 9_000.0)], vec![]);
        o.counter = s.view(t0, &k);
        assert_eq!(mjd_scram(&o, &k, &foes, &ego), Some(Act::Lock(A)));
        o.locked.push(A);
        assert_eq!(
            mjd_scram(&o, &k, &foes, &ego),
            Some(Act::Fire { gun: 9, target: A })
        );
        // Past the window the drive is gone.
        assert!(s.view(t0 + MJD_WINDOW, &k).engaging.is_empty());
        // Out of the scrambler's reach, or off: nothing.
        let far = [Foe {
            ball: ball(A, 50_000.0),
            ..foes[0]
        }];
        assert_eq!(mjd_scram(&o, &k, &far, &ego), None);
        assert_eq!(mjd_scram(&o, &CounterKnobs::at(0), &foes, &ego), None);
    }

    #[test]
    fn an_unanswered_drive_is_logged_only_for_a_scrambled_bot_and_a_foe() {
        let t0 = Instant::now();
        let mut s = Sense::default();
        s.on_fx(FX_MJD, B, t0); // a fleetmate's drive
        s.on_fx(FX_MJD, A, t0);
        let k = CounterKnobs::BRUTAL;
        let foes = [Foe {
            ball: ball(A, 5_000.0),
            logi: false,
            arm: Arm::Unknown,
        }];
        let mut o = obs(vec![gun(9, GunKind::Scram, 9_000.0)], vec![]);
        o.counter = s.view(t0, &k);
        assert_eq!(o.counter.engaging.len(), 2);
        // The foe, not the fleetmate, is the unanswered one.
        assert_eq!(mjd_logged(&o, &foes, None), Some((A, false)));
        assert_eq!(mjd_logged(&o, &foes, Some(A)), Some((A, true)));
        // No foe among the drives: nothing to log.
        assert_eq!(mjd_logged(&o, &[], None), None);
        // No scrambler: the bot was never meant to answer.
        let bare = {
            let mut b = obs(vec![gun(1, GunKind::Weapon, 9_000.0)], vec![]);
            b.counter = o.counter.clone();
            b
        };
        assert_eq!(mjd_logged(&bare, &foes, None), None);
        // An offline scrambler is no scrambler.
        let mut off = gun(9, GunKind::Scram, 9_000.0);
        off.online = false;
        let mut o2 = obs(vec![off], vec![]);
        o2.counter = o.counter.clone();
        assert_eq!(mjd_logged(&o2, &foes, None), None);
    }

    #[test]
    fn the_levels_order_the_discipline() {
        let d: Vec<f64> = (0..=4).map(|l| CounterKnobs::at(l).discipline).collect();
        assert_eq!(d[0], 0.0);
        assert!(d.windows(2).all(|w| w[0] <= w[1]), "{d:?}");
        assert!(CounterKnobs::BRUTAL.reaction() < CounterKnobs::EASY.reaction());
        assert_eq!(CounterKnobs::BRUTAL.reaction(), Duration::ZERO);
    }

    #[test]
    fn the_report_line_has_the_shape_the_report_reads() {
        assert_eq!(
            signal_line("counter", "Hunter1", "mjd-scram", "target=5", false),
            "[counter] Hunter1: mjd-scram target=5 ok=0"
        );
        assert_eq!(
            signal_line("camp", "Dictor1", "holding", "gate=6 system=7", true),
            "[camp] Dictor1: holding gate=6 system=7 ok=1"
        );
        assert_eq!(
            signal_line("camp", "Dictor1", "holding", "", true),
            "[camp] Dictor1: holding ok=1"
        );
    }

    #[test]
    fn a_decision_prints_once_until_it_changes() {
        assert!(log_decision(
            "counter",
            "T1",
            "test-kind",
            A,
            "target=1",
            true
        ));
        assert!(!log_decision(
            "counter",
            "T1",
            "test-kind",
            A,
            "target=1",
            true
        ));
        assert!(log_decision(
            "counter",
            "T1",
            "test-kind",
            B,
            "target=2",
            true
        ));
        assert!(log_decision(
            "counter",
            "T1",
            "test-kind",
            B,
            "target=2",
            false
        ));
        assert!(!log_decision(
            "counter",
            "T1",
            "test-kind",
            B,
            "target=2",
            false
        ));
    }

    #[test]
    fn the_engaged_target_is_judged_against_the_tackler() {
        // The two outcomes, each a change the log would print.
        let mut o = obs(vec![gun(1, GunKind::Weapon, 20_000.0)], vec![]);
        o.hunt.priority = vec![A];
        let m = memory();
        orbit(&m, &o, A);
        assert!(!log_decision(
            "counter",
            &m.name,
            "tackler-first",
            A,
            "",
            true
        ));
        orbit(&m, &o, B);
        assert!(!log_decision(
            "counter",
            &m.name,
            "tackler-first",
            B,
            "",
            false
        ));
    }
}
