//! Factional warfare capture points (#75, FW-7A): hold the point to take it.
//!
//! The client's space component `fwCapturePoint` ([V]
//! `spacecomponents/client/components/fwCapturePoint.py`, decompiled from
//! 3662168) shows a capture point as a bracket (`CapturePointUI`) in one of
//! three states, worked out on the client from who is within `range` of the
//! point (`GetState`, `:78-112`):
//!
//! - ships in the *capturing* type list, piloted and carrying a militia
//!   (`warFactionID`), are the capturing forces (`:81-85`); items in the
//!   *blocking* type list are blocking forces, named by their NPC owner or
//!   their militia (`:86-88`, `:114-118`);
//! - the holder is the point's occupier if an occupier pilot is there, else
//!   its attacker if an enemy of the occupier is (`:92-98`);
//! - a capturing force that is an enemy of the holder, or a blocking force
//!   that is, makes it `CONTESTED` (`:99-110`); nobody there is `EMPTY`;
//!   the holder alone is `UNCONTESTED`.
//!
//! [`judge`] is that function, over plain factions. What the client does
//! not decide is the timer. The server hands the client two slim keys,
//! `component_fwCapturePoint_factionInfo` = `(attackerID, occupierID)` and
//! `component_fwCapturePoint_nextPointTime` ([V] `:44-49`), and the bracket
//! counts down to the next point over `numberOfSeconds` ([V] `:49`), so the
//! shape is: an uncontested point pays `pointValue` victory points to its
//! holder every `numberOfSeconds` ([I]; `looping` repeats it, [I] from the
//! attribute names). [`slim_extras`] serves both keys.
//!
//! **The rules here ([I] from the above, [A] where marked):**
//!
//! - [`advance`]: progress grows while the point is `UNCONTESTED`; `EMPTY`
//!   and `CONTESTED` pause it **[A]** (a pause, not a reset: the issue says
//!   a blocker "pauses"); a new holder starts from 0 **[A]**.
//! - A cycle done: a **looping** point awards its holder [`Params::
//!   point_value`] victory points through [`crate::facwar::add_vp`] (the
//!   attacker raises the system's score, the occupier lowers it, the same
//!   direction as `facwar::credit`) and starts again; a **non-looping**
//!   point is the complex's capture: it completes the complex's population
//!   slot, which `facwar::tick` credits as before, and pays LP and tells
//!   the grid through `fw_lp::on_complete`. Once per capture.
//! - [`State::is_captured`] lets `site_spawn` skip its own clear-the-pocket
//!   LP for a complex a capture has already paid (coordinator hunk, see the
//!   lane report).
//!
//! **Not persisted.** Points live in memory: a restart loses every point's
//! progress and a taken flag (a complex then starts over, and one the
//! pocket cleared stays done through `site_spawn`'s own `completed`).
//!
//! **The values.** `numberOfSeconds`, `pointValue`, `range`, `looping` and
//! the two type lists are component attributes the bundle does not carry
//! (`npc/components.tsv` lists only which types have the component). They
//! are [`Params`], set by [`State::set_params`]; until set, [`assumed`]
//! stands **[A]**. Falsified by the client's component registry for the
//! capture point types (the `attributes` its constructor receives).

use crate::damage::BlueTime;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

/// The component name in `npc/components.tsv`.
pub const COMPONENT: &str = "fwCapturePoint";

/// The slim key the client reads for the two sides ([V] `:44-45`).
pub const KEY_FACTION_INFO: &str = "component_fwCapturePoint_factionInfo";

/// The slim key the client reads for the next point ([V] `:48-49`).
pub const KEY_NEXT_POINT: &str = "component_fwCapturePoint_nextPointTime";

/// How often points are looked at.
pub const CHECK_EVERY: BlueTime = 10_000_000;

/// FILETIME ticks per second.
const SECOND: i64 = 10_000_000;

/// The component's attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// `numberOfSeconds`.
    pub seconds: i64,
    /// `range`, metres.
    pub range: f64,
    /// `pointValue`.
    pub point_value: i64,
    /// `looping`.
    pub looping: bool,
    /// `capturingTypeListID`'s types; `None` is any ship.
    pub capturing: Option<BTreeSet<i64>>,
    /// `blockingTypeListID`'s types.
    pub blocking: BTreeSet<i64>,
}

