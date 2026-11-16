//! Skyhooks (SKY-1, issue #81): deploy over a planet, activation, the
//! reagent harvest, the gateway state, extraction and the post-claim
//! cleanup. Wire contract: `docs/re/sov-gateway.md` §2.6, §3, §6; the rule
//! of the harvest is the client's own, [`harvest`].
//!
//! **What a skyhook is.** An item of group [`GROUP_SKYHOOK`] **[V]**
//! (`inventorycommon/const`, see `tools/typedata.py`) in space at its solar
//! system ([`FLAG_IN_SPACE`]), owned by the launching corporation, plus one
//! row of kind [`KIND`] (`delta_kinds::KINDS`):
//!
//! ```text
//! skyhook <skyhookID> -> planetID systemID corpID active activatedAt
//!     builtUntil secured unsecured lastCycle vulnStart vulnEnd state
//! ```
//!
//! Every time is **unix seconds on the wall clock** (spec §1.4): the client
//! extrapolates the harvest from the record's `last_cycle` with
//! `GetWallclockTime`, so every rule here is a pure function of the stored
//! row and `now` and never reads the game clock. `state` (13th value) is the
//! `orbitalSkyhook.const` state a GM sets; the spec's row has twelve values,
//! this one trails them so a reader of the twelve still parses.
//!
//! **A ball over its planet**, global in its system like a hub **[A]**
//! (falsifier: the client never drawing it off-grid; then make it local like
//! the customs office). It stands where the customs office does, mirrored
//! across the warp-in point, so the two never overlap **[A]**.
//!
//! **Lifecycle.** [`launch_one`] (`ship.Drop`) places it unbuilt;
//! [`tick`] (stage `skyhook`) finishes the build, which activates it and tells
//! the owner corp (`OnSkyhookFirstActivation`) **[V]** name and arguments
//! (`sovereigntyResourceSvc.py:74-78`); the owner turns it on and off over
//! the gateway ([`gateway`]); [`extract`] takes reagents to a ship;
//! [`acquire`] and [`destroy`] are the new holder's cleanup.
//!
//! **What counts toward the hub**: [`counted`], read by
//! `sov_resources::harvested`. Every change calls [`changed`]; L6's
//! `sov_upgrade::resimulate` joins there when it lands.

use crate::delta::Deltas;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::gateway::{Ground, Reply, Target, put_bytes};
use crate::item::{Edit, Slot};
use crate::notify::Push;
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::sov;
use crate::sov_proto::{duration, id, put_bool, put_uint, read_id, status, timestamp, typed};
use crate::sov_resources::{self, Reagent};
use crate::space_component::CompCall;
use eve_marshal::PyValue;

mod slash_cmd;
#[cfg(test)]
mod tests;

pub use slash_cmd::slash;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "skyhook";

/// `invConst.groupSkyhook` **[V]** (`tools/typedata.py`, SOV-DATA).
pub const GROUP_SKYHOOK: i64 = 4736;

/// Where an item in space sits.
pub const FLAG_IN_SPACE: i64 = crate::combat::FLAG_IN_SPACE;

/// How far from a planet's skyhook spot a launch may be, centre to centre.
/// **[A]** 100 km (spec L8.2). Falsified by a client-side refusal naming
/// another distance, or retail launching at the pilot's own position.
pub const LAUNCH_RANGE: f64 = 100_000.0;

/// How far from the skyhook's surface a pilot may take reagents. Same as the
/// hub's fuel reach, `maxCargoContainerTransferDistance` **[V]**
/// (R-SOV §2); the skyhook window's own gate is `IsWithinConfigRange`
/// **[V]** (`celestialCheckers.py:2092`), whose number is not read here.
pub const TRANSFER_RANGE: f64 = crate::sov_hub::TRANSFER_RANGE;

/// Seconds a launched skyhook builds before it activates. **[A]** the
/// type's activate-component duration, which the bundle does not carry;
/// 120 s is the client's own fallback (`orbitalSkyhook.py:41`, `or 120`)
/// **[V]**. Falsified by a type row that names a duration.
pub const ACTIVATION_S: u64 = 120;

/// `sovereignty.hub.const.SKYHOOK_CLAIM_FEE` **[V]**
/// (`sovereignty/hub/const.py`, ISK per skyhook).
pub const SKYHOOK_CLAIM_FEE: f64 = 200_000_000.0;

/// `sovereignty.hub.const.ALLOWED_DURATION_AFTER_SOVHUB_CLAIM` **[V]**
/// (seconds): how long after a claim the new holder may acquire or destroy
/// the old holder's skyhooks.
pub const CLAIM_WINDOW_S: u64 = 86_400;

/// Ledger reference of the claim fee: entry type 184, "Skyhook claim fee"
/// (`ledger.rs`) **[V]**.
pub const REF_SKYHOOK_CLAIM_FEE: i64 = 184;

/// `orbitalSkyhook.const` states **[V]**.
pub const STATE_UNKNOWN: u32 = 0;
pub const STATE_SHIELD_VULNERABLE: u32 = 210;
pub const STATES: [u32; 6] = [0, 210, 211, 212, 213, 214];

/// The window a reagent skyhook reports until L9 schedules real ones: it
/// starts this far ahead and lasts [`SYNTH_WINDOW_S`] **[A]** (spec L8.7),
/// so the client's staleness rule never fires (RE §2.6).
pub const SYNTH_AHEAD_S: u64 = 7 * 86_400;

/// How far ahead of now an inactive skyhook's `last_cycle` is reported, so
/// the client's own extrapolation counts no cycle: ten years **[I]**.
pub const FROZEN_AHEAD_S: u64 = 10 * 365 * 86_400;
pub const SYNTH_WINDOW_S: u64 = 2 * 3_600;

/// The ball's radius when the type table has none. **[A]**
pub const DEFAULT_RADIUS: f64 = 2_000.0;

const NOTICE_PKG: &str = "eve_public.sovereignty.skyhook.api.";
const API_PKG: &str = "eve_public.sovereignty.skyhook.api.";

/// Seconds from the FILETIME epoch to the unix epoch.
const FILETIME_UNIX: u64 = 11_644_473_600;

/// A unix second as a FILETIME (the client's sim time).
pub fn filetime(unix: u64) -> i64 {
    ((unix.saturating_add(FILETIME_UNIX)).saturating_mul(10_000_000)) as i64
}

/// A FILETIME as a unix second (0 before the unix epoch).
pub fn unix_of(filetime: i64) -> u64 {
    ((filetime.max(0) as u64) / 10_000_000).saturating_sub(FILETIME_UNIX)
}

/// Wall-clock unix seconds now.
pub fn now() -> u64 {
    crate::sov_proto::unix_now()
}

// ---------------------------------------------------------------------------
// The harvest (the client's rule)
// ---------------------------------------------------------------------------

/// A skyhook's two stocks and the end of its last whole cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sim {
    pub secured: u32,
    pub unsecured: u32,
    pub last_cycle: u64,
}

/// `reagent_split_production_simulation.get_reagent_split_production_data_at_time`
/// **[V]** (`:8-33`), exactly: whole cycles since `last_cycle`; `produced =
/// cycles * per_cycle`; the secured share is `int(secure_ratio * produced)`
/// with `secure_ratio = float(pct) / 100.0`; each pool is capped, the
/// overflow fills the other pool's free room, then is lost; `last_cycle`
/// advances by whole cycles only.
///
/// The share is done in `f64`, as the client does, not in integers as the
/// spec first sketched: `int(0.29 * 100)` is 28 in Python and `29 * 100 /
/// 100` is 29, so the integer form would differ from the client's own
/// extrapolation for some percentages.
pub fn harvest(s: &Sim, r: &Reagent, now: u64) -> Sim {
    if r.cycle_s == 0 || now <= s.last_cycle {
        return *s;
    }
    let cycles = (now - s.last_cycle) / r.cycle_s;
    if cycles == 0 {
        return *s;
    }
    let produced = cycles.saturating_mul(u64::from(r.per_cycle));
    let ratio = f64::from(r.secured_pct) / 100.0;
    let share = (ratio * produced as f64) as u64;
    let mut secured = u64::from(s.secured).saturating_add(share);
    let mut unsecured = u64::from(s.unsecured).saturating_add(produced.saturating_sub(share));
    let (scap, ucap) = (u64::from(r.secured_cap), u64::from(r.unsecured_cap));
    let mut overflow = 0u64;
    if secured > scap {
        overflow += secured - scap;
        secured = scap;
    }
    if unsecured > ucap {
        overflow += unsecured - ucap;
        unsecured = ucap;
    }
    if overflow > 0 && secured < scap {
        let extra = overflow.min(scap - secured);
        secured += extra;
        overflow -= extra;
    }
    if overflow > 0 && unsecured < ucap {
        let extra = overflow.min(ucap - unsecured);
        unsecured += extra;
    }
    Sim {
        secured: secured as u32,
        unsecured: unsecured as u32,
        last_cycle: s
            .last_cycle
            .saturating_add(cycles.saturating_mul(r.cycle_s)),
    }
}

