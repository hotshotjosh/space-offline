//! **The ESS guard** (BOT-ESS-GUARD, #60): the defending side of the
//! Encounter Surveillance System for bots that rat.
//!
//! The hunter ([`crate::ess_raid`]) links to a system's main bank. A ratter
//! of that system does not let a stranger do it for free:
//!
//! 1. **Read** the bank, at most once per [`POLL_EVERY`] while ratting, with
//!    the hunter's own call (`essMgr.GetDataForClientSolarSystem`, run by the
//!    driver as `Act::Hunt(HuntAct::Ess(EssAct::Read))`; the answer lands in
//!    `Memory::hunt.ess.view`).  A system read once and found without an ESS
//!    is never read again.
//! 2. **Place the linker by affiliation, not by grid presence.** The bot's
//!    own corp and alliance and the linker's come from the driver
//!    (`EssAct::Whois`: the session's `corpid`/`allianceid` and
//!    `charMgr.GetPublicInfo`, the call the fleet invite rule already uses),
//!    once per linker, cached in [`State`]. A linker on the bot's grid is
//!    also judged by the hall's hostility. A link held by the bot, a corp or
//!    alliance mate or the fleet boss is not a theft.
//! 3. A **stranger** on the link is a theft in progress: the bot **posts a
//!    distress** (a fleet's `NeedBackup` broadcast, else the hall's board
//!    through the driver's `post_distress`, as [`crate::threat`] does) and,
//!    if its nerve allows ([`ENGAGE_NERVE`]) and it is armed, **warps to the
//!    beacon and fights the linker** (`hunt::fight`) once he is seen on the
//!    grid. A timid bot posts and carries on ratting.
//! 4. A linker the bot **cannot place** ([`Holder::Unknown`]) draws no
//!    distress: a bold armed bot warps to the beacon to look, and decides
//!    when it sees him. At most [`MAX_WARPS`] warps per link; then it goes
//!    back to ratting.
//! 5. The link ends (read says nobody holds it, or a mate does): **stop**,
//!    drop the fight and go back to the sites.
//!
//! The decision is [`decide`] (pure); [`step`] is the thin turn around it.
//!
//! **State.** The guard's own [`State`] (`Memory::ess_guard`); it does not
//! touch the hunter's `Memory::hunt.ess` except to read the view and the
//! list of systems found bare. **[A]** policy throughout; the ESS read and
//! its dict are **[V]** in `ess_raid`.

use crate::brain::{self, Act, Memory, Observation, ParkView, Place};
use crate::ess_raid::{EssAct, EssView};
use crate::explore_roam::Nerve;
use crate::fleet::{Affiliation, may_invite};
use crate::hunt::{HuntAct, Pilot};
use crate::threat::{Alert, Call, Cause};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How often a ratter reads its system's ESS **[A]**. A hunter holds a link
/// for minutes and the ESS's own theft timer is longer, so half a minute
/// sees a link in time to answer it while a 50-bot soak adds under two reads
/// a second. Falsified by a soak whose top call counts show
/// `GetDataForClientSolarSystem` (too short), or by a link that ends before
/// any guard noticed it (too long).
pub const POLL_EVERY: Duration = Duration::from_secs(30);

/// A view older than this is not believed to say the link still holds
/// **[A]**: reads that fail must not keep a fight going. Falsified by guards
/// that drop a fight for want of a read the driver was merely slow to make.
pub const STALE_AFTER: Duration = Duration::from_secs(4 * 30);

/// The nerve ([`Nerve::value`]) at which a bot goes to the beacon and fights
/// **[A]**: the explorers' own bold line, so the timid third of the ratters
/// only call for help. Falsified by guards lost at the beacon in numbers
/// (too low) or by banks emptied under the eyes of idle ratters (too high).
pub const ENGAGE_NERVE: f64 = crate::explore_roam::BOLD_NERVE;

/// Warps to the beacon per link **[A]**: one to go, one retry for a warp
/// that was interrupted. Then the bot is back at its sites. Falsified by
/// guards that stop short of a linker who was only a jump late, or by a
/// bot still shuttling to the beacon in the act log.
pub const MAX_WARPS: u32 = 2;

/// After a warp to the beacon is ordered, how long before the bot believes it
/// has landed and looked **[A]** (a warp in progress is seen on the ball).
/// Falsified by guards that give up while their warp is still under way.
pub const LOOK_FOR: Duration = Duration::from_secs(20);

