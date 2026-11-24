//! Factional warfare capture points (#75, FW-7A): hold the point to take it.
//!
//! The client's space component `fwCapturePoint` ([V]
//! `spacecomponents/client/components/fwCapturePoint.py`, decompiled from
//! 3662168; the `:N` line numbers below are as read on 3662168, and on the
//! current build `GetState` starts at `:81`, the capturing test is at
//! `:92-99` and the blocking one at `:93`, `:100-102`) shows a capture point
//! as a bracket (`CapturePointUI`) in one of
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
//!   direction as `facwar::credit`) and starts again; **at a battlefield**
//!   (archetype 68) it scores the site's scoreboard instead
//!   ([`crate::fw_scoreboard::credit`], #8) and never the VP, and a battlefield
//!   has one point per capture-point object (keyed `(site, item)`), none
//!   until they exist; a **non-looping**
//!   point is the complex's capture: it completes the complex's population
//!   slot, which `facwar::tick` credits as before, and pays LP and tells
//!   the grid through `fw_lp::on_complete`. Once per capture. At a
//!   battlefield a taken point scores its value and does not complete the
//!   slot (the board's win does).
//! - [`State::is_captured`] (any point of the site taken) lets `site_spawn`
//!   skip its own clear-the-pocket LP for a complex a capture has already
//!   paid; a battlefield is never completed by either.
//!
//! **Not persisted.** Points live in memory: a restart loses every point's
//! progress and a taken flag (a complex then starts over, and one the
//! pocket cleared stays done through `site_spawn`'s own `completed`).
//!
//! **The values.** `numberOfSeconds`, `pointValue`, `range`, `looping` and
//! the two type lists are the component's attributes. `facwar/capture_points.tsv`
//! carries them per capture point type when the bundle has it
//! ([`crate::facwar::Table::capture_point_of`]); [`params_for`] reads them,
//! and an absent file, type or field leaves [`assumed`] standing **[A]**
//! (falsified by the client's component registry for the capture point
//! types, the `attributes` its constructor receives). The client uses
//! `numberOfSeconds` for the countdown ([V] `:54`), `range` for who counts
//! ([V] `:87`) and the two list ids as `typelist` ids a type must be in:
//! first the capturing list ([V] `:92`, `:95`), else the blocking one ([V]
//! `:93`, `:100`); `pointValue` and `looping` the client component does not
//! read ([I] as above). **[A]** a row's `looping` is used only at a
//! battlefield (archetype 68); a complex keeps the looping behaviour it had
//! (assumed) until a client run proves what `looping` means. Falsified by a
//! client run in which a complex's capture point with `looping` 0 completes
//! the complex once (then the row's `looping` is honoured everywhere). A
//! GM override ([`State::set_params`]) wins over the table; `/fwcapture
//! <seconds>` overrides the cycle only, on top of the row.

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
    /// `capturingTypeListID`, a `typelist` id: when it names a list the
    /// bundle holds, a hull must be in it to capture ([V] `:92`, `:95`), and
    /// `capturing` is not consulted.
    pub capturing_list: Option<i64>,
    /// `blockingTypeListID`, likewise ([V] `:93`, `:100`): `blocking` is not
    /// consulted when the bundle holds the list.
    pub blocking_list: Option<i64>,
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
        capturing_list: None,
        blocking_list: None,
    }
}

/// The parameters of a capture point of type `type_id` (`None`: no object,
/// a complex judged at its anchor): a full GM override, else the bundle's
/// row for the type over [`assumed`], field by field; then the `/fwcapture`
/// seconds override. The row's `looping` counts only at a `battlefield`
/// **[A]** (the module docs).
pub fn params_for(reg: &Registry, type_id: Option<i64>, battlefield: bool) -> Params {
    let st = reg.ext::<crate::facwar::Facwar>().capture();
    let mut p = params_from_row(reg, type_id, battlefield, st);
    if let Some(s) = st.seconds_override {
        p.seconds = s;
    }
    p
}

