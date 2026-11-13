//! Skyhook theft windows and raids (SKY-2, issue #82): the scheduled
//! vulnerability windows, the reagent silo ball that stands while one is
//! open, and the `linkWithShip` component that moves the unsecured stock.
//! Wire contract: `docs/re/sov-gateway.md` §2.6 (the window, the map filter),
//! §2.8 (the three notices); client rule: `spacecomponents/client/components/
//! linkWithShip.py`, `skyhookReagentSilo.py`.
//!
//! **The rows.** One row of kind [`KIND`] per reagent skyhook:
//!
//! ```text
//! skyraid <skyhookID> -> start end link_state linked_ship link_started
//!     link_complete_at thief n silo secure noticed
//! ```
//!
//! The first six values are the spec's. `thief` is the pilot who started the
//! link, `n` the window's index, `silo` the silo's item id (0 while no window
//! is open), `secure` is 1 for an authorised pilot's link, `noticed` the bits
//! of the notices already queued for this window (1 scheduled, 2 started).
//! All times are unix seconds on the wall clock, like the skyhook rows.
//!
//! **Where the client reads the window.** The skyhook row already carries
//! `vuln_start`/`vuln_end`, which `skyhook::vulnerability` reports. This
//! module writes the current or next window into them, so the record encoder
//! needs no edit: it is vulnerable from `start` plus the client's 30 s grace
//! to `end` (the silo, notices and link gate use the same span, `Raid::open_at`),
//! and outside it the stored window is the next (or the last) one, never one
//! that is vulnerable at now (the client's staleness rule, `data_type.py:132-158` **[V]**).
//!
//! **The silo** is a real item of the silo group, owned by the skyhook's
//! corp, created when a window opens and destroyed when it closes. Its slim
//! carries `parentID = skyhookID` (`linkWithShip.py` `_effectTargetID =
//! slimItem.parentID`) **[V]**, `component_skyhookReagentSilo_skyhookID`
//! (`skyhookReagentSilo.py` `OnSlimItemUpdated`) **[V]** and
//! `component_linkWithShip = (addedAt, linkState, completeAt, linkedShipID)`
//! (`linkWithShip.py` `OnSlimItemUpdated`) **[V]**. The skyhook's own slim
//! carries `reagentSiloItemID` (`uiController.py` `reagentSiloItemID`) **[V]**.

use crate::delta::Deltas;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::gateway::{Target, put_bytes};
use crate::item::{Edit, Slot};
use crate::service::{Registry, ServiceError};
use crate::skyhook::{self, Skyhook};
use crate::sov_proto::{id, timestamp};
use crate::space_component::CompCall;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "skyraid";

/// The component name the client's call carries **[V]**
/// (`componentConst.py:64`).
pub const LINK_WITH_SHIP: &str = "linkWithShip";

/// `linkWithShip.py:6-9` **[V]**.
pub const LINKSTATE_IDLE: u8 = 1;
pub const LINKSTATE_RUNNING: u8 = 2;
pub const LINKSTATE_COMPLETED: u8 = 3;
pub const LINKSTATE_DISABLED: u8 = 4;

/// `inventorycommon/const.py:909` `groupSkyhookReagentSilo` **[V]**.
pub const GROUP_SILO: i64 = 4745;

/// A window lasts two hours **[A]**. Falsifier: the owner's ruling (Q3), or
/// the client's own schedule display contradicting it.
pub const RAID_WINDOW_S: u64 = 2 * 3_600;
/// The next window starts this many whole days after the last one ended,
/// plus a day by the hash: 3 or 4 **[A]** (same falsifier).
pub const MIN_GAP_DAYS: u64 = 3;
/// The prime hour used when the holder has none set **[A]** (falsifier: the
/// client showing a schedule for an alliance with no prime hour).
pub const FALLBACK_PRIME_HOUR: i64 = 19;
/// A link reaches this far from the silo's surface **[A]** (falsifier: the
/// `maxLinkRange` of the silo type's component; the client enforces its own).
pub const MAX_LINK_RANGE: f64 = 30_000.0;
/// Every link takes this long, thief's and owner's alike **[A]**: the client
/// draws one `linkDuration` from the silo type for both
/// (`linkWithShipController`/`linkWithShip.py:123`) **[V]**. Falsifier: the
/// silo type's component data, which we cannot read, naming another value.
pub const LINK_DURATION_S: u64 = 600;
/// A completed link goes back to idle after this long
/// (`DELAY_BEFORE_MODEL_REMOVAL`, `linkWithShip.py`) **[V]** name.
pub const DELAY_BEFORE_IDLE_S: u64 = 10;
/// The silo stands this far off the skyhook's centre, in skyhook radii **[A]**.
const SILO_OFFSET_RADII: f64 = 2.5;

const NOTICE_SCHEDULED: u8 = 1;
const NOTICE_STARTED: u8 = 2;

// ---------------------------------------------------------------------------
// The schedule
// ---------------------------------------------------------------------------

/// splitmix64 over `(skyhook, n)`: the one source of the spread.
fn mix(skyhook: i64, n: u32) -> u64 {
    let mut z = (skyhook as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(u64::from(n).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The window number `n` of `skyhook` that follows the moment `after`
/// (`(start, end)`, unix seconds). A pure function of its arguments, so a
/// test can predict it: 3 or 4 whole days after `after`'s day, centred on the
/// prime hour (UTC **[A]**) with a spread of up to half an hour either way.
pub fn window_after(skyhook: i64, n: u32, after: u64, prime_hour: i64) -> (u64, u64) {
    let h = mix(skyhook, n);
    let days = MIN_GAP_DAYS + (h & 1);
    let spread = ((h >> 8) % 3_600) as i64 - 1_800;
    let hour = prime_hour.clamp(0, 23) as u64;
    let centre = ((after / 86_400 + days) * 86_400 + hour * 3_600) as i64 + spread;
    let start = (centre - (RAID_WINDOW_S / 2) as i64).max(0) as u64;
    (start, start + RAID_WINDOW_S)
}

// ---------------------------------------------------------------------------
// The row
// ---------------------------------------------------------------------------

/// One skyhook's raid state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raid {
    pub id: i64,
    pub start: u64,
    pub end: u64,
    pub state: u8,
    pub thief: i64,
    pub ship: i64,
    pub link_started: u64,
    pub complete_at: u64,
    pub n: u32,
    pub silo: i64,
    pub secure: bool,
    pub noticed: u8,
}

fn parse_row(key: &[String], v: &[String]) -> Option<Raid> {
    let n = |i: usize| v.get(i)?.parse::<u64>().ok();
    let i = |i: usize| v.get(i)?.parse::<i64>().ok();
    Some(Raid {
        id: key.first()?.parse().ok()?,
        start: n(0)?,
        end: n(1)?,
        state: u8::try_from(n(2)?).ok()?,
        ship: i(3)?,
        link_started: n(4)?,
        complete_at: n(5)?,
        thief: i(6)?,
        n: u32::try_from(n(7)?).ok()?,
        silo: i(8)?,
        secure: n(9)? != 0,
        noticed: u8::try_from(n(10)?).ok()?,
    })
}

/// Every raid row, in id order.
pub fn all(d: &Deltas) -> Vec<Raid> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let row = parse_row(k, v);
            if row.is_none() {
                crate::errln!("[skyhook_theft] skipping row {k:?}: {v:?}");
            }
            row
        })
        .collect()
}

/// The raid row of skyhook `id`.
pub fn by_id(d: &Deltas, id: i64) -> Option<Raid> {
    let key = vec![id.to_string()];
    parse_row(&key, d.of(KIND).get(&key)?)
}

/// The raid row whose silo is item `silo`.
pub fn by_silo(d: &Deltas, silo: i64) -> Option<Raid> {
    if silo == 0 {
        return None;
    }
    all(d).into_iter().find(|r| r.silo == silo)
}

/// The silo of skyhook `id`, if a window is open (for the skyhook's slim).
pub fn silo_of(d: &Deltas, id: i64) -> Option<i64> {
    by_id(d, id).map(|r| r.silo).filter(|s| *s != 0)
}

fn put(d: &mut Deltas, r: &Raid) {
    d.put(
        KIND,
        vec![r.id.to_string()],
        vec![
            r.start.to_string(),
            r.end.to_string(),
            r.state.to_string(),
            r.ship.to_string(),
            r.link_started.to_string(),
            r.complete_at.to_string(),
            r.thief.to_string(),
            r.n.to_string(),
            r.silo.to_string(),
            u8::from(r.secure).to_string(),
            r.noticed.to_string(),
        ],
    );
}

impl Raid {
    fn idle(&mut self) {
        self.state = LINKSTATE_IDLE;
        self.thief = 0;
        self.ship = 0;
        self.link_started = 0;
        self.complete_at = 0;
        self.secure = false;
    }