/// **[A]** What stands until the values are set: one minute, 10 km, 1
/// point, looping, any ship captures, nothing blocks. Falsified by the
/// component's real attributes.
pub fn assumed() -> Params {
    Params {
        seconds: 60,
        range: 10_000.0,
        point_value: 1,
        looping: true,
        capturing: None,
        blocking: BTreeSet::new(),
    }
}

/// The client's three states (`CapturePointUI`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Empty,
    Contested,
    Uncontested,
}

/// `GetState` ([V] `:78-112`): the phase and the holder, from the point's
/// two sides, the factions of the piloted capturing ships in range and the
/// factions of the blocking items in range. A faction that is neither side
/// is nobody's enemy and counts for nothing.
pub fn judge(
    attacker: i64,
    occupier: i64,
    capturing: &[i64],
    blocking: &[i64],
) -> (Phase, Option<i64>) {
    let side = |f: i64| f == attacker || f == occupier;
    let against = |holder: i64, f: i64| side(f) && f != holder;
    let mut holder = None;
    for &f in capturing.iter().filter(|f| side(**f)) {
        match holder {
            None => holder = Some(f),
            Some(h) if against(h, f) => return (Phase::Contested, Some(h)),
            Some(_) => {}
        }
    }
    let Some(h) = holder else {
        return (Phase::Empty, None);
    };
    if blocking.iter().any(|f| against(h, *f)) {
        return (Phase::Contested, Some(h));
    }
    (Phase::Uncontested, Some(h))
}

/// One live capture point.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub site_id: i64,
    pub system: i64,
    /// The placed object carrying the component, when the complex has one.
    pub item_id: Option<i64>,
    pub position: [f64; 3],
    pub attacker: i64,
    pub occupier: i64,
    /// Time accrued towards the next point, FILETIME ticks.
    pub progress: i64,
    pub holder: Option<i64>,
    pub phase: Phase,
    pub last: BlueTime,
    /// Points paid so far (looping) .
    pub cycles: u32,
    /// A non-looping point that has been taken.
    pub taken: bool,
}

/// What a finished cycle did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Award {
    /// A looping point: `points` to `holder`.
    Points { holder: i64, points: i64 },
    /// A non-looping point: the complex is taken by `holder`.
    Taken { holder: i64 },
}

/// **The timer, pure**: bring `p` to `now` knowing the phase and holder that
/// held over the interval. Returns every award earned (a long gap can earn
/// several looping ones, never more than one capture).
pub fn advance(
    p: &mut Point,
    params: &Params,
    now: BlueTime,
    phase: Phase,
    holder: Option<i64>,
) -> Vec<Award> {
    let dt = now.saturating_sub(p.last).max(0);
    p.last = now;
    if p.taken {
        return Vec::new();
    }
    // The interval is credited to the holder and phase it began with.
    let held = p.phase == Phase::Uncontested;
    let prev = p.holder;
    let mut out = Vec::new();
    if let (true, Some(h), true) = (held, prev, params.seconds > 0) {
        p.progress += dt;
        let cycle = params.seconds * SECOND;
        while p.progress >= cycle {
            p.progress -= cycle;
            if params.looping {
                p.cycles += 1;
                out.push(Award::Points {
                    holder: h,
                    points: params.point_value,
                });
            } else {
                p.taken = true;
                p.progress = 0;
                out.push(Award::Taken { holder: h });
                break;
            }
        }
    }
    p.phase = phase;
    // A contested point keeps its holder: `judge` names the first side it met
    // there (by character order), which says nothing about who holds it.
    let holder = holder.filter(|_| phase != Phase::Contested);
    if let Some(h) = holder {
        if prev.is_some() && prev != holder {
            p.progress = 0;
        }
        p.holder = Some(h);
    }
    out
}

/// The `nextPointTime` the bracket counts to: now plus what is left of the
/// cycle, while the point is being held; `None` otherwise ([A] the bracket
/// shows no countdown when nobody holds it).
pub fn next_point(p: &Point, params: &Params, now: BlueTime) -> Option<BlueTime> {
    (p.phase == Phase::Uncontested).then(|| now + (params.seconds * SECOND - p.progress).max(0))
}

/// The world state (a field of `facwar::Facwar`).
#[derive(Debug, Clone, Default)]
pub struct State {
    params: Option<Params>,
    points: BTreeMap<i64, Point>,
    next_check: BlueTime,
}

impl State {
    /// Set the component's attributes (from the bundle once it carries them).
    pub fn set_params(&mut self, params: Params) {
        self.params = Some(params);
    }

    pub fn params(&self) -> Params {
        self.params.clone().unwrap_or_else(assumed)
    }