/// Most linkers whose affiliation is remembered; the cache is emptied past it.
const WHO_CAP: usize = 64;

/// The guard's memory (`Memory::ess_guard`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// When the ESS may next be read (`None`: now).
    pub next_read: Option<Instant>,
    /// The linker the rest of this state is about.
    pub linker: Option<i64>,
    /// A distress is out for this link.
    pub called: bool,
    /// When the last warp to the beacon was ordered.
    pub warped_at: Option<Instant>,
    /// Warps ordered for this link.
    pub warps: u32,
    /// The bot's own corp and alliance, from the driver.
    pub own: Option<Affiliation>,
    /// Linkers asked about; `None`: asked, and the answer did not come.
    pub who: BTreeMap<i64, Option<Affiliation>>,
}

impl State {
    /// The driver's answer to [`EssAct::Whois`].
    pub fn on_who(&mut self, char_id: i64, own: Option<Affiliation>, them: Option<Affiliation>) {
        self.own = own;
        self.who.insert(char_id, them);
    }

    /// A distress or a fight is under way for the current link.
    pub fn alarmed(&self) -> bool {
        self.called || self.warped_at.is_some()
    }
}

/// Who holds the link, as far as the bot can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// The bot itself.
    Me,
    /// A corp or alliance mate, or the fleet boss: not a thief.
    Mate,
    /// Placed, and not ours.
    Stranger,
    /// Not placed (the lookup failed or is not made yet).
    Unknown,
}

/// What the bot knows of the system's ESS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ess {
    /// Not read yet (or the view is of another system).
    Unknown,
    /// Read, and there is none.
    Absent,
    /// Read: `linked_by` is the character holding the main bank link.
    Present {
        linked_by: Option<i64>,
        /// Time since the read.
        age: Duration,
    },
}

/// What [`decide`] reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brief {
    pub ess: Ess,
    pub holder: Holder,
    /// The linker's affiliation has not been asked for yet.
    pub need_who: bool,
    /// [`Nerve::value`].
    pub nerve: f64,
    /// The bot has guns.
    pub armed: bool,
    /// Time since the ESS was last read (or asked for); `None`: never.
    pub since_read: Option<Duration>,
    /// A distress or a fight is out for the current link.
    pub alarmed: bool,
    /// The warps for this link are spent: no more trips to the beacon.
    pub spent: bool,
}

/// What the guard does this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing for the guard to do.
    Ignore,
    /// Read the ESS (the poll is due).
    Read,
    /// Ask whose corp the linker is in.
    Whois,
    /// A stranger holds the link; the bot only calls for help.
    Distress,
    /// A stranger holds the link; call, and go and fight.
    DistressAndEngage,
    /// Nobody can say who holds the link: go and look, no call.
    Look,
    /// The link is over: end the alarm and the fight.
    Stop,
}

/// **Who holds the link** (pure). `known` is the linker's affiliation from
/// the driver's lookup, `own` the bot's; `grid` is the linker when he is on
/// the bot's grid (his affiliation and whether the hall calls him hostile:
/// not allied, not the fleet boss). Anyone neither placed nor on the grid is
/// [`Holder::Unknown`], not a stranger.
pub fn holder(
    linker: i64,
    me: i64,
    boss: Option<i64>,
    own: Option<Affiliation>,
    known: Option<Affiliation>,
    grid: Option<(Affiliation, bool)>,
) -> Holder {
    if linker == me {
        return Holder::Me;
    }
    if boss == Some(linker) {
        return Holder::Mate;
    }
    if grid.is_some_and(|(_, hostile)| !hostile) {
        return Holder::Mate;
    }
    match (own, grid.map(|(a, _)| a).or(known)) {
        (Some(own), Some(them)) if may_invite(own, them) => Holder::Mate,
        (Some(_), Some(_)) => Holder::Stranger,
        _ => Holder::Unknown,
    }
}