fn params_from_row(reg: &Registry, type_id: Option<i64>, battlefield: bool, st: &State) -> Params {
    if let Some(p) = &st.params {
        return p.clone();
    }
    let mut p = assumed();
    let row = type_id.and_then(|t| {
        reg.content()
            .try_get::<crate::facwar::Table>()
            .and_then(|f| f.capture_point_of(t))
    });
    if let Some(r) = row {
        p.seconds = r.seconds.unwrap_or(p.seconds);
        p.range = r.range.map_or(p.range, |v| v as f64);
        p.point_value = r.point_value.unwrap_or(p.point_value);
        if battlefield {
            p.looping = r.looping.unwrap_or(p.looping);
        }
        p.capturing_list = r.capturing;
        p.blocking_list = r.blocking;
    }
    p
}

/// Whether a hull of `hull` is in typelist `list`. **[A]** a list the bundle
/// does not hold says nothing: `None`, and the caller keeps its assumed rule.
/// Falsified by the client treating an unknown list id as empty
/// (`evetypes.GetTypeIDsByListID`).
fn in_list(reg: &Registry, list: i64, hull: i64) -> Option<bool> {
    let l = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()?
        .typelists
        .get(&list)?;
    // A hull the type data lacks is in no list.
    let Some(r) = crate::typedata::installed_in(reg.content())
        .table()
        .get(hull)
    else {
        return Some(false);
    };
    Some(l.contains(hull, r.group_id, r.category_id))
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
    /// `/fwcapture <seconds>`: the cycle, over whatever else applies.
    seconds_override: Option<i64>,
    /// Keyed by `(site_id, item_id)`; a complex with no point object keeps
    /// one point under item 0.
    points: BTreeMap<(i64, i64), Point>,
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

    /// Every point of a site (a battlefield has one to three).
    pub fn points_of(&self, site_id: i64) -> impl Iterator<Item = &Point> {
        self.points
            .range((site_id, i64::MIN)..=(site_id, i64::MAX))
            .map(|(_, p)| p)
    }

    /// The first point of a site.
    pub fn point(&self, site_id: i64) -> Option<&Point> {
        self.points_of(site_id).next()
    }

    /// Whether a capture has taken this complex (any of its points).
    pub fn is_captured(&self, site_id: i64) -> bool {
        self.points_of(site_id).any(|p| p.taken)
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
    ids.iter()
        .filter_map(|id| {
            let p = st.points.values().find(|p| p.item_id == Some(*id))?;
            let params = params_for(
                reg,
                crate::site::object(reg, *id).map(|(o, _)| o.type_id),
                false,
            );
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
/// the factions of those whose hull blocks. The range is a **surface**
/// distance, strictly inside, as the client's `GetBallsInRange` tests it
/// ([V] `:87`; michelle `GetSurfaceDist < range`, [I]): the centre distance
/// less the point's radius and the ship's.
fn near(
    reg: &Registry,
    system: i64,
    position: [f64; 3],
    point_radius: f64,
    params: &Params,
    now: BlueTime,
) -> (Vec<i64>, Vec<i64>) {
    let (mut capturing, mut blocking) = (Vec::new(), Vec::new());
    for c in crate::facwar::in_system(reg, system) {
        let Some(p) = reg.roster().get(c) else {
            continue;
        };
        let hull = reg.pilot(c).map(|x| x.ship_type_id).unwrap_or(0);
        let ship_radius = f64::from(crate::destiny::hull::of_in(reg.content(), hull).radius);
        let surface =
            crate::space_state::distance(p.motion.position, position) - point_radius - ship_radius;
        if surface >= params.range {
            continue;
        }
        let Some(f) = crate::service::facwar_mgr::my_faction(reg, c, now) else {
            continue;
        };
        let can = params
            .capturing_list
            .and_then(|l| in_list(reg, l, hull))
            .unwrap_or_else(|| params.capturing.as_ref().is_none_or(|s| s.contains(&hull)));
        if can {
            capturing.push(f);
        } else if params
            .blocking_list
            .and_then(|l| in_list(reg, l, hull))
            .unwrap_or_else(|| params.blocking.contains(&hull))
        {
            blocking.push(f);
        }
    }
    (capturing, blocking)
}

// **[A]** NPC blockers are not seen: the client counts NPC items in range by
// their `ownerID` (`:86-88`, `:114-118`), `near` sees only enlisted pilots.
// Live now that a row can name a blocking list: a list holding an NPC hull
// never blocks here. Falsified by a capture point type whose real
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
pub(crate) fn push_slim(reg: &mut Registry, item: i64) {
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

/// `/fwcapture [<seconds> | reset]`: show the effective capture parameters,
/// or set the cycle (GM; the client-CI scenario shortens it). The seconds
/// override only the cycle, over the table row or the [`assumed`] values.
pub fn slash(
    w: &mut Registry,
    _actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    match words {
        [] => {
            let line = |label: String, p: &Params| {
                format!(
                    "{label}: {} s, {} point(s), range {} m, looping {}",
                    p.seconds, p.point_value, p.range, p.looping
                )
            };
            let types: Vec<i64> = w
                .content()
                .try_get::<crate::facwar::Table>()
                .map(|t| t.capture_point_types())
                .unwrap_or_default();
            let over = w
                .ext::<crate::facwar::Facwar>()
                .capture()
                .seconds_override
                .map_or("no seconds override".to_owned(), |s| {
                    format!("seconds override {s}")
                });
            let mut lines = vec![over];
            if types.is_empty() {
                lines.push(line("assumed".to_owned(), &params_for(w, None, false)));
            } else {
                lines.push("table rows apply".to_owned());
                for t in types.iter().take(8) {
                    lines.push(line(format!("type {t}"), &params_for(w, Some(*t), true)));
                }
            }
            say(&lines.join("; "))
        }
        ["reset"] => {
            let st = w.ext_mut::<crate::facwar::Facwar>().capture_mut();
            st.params = None;
            st.seconds_override = None;
            say("capture params back to the table values (or assumed)")
        }
        [n] => match n.parse::<i64>() {
            Ok(secs) if (1..=86_400).contains(&secs) => {
                w.ext_mut::<crate::facwar::Facwar>()
                    .capture_mut()
                    .seconds_override = Some(secs);
                say(&format!("capture cycle is now {secs} s"))
            }
            _ => say("seconds are 1 to 86400"),
        },
        _ => say("usage: /fwcapture [<seconds> | reset]"),
    }
}

/// A live complex: site, system, dungeon, anchor, and its point object.
type Live = (i64, i64, i64, [f64; 3], Vec<(i64, [f64; 3], i64, f64)>);

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
                    .filter(|o| npc.is_some_and(|n| n.has_component(o.type_id, COMPONENT)))
                    .map(|o| (o.item_id, o.position, o.type_id, o.radius))
                    .collect();
                (i.site_id, i.system, i.dungeon_id, i.anchor, obj)
            })
            .collect()
    };
    let live: BTreeSet<i64> = sites.iter().map(|s| s.0).collect();
    let items: BTreeSet<(i64, i64)> = sites
        .iter()
        .flat_map(|s| s.4.iter().map(|o| (s.0, o.0)))
        .collect();
    let st = reg.ext_mut::<crate::facwar::Facwar>().capture_mut();
    // A taken point is kept only while the population still lists its site.
    st.points
        .retain(|(id, item), _| live.contains(id) && (*item == 0 || items.contains(&(*id, *item))));
    for (site_id, system, dungeon, anchor, objs) in sites {
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
        // A battlefield's other points go on after one is taken (#8).
        if (reg
            .ext::<crate::facwar::Facwar>()
            .capture()
            .is_captured(site_id)
            && !crate::fw_scoreboard::is_battlefield(reg, site_id))
            || crate::site_spawn::is_completed(reg, site_id)
        {
            continue;
        }
        // A battlefield has no anchor fallback: it has no point until its
        // point objects exist, and none once its board is won (#8).
        let battlefield = crate::fw_scoreboard::is_battlefield(reg, site_id);
        if battlefield && (objs.is_empty() || crate::fw_scoreboard::is_won(reg, site_id)) {
            continue;
        }
        // One point per capture-point object; a complex without one is judged
        // at its anchor under item 0.
        type Target = (Option<i64>, [f64; 3], Option<i64>, f64);
        let targets: Vec<Target> = if objs.is_empty() {
            vec![(None, anchor, None, 0.0)]
        } else {
            objs.iter()
                .map(|o| (Some(o.0), o.1, Some(o.2), o.3))
                .collect()
        };
        for (item, position, kind, radius) in targets {
            let empty = objs.is_empty() && crate::site_spawn::is_empty_pocket(reg, site_id);
            let params = interim_params(&params_for(reg, kind, battlefield), empty);
            let (capturing, blocking) = near(reg, system, position, radius, &params, ft);
            let (phase, holder) = judge(o.attacker, o.occupier, &capturing, &blocking);
            let (awards, before, after) = {
                let st = reg.ext_mut::<crate::facwar::Facwar>().capture_mut();
                let p = st
                    .points
                    .entry((site_id, item.unwrap_or(0)))
                    .or_insert_with(|| Point {
                        site_id,
                        system,
                        item_id: item,
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
                        // A battlefield's points move its scoreboard, not the
                        // system's victory points (#8) **[A]**.
                        if battlefield {
                            crate::fw_scoreboard::credit(reg, site_id, holder, points, ft);
                        } else {
                            let delta = if holder == o.attacker {
                                points
                            } else {
                                -points
                            };
                            crate::facwar::add_vp(reg, system, delta, ft);
                        }
                    }
                    Award::Taken { holder } => {
                        // A taken point at a battlefield scores the point's
                        // value and never completes the slot: the board's win
                        // does (#8) **[A]**.
                        if battlefield {
                            crate::fw_scoreboard::credit(
                                reg,
                                site_id,
                                holder,
                                params.point_value,
                                ft,
                            );
                        } else if crate::population::complete(reg, system, site_id, ft) {
                            // Paid only when the slot was still live to take.
                            crate::fw_lp::on_complete(reg, site_id, system, anchor, ft);
                        }
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
            ..assumed()
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
            .insert((1, 3), p.clone());
        let got = slim_extras(&reg, &[3]);
        let keys: Vec<&str> = got[0].1.iter().map(|k| k.0).collect();
        assert_eq!(keys, vec![KEY_FACTION_INFO, KEY_NEXT_POINT]);
        advance(&mut p, &par, secs(1), Phase::Contested, Some(A));
        reg.ext_mut::<crate::facwar::Facwar>()
            .capture_mut()
            .points
            .insert((1, 3), p);
        let got = slim_extras(&reg, &[3]);
        assert_eq!(got[0].1.len(), 1, "no countdown while contested");
    }

    /// Invented capture-point type; the pilot of side B (the attacker of S1).
    const CP_TYPE: i64 = 990_101;

    fn put_pilot(reg: &mut Registry, at: [f64; 3]) {
        use crate::facwar::tests as fx;
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                fx::ME,
                fx::ME + 100,
                fx::S1,
                crate::space_state::Motion::arrived(at, [0.0; 3]),
            ));
    }

    fn run(reg: &mut Registry, from: BlueTime, to: BlueTime) {
        let mut t = from;
        while t < to {
            tick(reg, t);
            t += CHECK_EVERY;
        }
    }

    #[test]
    fn every_capture_point_of_a_site_is_its_own_point() {
        use crate::facwar::tests as fx;
        let mut reg = fx::world_with(|npc, _, _, _| {
            npc.components.insert((CP_TYPE, COMPONENT.to_owned()));
        });
        let now = reg.world().birth;
        let site = fx::fw_sites(&reg, fx::S1)
            .into_iter()
            .find(|s| fx::dungeons().dungeons[&s.dungeon_id].faction == Some(fx::A))
            .expect("a complex of S1's occupier");
        assert!(crate::site::instantiate(&mut reg, &site));
        let near = site.anchor;
        let far = [near[0] + 200_000.0, near[1], near[2]];
        let one = crate::site::add_object(&mut reg, site.site_id, CP_TYPE, near).expect("one");
        let two = crate::site::add_object(&mut reg, site.site_id, CP_TYPE, far).expect("two");
        crate::service::facwar_mgr::enlist(&mut reg, fx::ME, fx::B, now).expect("enlists");
        put_pilot(&mut reg, near);
        let cycle = assumed().seconds * SECOND;
        run(&mut reg, now, now + 2 * cycle);
        let st = reg.ext::<crate::facwar::Facwar>().capture();
        assert_eq!(st.points_of(site.site_id).count(), 2, "one per object");
        let at = |item: i64| {
            st.points
                .get(&(site.site_id, item))
                .expect("a point")
                .clone()
        };
        assert!(at(one.item_id).cycles >= 1, "held: paid");
        assert_eq!(at(two.item_id).cycles, 0);
        assert_eq!(at(two.item_id).phase, Phase::Empty);
        assert!(!st.is_captured(site.site_id));
        // At a complex a looping point still pays the system's victory points
        // (at a battlefield it scores the board: `fw_scoreboard`'s tests).
        assert!(
            crate::facwar::occupation(&reg, fx::S1).is_some_and(|o| o.score > 0),
            "the complex's point moved the VP"
        );
        // Walk to the other point: it is judged and paid on its own.
        put_pilot(&mut reg, far);
        run(&mut reg, now + 2 * cycle, now + 5 * cycle);
        let st = reg.ext::<crate::facwar::Facwar>().capture();
        let at = |item: i64| {
            st.points
                .get(&(site.site_id, item))
                .expect("a point")
                .clone()
        };
        assert!(at(two.item_id).cycles >= 1);
        assert_eq!(at(one.item_id).phase, Phase::Empty);
    }

    /// Invented typelist ids for the gating tests.
    const LIST_YES: i64 = 990_201;
    const LIST_NO: i64 = 990_202;
    /// The typedata fixture's frigate: a hull the type data knows.
    const HULL: i64 = 990_000_001;

    fn row() -> crate::facwar::CapturePointRow {
        crate::facwar::CapturePointRow {
            seconds: Some(45),
            range: Some(7_000),
            point_value: Some(2),
            looping: Some(false),
            capturing: Some(LIST_YES),
            blocking: Some(LIST_NO),
        }
    }

    #[test]
    fn the_tables_values_are_used_and_a_missing_row_falls_back() {
        use crate::facwar::tests as fx;
        let reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        let p = params_for(&reg, Some(CP_TYPE), true);
        assert_eq!(
            (p.seconds, p.range, p.point_value, p.looping),
            (45, 7_000.0, 2, false)
        );
        assert_eq!(
            (p.capturing_list, p.blocking_list),
            (Some(LIST_YES), Some(LIST_NO))
        );
        assert_eq!(
            params_for(&reg, Some(CP_TYPE + 1), true),
            assumed(),
            "no row"
        );
        assert_eq!(params_for(&reg, None, true), assumed(), "no object");
        // An empty field keeps its own assumed value.
        let part = crate::facwar::CapturePointRow {
            seconds: Some(5),
            ..Default::default()
        };
        let reg = fx::world_with_points([(CP_TYPE, part)].into(), |_, _, _, _| ());
        let p = params_for(&reg, Some(CP_TYPE), true);
        assert_eq!(
            (p.seconds, p.range, p.looping),
            (5, assumed().range, assumed().looping)
        );
    }

    #[test]
    fn a_rows_looping_counts_at_a_battlefield_only() {
        use crate::facwar::tests as fx;
        let reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        assert!(!params_for(&reg, Some(CP_TYPE), true).looping);
        assert_eq!(
            params_for(&reg, Some(CP_TYPE), false).looping,
            assumed().looping,
            "a complex keeps the assumed behaviour"
        );
    }

    #[test]
    fn a_full_gm_override_wins_over_the_table() {
        use crate::facwar::tests as fx;
        let mut reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        let over = Params {
            seconds: 3,
            ..assumed()
        };
        reg.ext_mut::<crate::facwar::Facwar>()
            .capture_mut()
            .set_params(over.clone());
        assert_eq!(params_for(&reg, Some(CP_TYPE), true), over);
    }

    #[test]
    fn fwcapture_seconds_override_only_the_cycle_on_top_of_the_row() {
        use crate::facwar::tests as fx;
        let mut reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        slash(&mut reg, None, &["9"]).expect("sets");
        let p = params_for(&reg, Some(CP_TYPE), true);
        assert_eq!(
            (p.seconds, p.range, p.point_value),
            (9, 7_000.0, 2),
            "the row's other values stay"
        );
        let shown = slash(&mut reg, None, &[]).expect("shows");
        let PyValue::Bytes(bytes) = shown else {
            panic!("the slash answers bytes");
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert!(text.contains("seconds override 9"), "{text}");
        assert!(text.contains("table rows apply"), "{text}");
        assert!(text.contains(&format!("type {CP_TYPE}")), "{text}");
        slash(&mut reg, None, &["reset"]).expect("resets");
        assert_eq!(params_for(&reg, Some(CP_TYPE), true).seconds, 45);
    }

    fn list_of(hull: i64) -> crate::dungeon_data::TypeList {
        crate::dungeon_data::TypeList {
            inc_types: vec![hull],
            ..Default::default()
        }
    }

    /// A world whose pilot ME flies [`HULL`], with the two lists holding
    /// `yes` and `no`.
    fn listed_world(yes: i64, no: i64) -> Registry {
        use crate::facwar::tests as fx;
        let mut reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, dun, _, _| {
            dun.typelists.insert(LIST_YES, list_of(yes));
            dun.typelists.insert(LIST_NO, list_of(no));
        });
        reg.character_mut(fx::ME).expect("a pilot").ship_type_id = HULL;
        let now = reg.world().birth;
        crate::service::facwar_mgr::enlist(&mut reg, fx::ME, fx::B, now).expect("enlists");
        put_pilot(&mut reg, [0.0; 3]);
        reg
    }

    fn seen_with(yes: i64, no: i64) -> (Vec<i64>, Vec<i64>) {
        use crate::facwar::tests as fx;
        let reg = listed_world(yes, no);
        let p = params_for(&reg, Some(CP_TYPE), true);
        near(&reg, fx::S1, [0.0; 3], 0.0, &p, reg.world().birth)
    }

    #[test]
    fn the_capturing_and_blocking_lists_say_who_captures_and_who_blocks() {
        use crate::facwar::tests as fx;
        let other = HULL + 1;
        assert_eq!(
            seen_with(HULL, other),
            (vec![fx::B], vec![]),
            "in the capturing list"
        );
        assert_eq!(
            seen_with(other, HULL),
            (vec![], vec![fx::B]),
            "in the blocking list"
        );
        assert_eq!(seen_with(other, other), (vec![], vec![]), "in neither");
        assert_eq!(
            seen_with(HULL, HULL),
            (vec![fx::B], vec![]),
            "the capturing list is asked first"
        );
    }

    #[test]
    fn a_hull_the_type_data_lacks_is_in_no_list() {
        use crate::facwar::tests as fx;
        let mut reg = listed_world(HULL, HULL);
        reg.character_mut(fx::ME).expect("a pilot").ship_type_id = HULL + 500;
        assert_eq!(in_list(&reg, LIST_YES, HULL + 500), Some(false));
        assert_eq!(in_list(&reg, LIST_YES, HULL), Some(true));
        assert_eq!(in_list(&reg, 1, HULL), None, "a list the bundle lacks");
    }

    #[test]
    fn a_row_naming_lists_the_bundle_lacks_does_not_panic_and_keeps_the_ids() {
        use crate::facwar::tests as fx;
        let reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        let p = params_for(&reg, Some(CP_TYPE), true);
        assert_eq!(p.capturing_list, Some(LIST_YES));
        assert_eq!(p.blocking_list, Some(LIST_NO));
    }

    #[test]
    fn the_range_is_a_strict_surface_distance() {
        use crate::facwar::tests as fx;
        let mut reg = fx::world_with_points([(CP_TYPE, row())].into(), |_, _, _, _| ());
        reg.character_mut(fx::ME).expect("a pilot").ship_type_id = HULL;
        let now = reg.world().birth;
        crate::service::facwar_mgr::enlist(&mut reg, fx::ME, fx::B, now).expect("enlists");
        let p = params_for(&reg, Some(CP_TYPE), true);
        assert_eq!(p.range, 7_000.0);
        let ship_r = f64::from(crate::destiny::hull::of_in(reg.content(), HULL).radius);
        let point_r = 400.0;
        let at = |d: f64, reg: &mut Registry| {
            put_pilot(reg, [d, 0.0, 0.0]);
            near(reg, fx::S1, [0.0; 3], point_r, &p, now).0
        };
        // Centre distance = range + both radii is the surface edge: out.
        let edge = p.range + point_r + ship_r;
        assert_eq!(at(edge - 1.0, &mut reg), vec![fx::B], "just inside");
        assert!(at(edge, &mut reg).is_empty(), "exactly at the range: out");
        assert!(at(edge + 1.0, &mut reg).is_empty(), "outside");
    }
}