// ---------------------------------------------------------------------------
// The row
// ---------------------------------------------------------------------------

/// One skyhook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skyhook {
    pub id: i64,
    pub planet: i64,
    pub system: i64,
    pub corp: i64,
    pub active: bool,
    /// Unix second the build finished; 0 while it is still building.
    pub activated_at: u64,
    /// Unix second the build finishes.
    pub built_until: u64,
    pub secured: u32,
    pub unsecured: u32,
    pub last_cycle: u64,
    /// A theft window L9 scheduled; 0 and 0 when none.
    pub vuln_start: u64,
    pub vuln_end: u64,
    pub state: u32,
}

impl Skyhook {
    /// Whether the build is over.
    pub fn built(&self) -> bool {
        self.activated_at > 0
    }

    pub fn sim(&self) -> Sim {
        Sim {
            secured: self.secured,
            unsecured: self.unsecured,
            last_cycle: self.last_cycle,
        }
    }
}

fn parse_row(key: &[String], v: &[String]) -> Option<Skyhook> {
    let n = |i: usize| v.get(i)?.parse::<u64>().ok();
    let i = |i: usize| v.get(i)?.parse::<i64>().ok();
    Some(Skyhook {
        id: key.first()?.parse().ok()?,
        planet: i(0)?,
        system: i(1)?,
        corp: i(2)?,
        active: n(3)? != 0,
        activated_at: n(4)?,
        built_until: n(5)?,
        secured: u32::try_from(n(6)?).ok()?,
        unsecured: u32::try_from(n(7)?).ok()?,
        last_cycle: n(8)?,
        vuln_start: n(9).unwrap_or(0),
        vuln_end: n(10).unwrap_or(0),
        state: n(11)
            .and_then(|s| u32::try_from(s).ok())
            .unwrap_or(STATE_SHIELD_VULNERABLE),
    })
}

/// Every skyhook, in id order. A row that does not parse is logged and left
/// out.
pub fn all(d: &Deltas) -> Vec<Skyhook> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let row = parse_row(k, v);
            if row.is_none() {
                crate::errln!("[skyhook] skipping row {k:?}: {v:?}");
            }
            row
        })
        .collect()
}

/// The skyhook `id`.
pub fn by_id(d: &Deltas, id: i64) -> Option<Skyhook> {
    let key = vec![id.to_string()];
    parse_row(&key, d.of(KIND).get(&key)?)
}

/// The skyhooks in `system`.
pub fn in_system(d: &Deltas, system: i64) -> Vec<Skyhook> {
    all(d).into_iter().filter(|s| s.system == system).collect()
}

/// The skyhook over `planet`, if any.
pub fn at_planet(d: &Deltas, planet: i64) -> Option<Skyhook> {
    all(d).into_iter().find(|s| s.planet == planet)
}

/// Write one row.
pub fn put(d: &mut Deltas, s: &Skyhook) {
    d.put(
        KIND,
        vec![s.id.to_string()],
        vec![
            s.planet.to_string(),
            s.system.to_string(),
            s.corp.to_string(),
            u8::from(s.active).to_string(),
            s.activated_at.to_string(),
            s.built_until.to_string(),
            s.secured.to_string(),
            s.unsecured.to_string(),
            s.last_cycle.to_string(),
            s.vuln_start.to_string(),
            s.vuln_end.to_string(),
            s.state.to_string(),
        ],
    );
}

fn reagent_of(reg: &Registry, planet: i64) -> Option<Reagent> {
    sov_resources::planet_yield(reg, planet)?.reagent
}

/// What the stocks read at `now`: the harvest while active, the stored
/// values while not (an inactive skyhook does not harvest).
pub fn current(sk: &Skyhook, reagent: Option<&Reagent>, now: u64) -> Sim {
    match reagent {
        Some(r) if sk.active => harvest(&sk.sim(), r, now),
        _ => sk.sim(),
    }
}

/// Whether the skyhook's owner corp is in the alliance that holds its
/// system. Spec L8.9 **[A]** (falsifier: TQ letting a skyhook of a dispossessed
/// owner keep running): after a claim changes hands the old holder's skyhooks
/// stay with their owner but do nothing -- they do not harvest, cannot be
/// switched on and cannot be emptied -- until the owner holds the system
/// again or the new holder acquires or destroys them.
pub fn held(d: &Deltas, sk: &Skyhook) -> bool {
    let holder = sov::holder(d, sk.system);
    holder.is_some() && corp_alliance(d, sk.corp) == holder
}

/// Whether the skyhook is working: switched on **and** [`held`].
pub fn running(d: &Deltas, sk: &Skyhook) -> bool {
    sk.active && held(d, sk)
}

/// A copy of `sk` whose `active` says whether it is [`running`], for the
/// readers of [`current`]. A running skyhook harvests from the later of its
/// `last_cycle`, the moment its owner's alliance took the system (the
/// claim's `claim_time`, which `sov::set_claim` resets on every change of
/// holder) and the moment the owner corp joined that alliance, so a system
/// lost and regained, or an owner that joined late, does not pay for the gap
/// **[A]** (falsifier: TQ paying the gap).
fn live(d: &Deltas, sk: &Skyhook) -> Skyhook {
    // One load of the alliances serves both `running` (the holder test) and
    // the join-time clamp.
    let member = crate::alliance::Alliances::load(d).member(sk.corp);
    let holder = sov::holder(d, sk.system);
    let active = sk.active && holder.is_some() && member.map(|m| m.alliance_id) == holder;
    let mut last_cycle = sk.last_cycle;
    if active {
        if let Some(t) = sov::claim(d, sk.system).and_then(|c| c.claim_time) {
            last_cycle = last_cycle.max(unix_of(t));
        }
        // And from when the owner corp joined the holder alliance: a corp
        // that stood outside it earned nothing, and joining does not pay the
        // gap since its last cycle **[A]** (falsifier: TQ paying it).
        if let Some(m) = member {
            last_cycle = last_cycle.max(unix_of(m.start_date));
        }
    }
    Skyhook {
        active,
        last_cycle,
        ..*sk
    }
}

/// Bring the stored stocks up to `now` (a no-op when not [`running`]) and
/// write the row. The skyhook as stored afterwards.
pub fn settle(reg: &mut Registry, id: i64, now: u64) -> Option<Skyhook> {
    let mut sk = by_id(reg.deltas(), id)?;
    let reagent = reagent_of(reg, sk.planet);
    let s = current(&live(reg.deltas(), &sk), reagent.as_ref(), now);
    if s != sk.sim() {
        sk.secured = s.secured;
        sk.unsecured = s.unsecured;
        sk.last_cycle = s.last_cycle;
        put(reg.deltas_mut(), &sk);
    }
    Some(sk)
}

// ---------------------------------------------------------------------------
// Who
// ---------------------------------------------------------------------------

/// The alliance `corp` is in.
pub fn corp_alliance(d: &Deltas, corp: i64) -> Option<i64> {
    crate::alliance::Alliances::load(d)
        .member(corp)
        .map(|m| m.alliance_id)
}

fn pilot(reg: &Registry, who: i64) -> Option<(i64, Option<i64>)> {
    reg.character(who)
        .map(|c| (c.corporation_id, c.alliance_id))
}