    /// Whether the window is open at `now`: from `start` plus the client's
    /// grace (`skyhook::VULN_GRACE_S`) to `end`, its last second included.
    /// This is the same test `skyhook::vulnerability` makes for its
    /// vulnerable flag, so the record, the silo, the Started notice and the
    /// link gate flip together and the flag never reads true, false, true.
    pub fn open_at(&self, now: u64) -> bool {
        self.start.saturating_add(skyhook::VULN_GRACE_S) <= now && now <= self.end
    }
}

// ---------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------

const NOTICE_PKG: &str = "eve_public.sovereignty.skyhook.api.";

/// To the system's pilots and, apart from them, the owner corp's online
/// members (RE §2.8).
fn notify(reg: &mut Registry, sk: &Skyhook, name: &str, payload: Vec<u8>) {
    let ty = format!("{NOTICE_PKG}{name}");
    let here = crate::service::structures::system_audience(reg, sk.system);
    let corp: Vec<i64> = crate::service::structures::corp_audience(reg, sk.corp)
        .into_iter()
        .filter(|c| !here.contains(c))
        .collect();
    crate::gateway_out::queue(
        reg,
        &ty,
        payload.clone(),
        Target::SolarSystem {
            id: sk.system,
            members: here,
        },
    );
    if !corp.is_empty() {
        crate::gateway_out::queue(
            reg,
            &ty,
            payload,
            Target::Corporation {
                id: sk.corp,
                members: corp,
            },
        );
    }
}

fn notice_scheduled(reg: &mut Registry, sk: &Skyhook, start: u64, end: u64) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(sk.id as u64));
    put_bytes(&mut p, 2, &timestamp(start));
    put_bytes(&mut p, 3, &timestamp(end));
    notify(reg, sk, "TheftVulnerabilityWindowScheduledNotice", p);
}

fn notice_started(reg: &mut Registry, sk: &Skyhook, end: u64) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(sk.id as u64));
    put_bytes(&mut p, 2, &timestamp(end));
    notify(reg, sk, "TheftVulnerabilityWindowStartedNotice", p);
}

fn notice_ended(reg: &mut Registry, sk: &Skyhook) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(sk.id as u64));
    notify(reg, sk, "TheftVulnerabilityWindowEndedNotice", p);
}

// ---------------------------------------------------------------------------
// The silo (item, ball, slim)
// ---------------------------------------------------------------------------

fn silo_type(reg: &Registry) -> Option<i64> {
    reg.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .types()
        .find(|t| t.group_id == GROUP_SILO)
        .map(|t| t.type_id)
}

fn type_of(reg: &Registry, item: i64) -> Option<i64> {
    reg.store().get(item).map(|i| i.type_id)
}

fn silo_radius(reg: &Registry, silo: i64) -> f64 {
    skyhook::radius(reg.content(), type_of(reg, silo).unwrap_or(0))
}

/// Where the silo stands: off the skyhook's centre along x.
fn silo_position(reg: &Registry, sk: &Skyhook) -> Option<[f64; 3]> {
    let at = skyhook::position(reg, sk)?;
    let r = skyhook::radius(reg.content(), type_of(reg, sk.id)?);
    Some([at[0] + r * SILO_OFFSET_RADII, at[1], at[2]])
}

/// The silo's ball: static, global in its system like the skyhook **[A]**.
pub fn ball(reg: &Registry, r: &Raid, sk: &Skyhook) -> Option<Ball> {
    let ty = type_of(reg, r.silo)?;
    Some(Ball {
        ball_id: r.silo,
        mode: Mode::Stop,
        radius: skyhook::radius(reg.content(), ty) as f32,
        position: silo_position(reg, sk)?,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(sk.corp).unwrap_or(-1),
        alliance_id: skyhook::corp_alliance(reg.deltas(), sk.corp)
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        physics: None,
        formation_id: NO_FORMATION,
    })
}

/// The silo's five basic slim attributes.
pub fn slim(reg: &Registry, r: &Raid, sk: &Skyhook) -> Option<Slim> {
    Some(Slim {
        item_id: r.silo,
        type_id: type_of(reg, r.silo)?,
        owner_id: sk.corp,
        char_id: None,
        corp_id: Some(sk.corp),
        response_thresholds: None,
    })
}

/// `component_linkWithShip` (`linkWithShip.py` `OnSlimItemUpdated`) **[V]**.
pub fn component_link(r: &Raid) -> PyValue {
    let at = |s: u64| {
        if s == 0 {
            PyValue::None
        } else {
            PyValue::Int(skyhook::filetime(s))
        }
    };
    PyValue::Tuple(vec![
        PyValue::Int(skyhook::filetime(r.start)),
        PyValue::Int(i64::from(r.state)),
        at(r.complete_at),
        if r.ship == 0 {
            PyValue::None
        } else {
            PyValue::Int(r.ship)
        },
    ])
}

/// The silo's extra slim keys.
pub fn extras_of(r: &Raid) -> Vec<(&'static str, PyValue)> {
    vec![
        ("parentID", PyValue::Int(r.id)),
        ("component_skyhookReagentSilo_skyhookID", PyValue::Int(r.id)),
        ("component_linkWithShip", component_link(r)),
    ]
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|r| r.silo != 0 && ids.contains(&r.silo))
        .map(|r| (r.silo, extras_of(r)))
        .collect()
}

/// `system`'s open silos' balls and slims, for `destiny::scene`
/// (coordinator: one loop there).
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|r| r.silo != 0)
        .filter_map(|r| {
            let sk = skyhook::by_id(w.deltas(), r.id).filter(|s| s.system == system)?;
            Some((ball(w, r, &sk)?, slim(w, r, &sk)?))
        })
        .collect()
}

/// Where the silo `ball_id` is, for `Registry::ball_target` (approach,
/// orbit, warp to).
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let r = by_silo(w.deltas(), ball_id)?;
    let sk = skyhook::by_id(w.deltas(), r.id)?;
    Some(crate::model::Target {
        position: silo_position(w, &sk)?,
        radius: silo_radius(w, ball_id),
        warp_in: None,
    })
}

/// `space_component::Locate` for the silo: the system of a silo that is a
/// live item.
pub fn locate(reg: &Registry, item: i64) -> Option<i64> {
    let r = by_silo(reg.deltas(), item)?;
    reg.store().get(item)?;
    skyhook::by_id(reg.deltas(), r.id).map(|s| s.system)
}

fn in_space(reg: &Registry, system: i64) -> Vec<i64> {
    reg.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| reg.character(*id).is_some())
        .collect()
}

fn announce_added(reg: &mut Registry, r: &Raid, sk: &Skyhook) {
    let to = in_space(reg, sk.system);
    let (Some(b), Some(s)) = (ball(reg, r, sk), slim(reg, r, sk)) else {
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
    .map(|a| a.with_extras(slim_extras(reg, &[r.silo])));
    if let Some(a) = added {
        reg.push(crate::sov_hub::HubAdded(a));
    }
}

fn announce_removed(reg: &mut Registry, system: i64, silo: i64) {
    let to = in_space(reg, system);
    let stamp = reg.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, vec![silo]) {
        reg.push(gone);
    }
}