/// **The decision** (pure; module header).
pub fn decide(b: &Brief) -> Verdict {
    let over = if b.alarmed {
        Verdict::Stop
    } else {
        Verdict::Ignore
    };
    let due = b.since_read.is_none_or(|t| t >= POLL_EVERY);
    let goes = b.armed && b.nerve >= ENGAGE_NERVE && !b.spent;
    match b.ess {
        // No ESS, no action.
        Ess::Absent => over,
        Ess::Unknown if due => Verdict::Read,
        Ess::Unknown => over,
        Ess::Present { .. } if due => Verdict::Read,
        Ess::Present { age, .. } if age >= STALE_AFTER => over,
        Ess::Present {
            linked_by: None, ..
        } => over,
        Ess::Present { .. } => match b.holder {
            Holder::Me | Holder::Mate => over,
            Holder::Unknown if b.need_who => Verdict::Whois,
            Holder::Unknown if goes => Verdict::Look,
            Holder::Unknown => Verdict::Ignore,
            Holder::Stranger if goes => Verdict::DistressAndEngage,
            Holder::Stranger => Verdict::Distress,
        },
    }
}

/// The turn: `Some(acts)` when the guard has the turn, `None` to carry on
/// ratting. Called by the site runner on a quiet turn.
pub fn step(m: &mut Memory, o: &Observation, park: &ParkView) -> Option<Vec<Act>> {
    let Place::InSpace { system, .. } = o.place else {
        return None;
    };
    // A view of another system is no view.
    if m.hunt.ess.view.as_ref().is_some_and(|v| v.system != system) {
        m.hunt.ess.view = None;
        m.ess_guard.next_read = None;
        stop(m);
    }
    let view = m.hunt.ess.view.clone();
    let known_none = m.hunt.ess.none.contains(&system);
    let linker = view.as_ref().and_then(|v| v.linked_by);
    // A new linker is a new case.
    if m.ess_guard.linker != linker {
        stop(m);
        m.ess_guard.linker = linker;
    }
    let since_read = m
        .ess_guard
        .next_read
        .map(|next| POLL_EVERY.saturating_sub(next.saturating_duration_since(o.now)));
    let pilot = linker.and_then(|c| o.hunt.pilots.iter().find(|p| p.char_id == c));
    let boss = o.fleet.and_then(|f| f.boss);
    let known = linker.and_then(|c| m.ess_guard.who.get(&c).copied().flatten());
    let holder = linker.map_or(Holder::Mate, |c| {
        holder(
            c,
            o.me,
            boss,
            m.ess_guard.own,
            known,
            pilot.map(|p| {
                (
                    Affiliation {
                        corp: p.who.corporation_id,
                        alliance: p.who.alliance_id,
                    },
                    p.hostile,
                )
            }),
        )
    });
    let need_who = linker.is_some_and(|c| {
        c != o.me && boss != Some(c) && pilot.is_none() && !m.ess_guard.who.contains_key(&c)
    });
    let ess = if known_none {
        Ess::Absent
    } else {
        match &view {
            None => Ess::Unknown,
            Some(v) => Ess::Present {
                linked_by: v.linked_by,
                age: o.now.saturating_duration_since(v.read_at),
            },
        }
    };
    let nerve = Nerve {
        risk: o.hunt.temper.risk,
        aggression: o.hunt.temper.aggression,
    }
    .value();
    let b = Brief {
        ess,
        holder,
        need_who,
        nerve,
        armed: !o.guns.is_empty(),
        since_read,
        alarmed: m.ess_guard.alarmed(),
        spent: m.ess_guard.warps >= MAX_WARPS && pilot.is_none(),
    };
    match decide(&b) {
        Verdict::Read => {
            m.ess_guard.next_read = Some(o.now + POLL_EVERY);
            let mut acts = vec![Act::Hunt(HuntAct::Ess(EssAct::Read { system }))];
            // The poll is added to a fight in progress, never put in its place.
            if m.ess_guard.warped_at.is_some() {
                let b = Brief {
                    since_read: Some(Duration::ZERO),
                    ..b
                };
                if let Some(more) = act_on(m, o, park, decide(&b), view.as_ref(), pilot, linker) {
                    acts.extend(more);
                }
            }
            Some(acts)
        }
        v => act_on(m, o, park, v, view.as_ref(), pilot, linker),
    }
}