    pub fn point(&self, site_id: i64) -> Option<&Point> {
        self.points.get(&site_id)
    }

    /// Whether a capture has taken this complex.
    pub fn is_captured(&self, site_id: i64) -> bool {
        self.points.get(&site_id).is_some_and(|p| p.taken)
    }
}

/// Whether `site_id` was taken by a capture (for `site_spawn`'s hook).
pub fn is_captured(reg: &Registry, site_id: i64) -> bool {
    reg.ext::<crate::facwar::Facwar>()
        .capture()
        .is_captured(site_id)
}

/// The slim extras of capture-point objects (`slim_extra::SOURCES` row).
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    let st = reg.ext::<crate::facwar::Facwar>().capture();
    let params = st.params();
    ids.iter()
        .filter_map(|id| {
            let p = st.points.values().find(|p| p.item_id == Some(*id))?;
            let mut v = vec![(
                KEY_FACTION_INFO,
                PyValue::Tuple(vec![PyValue::Int(p.attacker), PyValue::Int(p.occupier)]),
            )];
            if let Some(t) = next_point(p, &params, p.last) {
                v.push((KEY_NEXT_POINT, PyValue::Int(t)));
            }
            Some((*id, v))
        })
        .collect()
}

/// The pilots near a point: `(character, faction)` for each enlisted pilot
/// in the system within `range` of `position` whose hull may capture, and
/// the factions of those whose hull blocks.
fn near(
    reg: &Registry,
    system: i64,
    position: [f64; 3],
    params: &Params,
    now: BlueTime,
) -> (Vec<i64>, Vec<i64>) {
    let (mut capturing, mut blocking) = (Vec::new(), Vec::new());
    for c in crate::facwar::in_system(reg, system) {
        let Some(p) = reg.roster().get(c) else {
            continue;
        };
        if crate::space_state::distance(p.motion.position, position) > params.range {
            continue;
        }
        let Some(f) = crate::service::facwar_mgr::my_faction(reg, c, now) else {
            continue;
        };
        let hull = reg.pilot(c).map(|x| x.ship_type_id).unwrap_or(0);
        let can = params.capturing.as_ref().is_none_or(|s| s.contains(&hull));
        if can {
            capturing.push(f);
        } else if params.blocking.contains(&hull) {
            blocking.push(f);
        }
    }
    (capturing, blocking)
}

// **[A]** NPC blockers are not seen: the client counts NPC items in range by
// their `ownerID` (`:86-88`, `:114-118`), `near` sees only enlisted pilots.
// Moot while `Params::blocking` is empty (it is, until the component's
// attributes are extracted). Falsified by a capture point type whose real
// blocking list holds an NPC hull.

/// **Interim rule for an empty complex [A].** 3707965's FW complexes carry no
/// static spawns [V per expac-D-data 2], so one with no squad and no capture
/// point object has nothing to clear and no point to take, and the pocket
/// rule never completes it (`site_spawn`). Until the Conquests lane authors
/// the defenders, such a complex is taken by holding its anchor for one
/// cycle: its point is judged as a non-looping one, so [`Award::Taken`]
/// completes the population slot and pays LP. Falsified by the Conquests
/// defender spawns landing (the rule is then dead and goes).
pub fn interim_params(params: &Params, empty: bool) -> Params {
    Params {
        looping: params.looping && !empty,
        ..params.clone()
    }
}

/// What the client's bracket shows of a point: phase, holder, cycles paid, taken.
type Seen = (Phase, Option<i64>, u32, bool);

fn seen(p: &Point) -> Seen {
    (p.phase, p.holder, p.cycles, p.taken)
}

/// Whether the slim keys the client holds are stale: the phase, the holder or
/// the cycle moved (the countdown's end changes with any of them).
fn slim_stale(before: Seen, p: &Point) -> bool {
    before != seen(p)
}