/// `OnSlimItemChange` of the silo with its link as it now reads.
fn announce_slim(reg: &mut Registry, r: &Raid, sk: &Skyhook) {
    if r.silo == 0 {
        return;
    }
    let to = in_space(reg, sk.system);
    let Some(s) = slim(reg, r, sk) else { return };
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    reg.push(crate::sov_hub::HubSlimChange {
        to,
        stamp,
        slim: s,
        extras: extras_of(r),
    });
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

fn reagent_of(reg: &Registry, planet: i64) -> Option<crate::sov_resources::Reagent> {
    crate::sov_resources::planet_yield(reg, planet)?.reagent
}

/// The holder's prime hour, else the fallback **[A]**.
fn prime_hour(d: &Deltas, sk: &Skyhook, now: u64) -> i64 {
    crate::sov::holder(d, sk.system)
        .or_else(|| skyhook::corp_alliance(d, sk.corp))
        .and_then(|a| crate::sov::prime_hour(d, a, skyhook::filetime(now)).current)
        .unwrap_or(FALLBACK_PRIME_HOUR)
}

/// Write `[start, end)` into the skyhook's own row, where the record encoder
/// reads the window.
fn set_window(reg: &mut Registry, id: i64, start: u64, end: u64) {
    if let Some(mut sk) = skyhook::by_id(reg.deltas(), id)
        && (sk.vuln_start, sk.vuln_end) != (start, end)
    {
        sk.vuln_start = start;
        sk.vuln_end = end;
        skyhook::put(reg.deltas_mut(), &sk);
    }
}

fn blank(id: i64, n: u32, start: u64, end: u64) -> Raid {
    Raid {
        id,
        start,
        end,
        state: LINKSTATE_IDLE,
        thief: 0,
        ship: 0,
        link_started: 0,
        complete_at: 0,
        n,
        silo: 0,
        secure: false,
        noticed: NOTICE_SCHEDULED,
    }
}

/// The clock stage `skyhook_theft` (every 20 ticks): opens and closes
/// windows, schedules the next, runs the links.
/// Scheduled, not pushed: `pending` drains only after a client's call, as structure_space's clock pushes do (#44).
pub fn tick(reg: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    crate::sov_hub::on_the_clock(reg, |reg| tick_at(reg, skyhook::now()));
}

/// [`tick`] at `now`.
pub fn tick_at(reg: &mut Registry, now: u64) {
    if reg.deltas().of(skyhook::KIND).is_empty() && reg.deltas().of(KIND).is_empty() {
        return;
    }
    // A raid row whose skyhook is gone: its silo goes with it.
    for r in all(reg.deltas()) {
        if skyhook::by_id(reg.deltas(), r.id).is_none() {
            drop_orphan(reg, &r);
        }
    }
    let ids: Vec<i64> = skyhook::all(reg.deltas())
        .iter()
        .filter(|s| s.built() && reagent_of(reg, s.planet).is_some())
        .map(|s| s.id)
        .collect();
    for id in ids {
        step(reg, id, now);
    }
}

/// The skyhook changed hands (`skyhook::acquire`): the silo item follows its
/// owner, and a secure link of the old owner ends.
pub fn on_acquired(reg: &mut Registry, sk: &Skyhook) {
    let Some(mut r) = by_id(reg.deltas(), sk.id).filter(|r| r.silo != 0) else {
        return;
    };
    if let Err(e) = reg.apply_items_quiet(&[Edit::Move {
        item_id: r.silo,
        to: Slot::new(sk.corp, sk.system, skyhook::FLAG_IN_SPACE),
    }]) {
        crate::errln!("[skyhook_theft] silo {} not handed over: {e}", r.silo);
    }
    if r.secure && r.state != LINKSTATE_IDLE {
        r.idle();
        put(reg.deltas_mut(), &r);
    }
    announce_slim(reg, &r, sk);
}

fn drop_orphan(reg: &mut Registry, r: &Raid) {
    // The skyhook is gone, so the silo's own location names the system.
    let system = reg.store().get(r.silo).map(|i| i.location_id);
    if r.silo != 0
        && system.is_some()
        && let Err(e) = reg.apply_items_quiet(&[Edit::Destroy { item_id: r.silo }])
    {
        crate::errln!("[skyhook_theft] silo {} not destroyed: {e}", r.silo);
    } else if let Some(system) = system.filter(|_| r.silo != 0) {
        announce_removed(reg, system, r.silo);
    }
    reg.deltas_mut().del(KIND, &[r.id.to_string()]);
}

fn step(reg: &mut Registry, id: i64, now: u64) {
    let Some(sk) = skyhook::by_id(reg.deltas(), id) else {
        return;
    };
    let mut r = match by_id(reg.deltas(), id) {
        Some(r) => r,
        None => {
            let (start, end) = window_after(id, 0, now, prime_hour(reg.deltas(), &sk, now));
            let r = blank(id, 0, start, end);
            put(reg.deltas_mut(), &r);
            set_window(reg, id, start, end);
            notice_scheduled(reg, &sk, start, end);
            r
        }
    };
    // The skyhook row may have lost the window (a rewrite): put it back.
    set_window(reg, id, r.start, r.end);
    advance_link(reg, &mut r, &sk, now);
    let opened = r.noticed & NOTICE_STARTED != 0;
    if opened && now > r.end {
        close(reg, &mut r, &sk, now);
    } else if !opened && r.open_at(now) {
        open(reg, &mut r, &sk, now);
    } else if !opened && now > r.end {
        // A window that passed unseen (the server was down): the next one.
        reschedule(reg, &mut r, &sk, now);
    }
}

fn reschedule(reg: &mut Registry, r: &mut Raid, sk: &Skyhook, now: u64) {
    let after = if now > r.end + 86_400 { now } else { r.end };
    let n = r.n + 1;
    let (start, end) = window_after(r.id, n, after, prime_hour(reg.deltas(), sk, now));
    *r = Raid {
        noticed: NOTICE_SCHEDULED,
        ..blank(r.id, n, start, end)
    };
    put(reg.deltas_mut(), r);
    set_window(reg, r.id, start, end);
    notice_scheduled(reg, sk, start, end);
}

fn open(reg: &mut Registry, r: &mut Raid, sk: &Skyhook, now: u64) {
    let before = crate::sov_workforce::before(reg, sk.system, false);
    if let Some(ty) = silo_type(reg) {
        match reg.apply_items(&[Edit::Create {
            type_id: ty,
            quantity: 1,
            to: Slot::new(sk.corp, sk.system, skyhook::FLAG_IN_SPACE),
        }]) {
            Ok(changes) => {
                r.silo = changes
                    .iter()
                    .find_map(|c| match c {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                    .unwrap_or(0);
            }
            Err(e) => crate::errln!("[skyhook_theft] no silo for {}: {e}", sk.id),
        }
    } else {
        crate::errln!(
            "[skyhook_theft] the type table has no silo; {} opens bare",
            sk.id
        );
    }
    if r.noticed & NOTICE_STARTED == 0 {
        r.noticed |= NOTICE_STARTED;
        notice_started(reg, sk, r.end);
    }
    put(reg.deltas_mut(), r);
    announce_added(reg, r, sk);
    // The skyhook's slim names its silo; the system's record says vulnerable.
    skyhook::changed_since(reg, sk, now, &before);
}

fn close(reg: &mut Registry, r: &mut Raid, sk: &Skyhook, now: u64) {
    let before = crate::sov_workforce::before(reg, sk.system, false);
    // A link still running ends with the window and moves nothing.
    let silo = r.silo;
    r.idle();
    if reg.store().get(silo).is_some()
        && let Err(e) = reg.apply_items_quiet(&[Edit::Destroy { item_id: silo }])
    {
        crate::errln!("[skyhook_theft] silo {silo} not destroyed: {e}");
    }
    if silo != 0 {
        announce_removed(reg, sk.system, silo);
    }
    r.silo = 0;
    notice_ended(reg, sk);
    reschedule(reg, r, sk, now);
    skyhook::changed_since(reg, sk, now, &before);
}

// ---------------------------------------------------------------------------
// The link
// ---------------------------------------------------------------------------

/// Whether the link's pilot is still there: in space in the skyhook's
/// system, in the same ship, within range of the silo.
fn link_intact(reg: &mut Registry, r: &Raid, sk: &Skyhook) -> bool {
    if reg.character(r.thief).is_none() {
        return false;
    }
    reg.advance_character(r.thief);
    let Some(here) = reg.roster().get(r.thief).copied() else {
        return false;
    };
    here.ship_id == r.ship
        && here.solar_system_id == sk.system
        && reg.store().get(r.ship).is_some()
        && silo_position(reg, sk).is_some_and(|at| {
            crate::space_state::distance(here.motion.position, at) - silo_radius(reg, r.silo)
                <= MAX_LINK_RANGE
        })
}

fn advance_link(reg: &mut Registry, r: &mut Raid, sk: &Skyhook, now: u64) {
    match r.state {
        LINKSTATE_RUNNING => {
            if !link_intact(reg, r, sk) {
                r.idle();
            } else if now >= r.complete_at {
                complete(reg, r, sk, now);
            } else {
                return;
            }
            put(reg.deltas_mut(), r);
            announce_slim(reg, r, sk);
        }
        LINKSTATE_COMPLETED if now >= r.complete_at + DELAY_BEFORE_IDLE_S => {
            r.idle();
            put(reg.deltas_mut(), r);
            announce_slim(reg, r, sk);
        }
        _ => {}
    }
}

/// The link finished: move the stock.
fn complete(reg: &mut Registry, r: &mut Raid, sk: &Skyhook, now: u64) {
    let mut moved = 0;
    if let (Some(reagent), Some(mut s)) =
        (reagent_of(reg, sk.planet), skyhook::settle(reg, sk.id, now))
    {
        if r.secure {
            // An authorised pilot's link secures what it can **[A]**.
            moved = s
                .unsecured
                .min(reagent.secured_cap.saturating_sub(s.secured));
            s.secured += moved;
            s.unsecured -= moved;
        } else {
            moved = u32::try_from(thief_room(reg, r, &reagent))
                .unwrap_or(u32::MAX)
                .min(s.unsecured);
            if moved > 0
                && reg
                    .apply_items(&[Edit::Create {
                        type_id: reagent.type_id,
                        quantity: i64::from(moved),
                        to: Slot::new(r.thief, r.ship, crate::sov_hub::FLAG_CARGO),
                    }])
                    .is_err()
            {
                moved = 0;
            }
            s.unsecured -= moved;
        }
        if moved > 0 {
            skyhook::put(reg.deltas_mut(), &s);
            skyhook::announce_stocks(reg, &s, now);
        }
    }
    crate::errln!(
        "[skyhook_theft] link on {} by {} done: {moved} moved (secure={})",
        r.id,
        r.thief,
        r.secure
    );
    r.state = LINKSTATE_COMPLETED;
    r.complete_at = now;
}

/// How many units the thief's cargo hold has room for.
fn thief_room(reg: &Registry, r: &Raid, reagent: &crate::sov_resources::Reagent) -> i64 {
    let capacity = crate::fitting::cargo_capacity_in(reg, r.ship);
    let free = (capacity
        - reg
            .store()
            .used_volume_in(reg.content(), r.ship, crate::sov_hub::FLAG_CARGO))
    .max(0.0);
    let unit = crate::item::find_in(reg.content(), reagent.type_id).map_or(0.0, |t| t.volume);
    crate::fitting::units_that_fit(free, unit)
}

fn failed(text: &str) -> ServiceError {
    crate::mtu::notify(text)
}

/// `(LINK_WITH_SHIP, "InitiateLink")` on the silo.
pub fn initiate_link(reg: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
    initiate_at(reg, c, skyhook::now(), false)
}

/// `(LINK_WITH_SHIP, "InitiateLinkQuick")`: the GM menu's link, done on the
/// next stage run.
///
/// TODO: the client offers this entry only to sessions with `ROLE_GML |
/// ROLE_WORLDMOD` (`menusvc.py:1961-1962`) **[V]**; every session holds GML
/// today, so the server does not check the role. Check it when roles split.
pub fn initiate_link_quick(reg: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
    initiate_at(reg, c, skyhook::now(), true)
}

/// Start a link at `now` (`quick`: complete at once).
pub fn initiate_at(
    reg: &mut Registry,
    c: &CompCall<'_>,
    now: u64,
    quick: bool,
) -> Result<PyValue, ServiceError> {
    let mut r = by_silo(reg.deltas(), c.item_id).ok_or_else(|| failed("There is no silo here."))?;
    let sk = skyhook::by_id(reg.deltas(), r.id).ok_or_else(|| failed("There is no skyhook."))?;
    if !r.open_at(now) {
        return Err(failed("This skyhook is not open to theft now."));
    }
    if r.state != LINKSTATE_IDLE || r.ship != 0 {
        return Err(failed("The silo is already linked."));
    }
    let sk = skyhook::settle(reg, sk.id, now).unwrap_or(sk);
    if sk.unsecured == 0 {
        return Err(failed("There is nothing unsecured to take from this silo."));
    }
    let ship = reg
        .character(c.actor)
        .map(|ch| ch.ship_id)
        .ok_or_else(|| failed("You are not in space."))?;
    if all(reg.deltas())
        .iter()
        .any(|o| o.ship == ship && o.state != LINKSTATE_IDLE)
    {
        return Err(failed("Your ship is already linked to a silo."));
    }
    reg.advance_character(c.actor);
    let here = reg
        .roster()
        .get(c.actor)
        .copied()
        .ok_or_else(|| failed("You are not in space."))?;
    if here.solar_system_id != sk.system || here.ship_id != ship {
        return Err(failed("You are not in space here."));
    }
    let at = silo_position(reg, &sk).ok_or_else(|| failed("The silo has no place."))?;
    if crate::space_state::distance(here.motion.position, at) - silo_radius(reg, r.silo)
        > MAX_LINK_RANGE
    {
        return Err(failed("You are too far from the silo."));
    }
    let secure = skyhook::owns(reg, c.actor, &sk) && skyhook::held(reg.deltas(), &sk);
    // An owner's link that would secure nothing is refused, not run for
    // ten minutes to no effect **[A]** (falsifier: TQ running it anyway).
    if secure && reagent_of(reg, sk.planet).is_some_and(|rg| sk.secured >= rg.secured_cap) {
        return Err(failed("The secured stock is already full."));
    }
    // Not checked, each an **[A]**: any hull may link (spec L9.3; falsifier:
    // TQ naming a hull class); a warping or cloaked ship may link
    // (`celestialCheckers.py:375-378` is the client's own check at
    // InitiateLink; falsifier: the server refusing it); cloaking under a
    // running link does not break it (falsifier: TQ breaking it); a link
    // that cannot finish before the window ends simply breaks at the close.
    r.state = LINKSTATE_RUNNING;
    r.thief = c.actor;
    r.ship = ship;
    r.secure = secure;
    r.link_started = now;
    r.complete_at = if quick { now } else { now + LINK_DURATION_S };
    put(reg.deltas_mut(), &r);
    announce_slim(reg, &r, &sk);
    if !secure {
        // A thief's link is a suspect act **[A]** (falsifier: TQ flagging
        // it differently).
        crate::crimewatch::suspect(reg, c.actor, crate::pyutil::filetime_now());
    }
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// Map filter answers (RE §2.6)
// ---------------------------------------------------------------------------

/// Systems that hold a skyhook vulnerable at `now`.
pub fn vulnerable_systems(d: &Deltas, now: u64) -> Vec<i64> {
    let mut v: Vec<i64> = skyhook::all(d)
        .iter()
        .filter(|s| skyhook::vulnerability(s, now).0)
        .map(|s| s.system)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// `GetSolarSystemsWithTheftVulnerableSkyhooksResponse`.
pub fn systems_response(d: &Deltas, now: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for s in vulnerable_systems(d, now) {
        put_bytes(&mut out, 1, &id(s as u64));
    }
    out
}

/// `GetTheftVulnerableSkyhooksInSolarSystemResponse` for `system`.
pub fn in_system_response(d: &Deltas, system: i64, now: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for s in skyhook::in_system(d, system) {
        let (vulnerable, start, end) = skyhook::vulnerability(&s, now);
        if !vulnerable {
            continue;
        }
        let mut m = Vec::new();
        put_bytes(&mut m, 1, &id(s.id as u64));
        put_bytes(&mut m, 2, &id(s.planet as u64));
        put_bytes(&mut m, 3, &timestamp(end));
        put_bytes(&mut m, 4, &timestamp(start));
        put_bytes(&mut out, 1, &m);
    }
    out
}

// ---------------------------------------------------------------------------
// GM (`/skyhook window <id> now`, `/skyhook link finish <id>`)
// ---------------------------------------------------------------------------

/// Open a window on skyhook `id` right now.
pub fn dev_window(reg: &mut Registry, id: i64, now: u64) -> String {
    let Some(sk) = skyhook::by_id(reg.deltas(), id) else {
        return format!("no skyhook {id}");
    };
    if !sk.built() || reagent_of(reg, sk.planet).is_none() {
        return format!("skyhook {id} is not a built reagent skyhook");
    }
    let mut r = by_id(reg.deltas(), id).unwrap_or_else(|| blank(id, 0, now, now));
    if r.silo != 0 {
        return format!("skyhook {id} already has a window open until {}", r.end);
    }
    // The grace already served: the window is vulnerable at once.
    r.start = now.saturating_sub(skyhook::VULN_GRACE_S);
    r.end = r.start + RAID_WINDOW_S;
    r.noticed &= !NOTICE_STARTED;
    put(reg.deltas_mut(), &r);
    set_window(reg, id, r.start, r.end);
    tick_at(reg, now);
    match by_id(reg.deltas(), id) {
        Some(r) if r.silo != 0 => format!(
            "skyhook {id} is open to theft until {}, silo {}",
            r.end, r.silo
        ),
        _ => format!(
            "skyhook {id} is open to theft until {}, but there is no silo type",
            r.end
        ),
    }
}

/// Complete the running link on skyhook `id` now.
pub fn dev_finish(reg: &mut Registry, id: i64, now: u64) -> String {
    let Some(mut r) = by_id(reg.deltas(), id) else {
        return format!("no raid on skyhook {id}");
    };
    if r.state != LINKSTATE_RUNNING {
        return format!("skyhook {id} has no running link");
    }
    r.complete_at = now;
    put(reg.deltas_mut(), &r);
    tick_at(reg, now);
    match by_id(reg.deltas(), id) {
        Some(r) if r.state == LINKSTATE_COMPLETED => format!("link on skyhook {id} completed"),
        _ => format!("link on skyhook {id} broke (the pilot is gone or out of range)"),
    }
}

#[cfg(test)]
mod tests {
    //! Every id, type and number here is invented; the constants of this
    //! module appear by name, never by value.

    use super::*;
    use crate::gateway::{get_bytes, get_uint};
    use crate::sov_hub::fixture::{ALICE, BOB, CAROL, CORP, DAVE, LONE, RIVAL, SYSTEM, ship};
    use crate::sov_resources::{FIXTURE_REAGENT_TYPE, fixture_planets, fixture_stars};
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const NOW: u64 = 2_000_000_000;
    const STAR: i64 = 40_910_000;
    const P_REAGENT: i64 = 40_910_001;
    const P_POWER: i64 = 40_910_011;
    const SKYHOOK_TYPE: i64 = 990_910_001;
    const SILO_TYPE: i64 = 990_910_002;
    const ROOMY_HULL: i64 = 990_910_010;
    const TINY_HULL: i64 = 990_910_011;
    const SK: i64 = 8_960_001;
    const SK_POWER: i64 = 8_960_002;

    fn content() -> crate::content::Content {
        let mut c = crate::sov_hub::fixture::content();
        c.insert("sov planets", fixture_planets());
        c.insert("sov stars", fixture_stars());
        let text = format!(
            "{}type\t{SKYHOOK_TYPE}\t{}\t46\t1\t1\t100.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Skyhook\n\
             type\t{SILO_TYPE}\t{GROUP_SILO}\t2\t1\t1\t100.0\t0.0\t1000.0\t700.0\t0\t\t\tInvented Silo\n\
             type\t{ROOMY_HULL}\t25\t6\t1\t1\t100.0\t5000.0\t1000.0\t20.0\t0\t\t\tInvented Roomy Hull\n\
             type\t{TINY_HULL}\t25\t6\t1\t1\t100.0\t120.0\t1000.0\t20.0\t0\t\t5646=30.0\tInvented Tiny Hull\n\
             type\t{FIXTURE_REAGENT_TYPE}\t{}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n",
            crate::typedata::FIXTURE,
            skyhook::GROUP_SKYHOOK,
            crate::sov_resources::GROUP_COLONY_REAGENTS,
        );
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn in_space(c: &crate::content::Content, item_id: i64, corp: i64) -> crate::item::Item {
        let t = crate::item::find_in(c, SKYHOOK_TYPE).expect("a test type");
        crate::item::Item {
            item_id,
            type_id: SKYHOOK_TYPE,
            owner_id: corp,
            location_id: SYSTEM,
            flag_id: skyhook::FLAG_IN_SPACE,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        }
    }

    fn place_at(reg: &mut Registry, who: i64, at: [f64; 3]) {
        let _ = reg.roster_mut().leave(who);
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship(who),
            SYSTEM,
            Motion::arrived(at, [0.0; 3]),
        ));
    }

    /// Alice and Bob in the holder's corp, Carol alone, Dave in a rival
    /// alliance. Alice, Dave: roomy hulls; Bob, Carol: tiny ones.
    fn world() -> Registry {
        let pilot = |id: i64, name: &str| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship(id),
            solar_system_id: Some(SYSTEM),
            ..crate::model::Character::default()
        };
        let content = Arc::new(content());
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
                pilot(DAVE, "Dave"),
            ],
            celestials: crate::celestial::Catalogue::parse(&format!(
                "star\t{STAR}\t9000\t4.0e8\t0\t0\t0\t{SYSTEM}\t0\t0\n\
                 planet\t{P_REAGENT}\t9003\t6.0e6\t1.0e10\t2.0e8\t3.0e9\t{SYSTEM}\t0\t0\n\
                 planet\t{P_POWER}\t9001\t6.0e6\t-4.0e9\t1.0e8\t9.0e9\t{SYSTEM}\t0\t0\n"
            )),
            ..crate::model::SandboxConfig::default()
        })
        .with_content(content.clone());
        let mut items = reg.store().items().to_vec();
        for (who, ty) in [
            (ALICE, ROOMY_HULL),
            (BOB, TINY_HULL),
            (CAROL, TINY_HULL),
            (DAVE, ROOMY_HULL),
        ] {
            let t = *crate::item::find_in(&content, ty).expect("a test hull");
            for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
                i.type_id = ty;
                i.group_id = t.group_id;
                i.category_id = t.category_id;
            }
        }
        items.extend([
            in_space(&content, SK, CORP),
            in_space(&content, SK_POWER, CORP),
        ]);
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_911_000,
            ..crate::store::Snapshot::default()
        });
        crate::service::sov_hub::test_corp(&mut reg, CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
        crate::service::sov_hub::test_corp(&mut reg, LONE, CAROL, &[(CAROL, 0)]);
        crate::service::sov_hub::test_corp(&mut reg, RIVAL, DAVE, &[(DAVE, 0)]);
        let mut st = crate::alliance::Alliances::load(reg.deltas());
        let a = st
            .create(CORP, ALICE, "Invented Holders", "IHL", "", "", 1)
            .expect("an alliance");
        let d = st
            .create(RIVAL, DAVE, "Invented Rivals", "IRV", "", "", 1)
            .expect("a second alliance");
        st.save(reg.deltas_mut());
        for (who, corp, alliance) in [
            (ALICE, CORP, Some(a)),
            (BOB, CORP, Some(a)),
            (CAROL, LONE, None),
            (DAVE, RIVAL, Some(d)),
        ] {
            let c = reg.character_mut(who).expect("a pilot");
            c.corporation_id = corp;
            c.alliance_id = alliance;
        }
        crate::sov::set_claim(
            reg.deltas_mut(),
            SYSTEM,
            Some((a, CORP)),
            skyhook::filetime(1_000_000_000),
        );
        let planet = reg.celestials().get(P_REAGENT).expect("a planet");
        let spot = skyhook::position_of(planet);
        for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
            place_at(&mut reg, who, [spot[0], spot[1], spot[2] + 5.0e6]);
            let n = n as i64 + 1;
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        let _ = crate::gateway_out::take(&mut reg);
        reg
    }

    /// A built, active skyhook with the given stocks over `planet`.
    fn stand(reg: &mut Registry, id: i64, planet: i64, secured: u32, unsecured: u32) {
        skyhook::put(
            reg.deltas_mut(),
            &Skyhook {
                id,
                planet,
                system: SYSTEM,
                corp: CORP,
                // Switched off: the stock stays as stood (no harvest, no caps).
                active: false,
                activated_at: NOW - 5_000,
                built_until: NOW - 5_000,
                secured,
                unsecured,
                last_cycle: NOW,
                vuln_start: 0,
                vuln_end: 0,
                state: skyhook::STATE_SHIELD_VULNERABLE,
            },
        );
    }

    /// The reagent skyhook `SK`, a window open now, its silo standing.
    fn open_world(secured: u32, unsecured: u32) -> (Registry, i64) {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, secured, unsecured);
        let said = dev_window(&mut reg, SK, NOW);
        let silo = by_id(reg.deltas(), SK).expect("a row").silo;
        assert_ne!(silo, 0, "{said}");
        let _ = reg.take_pending();
        let _ = crate::gateway_out::take(&mut reg);
        (reg, silo)
    }

    fn near_silo(reg: &mut Registry, who: i64, metres: f64) {
        let sk = skyhook::by_id(reg.deltas(), SK).expect("a skyhook");
        let at = silo_position(reg, &sk).expect("a place");
        place_at(reg, who, [at[0], at[1], at[2] + metres]);
    }

    fn call(actor: i64, item: i64) -> CompCall<'static> {
        CompCall {
            actor,
            system: SYSTEM,
            item_id: item,
            args: &[],
            kwargs: &[],
        }
    }

    fn cargo_of(reg: &Registry, who: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.location_id == ship(who) && i.type_id == FIXTURE_REAGENT_TYPE)
            .map(|i| i.quantity)
            .sum()
    }

    fn stocks(reg: &Registry) -> (u32, u32) {
        let s = skyhook::by_id(reg.deltas(), SK).expect("a skyhook");
        (s.secured, s.unsecured)
    }

    fn notices(reg: &mut Registry) -> Vec<String> {
        crate::gateway_out::take(reg)
            .into_iter()
            .map(|(ty, ..)| ty.rsplit('.').next().unwrap_or("").to_owned())
            .collect()
    }

    fn count(list: &[String], name: &str) -> usize {
        list.iter().filter(|n| *n == name).count()
    }

    /// The client's staleness rule (`data_type.py:132-158`) **[V]**, as the
    /// skyhook tests state it.
    fn client_says_stale(vulnerable: bool, start: u64, end: u64, now: u64) -> bool {
        // `data_type.py:150-157` **[V]**, grace 30 s.
        let (s, e) = (start + 30, end + 30);
        if vulnerable {
            e < now || s > now
        } else {
            s < now && e > now
        }
    }

    // -- the schedule --------------------------------------------------------

    #[test]
    fn a_window_is_a_pure_function_of_skyhook_index_and_time() {
        for id in [SK, SK_POWER, 17] {
            let mut after = NOW;
            for n in 0..30 {
                let w = window_after(id, n, after, 12);
                assert_eq!(w, window_after(id, n, after, 12), "deterministic");
                assert!(w.0 > after, "starts after the last one ended");
                assert_eq!(w.1 - w.0, RAID_WINDOW_S);
                after = w.1;
            }
        }
    }

    #[test]
    fn a_changed_prime_hour_applies_to_the_schedule_after_its_delay() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 0);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        let before = prime_hour(reg.deltas(), &sk, NOW);
        let hour = (before + 7) % 24;
        let holder = crate::sov::holder(reg.deltas(), SYSTEM).expect("a holder");
        // A delay of one hour, in FILETIME ticks, chosen by the test.
        let delay = 3_600 * 10_000_000;
        crate::sov::set_prime_hour(
            reg.deltas_mut(),
            holder,
            hour,
            skyhook::filetime(NOW),
            delay,
        )
        .expect("set");
        assert_eq!(prime_hour(reg.deltas(), &sk, NOW), before, "not yet");
        assert_eq!(prime_hour(reg.deltas(), &sk, NOW + 7_200), hour, "after");
    }

    #[test]
    fn the_spread_and_the_gap_differ_between_windows_and_skyhooks() {
        let starts: std::collections::BTreeSet<u64> = (0..20)
            .map(|n| window_after(SK, n, NOW, 12).0 % 86_400)
            .collect();
        assert!(starts.len() > 1, "a fixed offset is no spread");
        assert_ne!(
            window_after(SK, 0, NOW, 12),
            window_after(SK_POWER, 0, NOW, 12)
        );
        assert_ne!(
            window_after(SK, 0, NOW, 12).0 % 86_400,
            window_after(SK, 0, NOW, 3).0 % 86_400,
            "the prime hour moves the window"
        );
    }

    #[test]
    fn the_stage_schedules_one_window_for_a_reagent_skyhook_and_none_for_others() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 0);
        stand(&mut reg, SK_POWER, P_POWER, 0, 0);
        tick_at(&mut reg, NOW);
        let r = by_id(reg.deltas(), SK).expect("a row for the reagent skyhook");
        assert!(r.start > NOW && r.silo == 0);
        assert!(
            by_id(reg.deltas(), SK_POWER).is_none(),
            "no reagent, no raid"
        );
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        assert_eq!((sk.vuln_start, sk.vuln_end), (r.start, r.end));
        let n = notices(&mut reg);
        assert_eq!(count(&n, "TheftVulnerabilityWindowScheduledNotice"), 1);
        // Run again: nothing more is said.
        tick_at(&mut reg, NOW + 20);
        assert_eq!(notices(&mut reg), Vec::<String>::new());
    }

    /// The clock stage's pushes leave on the clock, as structure_space's clock pushes do (#44): `pending`
    /// drains only after a client's call.
    #[test]
    fn a_window_opening_on_the_clock_is_scheduled_not_pushed() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        tick_at(&mut reg, NOW);
        // A window that began past the grace before the wall clock.
        let mut r = by_id(reg.deltas(), SK).expect("a row");
        r.start = skyhook::now() - skyhook::VULN_GRACE_S - 5;
        r.end = r.start + RAID_WINDOW_S;
        put(reg.deltas_mut(), &r);
        set_window(&mut reg, SK, r.start, r.end);
        place_at(&mut reg, ALICE, [0.0; 3]);
        let _ = reg.take_pending_raw();
        tick(&mut reg, std::time::Instant::now(), 0);
        assert!(
            by_id(reg.deltas(), SK).unwrap().silo != 0,
            "the silo stands"
        );
        use crate::notify::Push;
        let silo = by_id(reg.deltas(), SK).unwrap().silo;
        let adds_silo = |p: &Box<dyn Push>| matches!(p.grid(), crate::grid::GridRole::Spawns { adds, .. } if adds.contains(&silo));
        let pending = reg.take_pending_raw();
        assert!(
            !pending.iter().any(|(_, p)| adds_silo(p)),
            "nothing waits for somebody's call"
        );
        let due = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60));
        assert!(
            due.iter()
                .any(|p| adds_silo(p) && p.audience().is_some_and(|t| t.contains(&ALICE))),
            "the clock sends the silo's ball to the pilot"
        );
    }

    #[test]
    fn the_window_is_never_stale_by_the_clients_rule_and_the_silo_follows_it() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        let mut windows = 0;
        let mut said = Vec::new();
        let mut was_open = false;
        let mut now = NOW;
        // Run to the first scheduled window's end plus one stage, however far
        // the gap rule puts it.
        tick_at(&mut reg, NOW);
        let horizon = by_id(reg.deltas(), SK).unwrap().end + 20;
        while now <= horizon {
            // The stage runs every twenty seconds (inside the client's
            // thirty-second grace); the client is asked every five
            // seconds, so the reads fall inside the first seconds of a
            // window and between its end and the next stage run.
            if (now - NOW).is_multiple_of(20) {
                tick_at(&mut reg, now);
                let r = by_id(reg.deltas(), SK).unwrap();
                assert_eq!(
                    r.silo != 0,
                    r.open_at(now),
                    "a silo stands exactly while the window is open"
                );
                said.extend(notices(&mut reg));
            }
            let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
            let (v, s, e) = skyhook::vulnerability(&sk, now);
            assert!(
                !client_says_stale(v, s, e, now),
                "stale at {now}: {v} {s} {e}"
            );
            let r = by_id(reg.deltas(), SK).unwrap();
            if v {
                assert!(r.open_at(now), "vulnerable only while open");
            }
            if v && !was_open {
                windows += 1;
            }
            was_open = v;
            now += 5;
        }
        assert!(windows >= 1, "a window opened: {windows}");
        let started = count(&said, "TheftVulnerabilityWindowStartedNotice");
        assert!(started == windows || started == windows + 1);
        let ended = count(&said, "TheftVulnerabilityWindowEndedNotice");
        assert!(ended == started || ended + 1 == started);
        assert!(count(&said, "TheftVulnerabilityWindowScheduledNotice") > windows);
    }

    #[test]
    fn the_flag_stays_false_and_the_gate_shut_through_the_grace() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        tick_at(&mut reg, NOW);
        let r = by_id(reg.deltas(), SK).unwrap();
        let grace = skyhook::VULN_GRACE_S;
        for t in [r.start, r.start + 1, r.start + grace - 1] {
            tick_at(&mut reg, t);
            let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
            assert!(!skyhook::vulnerability(&sk, t).0, "flag false at {t}");
            let r = by_id(reg.deltas(), SK).unwrap();
            assert!(!r.open_at(t) && r.silo == 0, "no silo at {t}");
            assert!(
                notices(&mut reg)
                    .iter()
                    .all(|n| !n.contains("StartedNotice"))
            );
        }
        tick_at(&mut reg, r.start + grace);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        assert!(skyhook::vulnerability(&sk, r.start + grace).0);
        assert!(by_id(reg.deltas(), SK).unwrap().silo != 0);
        // A GM window is vulnerable at once.
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        dev_window(&mut reg, SK, NOW);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        assert!(skyhook::vulnerability(&sk, NOW).0);
        assert!(by_id(reg.deltas(), SK).unwrap().silo != 0);
    }

    #[test]
    fn the_client_would_call_a_vulnerable_flag_inside_the_grace_stale() {
        // The helper and the rule can fail: the first seconds of a window
        // read as not vulnerable, and a read just after the end names the
        // next window, never the one that ended.
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 0);
        let mut sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        sk.vuln_start = NOW;
        sk.vuln_end = NOW + RAID_WINDOW_S;
        let (v, s, e) = skyhook::vulnerability(&sk, NOW + 1);
        assert!(!v && !client_says_stale(v, s, e, NOW + 1));
        assert!(client_says_stale(true, s, e, NOW + 1));
        let (v, s, e) = skyhook::vulnerability(&sk, NOW + RAID_WINDOW_S + 1);
        assert!(!v && s > NOW + RAID_WINDOW_S + 1);
        assert!(!client_says_stale(v, s, e, NOW + RAID_WINDOW_S + 1));
        assert!(client_says_stale(
            false,
            NOW,
            NOW + RAID_WINDOW_S,
            NOW + RAID_WINDOW_S + 1
        ));
    }

    // -- the silo ------------------------------------------------------------

    #[test]
    fn an_open_window_stands_a_silo_with_the_clients_slim_keys_and_closing_removes_it() {
        let (mut reg, silo) = open_world(3, 9);
        assert_eq!(reg.store().get(silo).unwrap().type_id, SILO_TYPE);
        let r = by_id(reg.deltas(), SK).unwrap();
        let (_, extras) = &slim_extras(&reg, &[silo, SK])[0];
        let get = |k: &str| extras.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("parentID"), Some(PyValue::Int(SK)));
        assert_eq!(
            get("component_skyhookReagentSilo_skyhookID"),
            Some(PyValue::Int(SK))
        );
        let PyValue::Tuple(t) = get("component_linkWithShip").unwrap() else {
            panic!("a tuple")
        };
        assert_eq!(t[1], PyValue::Int(i64::from(LINKSTATE_IDLE)));
        assert_eq!((t[2].clone(), t[3].clone()), (PyValue::None, PyValue::None));
        // The skyhook names its silo.
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        let named = skyhook::extras_of(&reg, &sk);
        assert_eq!(
            named
                .iter()
                .find(|(n, _)| *n == "reagentSiloItemID")
                .unwrap()
                .1,
            PyValue::Int(silo)
        );
        assert_eq!(balls_in(&reg, SYSTEM).len(), 1);
        assert_eq!(locate(&reg, silo), Some(SYSTEM));
        assert!(target(&reg, silo).is_some());
        // The window ends: ball, item and name go; a later window is booked.
        tick_at(&mut reg, r.end);
        assert!(reg.store().get(silo).is_some(), "the last second is inside");
        tick_at(&mut reg, r.end + 1);
        assert!(reg.store().get(silo).is_none());
        assert!(balls_in(&reg, SYSTEM).is_empty());
        assert_eq!(locate(&reg, silo), None);
        let after = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((after.silo, after.n), (0, r.n + 1));
        assert!(after.start > r.end);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        assert_eq!((sk.vuln_start, sk.vuln_end), (after.start, after.end));
        let n = notices(&mut reg);
        assert_eq!(count(&n, "TheftVulnerabilityWindowEndedNotice"), 1);
        assert_eq!(count(&n, "TheftVulnerabilityWindowScheduledNotice"), 1);
    }

    #[test]
    fn a_window_opening_and_closing_moves_no_budget_and_sends_no_state_notice() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 3, 9);
        let _ = crate::gateway_out::take(&mut reg);
        let said = dev_window(&mut reg, SK, NOW);
        let r = by_id(reg.deltas(), SK).expect("a row");
        assert_ne!(r.silo, 0, "{said}");
        tick_at(&mut reg, r.end + 1);
        assert_eq!(by_id(reg.deltas(), SK).expect("a row").silo, 0, "closed");
        // The harvest is the same open or closed, so no hub is told.
        let told = crate::gateway_out::take(&mut reg)
            .iter()
            .filter(|(t, _, _)| t.ends_with("StateChangedNotice"))
            .count();
        assert_eq!(told, 0);
    }

    #[test]
    fn a_window_that_passed_unseen_books_the_next_one_without_a_silo() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        tick_at(&mut reg, NOW);
        let r = by_id(reg.deltas(), SK).unwrap();
        tick_at(&mut reg, r.end + 5);
        let n = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((n.n, n.silo), (r.n + 1, 0));
        assert!(n.start > r.end);
        assert!(reg.store().items().iter().all(|i| i.type_id != SILO_TYPE));
    }

    #[test]
    fn a_skyhook_that_is_gone_takes_its_silo_with_it() {
        let (mut reg, silo) = open_world(0, 5);
        reg.deltas_mut().del(skyhook::KIND, &[SK.to_string()]);
        tick_at(&mut reg, NOW + 20);
        assert!(by_id(reg.deltas(), SK).is_none());
        assert!(reg.store().get(silo).is_none());
    }

    // -- the link ------------------------------------------------------------

    #[test]
    fn a_link_is_refused_outside_the_window_on_an_empty_silo_and_out_of_range() {
        // Out of range.
        let (mut reg, silo) = open_world(0, 5);
        near_silo(&mut reg, DAVE, MAX_LINK_RANGE * 4.0);
        assert!(initiate_at(&mut reg, &call(DAVE, silo), NOW, false).is_err());
        // Empty silo.
        let (mut empty, silo2) = open_world(9, 0);
        near_silo(&mut empty, DAVE, 100.0);
        assert!(initiate_at(&mut empty, &call(DAVE, silo2), NOW, false).is_err());
        // Outside the window, before the stage has taken the silo away.
        let (mut late, silo3) = open_world(0, 5);
        near_silo(&mut late, DAVE, 100.0);
        let end = by_id(late.deltas(), SK).unwrap().end + 1;
        assert!(initiate_at(&mut late, &call(DAVE, silo3), end, false).is_err());
        // Not a silo at all.
        assert!(initiate_at(&mut late, &call(DAVE, SK), NOW, false).is_err());
        // None of it started anything.
        for r in [&reg, &empty, &late] {
            let row = by_id(r.deltas(), SK).unwrap();
            assert_eq!((row.state, row.ship), (LINKSTATE_IDLE, 0));
        }
        // Once the window has closed the silo is gone and the component
        // router would not locate it.
        tick_at(&mut late, end);
        assert_eq!(locate(&late, silo3), None);
    }

    #[test]
    fn a_running_link_refuses_a_second_and_shows_in_the_slim() {
        let (mut reg, silo) = open_world(0, 50);
        near_silo(&mut reg, DAVE, 100.0);
        near_silo(&mut reg, CAROL, 200.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).expect("the link starts");
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!(
            (r.state, r.ship, r.thief),
            (LINKSTATE_RUNNING, ship(DAVE), DAVE)
        );
        assert!(r.complete_at > NOW, "a link takes time");
        let (_, extras) = &slim_extras(&reg, &[silo])[0];
        let link = &extras
            .iter()
            .find(|(n, _)| *n == "component_linkWithShip")
            .unwrap()
            .1;
        let PyValue::Tuple(t) = link else { panic!() };
        assert_eq!(t[1], PyValue::Int(i64::from(LINKSTATE_RUNNING)));
        assert_eq!(t[3], PyValue::Int(ship(DAVE)));
        assert!(initiate_at(&mut reg, &call(CAROL, silo), NOW + 1, false).is_err());
        assert_eq!(by_id(reg.deltas(), SK).unwrap().thief, DAVE);
        // A thief is a suspect from the moment the link starts.
        assert!(crate::crimewatch::is_suspect(
            &reg,
            DAVE,
            crate::pyutil::filetime_now()
        ));
        assert!(!crate::crimewatch::is_suspect(
            &reg,
            CAROL,
            crate::pyutil::filetime_now()
        ));
    }

    #[test]
    fn a_thiefs_link_moves_exactly_the_unsecured_stock_when_it_completes() {
        let (mut reg, silo) = open_world(11, 40);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        let done = by_id(reg.deltas(), SK).unwrap().complete_at;
        // Not before its time.
        tick_at(&mut reg, done - 1);
        assert_eq!((stocks(&reg), cargo_of(&reg, DAVE)), ((11, 40), 0));
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_RUNNING);
        let _ = notices(&mut reg);
        tick_at(&mut reg, done);
        assert_eq!(stocks(&reg), (11, 0), "the secured stock is not touched");
        assert_eq!(cargo_of(&reg, DAVE), 40);
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!(r.state, LINKSTATE_COMPLETED);
        assert!(notices(&mut reg).contains(&"ReagentSimulationsNotice".to_owned()));
        // Idle again after the delay, and nothing more to take.
        tick_at(&mut reg, done + DELAY_BEFORE_IDLE_S - 1);
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_COMPLETED);
        tick_at(&mut reg, done + DELAY_BEFORE_IDLE_S);
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((r.state, r.ship, r.thief), (LINKSTATE_IDLE, 0, 0));
        near_silo(&mut reg, DAVE, 100.0);
        assert!(
            initiate_at(
                &mut reg,
                &call(DAVE, silo),
                done + DELAY_BEFORE_IDLE_S + 20,
                false
            )
            .is_err(),
            "the silo is empty now"
        );
    }

    #[test]
    fn a_thiefs_hold_takes_only_what_it_has_room_for_and_the_rest_stays() {
        // Carol's hull is the tiny one.
        let (mut reg, silo) = open_world(0, 5_000);
        near_silo(&mut reg, CAROL, 100.0);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        let reagent = reagent_of(&reg, sk.planet).unwrap();
        let r = Raid {
            ship: ship(CAROL),
            ..by_id(reg.deltas(), SK).unwrap()
        };
        let room = thief_room(&reg, &r, &reagent);
        assert!(
            room > 0 && room < 5_000,
            "the fixture hold is small: {room}"
        );
        initiate_at(&mut reg, &call(CAROL, silo), NOW, false).unwrap();
        let done = by_id(reg.deltas(), SK).unwrap().complete_at;
        tick_at(&mut reg, done);
        assert_eq!(cargo_of(&reg, CAROL), room);
        assert_eq!(stocks(&reg), (0, 5_000 - room as u32));
    }

    #[test]
    fn an_authorised_pilots_link_secures_the_stock_up_to_the_cap() {
        let (mut reg, silo) = open_world(0, 30);
        let sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        let cap = reagent_of(&reg, sk.planet).unwrap().secured_cap;
        let mut s = sk;
        s.secured = cap - 4;
        skyhook::put(reg.deltas_mut(), &s);
        near_silo(&mut reg, ALICE, 100.0);
        initiate_at(&mut reg, &call(ALICE, silo), NOW, false).unwrap();
        let r = by_id(reg.deltas(), SK).unwrap();
        assert!(r.secure);
        tick_at(&mut reg, r.complete_at);
        assert_eq!(stocks(&reg), (cap, 26), "four moved, to the cap");
        assert_eq!(cargo_of(&reg, ALICE), 0, "nothing left the skyhook");
        assert!(!crate::crimewatch::is_suspect(
            &reg,
            ALICE,
            crate::pyutil::filetime_now()
        ));
    }

    #[test]
    fn a_link_that_breaks_moves_nothing() {
        // The thief leaves the range.
        let (mut reg, silo) = open_world(2, 40);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        near_silo(&mut reg, DAVE, MAX_LINK_RANGE * 4.0);
        let done = by_id(reg.deltas(), SK).unwrap().complete_at;
        tick_at(&mut reg, done);
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((r.state, r.ship), (LINKSTATE_IDLE, 0));
        assert_eq!((stocks(&reg), cargo_of(&reg, DAVE)), ((2, 40), 0));
        // The thief is gone altogether (docked or dead).
        let (mut reg, silo) = open_world(2, 40);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        let _ = reg.roster_mut().leave(DAVE);
        let done = by_id(reg.deltas(), SK).unwrap().complete_at;
        tick_at(&mut reg, done);
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_IDLE);
        assert_eq!((stocks(&reg), cargo_of(&reg, DAVE)), ((2, 40), 0));
        // The window ends under a running link.
        let (mut reg, silo) = open_world(2, 40);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        let end = by_id(reg.deltas(), SK).unwrap().end + 1;
        let mut s = by_id(reg.deltas(), SK).unwrap();
        s.complete_at = end + 100;
        put(reg.deltas_mut(), &s);
        tick_at(&mut reg, end);
        assert_eq!((stocks(&reg), cargo_of(&reg, DAVE)), ((2, 40), 0));
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((r.state, r.silo), (LINKSTATE_IDLE, 0));
    }

    #[test]
    fn an_owners_link_is_refused_when_the_secured_stock_is_full() {
        let (mut reg, silo) = open_world(0, 30);
        let mut sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        sk.secured = reagent_of(&reg, sk.planet).unwrap().secured_cap;
        skyhook::put(reg.deltas_mut(), &sk);
        near_silo(&mut reg, ALICE, 100.0);
        assert!(initiate_at(&mut reg, &call(ALICE, silo), NOW, false).is_err());
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_IDLE);
        // A thief is not refused by the cap: the stock leaves the skyhook.
        near_silo(&mut reg, DAVE, 100.0);
        assert!(initiate_at(&mut reg, &call(DAVE, silo), NOW, false).is_ok());
    }

    #[test]
    fn a_thief_and_an_owner_link_take_the_same_time() {
        let (mut reg, silo) = open_world(0, 30);
        near_silo(&mut reg, ALICE, 100.0);
        initiate_at(&mut reg, &call(ALICE, silo), NOW, false).unwrap();
        let owner = by_id(reg.deltas(), SK).unwrap().complete_at;
        let (mut reg, silo) = open_world(0, 30);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        assert_eq!(by_id(reg.deltas(), SK).unwrap().complete_at, owner);
    }

    #[test]
    fn a_silo_that_loses_its_skyhook_is_removed_from_the_pilots_in_space() {
        let (mut reg, silo) = open_world(0, 5);
        near_silo(&mut reg, DAVE, 100.0);
        let _ = reg.take_pending();
        reg.deltas_mut().del(skyhook::KIND, &[SK.to_string()]);
        tick_at(&mut reg, NOW + 20);
        assert!(reg.store().get(silo).is_none());
        let pushes = format!("{:?}", reg.take_pending());
        assert!(pushes.contains("BallsRemoved"), "{pushes}");
    }

    #[test]
    fn a_new_owner_takes_the_silo_and_ends_the_old_owners_secure_link() {
        let (mut reg, silo) = open_world(0, 30);
        near_silo(&mut reg, ALICE, 100.0);
        initiate_at(&mut reg, &call(ALICE, silo), NOW, false).unwrap();
        assert!(by_id(reg.deltas(), SK).unwrap().secure);
        let mut sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        sk.corp = RIVAL;
        skyhook::put(reg.deltas_mut(), &sk);
        on_acquired(&mut reg, &sk);
        assert_eq!(reg.store().get(silo).unwrap().owner_id, RIVAL);
        let r = by_id(reg.deltas(), SK).unwrap();
        assert_eq!((r.state, r.ship, r.secure), (LINKSTATE_IDLE, 0, false));
        // A thief's link is not the owner's: it runs on.
        let (mut reg, silo) = open_world(0, 30);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        let mut sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        sk.corp = RIVAL;
        on_acquired(&mut reg, &sk);
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_RUNNING);
    }

    #[test]
    fn the_quick_link_completes_on_the_next_stage_run_and_the_gm_commands_work() {
        let (mut reg, silo) = open_world(0, 12);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, true).unwrap();
        tick_at(&mut reg, NOW);
        assert_eq!(cargo_of(&reg, DAVE), 12);
        // `/skyhook link finish` finishes a running link now.
        let (mut reg, silo) = open_world(0, 12);
        near_silo(&mut reg, DAVE, 100.0);
        initiate_at(&mut reg, &call(DAVE, silo), NOW, false).unwrap();
        let said = dev_finish(&mut reg, SK, NOW + 1);
        assert!(said.contains("completed"), "{said}");
        assert_eq!((stocks(&reg), cargo_of(&reg, DAVE)), ((0, 0), 12));
        assert!(dev_finish(&mut reg, SK, NOW + 2).contains("no running link"));
        assert!(dev_window(&mut reg, 1, NOW).contains("no skyhook"));
        assert!(dev_window(&mut reg, SK, NOW).contains("already"));
    }

    #[test]
    fn the_slash_arms_reach_the_window_and_the_finish() {
        let mut reg = world();
        let wall = skyhook::now();
        // The slash line runs on the wall clock: the skyhook is stood at it.
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        let mut sk = skyhook::by_id(reg.deltas(), SK).unwrap();
        sk.activated_at = wall - 5_000;
        sk.built_until = wall - 5_000;
        sk.last_cycle = wall;
        skyhook::put(reg.deltas_mut(), &sk);
        skyhook::slash(&mut reg, None, &["window", &SK.to_string(), "now"]).unwrap();
        assert_ne!(by_id(reg.deltas(), SK).expect("a raid").silo, 0);
        skyhook::slash(&mut reg, None, &["link", "finish", &SK.to_string()]).unwrap();
        assert_eq!(by_id(reg.deltas(), SK).unwrap().state, LINKSTATE_IDLE);
    }

    // -- the map filter ------------------------------------------------------

    #[test]
    fn the_map_filter_lists_a_skyhook_only_inside_its_window() {
        let mut reg = world();
        stand(&mut reg, SK, P_REAGENT, 0, 5);
        tick_at(&mut reg, NOW);
        let r = by_id(reg.deltas(), SK).unwrap();
        assert!(systems_response(reg.deltas(), NOW).is_empty());
        assert!(in_system_response(reg.deltas(), SYSTEM, NOW).is_empty());
        tick_at(&mut reg, r.start);
        // The client calls an earlier "vulnerable" stale: the first seconds
        // of the window still read as not open.
        assert!(systems_response(reg.deltas(), r.start).is_empty());
        let at = r.start + skyhook::VULN_GRACE_S;
        let systems = systems_response(reg.deltas(), at);
        assert_eq!(
            crate::sov_proto::read_id(&systems, 1),
            Some(SYSTEM as u64),
            "the system is listed"
        );
        let body = in_system_response(reg.deltas(), SYSTEM, at);
        let entry = get_bytes(&body, 1).expect("one skyhook");
        assert_eq!(crate::sov_proto::read_id(&entry, 1), Some(SK as u64));
        assert_eq!(crate::sov_proto::read_id(&entry, 2), Some(P_REAGENT as u64));
        let ts = |n| get_uint(&get_bytes(&entry, n).unwrap(), 1).unwrap();
        assert_eq!((ts(3), ts(4)), (r.end, r.start), "expiry, then start");
        assert!(in_system_response(reg.deltas(), SYSTEM + 1, at).is_empty());
        assert!(systems_response(reg.deltas(), r.end + 1).is_empty());
    }

    #[test]
    fn a_row_round_trips() {
        let r = Raid {
            id: 7,
            start: 10,
            end: 20,
            state: LINKSTATE_RUNNING,
            thief: 3,
            ship: 4,
            link_started: 11,
            complete_at: 15,
            n: 2,
            silo: 9,
            secure: true,
            noticed: 3,
        };
        let mut reg = world();
        put(reg.deltas_mut(), &r);
        assert_eq!(by_id(reg.deltas(), 7), Some(r));
        assert_eq!(by_silo(reg.deltas(), 9), Some(r));
        assert_eq!(by_silo(reg.deltas(), 0), None);
    }
}