/// Carry out every verdict but `Read`.
fn act_on(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    verdict: Verdict,
    view: Option<&EssView>,
    pilot: Option<&Pilot>,
    linker: Option<i64>,
) -> Option<Vec<Act>> {
    match verdict {
        Verdict::Ignore | Verdict::Read => None,
        Verdict::Stop => {
            stop(m);
            None
        }
        Verdict::Whois => {
            let char_id = linker?;
            // Asked once: a lookup that fails leaves the linker unplaced.
            if m.ess_guard.who.len() >= WHO_CAP {
                m.ess_guard.who.clear();
            }
            m.ess_guard.who.insert(char_id, None);
            Some(vec![Act::Hunt(HuntAct::Ess(EssAct::Whois { char_id }))])
        }
        Verdict::Distress => call(m, o),
        Verdict::DistressAndEngage => {
            let mut acts = call(m, o).unwrap_or_default();
            acts.extend(engage(m, o, park, view?, pilot));
            Some(acts)
        }
        Verdict::Look => {
            let acts = engage(m, o, park, view?, pilot);
            Some(acts).filter(|a| !a.is_empty())
        }
    }
}

/// The distress, once per link: a fleet's broadcast, else the hall's board
/// (the alert's `Pending` call, posted by the driver after this turn). The
/// alert of a non-civilian role is cleared by `threat::act` next turn, so it
/// never makes the ratter run.
fn call(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if m.ess_guard.called {
        return None;
    }
    m.ess_guard.called = true;
    match (o.fleet, o.ship) {
        (Some(_), Some(ship)) => Some(vec![Act::Fleet(crate::fleet::Order::Broadcast {
            name: crate::fleet::BROADCAST_NEED_BACKUP,
            item: ship.ship_id,
            type_id: None,
        })]),
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
            None
        }
    }
}