/// Whether the skyhook is the pilot's corp's.
pub fn owns(reg: &Registry, who: i64, sk: &Skyhook) -> bool {
    pilot(reg, who).is_some_and(|(corp, _)| corp == sk.corp)
}

/// Who may read a skyhook's state: its corp, or the alliance that holds its
/// system **[A]** (falsifier: a retail window open to anyone in the system).
pub fn may_read(reg: &Registry, who: i64, sk: &Skyhook) -> bool {
    let Some((corp, alliance)) = pilot(reg, who) else {
        return false;
    };
    corp == sk.corp || (alliance.is_some() && sov::holder(reg.deltas(), sk.system) == alliance)
}

/// Whether `who` may switch or launch skyhooks for their corp: a director,
/// the CEO or a station manager **[A]** (the hub's rule).
pub fn may_manage(reg: &Registry, who: i64) -> bool {
    crate::service::sov_hub::may_launch(reg, who)
}

// ---------------------------------------------------------------------------
// What counts toward the hub (owner Q2)
// ---------------------------------------------------------------------------

/// `(power, workforce)` the **active** skyhooks of corps in `system`'s
/// holder alliance give. Reagent planets give neither.
pub fn counted(reg: &Registry, system: i64) -> (u64, u64) {
    let d = reg.deltas();
    if d.of(KIND).is_empty() || sov::holder(d, system).is_none() {
        return (0, 0);
    }
    let alliances = crate::alliance::Alliances::load(d);
    let holder = sov::holder(d, system);
    let mut out = (0u64, 0u64);
    for sk in in_system(d, system) {
        let same = alliances.member(sk.corp).map(|m| m.alliance_id) == holder;
        if !(sk.active && same) {
            continue;
        }
        if let Some(y) = sov_resources::planet_yield(reg, sk.planet) {
            out.0 += y.power;
            out.1 += y.workforce;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Placement, the ball, the slim
// ---------------------------------------------------------------------------

/// Where the skyhook over `planet` stands: the customs office's spot
/// mirrored across the planet's warp-in point **[A]** (falsifier: the two
/// overlapping on a planet the client draws).
pub fn position_of(planet: &crate::celestial::Celestial) -> [f64; 3] {
    let c = planet.position;
    let w = crate::warp_in::warp_in_point(planet).unwrap_or([
        c[0] + planet.radius + 1_000_000.0,
        c[1],
        c[2],
    ]);
    let d = [w[0] - c[0], w[1] - c[1], w[2] - c[2]];
    let out = crate::space_state::unit(d).unwrap_or([1.0, 0.0, 0.0]);
    let k = crate::planet::CUSTOMS_OFFICE_STANDOFF;
    let at = [w[0] - out[0] * k, w[1] - out[1] * k, w[2] - out[2] * k];
    if at.iter().all(|v| v.is_finite()) {
        at
    } else {
        // A planet on the system's y axis has no warp-in angle
        // (`planet_warp_in` divides 0 by 0); stand it off along +x.
        [c[0] + planet.radius + 1_000_000.0 - k, c[1], c[2]]
    }
}

/// The skyhook's position, from its planet in the catalogue.
pub fn position(reg: &Registry, sk: &Skyhook) -> Option<[f64; 3]> {
    reg.celestials().get(sk.planet).map(position_of)
}

/// The ball's radius: the type table's, else [`DEFAULT_RADIUS`].
pub fn radius(content: &crate::content::Content, type_id: i64) -> f64 {
    content
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(type_id))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(DEFAULT_RADIUS)
}

fn type_of(reg: &Registry, id: i64) -> Option<i64> {
    reg.store().get(id).map(|i| i.type_id)
}

fn owner_alliance(d: &Deltas, sk: &Skyhook) -> Option<i64> {
    corp_alliance(d, sk.corp)
}

/// The skyhook's ball: static, global in its system **[A]**.
pub fn ball(reg: &Registry, sk: &Skyhook) -> Option<Ball> {
    let ty = type_of(reg, sk.id)?;
    Some(Ball {
        ball_id: sk.id,
        mode: Mode::Stop,
        radius: radius(reg.content(), ty) as f32,
        position: position(reg, sk)?,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(sk.corp).unwrap_or(-1),
        alliance_id: owner_alliance(reg.deltas(), sk)
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        physics: None,
        formation_id: NO_FORMATION,
    })
}

/// The five basic slim attributes; the rest ride in [`extras_of`].
pub fn slim(reg: &Registry, sk: &Skyhook) -> Option<Slim> {
    Some(Slim {
        item_id: sk.id,
        type_id: type_of(reg, sk.id)?,
        owner_id: sk.corp,
        char_id: None,
        corp_id: Some(sk.corp),
        response_thresholds: None,
    })
}

/// `skyhook_state` as the slim carries it: unknown (offline) until the
/// build is done, then the stored state **[A]** (falsifier: the bracket
/// showing "reinforced" for a plain skyhook).
pub fn slim_state(sk: &Skyhook) -> u32 {
    if sk.built() { sk.state } else { STATE_UNKNOWN }
}

/// `component_activate = (done, activation time)` (RE §3.4) **[V]**: the
/// model builds over `GetActivationDurationForItem` and, while not done,
/// counts down to the sim time named.
pub fn component_activate(sk: &Skyhook) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Bool(sk.built()),
        PyValue::Int(filetime(if sk.built() {
            sk.activated_at
        } else {
            sk.built_until
        })),
    ])
}

/// The extra slim keys (RE §3.4) **[V]**: `planetID`, `skyhook_state`,
/// `skyhook_timer` (none), `dunRotation` (zeros **[A]**: the client then builds a
/// rotation adapter; falsifier: the traceback persists), `component_activate` and `allianceID` (the
/// cleanup window compares it with the session's, `sovSvc.py:408`).
pub fn extras_of(reg: &Registry, sk: &Skyhook) -> Vec<(&'static str, PyValue)> {
    vec![
        ("planetID", PyValue::Int(sk.planet)),
        ("skyhook_state", PyValue::Int(i64::from(slim_state(sk)))),
        ("skyhook_timer", PyValue::None),
        // BasicOrbital.SetStaticRotation leaves `rotationCurve` None without
        // it, and OrbitalSkyhook._UpdateModel (called from OnLoadObject:171)
        // reads `.value` on that [V] (basicOrbital.py:14,
        // spaceObject.py:505-524,
        // spacecomponents/client/components/orbitalSkyhook.py:191-192).
        (
            "dunRotation",
            PyValue::Tuple(vec![
                PyValue::Real(0.0),
                PyValue::Real(0.0),
                PyValue::Real(0.0),
            ]),
        ),
        ("component_activate", component_activate(sk)),
        (
            "allianceID",
            owner_alliance(reg.deltas(), sk).map_or(PyValue::None, PyValue::Int),
        ),
        // SKY-2: the silo that stands while a theft window is open.
        (
            "reagentSiloItemID",
            crate::skyhook_theft::silo_of(reg.deltas(), sk.id).map_or(PyValue::None, PyValue::Int),
        ),
    ]
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|s| ids.contains(&s.id))
        .map(|s| (s.id, extras_of(w, s)))
        .collect()
}

/// `system`'s skyhook balls and slims, for `destiny::scene` (coordinator:
/// one loop there). Global: in no `grid::local_balls`.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    in_system(w.deltas(), system)
        .iter()
        .filter_map(|s| Some((ball(w, s)?, slim(w, s)?)))
        .collect()
}

/// Where the skyhook `ball_id` is, for `Registry::ball_target` (a hot line
/// in `service.rs`, named in the report): approach, orbit, warp to.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let sk = by_id(w.deltas(), ball_id)?;
    Some(crate::model::Target {
        position: position(w, &sk)?,
        radius: radius(w.content(), type_of(w, ball_id)?),
        warp_in: None,
    })
}