/// `OnSlimItemChange` for a capture point's object, to the pilots in its
/// system, with every source's extras (ours among them).
fn push_slim(reg: &mut Registry, item: i64) {
    let Some((placed, system)) = crate::site::object(reg, item) else {
        return;
    };
    let to = crate::dungeon::audience(reg, system);
    if to.is_empty() {
        return;
    }
    let extras: Vec<(&'static str, PyValue)> = crate::slim_extra::for_slims(reg, &[item])
        .into_iter()
        .filter(|(id, _)| *id == item)
        .flat_map(|(_, pairs)| pairs)
        .collect();
    let stamp = reg.next_destiny_stamp();
    reg.schedule(
        std::time::Duration::ZERO,
        crate::hacking::CanSlimChange {
            to,
            stamp,
            slim: crate::site::slim(&placed),
            extras,
        },
    );
}

/// `/fwcapture [<seconds> | reset]`: show or set the capture cycle (GM; the
/// client-CI scenario shortens it). Keeps the other [`assumed`] values.
pub fn slash(
    w: &mut Registry,
    _actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let st = w.ext_mut::<crate::facwar::Facwar>().capture_mut();
    match words {
        [] => {
            let p = st.params();
            say(&format!(
                "capture cycle {} s, {} point(s), range {} m, looping {}",
                p.seconds, p.point_value, p.range, p.looping
            ))
        }
        ["reset"] => {
            st.params = None;
            say("capture params back to the assumed values")
        }
        [n] => match n.parse::<i64>() {
            Ok(secs) if (1..=86_400).contains(&secs) => {
                st.set_params(Params {
                    seconds: secs,
                    ..assumed()
                });
                say(&format!("capture cycle is now {secs} s"))
            }
            _ => say("seconds are 1 to 86400"),
        },
        _ => say("usage: /fwcapture [<seconds> | reset]"),
    }
}

/// A live complex: site, system, dungeon, anchor, and its point object.
type Live = (i64, i64, i64, [f64; 3], Option<(i64, [f64; 3])>);

/// **The stage** (called from `facwar::tick`): every [`CHECK_EVERY`], each
/// live FW complex in a warzone system has its point judged and advanced;
/// awards are paid.
pub fn tick(reg: &mut Registry, ft: BlueTime) {
    if ft < reg.ext::<crate::facwar::Facwar>().capture().next_check {
        return;
    }
    reg.ext_mut::<crate::facwar::Facwar>()
        .capture_mut()
        .next_check = ft + CHECK_EVERY;
    let base_params = reg.ext::<crate::facwar::Facwar>().capture().params();
    // The live complexes: (site, system, dungeon, anchor, point object).
    let sites: Vec<Live> = {
        let npc = reg.content().try_get::<crate::npc_data::Table>();
        reg.ext::<crate::site::Sites>()
            .iter()
            .filter(|i| {
                reg.ext::<crate::facwar::Facwar>()
                    .system(i.system)
                    .is_some()
            })
            .map(|i| {
                let obj = i
                    .objects
                    .iter()
                    .find(|o| npc.is_some_and(|n| n.has_component(o.type_id, COMPONENT)))
                    .map(|o| (o.item_id, o.position));
                (i.site_id, i.system, i.dungeon_id, i.anchor, obj)
            })
            .collect()
    };
    let live: BTreeSet<i64> = sites.iter().map(|s| s.0).collect();
    let st = reg.ext_mut::<crate::facwar::Facwar>().capture_mut();
    // A taken point is kept only while the population still lists its site.
    st.points.retain(|id, _| live.contains(id));
    for (site_id, system, dungeon, anchor, obj) in sites {
        let Some((archetype, owner)) = reg
            .content()
            .try_get::<crate::dungeon_data::Table>()
            .and_then(|t| t.dungeons.get(&dungeon))
            .map(|d| (d.archetype, d.faction))
        else {
            continue;
        };
        if !crate::facwar::FW_ARCHETYPES.contains(&archetype) || owner.is_none() {
            continue;
        }
        let Some(o) = crate::facwar::occupation(reg, system) else {
            continue;
        };
        // Taken by capture, or cleared by killing the pocket (`site_spawn`
        // paid the LP then): nothing more to judge, nothing to pay twice.
        if reg
            .ext::<crate::facwar::Facwar>()
            .capture()
            .is_captured(site_id)
            || crate::site_spawn::is_completed(reg, site_id)
        {
            continue;
        }
        let position = obj.map_or(anchor, |x| x.1);
        let empty = obj.is_none() && crate::site_spawn::is_empty_pocket(reg, site_id);
        let params = interim_params(&base_params, empty);
        let (capturing, blocking) = near(reg, system, position, &params, ft);
        let (phase, holder) = judge(o.attacker, o.occupier, &capturing, &blocking);
        let (awards, before, after) = {
            let st = reg.ext_mut::<crate::facwar::Facwar>().capture_mut();
            let p = st.points.entry(site_id).or_insert_with(|| Point {
                site_id,
                system,
                item_id: obj.map(|x| x.0),
                position,
                attacker: o.attacker,
                occupier: o.occupier,
                progress: 0,
                holder: None,
                phase: Phase::Empty,
                last: ft,
                cycles: 0,
                taken: false,
            });
            p.attacker = o.attacker;
            p.occupier = o.occupier;
            let before = seen(p);
            let awards = advance(p, &params, ft, phase, holder);
            (awards, before, p.clone())
        };
        // The client reads `nextPointTime` and `factionInfo` only when it
        // builds the item or on `OnSlimItemChange` ([V] `:39-49`).
        if let (true, Some(item)) = (slim_stale(before, &after), after.item_id) {
            push_slim(reg, item);
        }
        for a in awards {
            match a {
                Award::Points { holder, points } => {
                    let delta = if holder == o.attacker {
                        points
                    } else {
                        -points
                    };
                    crate::facwar::add_vp(reg, system, delta, ft);
                }
                Award::Taken { .. } => {
                    // Pay only when the slot was still live to take.
                    if crate::population::complete(reg, system, site_id, ft) {
                        crate::fw_lp::on_complete(reg, site_id, system, anchor, ft);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i64 = 7_001;
    const B: i64 = 7_002;
    const C: i64 = 7_009;

    fn params(looping: bool) -> Params {
        Params {
            seconds: 30,
            range: 5_000.0,
            point_value: 3,
            looping,
            capturing: None,
            blocking: BTreeSet::new(),
        }
    }

    fn point() -> Point {
        Point {
            site_id: 1,
            system: 2,
            item_id: Some(3),
            position: [0.0; 3],
            attacker: A,
            occupier: B,
            progress: 0,
            holder: None,
            phase: Phase::Empty,
            last: 0,
            cycles: 0,
            taken: false,
        }
    }

    fn secs(n: i64) -> BlueTime {
        n * SECOND
    }

    #[test]
    fn nobody_in_range_is_empty_and_the_holder_alone_is_uncontested() {
        assert_eq!(judge(A, B, &[], &[]), (Phase::Empty, None));
        assert_eq!(judge(A, B, &[A, A], &[]), (Phase::Uncontested, Some(A)));
        assert_eq!(judge(A, B, &[B], &[]), (Phase::Uncontested, Some(B)));
    }

    #[test]
    fn both_sides_present_contest_it_and_a_bystander_does_not() {
        assert_eq!(judge(A, B, &[A, B], &[]).0, Phase::Contested);
        assert_eq!(judge(A, B, &[A, C], &[]), (Phase::Uncontested, Some(A)));
        assert_eq!(judge(A, B, &[C], &[]), (Phase::Empty, None));
    }

    #[test]
    fn a_blocker_of_the_other_side_contests_and_the_holders_own_does_not() {
        assert_eq!(judge(A, B, &[A], &[B]), (Phase::Contested, Some(A)));
        assert_eq!(judge(A, B, &[A], &[A]), (Phase::Uncontested, Some(A)));
        assert_eq!(judge(A, B, &[A], &[C]), (Phase::Uncontested, Some(A)));
    }

    #[test]
    fn a_held_looping_point_pays_every_cycle() {
        let (par, mut p) = (params(true), point());
        assert!(advance(&mut p, &par, 0, Phase::Uncontested, Some(A)).is_empty());
        assert!(advance(&mut p, &par, secs(29), Phase::Uncontested, Some(A)).is_empty());
        let got = advance(&mut p, &par, secs(31), Phase::Uncontested, Some(A));
        assert_eq!(
            got,
            vec![Award::Points {
                holder: A,
                points: 3
            }]
        );
        let got = advance(&mut p, &par, secs(95), Phase::Uncontested, Some(A));
        assert_eq!(got.len(), 2);
        assert_eq!(p.cycles, 3);
    }

    #[test]
    fn a_blocker_pauses_the_timer_and_it_resumes_where_it_stopped() {
        let (par, mut p) = (params(true), point());
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        advance(&mut p, &par, secs(20), Phase::Contested, Some(A));
        assert_eq!(p.progress, secs(20));
        // The 20 s while it was contested do not count.
        advance(&mut p, &par, secs(40), Phase::Uncontested, Some(A));
        assert_eq!(p.progress, secs(20));
        let got = advance(&mut p, &par, secs(50), Phase::Uncontested, Some(A));
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn leaving_the_range_pauses_and_a_new_holder_starts_over() {
        let (par, mut p) = (params(true), point());
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        advance(&mut p, &par, secs(10), Phase::Empty, None);
        assert_eq!(p.progress, secs(10), "out of range: paused, not reset");
        advance(&mut p, &par, secs(20), Phase::Uncontested, Some(B));
        advance(&mut p, &par, secs(40), Phase::Uncontested, Some(B));
        assert_eq!(p.progress, secs(20));
        advance(&mut p, &par, secs(45), Phase::Uncontested, Some(A));
        assert_eq!(p.progress, 0);
    }

    #[test]
    fn an_empty_complex_is_judged_as_a_non_looping_point() {
        let looping = params(true);
        assert!(interim_params(&looping, false).looping);
        assert!(!interim_params(&looping, true).looping);
        let mut p = point();
        let held = |p: &mut Point, at: i64| {
            advance(
                p,
                &interim_params(&looping, true),
                secs(at),
                Phase::Uncontested,
                Some(A),
            )
        };
        held(&mut p, 0);
        let got = held(&mut p, looping.seconds + 1);
        assert_eq!(got, vec![Award::Taken { holder: A }]);
    }

    #[test]
    fn a_non_looping_point_is_taken_once() {
        let (par, mut p) = (params(false), point());
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        let got = advance(&mut p, &par, secs(100), Phase::Uncontested, Some(A));
        assert_eq!(got, vec![Award::Taken { holder: A }]);
        assert!(p.taken);
        assert!(advance(&mut p, &par, secs(200), Phase::Uncontested, Some(A)).is_empty());
    }

    #[test]
    fn the_countdown_is_what_is_left_and_only_while_held() {
        let (par, mut p) = (params(true), point());
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        advance(&mut p, &par, secs(10), Phase::Uncontested, Some(A));
        assert_eq!(next_point(&p, &par, secs(10)), Some(secs(30)));
        advance(&mut p, &par, secs(12), Phase::Contested, Some(A));
        assert_eq!(next_point(&p, &par, secs(12)), None);
    }

    #[test]
    fn a_contested_point_keeps_its_holder_and_its_progress() {
        let (par, mut p) = (params(true), point());
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        advance(&mut p, &par, secs(10), Phase::Uncontested, Some(A));
        // `judge` met B first this time: contested, "holder" B.
        advance(&mut p, &par, secs(20), Phase::Contested, Some(B));
        assert_eq!((p.holder, p.progress), (Some(A), secs(20)));
        advance(&mut p, &par, secs(30), Phase::Uncontested, Some(A));
        assert_eq!(p.progress, secs(20), "the contested ten seconds paused it");
    }

    #[test]
    fn the_slim_goes_stale_when_phase_holder_or_cycle_moves() {
        let (par, mut p) = (params(true), point());
        let b = seen(&p);
        advance(&mut p, &par, 0, Phase::Empty, None);
        assert!(!slim_stale(b, &p), "nothing changed");
        let b = seen(&p);
        advance(&mut p, &par, secs(1), Phase::Uncontested, Some(A));
        assert!(slim_stale(b, &p), "phase and holder moved");
        let b = seen(&p);
        advance(&mut p, &par, secs(5), Phase::Uncontested, Some(A));
        assert!(
            !slim_stale(b, &p),
            "accruing inside a cycle changes nothing"
        );
        advance(&mut p, &par, secs(35), Phase::Uncontested, Some(A));
        let b = seen(&p);
        advance(&mut p, &par, secs(70), Phase::Uncontested, Some(A));
        assert!(slim_stale(b, &p), "a cycle was paid");
        let b = seen(&p);
        advance(&mut p, &par, secs(71), Phase::Contested, Some(A));
        assert!(slim_stale(b, &p), "contested");
    }

    #[test]
    fn the_slim_keys_follow_the_phase() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let par = assumed();
        let mut p = point();
        advance(&mut p, &par, 0, Phase::Uncontested, Some(A));
        reg.ext_mut::<crate::facwar::Facwar>()
            .capture_mut()
            .points
            .insert(1, p.clone());
        let got = slim_extras(&reg, &[3]);
        let keys: Vec<&str> = got[0].1.iter().map(|k| k.0).collect();
        assert_eq!(keys, vec![KEY_FACTION_INFO, KEY_NEXT_POINT]);
        advance(&mut p, &par, secs(1), Phase::Contested, Some(A));
        reg.ext_mut::<crate::facwar::Facwar>()
            .capture_mut()
            .points
            .insert(1, p);
        let got = slim_extras(&reg, &[3]);
        assert_eq!(got[0].1.len(), 1, "no countdown while contested");
    }
}