/// Fight the linker if he is on the grid, else warp to the beacon to find
/// him: at most [`MAX_WARPS`] times per link, then nothing (back to ratting).
fn engage(
    m: &mut Memory,
    o: &Observation,
    park: &ParkView,
    view: &EssView,
    pilot: Option<&Pilot>,
) -> Vec<Act> {
    if let Some(p) = pilot.filter(|p| p.hostile) {
        return crate::hunt::fight(m, o, park, p, Vec::new());
    }
    if park.ego_warping {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    if m.ess_guard
        .warped_at
        .is_some_and(|t| o.now.saturating_duration_since(t) < LOOK_FOR)
    {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    if m.ess_guard.warps >= MAX_WARPS {
        // Looked, and he is not there: back to the sites.
        return Vec::new();
    }
    m.ess_guard.warps += 1;
    m.ess_guard.warped_at = Some(o.now);
    vec![Act::WarpToItem {
        id: view.beacon,
        range: 0.0,
    }]
}

/// End the alarm and the fight it started.
fn stop(m: &mut Memory) {
    if m.ess_guard.warped_at.is_some() {
        m.hunt.prey = None;
        m.target = None;
    }
    m.ess_guard.called = false;
    m.ess_guard.warped_at = None;
    m.ess_guard.warps = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: i64 = 90_000_001;
    const MATE: i64 = 90_000_002;
    const THIEF: i64 = 90_000_003;
    const CORP: i64 = 98_000_001;
    const OTHER_CORP: i64 = 98_000_009;
    const ALLIANCE: i64 = 99_000_001;

    fn brief(linked_by: Option<i64>, holder: Holder) -> Brief {
        Brief {
            ess: Ess::Present {
                linked_by,
                age: Duration::from_secs(1),
            },
            holder,
            need_who: false,
            nerve: 0.8,
            armed: true,
            since_read: Some(Duration::from_secs(1)),
            alarmed: false,
            spent: false,
        }
    }

    #[test]
    fn a_corp_mate_or_the_bot_itself_is_ignored() {
        let b = brief(Some(MATE), Holder::Mate);
        assert_eq!(decide(&b), Verdict::Ignore);
        let b = brief(Some(ME), Holder::Me);
        assert_eq!(decide(&b), Verdict::Ignore);
        // Placed by corp, by alliance, by hostility, by the fleet boss.
        let aff = |corp, alliance| Affiliation { corp, alliance };
        let own = Some(aff(CORP, Some(ALLIANCE)));
        assert_eq!(holder(ME, ME, None, None, None, None), Holder::Me);
        assert_eq!(
            holder(MATE, ME, None, own, Some(aff(CORP, None)), None),
            Holder::Mate
        );
        assert_eq!(
            holder(
                MATE,
                ME,
                None,
                own,
                Some(aff(OTHER_CORP, Some(ALLIANCE))),
                None
            ),
            Holder::Mate
        );
        assert_eq!(
            holder(
                MATE,
                ME,
                None,
                None,
                None,
                Some((aff(OTHER_CORP, None), false))
            ),
            Holder::Mate
        );
        assert_eq!(holder(MATE, ME, Some(MATE), None, None, None), Holder::Mate);
        assert_eq!(
            holder(THIEF, ME, None, own, Some(aff(OTHER_CORP, None)), None),
            Holder::Stranger
        );
        // Nobody can place him: not a stranger.
        assert_eq!(holder(THIEF, ME, None, own, None, None), Holder::Unknown);
        assert_eq!(
            holder(THIEF, ME, None, None, Some(aff(OTHER_CORP, None)), None),
            Holder::Unknown
        );
    }

    #[test]
    fn a_stranger_draws_a_call_and_a_bold_armed_bot_goes() {
        let b = brief(Some(THIEF), Holder::Stranger);
        assert_eq!(decide(&b), Verdict::DistressAndEngage);
    }

    #[test]
    fn a_timid_or_unarmed_bot_posts_distress_but_does_not_go() {
        let mut b = brief(Some(THIEF), Holder::Stranger);
        b.nerve = 0.1;
        assert_eq!(decide(&b), Verdict::Distress);
        let mut b = brief(Some(THIEF), Holder::Stranger);
        b.armed = false;
        assert_eq!(decide(&b), Verdict::Distress);
    }

    #[test]
    fn no_ess_means_no_action() {
        let mut b = brief(None, Holder::Mate);
        b.ess = Ess::Absent;
        b.since_read = None;
        assert_eq!(decide(&b), Verdict::Ignore);
        b.since_read = Some(Duration::from_secs(3600));
        assert_eq!(decide(&b), Verdict::Ignore);
    }

    #[test]
    fn the_link_ending_stops_an_alarm_and_idles_otherwise() {
        let mut b = brief(None, Holder::Mate);
        assert_eq!(decide(&b), Verdict::Ignore);
        b.alarmed = true;
        assert_eq!(decide(&b), Verdict::Stop);
        // A mate taking over the link ends it too.
        let mut b = brief(Some(MATE), Holder::Mate);
        b.alarmed = true;
        assert_eq!(decide(&b), Verdict::Stop);
        // A view too old to say the link holds does not keep a fight going.
        let mut b = brief(Some(THIEF), Holder::Stranger);
        b.alarmed = true;
        b.ess = Ess::Present {
            linked_by: Some(THIEF),
            age: STALE_AFTER,
        };
        assert_eq!(decide(&b), Verdict::Stop);
    }

    #[test]
    fn the_poll_interval_is_respected() {
        // Never read: read now.
        let mut b = brief(Some(THIEF), Holder::Stranger);
        b.ess = Ess::Unknown;
        b.since_read = None;
        assert_eq!(decide(&b), Verdict::Read);
        // Just read: wait, whatever the view says.
        b.since_read = Some(POLL_EVERY - Duration::from_secs(1));
        assert_eq!(decide(&b), Verdict::Ignore);
        // Due: read, even with a link on (the end must be seen).
        let mut b = brief(Some(THIEF), Holder::Stranger);
        b.since_read = Some(POLL_EVERY);
        assert_eq!(decide(&b), Verdict::Read);
    }

    // ---- the turn ---------------------------------------------------------

    use crate::brain::{Ball, Policy, ShipInfo};
    use std::collections::BTreeMap;
    use std::time::Instant;

    const SYSTEM: i64 = 30_990_777;
    const BEACON: i64 = 100_000_777;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn park() -> ParkView {
        ParkView {
            ego: ball(1, 0.0),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        }
    }

    fn obs(now: Instant) -> Observation {
        let mut o = Observation::empty(now);
        o.place = Place::InSpace {
            system: SYSTEM,
            bound: true,
        };
        o.me = ME;
        o.ship = Some(ShipInfo {
            ship_id: 1,
            type_id: Some(1),
        });
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
        o.park = Some(park());
        o.hunt.temper.risk = 0.9;
        o.hunt.temper.aggression = 0.9;
        o
    }

    fn linked(now: Instant, by: Option<i64>) -> EssView {
        EssView {
            system: SYSTEM,
            beacon: BEACON,
            ess: 100_000_778,
            value: 1.0,
            linked_by: by,
            read_at: now,
            gap: None,
        }
    }

    /// The driver's answer to a `Whois`: the bot is in `CORP`, `who` in `corp`.
    fn place(m: &mut Memory, who: i64, corp: i64) {
        m.ess_guard.on_who(
            who,
            Some(Affiliation {
                corp: CORP,
                alliance: None,
            }),
            Some(Affiliation {
                corp,
                alliance: None,
            }),
        );
    }

    #[test]
    fn the_turn_reads_once_then_waits_out_the_poll() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let o = obs(now);
        let p = park();
        let acts = step(&mut m, &o, &p).expect("first read");
        assert_eq!(
            acts,
            vec![Act::Hunt(HuntAct::Ess(EssAct::Read { system: SYSTEM }))]
        );
        // The answer has not come (a failed read): not asked again at once.
        assert_eq!(step(&mut m, &obs(now + Duration::from_secs(5)), &p), None);
        // Due again.
        assert!(step(&mut m, &obs(now + POLL_EVERY), &p).is_some());
        // A system read and found bare is left alone for good.
        m.hunt.ess.on_read(SYSTEM, None);
        assert_eq!(step(&mut m, &obs(now + POLL_EVERY * 9), &p), None);
    }

    #[test]
    fn a_stranger_link_posts_one_call_and_sends_a_bold_bot_to_the_beacon() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        let p = park();
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        place(&mut m, THIEF, OTHER_CORP);
        o.now = now + Duration::from_secs(2);
        let acts = step(&mut m, &o, &p).expect("engage");
        assert_eq!(
            acts,
            vec![Act::WarpToItem {
                id: BEACON,
                range: 0.0
            }]
        );
        let alert = m.threat.alert.as_ref().expect("the board call");
        assert_eq!(alert.call, Call::Pending);
        // The next turn: the warp is ordered, so wait; no second call.
        m.threat.alert = None;
        o.now += Duration::from_secs(2);
        let acts = step(&mut m, &o, &p).expect("wait");
        assert!(matches!(acts[..], [Act::Wait(_)]), "{acts:?}");
        assert!(m.threat.alert.is_none());
        // The link ends: the alarm is over.
        m.hunt.ess.view = Some(linked(o.now, None));
        assert_eq!(step(&mut m, &o, &p), None);
        assert!(!m.ess_guard.alarmed());
    }

    #[test]
    fn a_timid_bot_only_calls() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        o.hunt.temper.risk = 0.1;
        o.hunt.temper.aggression = 0.1;
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        place(&mut m, THIEF, OTHER_CORP);
        let acts = step(&mut m, &o, &park());
        assert_eq!(acts, None);
        assert_eq!(m.threat.alert.as_ref().map(|a| a.call), Some(Call::Pending));
    }

    #[test]
    fn a_fleet_calls_by_broadcast() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        o.hunt.temper.risk = 0.1;
        o.hunt.temper.aggression = 0.1;
        o.fleet = Some(brain::FleetView {
            fleet_id: 5,
            boss: None,
        });
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        place(&mut m, THIEF, OTHER_CORP);
        let acts = step(&mut m, &o, &park()).expect("broadcast");
        assert!(matches!(
            acts[..],
            [Act::Fleet(crate::fleet::Order::Broadcast { .. })]
        ));
        assert!(m.threat.alert.is_none());
    }

    /// H2: a corp mate holds the link, he is not on the ratter's grid
    /// (`o.hunt.pilots` is empty), and the bot is not in a fleet: nothing
    /// happens, however often the turn runs.
    #[test]
    fn a_corp_mates_link_is_ignored_off_grid() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        m.hunt.ess.view = Some(linked(now, Some(MATE)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        place(&mut m, MATE, CORP);
        for s in 1..20 {
            o.now = now + Duration::from_secs(s);
            assert_eq!(step(&mut m, &o, &park()), None);
        }
        assert!(m.threat.alert.is_none());
        assert!(!m.ess_guard.alarmed());
    }

    /// An uncached linker is asked about once, and the answer decides.
    #[test]
    fn the_linker_is_looked_up_once_then_judged_by_corp() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        m.hunt.ess.view = Some(linked(now, Some(MATE)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        o.now = now + Duration::from_secs(1);
        let acts = step(&mut m, &o, &park()).expect("whois");
        assert_eq!(
            acts,
            vec![Act::Hunt(HuntAct::Ess(EssAct::Whois { char_id: MATE }))]
        );
        // The driver is slow: no second question, no call.
        assert_eq!(step(&mut m, &o, &park()).map(|a| a.len()), Some(1));
        assert!(m.threat.alert.is_none());
        place(&mut m, MATE, CORP);
        assert_eq!(step(&mut m, &o, &park()), None);
        assert!(m.threat.alert.is_none());
    }

    /// An unplaceable linker: warp to look, no distress, and not for ever.
    #[test]
    fn an_unplaced_linker_is_looked_at_without_a_call_and_not_for_ever() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        // The lookup failed.
        m.ess_guard.who.insert(THIEF, None);
        let mut warps = 0;
        let mut quiet_at = None;
        for s in 1..25u64 {
            // Re-reads keep the view fresh and the link on.
            let t = now + Duration::from_secs(s * 10);
            o.now = t;
            m.hunt.ess.view = Some(linked(t, Some(THIEF)));
            m.ess_guard.next_read = Some(t + POLL_EVERY);
            let acts = step(&mut m, &o, &park());
            warps += acts
                .iter()
                .flatten()
                .filter(|a| matches!(a, Act::WarpToItem { .. }))
                .count();
            if acts.is_none() && quiet_at.is_none() {
                quiet_at = Some(s);
            }
            assert!(
                m.threat.alert.is_none(),
                "no distress for an unplaced linker"
            );
        }
        assert_eq!(warps, MAX_WARPS as usize);
        assert!(quiet_at.is_some(), "back to ratting");
        // The last turns are all quiet.
        o.now = now + Duration::from_secs(400);
        m.hunt.ess.view = Some(linked(o.now, Some(THIEF)));
        m.ess_guard.next_read = Some(o.now + POLL_EVERY);
        assert_eq!(step(&mut m, &o, &park()), None);
    }

    /// A timid bot does not go to look.
    #[test]
    fn a_timid_bot_does_not_go_to_look_at_an_unplaced_linker() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        o.hunt.temper.risk = 0.1;
        o.hunt.temper.aggression = 0.1;
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        m.ess_guard.who.insert(THIEF, None);
        assert_eq!(step(&mut m, &o, &park()), None);
        assert!(m.threat.alert.is_none());
    }

    /// L2: a due poll during an engagement is added to the fight acts.
    #[test]
    fn a_due_poll_is_added_to_the_fight_not_put_in_its_place() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        place(&mut m, THIEF, OTHER_CORP);
        o.now = now + Duration::from_secs(1);
        assert!(step(&mut m, &o, &park()).is_some()); // call and warp
        // The thief is on the grid; the poll is due.
        o.hunt.pilots = vec![Pilot {
            ball: ball(THIEF, 5_000.0),
            char_id: THIEF,
            type_id: None,
            who: eve_server::group_contacts::Affiliation {
                character_id: THIEF,
                corporation_id: OTHER_CORP,
                alliance_id: None,
            },
            hostile: true,
            relation: 0.0,
            war: false,
        }];
        o.now = now + POLL_EVERY + Duration::from_secs(5);
        m.hunt.ess.view = Some(linked(o.now, Some(THIEF)));
        m.ess_guard.next_read = Some(now + Duration::from_secs(1));
        let acts = step(&mut m, &o, &park()).expect("read and fight");
        assert!(
            matches!(acts[0], Act::Hunt(HuntAct::Ess(EssAct::Read { .. }))),
            "{acts:?}"
        );
        assert!(acts.len() > 1, "the fight acts follow the read: {acts:?}");
    }

    #[test]
    fn the_memory_field_leaves_the_hunters_ess_mind_alone() {
        let mut m = Memory::new("Guard1", Policy::fast());
        let now = Instant::now();
        let mut o = obs(now);
        m.hunt.ess.view = Some(linked(now, Some(THIEF)));
        place(&mut m, THIEF, OTHER_CORP);
        m.ess_guard.next_read = Some(now + POLL_EVERY);
        o.now = now + Duration::from_secs(1);
        let _ = step(&mut m, &o, &park());
        assert_eq!(m.hunt.ess.stage, crate::ess_raid::Stage::Idle);
        assert_eq!(m.hunt.ess.cooled_until, None);
    }
}