fn in_space(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnSkyhookFirstActivation(skyhookID, planetID)` to the owner corp's online
/// members: the client flushes its skyhook cache and re-reads the
/// vulnerability **[V]** (`sovereigntyResourceSvc.py:74-78`). Nothing in the
/// client scatters it, so it is a server notify **[I]**.
#[derive(Debug, Clone, PartialEq)]
pub struct FirstActivation {
    pub to: Vec<i64>,
    pub skyhook: i64,
    pub planet: i64,
}

impl Push for FirstActivation {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSkyhookFirstActivation",
            vec![PyValue::Int(self.skyhook), PyValue::Int(self.planet)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `AddBalls2` of a skyhook to every park in its system.
fn announce_added(reg: &mut Registry, sk: &Skyhook) {
    let to = in_space(reg, sk.system);
    let (Some(b), Some(s)) = (ball(reg, sk), slim(reg, sk)) else {
        return;
    };
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![b],
        },
        vec![(s, None)],
    )
    .map(|a| a.with_extras(slim_extras(reg, &[sk.id])));
    if let Some(a) = added {
        reg.push(crate::sov_hub::HubAdded(a));
    }
}

/// `OnSlimItemChange` with the skyhook's slim as it now reads.
fn announce_slim(reg: &mut Registry, sk: &Skyhook) {
    let to = in_space(reg, sk.system);
    let Some(s) = slim(reg, sk) else { return };
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let push = crate::sov_hub::HubSlimChange {
        to,
        stamp,
        slim: s,
        extras: extras_of(reg, sk),
    };
    reg.push(push);
}

fn announce_removed(reg: &mut Registry, system: i64, id: i64) {
    let to = in_space(reg, system);
    let stamp = reg.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, vec![id]) {
        reg.push(gone);
    }
}

fn system_target(reg: &Registry, system: i64) -> Target {
    Target::SolarSystem {
        id: system,
        members: crate::service::structures::system_audience(reg, system),
    }
}

/// `ActivationNotice` and `AllInSolarSystemNotice` to the system: the
/// client re-primes its local cache (`notice_listener.py:62-80`) **[V]**.
fn announce_state(reg: &mut Registry, sk: &Skyhook, now: u64) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(sk.system as u64));
    put_bytes(&mut p, 2, &id(sk.planet as u64));
    put_bytes(&mut p, 3, &id(sk.id as u64));
    put_bool(&mut p, 4, running(reg.deltas(), sk));
    let target = system_target(reg, sk.system);
    crate::gateway_out::queue(reg, &format!("{NOTICE_PKG}ActivationNotice"), p, target);
    announce_all(reg, sk.system, now);
}

/// `AllInSolarSystemNotice` for `system`.
pub fn announce_all(reg: &mut Registry, system: i64, now: u64) {
    let mut p = Vec::new();
    for sk in in_system(reg.deltas(), system) {
        put_bytes(&mut p, 1, &record(Msg::AllInSystem, &view(reg, &sk, now)));
    }
    put_bytes(&mut p, 2, &id(system as u64));
    let target = system_target(reg, system);
    crate::gateway_out::queue(
        reg,
        &format!("{NOTICE_PKG}AllInSolarSystemNotice"),
        p,
        target,
    );
}

/// `ReagentSimulationsNotice` for one skyhook: the stocks changed.
pub fn announce_stocks(reg: &mut Registry, sk: &Skyhook, now: u64) {
    let p = record(Msg::Simulations, &view(reg, sk, now));
    let target = system_target(reg, sk.system);
    crate::gateway_out::queue(
        reg,
        &format!("{NOTICE_PKG}ReagentSimulationsNotice"),
        p,
        target,
    );
}

/// Everything that follows a state change of `sk`: the hub's budget, the
/// system's notices and the slim. The system's hub re-simulates its upgrades
/// (the budget moved), then the resources window is told.
pub fn changed(reg: &mut Registry, sk: &Skyhook, now: u64) {
    changed_since(reg, sk, now, &crate::sov_workforce::Before::default());
}

/// [`changed`], telling the peer hubs whose workforce state moved since
/// `before` (`sov_workforce::before`, taken ahead of the change).
pub fn changed_since(
    reg: &mut Registry,
    sk: &Skyhook,
    now: u64,
    before: &crate::sov_workforce::Before,
) {
    if let Some(h) = crate::sov_hub::hub(reg.deltas(), sk.system) {
        crate::sov_upgrade::resimulate(reg, h.hub_id);
        sov_resources::announce(reg, h.hub_id);
    }
    crate::sov_workforce::budgets_changed_since(reg, sk.system, before);
    announce_slim(reg, sk);
    announce_state(reg, sk, now);
}

// ---------------------------------------------------------------------------
// Deploy (`ship.Drop`)
// ---------------------------------------------------------------------------

/// Why a launch was refused, in words for a `CustomNotify` **[A]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotInSpace,
    NotInHold,
    NotASkyhook,
    Warping,
    NoAlliance,
    NoRole,
    NotHeld,
    NoPlanet,
    TooFar { metres: i64 },
    HasSkyhook,
    NothingToHarvest,
    Store(String),
}

impl Refused {
    pub fn text(&self) -> String {
        match self {
            Refused::NotInSpace => "You can only launch that in space, in your ship.".to_owned(),
            Refused::NotInHold => {
                "That is not in your ship's cargo hold, fleet hangar or infrastructure hangar."
                    .to_owned()
            }
            Refused::NotASkyhook => "That is not a skyhook.".to_owned(),
            Refused::Warping => "You cannot launch anything while in warp.".to_owned(),
            Refused::NoAlliance => {
                "A skyhook can only be launched for a corporation in an alliance.".to_owned()
            }
            Refused::NoRole => {
                "Only a director, the CEO or a station manager can launch a skyhook for the corporation."
                    .to_owned()
            }
            Refused::NotHeld => {
                "Your alliance does not hold sovereignty in this system.".to_owned()
            }
            Refused::NoPlanet => "There is no planet in this system to launch over.".to_owned(),
            Refused::TooFar { metres } => format!(
                "A skyhook is launched over a planet; fly within {} km of its spot (you are {} km away).",
                (LAUNCH_RANGE / 1000.0) as i64,
                metres / 1000
            ),
            Refused::HasSkyhook => "This planet already has a skyhook.".to_owned(),
            Refused::NothingToHarvest => "This planet has nothing to harvest.".to_owned(),
            Refused::Store(e) => format!("The launch failed: {e}"),
        }
    }
}

/// The planets of `system` as `(planet, spot)`.
fn planets(reg: &Registry, system: i64) -> Vec<(i64, [f64; 3])> {
    reg.celestials()
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Planet)
        .map(|c| (c.id, position_of(c)))
        .collect()
}

/// The checks of one launch, in the order a pilot meets them (spec L8.2).
/// `Ok` with the item, the system and the planet it goes over.
fn check_launch(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
) -> Result<(crate::item::Item, i64, i64), Refused> {
    let c = reg.character(actor).cloned().ok_or(Refused::NotInSpace)?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or(Refused::NotInSpace)?;
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor
                && i.location_id == c.ship_id
                && crate::sov_hub::LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .ok_or(Refused::NotInHold)?;
    if item.group_id != GROUP_SKYHOOK {
        return Err(Refused::NotASkyhook);
    }
    if here.motion.warp_active() {
        return Err(Refused::Warping);
    }
    let Some(alliance) = c.alliance_id else {
        return Err(Refused::NoAlliance);
    };
    if !may_manage(reg, actor) {
        return Err(Refused::NoRole);
    }
    let system = here.solar_system_id;
    if sov::holder(reg.deltas(), system) != Some(alliance) {
        return Err(Refused::NotHeld);
    }
    // The nearest planet wins.
    let (planet, away) = planets(reg, system)
        .into_iter()
        .map(|(p, at)| (p, crate::space_state::distance(here.motion.position, at)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .ok_or(Refused::NoPlanet)?;
    if away > LAUNCH_RANGE {
        return Err(Refused::TooFar {
            metres: away as i64,
        });
    }
    if at_planet(reg.deltas(), planet).is_some() {
        return Err(Refused::HasSkyhook);
    }
    if sov_resources::planet_yield(reg, planet).is_none() {
        return Err(Refused::NothingToHarvest);
    }
    Ok((item, system, planet))
}

/// The new item id among a plan's changes, else `fallback`.
fn created(changes: &[crate::item::Change], fallback: i64) -> i64 {
    changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// A new, unbuilt row for an item already in space.
fn place(reg: &mut Registry, id: i64, planet: i64, system: i64, corp: i64, now: u64) -> Skyhook {
    let sk = Skyhook {
        id,
        planet,
        system,
        corp,
        active: false,
        activated_at: 0,
        built_until: now + ACTIVATION_S,
        secured: 0,
        unsecured: 0,
        last_cycle: now,
        vuln_start: 0,
        vuln_end: 0,
        state: STATE_SHIELD_VULNERABLE,
    };
    put(reg.deltas_mut(), &sk);
    announce_added(reg, &sk);
    sk
}

/// Launch one skyhook from `actor`'s hold at `now`: the checks, the item
/// into space owned by the pilot's corporation, the row, the ball. The new
/// skyhook's id.
pub fn launch_one_at(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
    now: u64,
) -> Result<i64, Refused> {
    let (item, system, planet) = check_launch(reg, actor, item_id)?;
    let corp = reg.character(actor).map_or(0, |c| c.corporation_id);
    let to = Slot::new(corp, system, FLAG_IN_SPACE);
    let edit = if item.quantity > 1 {
        Edit::Split {
            item_id,
            quantity: 1,
            to,
        }
    } else {
        Edit::Move { item_id, to }
    };
    let changes = reg
        .apply_items(&[edit])
        .map_err(|e| Refused::Store(e.to_string()))?;
    let before = crate::sov_workforce::before(reg, system, false);
    let sk = place(reg, created(&changes, item_id), planet, system, corp, now);
    changed_since(reg, &sk, now, &before);
    crate::errln!(
        "[skyhook] {} launched over {planet} in {system} by {actor}",
        sk.id
    );
    Ok(sk.id)
}

/// [`launch_one_at`] on the wall clock.
pub fn launch_one(reg: &mut Registry, actor: i64, item_id: i64) -> Result<i64, Refused> {
    launch_one_at(reg, actor, item_id, now())
}

fn refusal(text: &str) -> PyValue {
    crate::mtu::refusal(text)
}

/// `ship.Drop([(itemID, qty)], whoseBehalfID, ignoreWarning)`'s link for a
/// skyhook (`service/inventory.rs`, one line before `sov_hub::drop_call`):
/// `Some` only when one of the items is of [`GROUP_SKYHOOK`]. The answer is
/// the hub's: `{itemID: [newID]}` or `{itemID: [(label, args)]}` **[V]**
/// (`eveMisc.LaunchFromShip`). Both launch entries, "Launch for
/// Corporation" and the deploy-component one, send this call
/// (`menusvc.py:341-353`, `:3329-3340`) **[V]**.
pub(crate) fn drop_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let list = Args::new(req).list(0)?;
    let items: Vec<i64> = list
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => match p.first() {
                Some(PyValue::Int(id)) => Some(*id),
                _ => None,
            },
            PyValue::Int(id) => Some(*id),
            _ => None,
        })
        .collect();
    if !items
        .iter()
        .any(|id| reg.item(*id).is_some_and(|i| i.group_id == GROUP_SKYHOOK))
    {
        return None;
    }
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    for item in items {
        let entry = match req.actor {
            None => refusal(&Refused::NotInSpace.text()),
            Some(a) => match launch_one(reg, a, item) {
                Ok(id) => PyValue::Int(id),
                Err(r) => refusal(&r.text()),
            },
        };
        answer.push((PyValue::List(vec![entry]), PyValue::Int(item)));
    }
    Some(Ok(PyValue::Dict(answer)))
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// Finish `id`'s build now: it activates, starts harvesting from `now`, and
/// the owner corp's online members are told.
pub fn finish_build(reg: &mut Registry, id: i64, now: u64) -> Option<Skyhook> {
    let mut sk = by_id(reg.deltas(), id)?;
    if sk.built() {
        return Some(sk);
    }
    let before = crate::sov_workforce::before(reg, sk.system, false);
    sk.activated_at = now.max(1);
    sk.built_until = sk.built_until.min(now);
    sk.active = true;
    sk.last_cycle = now;
    put(reg.deltas_mut(), &sk);
    let to = crate::service::structures::corp_audience(reg, sk.corp);
    if !to.is_empty() {
        reg.push(FirstActivation {
            to,
            skyhook: sk.id,
            planet: sk.planet,
        });
    }
    changed_since(reg, &sk, now, &before);
    Some(sk)
}

/// The clock stage `skyhook` (every 4 ticks, phase 1; `CADENCE_OK`): builds
/// whose time is up finish. Keyed on stored times, so a late run does what
/// an early one would.
/// Scheduled, not pushed: `pending` drains only after a client's call, as structure_space's clock pushes do (#44).
pub fn tick(reg: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    crate::sov_hub::on_the_clock(reg, |reg| tick_at(reg, now()));
}

/// [`tick`] at `now`.
pub fn tick_at(reg: &mut Registry, now: u64) {
    if reg.deltas().of(KIND).is_empty() {
        return;
    }
    let due: Vec<i64> = all(reg.deltas())
        .iter()
        .filter(|s| !s.built() && s.built_until <= now)
        .map(|s| s.id)
        .collect();
    for id in due {
        finish_build(reg, id, now);
    }
}

/// Why a switch was refused, as the gateway status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// 404: no such skyhook.
    Unknown,
    /// 403: not the owner corp's manager, or still building.
    Forbidden,
}

/// Turn a skyhook on or off for `who` (the gateway's `Activate` and
/// `Deactivate`): the owner corp's director, CEO or station manager. Off
/// settles the harvest and freezes the stocks; on restarts the cycle at
/// `now` **[I]**, since the client extrapolates only from the record.
pub fn set_active(reg: &mut Registry, who: i64, id: i64, on: bool, now: u64) -> Result<(), Switch> {
    let sk = by_id(reg.deltas(), id).ok_or(Switch::Unknown)?;
    if !owns(reg, who, &sk) || !may_manage(reg, who) || !sk.built() {
        return Err(Switch::Forbidden);
    }
    // A skyhook its owner no longer holds the system for cannot be switched
    // on; switching it off is always allowed.
    if on && !held(reg.deltas(), &sk) {
        return Err(Switch::Forbidden);
    }
    if sk.active == on {
        return Ok(());
    }
    let before = crate::sov_workforce::before(reg, sk.system, false);
    let mut sk = settle(reg, id, now).ok_or(Switch::Unknown)?;
    sk.active = on;
    if on {
        sk.last_cycle = now;
    }
    put(reg.deltas_mut(), &sk);
    changed_since(reg, &sk, now, &before);
    Ok(())
}

// ---------------------------------------------------------------------------
// The wire: one encoder per message
// ---------------------------------------------------------------------------

/// The messages a skyhook record rides in. Each has its **own** field
/// number for the reagent type (RE §2.6, the field-number trap): the wrong
/// one writes reagent type 0, which the client silently drops
/// (`data_type.py:64,72`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Get,
    AllLocal,
    ByCorp,
    AllInSystem,
    Simulations,
    Definitions,
}

impl Msg {
    /// `reagent_type` in a simulation entry (`gw-protos.txt`); 0 when the
    /// message has none.
    pub fn sim_type_field(self) -> u64 {
        match self {
            Msg::Get | Msg::AllLocal | Msg::AllInSystem => 5,
            Msg::ByCorp => 1,
            Msg::Simulations => 6,
            Msg::Definitions => 0,
        }
    }

    /// `reagent_type` in a definition entry; 0 when the message has none.
    pub fn def_type_field(self) -> u64 {
        match self {
            Msg::Get => 5,
            Msg::AllLocal | Msg::Definitions => 3,
            Msg::ByCorp => 1,
            Msg::AllInSystem => 6,
            Msg::Simulations => 0,
        }
    }
}

/// A skyhook's workforce oneof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workforce {
    /// A workforce planet: its amount.
    Amount(u64),
    /// A power planet: `none` **[A]** (falsifier: the window showing a
    /// workforce for a power skyhook).
    None,
    /// A reagent planet: the field is left unset.
    Absent,
}

/// What a record says about one skyhook at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub id: i64,
    pub planet: i64,
    pub active: bool,
    /// The reagent's definition and its stocks.
    pub reagent: Option<(Reagent, Sim)>,
    /// `(vulnerable, start, end)`, unix seconds; `None` for a skyhook that
    /// has no reagent.
    pub vulnerability: Option<(bool, u64, u64)>,
    pub workforce: Workforce,
    pub version: (u32, u32, u32),
}

/// The client's staleness grace on a vulnerability window, seconds
/// (`data_type.py:10`) **[V]**.
pub const VULN_GRACE_S: u64 = 30;

/// The theft vulnerability a reagent skyhook reports at `now`. A window L9
/// stored is reported as it is (vulnerable from its start plus
/// `VULN_GRACE_S` to its end); none stored
/// reports the synthetic far-ahead one, which never contains `now`.
pub fn vulnerability(sk: &Skyhook, now: u64) -> (bool, u64, u64) {
    // The client calls a vulnerable answer stale before `start + grace` and
    // a not-vulnerable one stale between `start + grace` and `end + grace`
    // (`data_type.py:150-157`, grace `:10`) **[V]**: report vulnerable from
    // `start + grace`, and a window that has ended as no window at all.
    if sk.vuln_end > sk.vuln_start && now <= sk.vuln_end {
        return (
            now >= sk.vuln_start + VULN_GRACE_S,
            sk.vuln_start,
            sk.vuln_end,
        );
    }
    let start = now + SYNTH_AHEAD_S;
    (false, start, start + SYNTH_WINDOW_S)
}

/// The view of `sk` at `now`.
pub fn view(reg: &Registry, sk: &Skyhook, now: u64) -> View {
    let y = sov_resources::planet_yield(reg, sk.planet);
    let reagent = y.as_ref().and_then(|y| y.reagent.clone());
    let workforce = match &y {
        Some(y) if y.reagent.is_none() && y.workforce > 0 => Workforce::Amount(y.workforce),
        Some(y) if y.reagent.is_none() => Workforce::None,
        _ => Workforce::Absent,
    };
    let on = running(reg.deltas(), sk);
    View {
        id: sk.id,
        planet: sk.planet,
        active: on,
        vulnerability: reagent.as_ref().map(|_| vulnerability(sk, now)),
        reagent: reagent.map(|r| {
            let mut s = current(&live(reg.deltas(), sk), Some(&r), now);
            if !on {
                // The client extrapolates a stock from `last_cycle` whether or
                // not the skyhook is active (`sovereigntyResourceSvc.py:158-167`,
                // `orbitalSkyhook.py:221`, `uiController.py:326`), but counts
                // no cycle when `last_cycle` is ahead of its clock
                // (`reagent_split_production_simulation.py:11`, `max(0, ..)`)
                // **[V]**. A skyhook that does nothing therefore reports a
                // `last_cycle` far ahead, so the stock the client shows stays
                // frozen. **[I]** Trade-off: the "next harvest" timer of an
                // inactive skyhook reads far ahead too.
                s.last_cycle = now.saturating_add(FROZEN_AHEAD_S);
            }
            (r, s)
        }),
        workforce,
        version: sov_resources::current_version(reg.content()),
    }
}

fn sim_bytes(s: &Sim) -> Vec<u8> {
    let mut m = Vec::new();
    put_uint(&mut m, 1, u64::from(s.secured));
    put_uint(&mut m, 2, u64::from(s.unsecured));
    put_bytes(&mut m, 3, &timestamp(s.last_cycle));
    m
}

/// `reagent.Definition` (`gw-protos.txt`): amount, period, secured
/// percentage, secured capacity, unsecured capacity.
fn def_bytes(r: &Reagent) -> Vec<u8> {
    let mut d = Vec::new();
    put_uint(&mut d, 1, u64::from(r.per_cycle));
    put_bytes(&mut d, 2, &duration(r.cycle_s));
    put_uint(&mut d, 3, u64::from(r.secured_pct));
    put_uint(&mut d, 4, u64::from(r.secured_cap));
    put_uint(&mut d, 5, u64::from(r.unsecured_cap));
    d
}

fn version_bytes(v: (u32, u32, u32)) -> Vec<u8> {
    let mut m = Vec::new();
    put_uint(&mut m, 1, u64::from(v.0));
    put_uint(&mut m, 2, u64::from(v.1));
    put_uint(&mut m, 3, u64::from(v.2));
    m
}

fn vuln_bytes(v: (bool, u64, u64)) -> Vec<u8> {
    let mut m = Vec::new();
    put_bool(&mut m, 1, v.0);
    put_bytes(&mut m, 2, &timestamp(v.1));
    put_bytes(&mut m, 3, &timestamp(v.2));
    m
}

fn workforce_bytes(w: Workforce) -> Option<Vec<u8>> {
    let mut m = Vec::new();
    match w {
        Workforce::Amount(a) => put_uint(&mut m, 1, a),
        Workforce::None => put_bool(&mut m, 2, true),
        Workforce::Absent => return None,
    }
    Some(m)
}

/// One message of the skyhook API for `v`: the record inside `GetResponse`
/// and the list entries (`Get`, `AllLocal`, `ByCorp`, `AllInSystem`), or the
/// whole notice (`Simulations`, `Definitions`). One encoder per message, each
/// with its own field numbers (`gw-protos.txt`).
pub fn record(msg: Msg, v: &View) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &id(v.id as u64));
    match msg {
        Msg::Simulations => {
            put_bytes(&mut out, 2, &id(v.planet as u64));
            if let Some((r, s)) = &v.reagent {
                let mut e = Vec::new();
                put_bytes(&mut e, msg.sim_type_field(), &id(r.type_id as u64));
                put_bytes(&mut e, 4, &sim_bytes(s));
                put_bytes(&mut out, 3, &e);
            }
        }
        Msg::Definitions => {
            put_bytes(&mut out, 2, &id(v.planet as u64));
            if let Some((r, _)) = &v.reagent {
                let mut e = Vec::new();
                put_bytes(&mut e, msg.def_type_field(), &id(r.type_id as u64));
                put_bytes(&mut e, 2, &def_bytes(r));
                put_bytes(&mut out, 3, &e);
            }
            put_bytes(&mut out, 4, &version_bytes(v.version));
        }
        _ => {
            if let Some((r, s)) = &v.reagent {
                let mut e = Vec::new();
                put_bytes(&mut e, msg.sim_type_field(), &id(r.type_id as u64));
                put_bytes(&mut e, 4, &sim_bytes(s));
                put_bytes(&mut out, 2, &e);
            }
            put_bool(&mut out, 3, v.active);
            if let Some((r, _)) = &v.reagent {
                let mut e = Vec::new();
                put_bytes(&mut e, msg.def_type_field(), &id(r.type_id as u64));
                put_bytes(&mut e, 2, &def_bytes(r));
                put_bytes(&mut out, 4, &e);
            }
            match v.vulnerability {
                Some(w) => put_bytes(&mut out, 5, &vuln_bytes(w)),
                None => put_bool(&mut out, 6, true),
            }
            let (wf, ver) = if msg == Msg::AllInSystem {
                (9, 10)
            } else {
                (7, 8)
            };
            if let Some(w) = workforce_bytes(v.workforce) {
                put_bytes(&mut out, wf, &w);
            }
            put_bytes(&mut out, ver, &version_bytes(v.version));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The gateway arm
// ---------------------------------------------------------------------------

fn switch_status(e: Switch) -> u32 {
    match e {
        Switch::Unknown => 404,
        Switch::Forbidden => 403,
    }
}

fn read_reply(reg: &Registry, rest: &str, ty: &str, body: &[u8], who: i64, now: u64) -> Reply {
    match rest {
        "GetRequest" => {
            // An unknown id answers 403 too: the messenger branches on no
            // 404 here, and only this 403 is exempt from backoff **[A]**
            // (falsifier: a window that shows "unknown" on 404).
            let sk = read_id(body, 1)
                .and_then(|i| by_id(reg.deltas(), i as i64))
                .filter(|s| may_read(reg, who, s));
            match sk {
                Some(sk) => typed(ty, record(Msg::Get, &view(reg, &sk, now))),
                None => status(403),
            }
        }
        "GetAllLocalRequest" => {
            let system = reg.session_system(who).unwrap_or(0);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &id(system.max(0) as u64));
            for sk in in_system(reg.deltas(), system) {
                put_bytes(&mut out, 2, &record(Msg::AllLocal, &view(reg, &sk, now)));
            }
            typed(ty, out)
        }
        _ => {
            // GetAllByCorporationRequest: another corporation's is 403.
            let corp = read_id(body, 1).map(|c| c as i64);
            let mine = reg.character(who).map(|c| c.corporation_id);
            if corp.is_none() || corp != mine {
                return status(403);
            }
            let mut out = Vec::new();
            for sk in all(reg.deltas()).iter().filter(|s| Some(s.corp) == corp) {
                put_bytes(&mut out, 1, &record(Msg::ByCorp, &view(reg, sk, now)));
            }
            typed(ty, out)
        }
    }
}

/// The gateway arm (`gateway_ext::CHAIN`, above `sov_gw_stub`, which keeps
/// its refusals for the types this does not own). Answers from stored state.
/// Every "none" answer is a 200: the skyhook messenger backs a failing
/// request off for up to 300 s (RE §1). On a [`Ground::Local`] there is no
/// world: reads of a skyhook answer 403 (none known), `GetAllLocal` falls
/// through to the stub, writes answer 404.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(API_PKG)?;
    match rest {
        "GetRequest" | "GetAllLocalRequest" | "GetAllByCorporationRequest" => {
            let Ground::Server(w) = g else {
                return match rest {
                    "GetAllLocalRequest" => None,
                    "GetAllByCorporationRequest" => Some(typed(ty, Vec::new())),
                    _ => Some(status(403)),
                };
            };
            let reg = crate::service::world_read_timed(w, "skyhook_gateway");
            Some(read_reply(&reg, rest, ty, body, character, now()))
        }
        "ActivateRequest" | "DeactivateRequest" => {
            let Ground::Server(w) = g else {
                return Some(status(404));
            };
            let Some(sk) = read_id(body, 1) else {
                return Some(status(404));
            };
            let mut reg = crate::service::world_write_timed(w, "skyhook_gateway_write");
            let on = rest == "ActivateRequest";
            Some(
                match set_active(&mut reg, character, sk as i64, on, now()) {
                    Ok(()) => typed(ty, Vec::new()),
                    Err(e) => status(switch_status(e)),
                },
            )
        }
        // The map filter answers from the stored windows (SKY-2).
        "GetSolarSystemsWithTheftVulnerableSkyhooksRequest" => {
            let Ground::Server(w) = g else {
                return Some(typed(ty, Vec::new()));
            };
            let reg = crate::service::world_read_timed(w, "skyhook_gateway");
            Some(typed(
                ty,
                crate::skyhook_theft::systems_response(reg.deltas(), now()),
            ))
        }
        "GetTheftVulnerableSkyhooksInSolarSystemRequest" => {
            let Ground::Server(w) = g else {
                return Some(typed(ty, Vec::new()));
            };
            let reg = crate::service::world_read_timed(w, "skyhook_gateway");
            let system = read_id(body, 1).map_or(0, |s| s as i64);
            Some(typed(
                ty,
                crate::skyhook_theft::in_system_response(reg.deltas(), system, now()),
            ))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Component calls (`beyonce.CallComponentFromClient`)
// ---------------------------------------------------------------------------

/// The `componentConst.ORBITAL_SKYHOOK` name **[V]** (`componentConst.py`).
pub const ORBITAL_SKYHOOK: &str = "orbitalSkyhook";

/// `space_component::Locate` for the skyhook: the system of a skyhook that
/// is a live item.
pub fn locate(reg: &Registry, item: i64) -> Option<i64> {
    let sk = by_id(reg.deltas(), item)?;
    reg.store().get(item).map(|_| sk.system)
}

fn failed(text: &str) -> ServiceError {
    // The client wraps any error into `SkyhookExtractionFailed`
    // (`uiController.py:393-394`) **[V]**.
    crate::mtu::notify(text)
}

/// `IsCharacterAuthorised()`: the pilot is in the skyhook's owner corp
/// **[A]** (falsifier: an alliance-mate allowed on TQ; then the holder
/// alliance).
pub fn is_character_authorised(
    reg: &mut Registry,
    c: &CompCall<'_>,
) -> Result<PyValue, ServiceError> {
    let sk = by_id(reg.deltas(), c.item_id).ok_or_else(|| failed("No such skyhook."))?;
    Ok(PyValue::Bool(
        owns(reg, c.actor, &sk) && held(reg.deltas(), &sk),
    ))
}

/// `ExtractReagentsToShip(flagID, qty)` -> the quantity moved. Authorised,
/// within [`TRANSFER_RANGE`] of the skyhook's surface; settles the harvest;
/// takes the **unsecured** stock first **[A]** (so the owner empties the
/// raidable surplus; falsifier: TQ taking secured first); makes the items
/// in the flag's hold of the pilot's ship. The client already clipped `qty`
/// to the free volume **[V]** (`uiController.py:372-382`).
pub fn extract(reg: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
    extract_at(reg, c, now())
}

/// [`extract`] at `now`.
pub fn extract_at(reg: &mut Registry, c: &CompCall<'_>, now: u64) -> Result<PyValue, ServiceError> {
    let flag = c.args.int(0).ok_or_else(|| failed("No hold."))?;
    let qty = c.args.int(1).unwrap_or(0);
    if !matches!(
        flag,
        crate::sov_hub::FLAG_CARGO | crate::sov_hub::FLAG_INFRASTRUCTURE_HANGAR
    ) || qty <= 0
    {
        return Err(failed("That cannot be taken there."));
    }
    let sk = by_id(reg.deltas(), c.item_id).ok_or_else(|| failed("No such skyhook."))?;
    if !owns(reg, c.actor, &sk) {
        return Err(failed("You are not authorised to take from this skyhook."));
    }
    if !held(reg.deltas(), &sk) {
        return Err(failed(
            "Your alliance no longer holds this system; the skyhook cannot be emptied.",
        ));
    }
    let ship = reg
        .character(c.actor)
        .map(|ch| ch.ship_id)
        .ok_or_else(|| failed("You are not in space."))?;
    reg.advance_character(c.actor);
    let here = reg
        .roster()
        .get(c.actor)
        .copied()
        .ok_or_else(|| failed("You are not in space."))?;
    let at = position(reg, &sk).ok_or_else(|| failed("The skyhook has no place."))?;
    let r = radius(reg.content(), type_of(reg, sk.id).unwrap_or(0));
    if crate::space_state::distance(here.motion.position, at) - r > TRANSFER_RANGE {
        return Err(failed("You are too far from the skyhook."));
    }
    let reagent =
        reagent_of(reg, sk.planet).ok_or_else(|| failed("This skyhook holds nothing."))?;
    let sk = settle(reg, sk.id, now).ok_or_else(|| failed("No such skyhook."))?;
    let stock = i64::from(sk.secured) + i64::from(sk.unsecured);
    // Only what the hold has room for: free volume over the reagent's unit
    // volume, by the client's own arithmetic (`units_that_fit`).
    let ship_type = reg.store().get(ship).map_or(0, |s| s.type_id);
    let capacity = if flag == crate::sov_hub::FLAG_CARGO {
        crate::fitting::cargo_capacity_in(reg, ship)
    } else {
        crate::item::hold_capacity_in(reg.content(), ship_type, flag)
    };
    let free = (capacity - reg.store().used_volume_in(reg.content(), ship, flag)).max(0.0);
    let unit = crate::item::find_in(reg.content(), reagent.type_id).map_or(0.0, |t| t.volume);
    let room = crate::fitting::units_that_fit(free, unit);
    let moved = qty.min(stock).min(room);
    if moved == 0 {
        return Err(failed(if room == 0 {
            "There is no room in that hold."
        } else {
            "The skyhook holds nothing to take."
        }));
    }
    let from_unsecured = moved.min(i64::from(sk.unsecured));
    let from_secured = moved - from_unsecured;
    reg.apply_items(&[Edit::Create {
        type_id: reagent.type_id,
        quantity: moved,
        to: Slot::new(c.actor, ship, flag),
    }])
    .map_err(|e| failed(&format!("The reagents could not be moved: {e}")))?;
    let mut sk = sk;
    sk.unsecured -= from_unsecured as u32;
    sk.secured -= from_secured as u32;
    put(reg.deltas_mut(), &sk);
    announce_stocks(reg, &sk, now);
    Ok(PyValue::Int(moved))
}

// ---------------------------------------------------------------------------
// The cleanup after a claim (macho)
// ---------------------------------------------------------------------------

/// Whether `now` is inside the cleanup window of `system`'s claim.
fn in_window(d: &Deltas, system: i64, now: u64) -> bool {
    sov::claim(d, system)
        .and_then(|c| c.claim_time)
        .is_some_and(|t| now <= unix_of(t).saturating_add(CLAIM_WINDOW_S))
}

/// The skyhooks the caller may clean up among `ids`: the caller's alliance
/// holds the system, the claim is inside the window, the pilot may manage
/// for their corp, and the skyhook's owner is not in the holder alliance.
fn cleanable(reg: &Registry, actor: i64, ids: &[i64], now: u64) -> Vec<Skyhook> {
    let Some((corp, Some(alliance))) = pilot(reg, actor) else {
        return Vec::new();
    };
    if !may_manage(reg, actor) {
        return Vec::new();
    }
    let d = reg.deltas();
    // Each id once: a repeated id must not be charged twice.
    let mut once: Vec<i64> = ids.to_vec();
    once.sort_unstable();
    once.dedup();
    once.iter()
        .filter_map(|i| by_id(d, *i))
        .filter(|s| {
            sov::holder(d, s.system) == Some(alliance)
                // The client offers the cleanup only to the corporation that
                // owns the hub (`sovSvc.CanCleanUpSystem`, `hubInfo.ownerID ==
                // session.corpid`) **[V]**.
                && sov::claim(d, s.system).is_some_and(|c| c.corporation_id == corp)
                && in_window(d, s.system, now)
                && corp_alliance(d, s.corp) != Some(alliance)
        })
        .collect()
}

/// `sovMgr.AcquireSkyhooks(skyhookIDs, accessListID)` -> the ids acquired.
/// The fee ([`SKYHOOK_CLAIM_FEE`], ref 184) is charged to the caller's
/// corporation master wallet for the **whole** batch up front: a wallet too
/// short refuses with `NotEnoughMoneyCorp` and nothing moves. The access
/// list is not kept **[A]** (falsifier: a window that reads it back).
pub fn acquire(
    reg: &mut Registry,
    actor: i64,
    ids: &[i64],
    now: u64,
) -> Result<Vec<i64>, ServiceError> {
    let list = cleanable(reg, actor, ids, now);
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let corp = reg.character(actor).map_or(0, |c| c.corporation_id);
    let total = SKYHOOK_CLAIM_FEE * list.len() as f64;
    crate::service::corp_wallet::check_fee(reg, corp, crate::corp_wallet::MASTER_KEY, total)?;
    let mut done = Vec::new();
    for sk in list {
        // Hand the item over first: a skyhook that cannot move is skipped,
        // costs nothing and keeps its row.
        let old_owner = reg.store().get(sk.id).map_or(sk.corp, |i| i.owner_id);
        if let Err(e) = reg.apply_items_quiet(&[Edit::Move {
            item_id: sk.id,
            to: Slot::new(corp, sk.system, FLAG_IN_SPACE),
        }]) {
            crate::errln!("[skyhook] {} not acquired: {e}", sk.id);
            continue;
        }
        if crate::service::corp_wallet::charge_fee(
            reg,
            corp,
            crate::corp_wallet::MASTER_KEY,
            SKYHOOK_CLAIM_FEE,
            REF_SKYHOOK_CLAIM_FEE,
            actor,
            sk.id,
        )
        .is_none()
        {
            // The wallet was checked for the batch; if it still fails, undo
            // the move and stop.
            let _ = reg.apply_items_quiet(&[Edit::Move {
                item_id: sk.id,
                to: Slot::new(old_owner, sk.system, FLAG_IN_SPACE),
            }]);
            break;
        }
        // Settled under the old owner (a skyhook it no longer holds does not
        // accrue), then handed over; the new owner's cycle starts now so the
        // time it stood idle is not harvested.
        let before = crate::sov_workforce::before(reg, sk.system, false);
        let mut s = settle(reg, sk.id, now).unwrap_or(sk);
        s.corp = corp;
        s.last_cycle = now;
        put(reg.deltas_mut(), &s);
        crate::skyhook_theft::on_acquired(reg, &s);
        changed_since(reg, &s, now, &before);
        done.push(s.id);
    }
    Ok(done)
}

/// Take a skyhook out of the world: the item, the row, the ball.
pub fn remove(reg: &mut Registry, sk: &Skyhook, now: u64) {
    if reg.store().get(sk.id).is_some()
        && let Err(e) = reg.apply_items_quiet(&[Edit::Destroy { item_id: sk.id }])
    {
        crate::errln!("[skyhook] {} not destroyed: {e}", sk.id);
    }
    let before = crate::sov_workforce::before(reg, sk.system, false);
    reg.deltas_mut().del(KIND, &[sk.id.to_string()]);
    announce_removed(reg, sk.system, sk.id);
    if let Some(h) = crate::sov_hub::hub(reg.deltas(), sk.system) {
        // The planet's yield leaves the budget: re-simulate before the hub is told (spec L8.6).
        crate::sov_upgrade::resimulate(reg, h.hub_id);
        sov_resources::announce(reg, h.hub_id);
    }
    crate::sov_workforce::budgets_changed_since(reg, sk.system, &before);
    announce_all(reg, sk.system, now);
}

/// `sovMgr.DestroySkyhooks(skyhookIDs)` -> the ids destroyed: the same
/// window and holder rules as [`acquire`], no fee.
pub fn destroy(reg: &mut Registry, actor: i64, ids: &[i64], now: u64) -> Vec<i64> {
    let list = cleanable(reg, actor, ids, now);
    let mut done = Vec::new();
    for sk in list {
        remove(reg, &sk, now);
        done.push(sk.id);
    }
    done
}

/// The state a corp's list shows (`skyhookBrowserController`) **[I]**.
fn list_row(reg: &Registry, sk: &Skyhook) -> PyValue {
    crate::pyutil::keyval(vec![
        ("skyhookID", PyValue::Int(sk.id)),
        ("planetID", PyValue::Int(sk.planet)),
        ("solarSystemID", PyValue::Int(sk.system)),
        ("typeID", PyValue::Int(type_of(reg, sk.id).unwrap_or(0))),
        ("state", PyValue::Int(i64::from(slim_state(sk)))),
    ])
}

/// The macho arms (`service/sov.rs` hands them over): `colonyResourcesMgr
/// .GetMyCorporationSkyhooks`, `sovMgr.AcquireSkyhooks` and
/// `sovMgr.DestroySkyhooks`.
pub(crate) fn macho(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    match (req.service, req.method) {
        ("colonyResourcesMgr", "GetMyCorporationSkyhooks") => {
            let corp = req
                .actor
                .and_then(|id| reg.character(id))
                .map(|c| c.corporation_id);
            let rows = all(reg.deltas())
                .iter()
                .filter(|s| Some(s.corp) == corp)
                .map(|s| list_row(reg, s))
                .collect();
            Ok(PyValue::List(rows))
        }
        ("sovMgr", "AcquireSkyhooks") => {
            let (Some(actor), Some(ids)) = (req.actor, a.ids(0)) else {
                return Err(a.expected("(skyhookIDs, accessListID)"));
            };
            let done = acquire(reg, actor, &ids, now())?;
            Ok(PyValue::List(done.into_iter().map(PyValue::Int).collect()))
        }
        ("sovMgr", "DestroySkyhooks") => {
            let (Some(actor), Some(ids)) = (req.actor, a.ids(0)) else {
                return Err(a.expected("(skyhookIDs)"));
            };
            let done = destroy(reg, actor, &ids, now());
            Ok(PyValue::List(done.into_iter().map(PyValue::Int).collect()))
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}
