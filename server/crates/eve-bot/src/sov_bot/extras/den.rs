//! **The den runner** (SOVX-DEN, #221): a hauler of a sov alliance that has
//! the Mercenary Den Management skill keeps mercenary dens beside the
//! alliance's workforce skyhooks, empties their infomorphs, lists them, starts
//! the activities a den offers and runs the activity's site as a guard post.
//!
//! The den runner is a [`Duty::Haul`] pilot (as the steward). The pure core is
//! [`den_plan`] over a [`DenView`]; [`arm`] reads the view over the gateway
//! (the den window's own requests), runs the one act the plan names and
//! answers a `sov den:` line, or `Ok(None)` when it did nothing (an answered
//! line skips the pilot's campaign duty for the turn).
//!
//! Every act is a client verb: a drop from the ship (the client's Launch), a
//! ballpark component call (the den window's Empty), a gateway request (the
//! activity cards' Start), a warp to the Expeditions entry. Nothing is made
//! from nowhere: the den comes from the pilot's hangar or the market (a
//! builder makes it), the infomorphs from the den.
//!
//! **Acting only here.** A den is launched, emptied and its infomorphs are
//! carried only in the system of the hub the pilot hauls for; an activity is
//! started and run only when its den's system is one of the alliance's own hubs
//! (`plan.fuel`). A skyhook or a den the park does not show is left alone.
//!
//! **[A] B3-1** [`extract_at`] is half the hold's room in infomorphs (at most
//! the den's cap: a den never holds more): a den is emptied when it holds that
//! many. Falsified by dens sitting at their cap
//! in a soak.
//!
//! **[A] B3-2** infomorphs have no NPC buyer (decision 24's default), so they
//! are listed on the market at the reservation price. Falsified by an owner
//! ruling (or listings that never sell in a soak).
//!
//! **[A] B3-3** the activity's site is in the pilot's Expeditions list
//! (`GetMyEscalatingPathDetails`) with the activity's expiry (matched within
//! [`SITE_MATCH_S`]); no match means no run. Falsified by a
//! started activity with no escalation row (the note says so); the run would
//! then read the site id from the activity itself.
//!
//! **[A] B3-4** [`SITE_QUIET`] is `site_runner::SITE_SETTLE`, and the escort
//! rules clear a den site; a run also ends after [`RUN_LIMIT`]. Falsified by
//! soak den runners dying in or abandoning sites.
//!
//! **[A] B3-5** [`IDLE_PASS`] (5 min) between passes of a pilot with nothing
//! to do, [`SKILL_RECHECK`] (30 min) for a pilot without the skill,
//! [`REFUSE_BACKOFF`] (the steward's) per refused target. Falsified by a den
//! left empty for long while its pilot idles (too long), or gateway requests
//! that dominate the server's turn time (too short).
//!
//! **[A] B3-6** the den is bought only at the docked station's asks (no
//! fetch trip); otherwise a want goes on the builders' board. Falsified by
//! den runners that wait hours for a den another station sells.

use super::Mind as ExtrasMind;
use super::gw::{self, Gw};
use super::steward::{LAUNCH_STANDOFF, REFUSE_BACKOFF};
use crate::brain::Memory;
use crate::clock;
use crate::sov_bot::{Ctx, Duty, Plan, RANGE_USE, WALLET_SHARE};
use crate::verbs::{FLAG_CARGO, ItemRow, SellLine};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::merc_den::{GROUP_MERCENARY_DEN, TRANSFER_RANGE};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A pilot with nothing to do waits this long (**[A] B3-5**).
pub const IDLE_PASS: Duration = Duration::from_secs(5 * 60);
/// A pilot without the skill looks again after this long (**[A] B3-5**).
pub const SKILL_RECHECK: Duration = Duration::from_secs(30 * 60);
/// A site with no NPC on the grid this long is clear (**[A] B3-4**).
pub const SITE_QUIET: Duration = crate::site_runner::SITE_SETTLE;
/// A site run ends after this long whatever the grid shows (**[A] B3-4**).
pub const RUN_LIMIT: Duration = Duration::from_secs(40 * 60);
/// An escalation row is the activity's site when its expiry is this near the
/// activity's, in seconds (**[A] B3-3**).
pub const SITE_MATCH_S: i64 = 30;

const ALL_OWNED: &str = "GetAllOwnedRequest";
const AS_OWNER: &str = "GetAsOwnerRequest";
const MAXIMUM: &str = "GetMaximumForCharacterRequest";
const ACTIVITIES: &str = "GetAllRequest";
const CAPACITY: &str = "GetCapacityRequest";
const START: &str = "StartRequest";
const HOOKS_BY_CORP: &str = "GetAllByCorporationRequest";
const EXTRACT: &str = "ExtractInfomorphsToShip";

// ---- what the den runner reads ---------------------------------------------------------

/// One own den (`GetAsOwnerResponse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DenRow {
    pub id: i64,
    pub skyhook: i64,
    pub system: i64,
    /// The infomorphs it holds now.
    pub infomorphs: u64,
}

/// One activity of the pilot's dens (`GetAllResponse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActRow {
    pub id: [u8; 16],
    pub den: i64,
    pub started: bool,
    /// Wall unix seconds.
    pub expiry: u64,
    pub system: i64,
}

/// One own skyhook (`GetAllByCorporationResponse`): what a den needs of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookRow {
    pub id: i64,
    /// Switched on and held (the record's own `active`).
    pub active: bool,
    /// Over a workforce planet (the record's workforce amount).
    pub workforce: bool,
    /// Has a reagent: a den does not stand over one.
    pub reagent: bool,
}

/// Everything [`den_plan`] reads.
pub struct DenView<'a> {
    /// Dens the skill allows now (`GetMaximumForCharacter`'s first field).
    pub max_dens: u64,
    pub dens: &'a [DenRow],
    pub activities: &'a [ActRow],
    /// Activities the skill allows at once.
    pub capacity: u64,
    pub hooks: &'a [HookRow],
    /// The system of the hub the pilot hauls for.
    pub system: i64,
    /// The systems of the alliance's own hubs.
    pub held: &'a [i64],
    /// Infomorph units the hold takes now, at most a den's cap.
    pub hold_room: i64,
    /// The infomorph type of the den table, if known.
    pub infomorph: Option<i64>,
    /// Infomorphs in the hold or the hangar.
    pub infomorphs_held: i64,
    /// A den item in the hold or the hangar.
    pub den_item: Option<i64>,
    /// The type of `den_item`.
    pub den_item_type: Option<i64>,
    pub den_type: Option<i64>,
    /// Guns fitted: only such a pilot runs the sites.
    pub guns: bool,
    pub docked: bool,
    /// Targets refused lately: dens, skyhooks, the den type.
    pub refused: &'a [i64],
    /// Activities left alone for now.
    pub quiet: &'a [[u8; 16]],
}

/// One thing the den runner does.
#[derive(Debug, Clone, PartialEq)]
pub enum DenAct {
    /// Buy or bid for the den type (docked).
    Want(i64),
    /// Fly beside the skyhook and launch the den item.
    Launch {
        item: i64,
        skyhook: i64,
    },
    /// Empty `qty` infomorphs of the den into the hold.
    Extract {
        den: i64,
        qty: i64,
    },
    /// List the infomorphs of this type (docked).
    Sell(i64),
    /// Start an activity.
    Start {
        activity: [u8; 16],
    },
    /// Fly to the started activity's site and hold it.
    Run {
        site_system: i64,
        activity: [u8; 16],
    },
    None,
}

/// The least infomorphs a den holds before it is worth emptying: half the
/// hold's room (**[A] B3-1**).
pub fn extract_at(hold_room: i64) -> u64 {
    (hold_room / 2).max(1) as u64
}

/// **What the den runner does this pass** (pure), first match: run a started
/// activity, start an offered one, empty a den, list the infomorphs, launch
/// (or want) a den beside a free workforce skyhook. Nothing without the skill.
pub fn den_plan(v: &DenView<'_>) -> DenAct {
    if v.max_dens == 0 {
        return DenAct::None;
    }
    let is_held = |s: i64| v.held.contains(&s);
    let own_den = |id: i64| v.dens.iter().any(|d| d.id == id);
    let left_alone = |a: &ActRow| v.quiet.contains(&a.id);
    // 1. A started activity in a held system is run.
    if v.guns
        && let Some(a) = v
            .activities
            .iter()
            .find(|a| a.started && !left_alone(a) && is_held(a.system) && own_den(a.den))
    {
        return DenAct::Run {
            site_system: a.system,
            activity: a.id,
        };
    }
    // 2. An offered one is started while the capacity allows.
    let started = v.activities.iter().filter(|a| a.started).count() as u64;
    if v.guns
        && started < v.capacity
        && let Some(a) = v
            .activities
            .iter()
            .find(|a| !a.started && !left_alone(a) && is_held(a.system) && own_den(a.den))
    {
        return DenAct::Start { activity: a.id };
    }
    // 3. A den worth emptying, while the hold has room.
    if v.hold_room > 0 {
        let at = extract_at(v.hold_room);
        if let Some(d) = v
            .dens
            .iter()
            .find(|d| d.system == v.system && !v.refused.contains(&d.id) && d.infomorphs >= at)
        {
            return DenAct::Extract {
                den: d.id,
                qty: (d.infomorphs as i64).min(v.hold_room),
            };
        }
    }
    // 4. Infomorphs on board are listed (docked).
    if v.docked
        && v.infomorphs_held > 0
        && let Some(t) = v.infomorph.filter(|t| !v.refused.contains(t))
    {
        return DenAct::Sell(t);
    }
    // 5. A den beside a free workforce skyhook while the skill allows one more.
    if (v.dens.len() as u64) < v.max_dens
        && let Some(h) = v.hooks.iter().find(|h| {
            h.active
                && h.workforce
                && !h.reagent
                && !v.refused.contains(&h.id)
                && !v.dens.iter().any(|d| d.skyhook == h.id)
        })
    {
        if let Some(item) = v
            .den_item
            .filter(|_| !v.den_item_type.is_some_and(|t| v.refused.contains(&t)))
        {
            return DenAct::Launch {
                item,
                skyhook: h.id,
            };
        }
        if v.docked
            && let Some(t) = v.den_type.filter(|t| !v.refused.contains(t))
        {
            return DenAct::Want(t);
        }
    }
    DenAct::None
}

/// The escalation of this system that is the activity's site (pure): the
/// row whose expiry is within [`SITE_MATCH_S`] of `expiry` (a FILETIME), the
/// nearest. No match is `None`: the activity goes quiet rather than the pilot
/// running another site (**[A] B3-3**).
///
/// [V] the site is an `escal` row opened by `activity::start` for the owner
/// alone, expiring with the activity (`merc_den/activity.rs` `open_site`,
/// `start`: "one clock with the site's, so the two end together").
pub fn pick_site(
    rows: &[crate::site_runner::EscalationRow],
    system: i64,
    expiry: i64,
    now: i64,
) -> Option<i64> {
    let tol = SITE_MATCH_S * 10_000_000;
    rows.iter()
        .filter(|r| r.system == system && r.expiry > now && (r.expiry - expiry).abs() <= tol)
        .min_by_key(|r| (r.expiry - expiry).abs())
        .map(|r| r.instance_id)
}

// ---- decoders (the server's own field layouts) -----------------------------------------

/// `GetAllOwnedResponse`: `{1* id(den)}`.
pub fn decode_owned(payload: &[u8]) -> Vec<i64> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|d| gw::uint(d, 1).and_then(|n| i64::try_from(n).ok()))
        .collect()
}

/// `GetAsOwnerResponse`: `{1 attributes {2 id(skyhook), 3 id(system)}, 4
/// infomorphs {2 contents {1 count}}}`; `id` is the den asked for.
pub fn decode_den(id: i64, payload: &[u8]) -> Option<DenRow> {
    let attrs = gw::bytes(payload, 1)?;
    Some(DenRow {
        id,
        skyhook: i64::try_from(gw::read_id(attrs, 2)?).ok()?,
        system: i64::try_from(gw::read_id(attrs, 3)?).ok()?,
        infomorphs: gw::bytes(payload, 4)
            .and_then(|i| gw::bytes(i, 2))
            .and_then(|c| gw::uint(c, 1))
            .unwrap_or(0),
    })
}

/// `GetMaximumForCharacterResponse` / `GetCapacityResponse`: `{1 n}`.
pub fn decode_first(payload: &[u8]) -> u64 {
    gw::uint(payload, 1).unwrap_or(0)
}

/// `activity.GetAllResponse`: `{1* {1 {1 uuid}, 2 {3 started, 4 {1 expiry},
/// 2 id(den), 5 id(system)}}}`.
pub fn decode_activities(payload: &[u8]) -> Vec<ActRow> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|e| {
            let uuid = gw::bytes(gw::bytes(e, 1)?, 1)?;
            let attrs = gw::bytes(e, 2)?;
            Some(ActRow {
                id: <[u8; 16]>::try_from(uuid).ok()?,
                den: i64::try_from(gw::read_id(attrs, 2)?).ok()?,
                started: gw::uint(attrs, 3) == Some(1),
                expiry: gw::bytes(attrs, 4)
                    .and_then(|t| gw::uint(t, 1))
                    .unwrap_or(0),
                system: i64::try_from(gw::read_id(attrs, 5)?).ok()?,
            })
        })
        .collect()
}

/// `skyhook.GetAllByCorporationResponse`: `{1* {1 id, 2 {reagent stock}, 3
/// active, 7 workforce {1 amount}}}`.
pub fn decode_hooks(payload: &[u8]) -> Vec<HookRow> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|r| {
            Some(HookRow {
                id: i64::try_from(gw::read_id(r, 1)?).ok()?,
                active: gw::uint(r, 3) == Some(1),
                workforce: gw::bytes(r, 7)
                    .and_then(|w| gw::uint(w, 1))
                    .is_some_and(|a| a > 0),
                reagent: gw::bytes(r, 2).is_some(),
            })
        })
        .collect()
}

/// The `StartRequest` body: `{1 {1 uuid}}`.
pub fn start_body(uuid: &[u8; 16]) -> Vec<u8> {
    let mut inner = Vec::new();
    gw::put_bytes(&mut inner, 1, uuid);
    let mut b = Vec::new();
    gw::put_bytes(&mut b, 1, &inner);
    b
}

// ---- what a den runner remembers -------------------------------------------------------

/// A site run in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub activity: [u8; 16],
    pub system: i64,
    pub started: Instant,
    /// Since when no NPC has been on the grid.
    pub quiet_since: Option<Instant>,
    /// The activity's own expiry, wall unix seconds.
    pub expiry: u64,
    /// The ship the run started in.
    pub ship: Option<i64>,
}

/// A den runner's memory (a field of [`super::Mind`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// The next pass is not before this (nothing to do, or no skill).
    pub wait_until: Option<Instant>,
    /// Targets (dens, skyhooks, the den type) refused, and when.
    pub refused: BTreeMap<i64, Instant>,
    /// Activities left alone, until this wall unix second.
    pub quiet: BTreeMap<[u8; 16], u64>,
    /// Own skyhooks seen not to be in the hub's system (no expiry).
    pub elsewhere: std::collections::BTreeSet<i64>,
    /// The site being run.
    pub run: Option<Run>,
}

// ---- the arm ---------------------------------------------------------------------------

/// A session that is dropped after the first `MISSING gateway`.
struct Session {
    gw: Option<Gw>,
}

impl Session {
    fn ask(&mut self, mind: &mut ExtrasMind, ty: &str, body: &[u8]) -> Option<(u32, Vec<u8>)> {
        let g = self.gw.as_mut()?;
        match g.ask(mind, ty, body) {
            Ok(r) => Some(r),
            Err(_) => {
                self.gw = None;
                None
            }
        }
    }

    /// A 200 answer's payload.
    fn read(&mut self, mind: &mut ExtrasMind, pkg: &str, ty: &str, body: &[u8]) -> Option<Vec<u8>> {
        match self.ask(mind, &format!("{pkg}{ty}"), body) {
            Some((200, payload)) => Some(payload),
            _ => None,
        }
    }
}

fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

fn id_body(id: i64) -> Vec<u8> {
    super::steward::id_body(id)
}

/// **The den arm** ([`super::ARMS`]): `Ok(None)` for a pilot that is not a
/// hauler of an own hub, has no den skill, or whose pass found nothing to do.
pub fn arm(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    duty: Duty,
) -> Result<Option<String>> {
    // A site being run is held first: it needs no gateway, and no duty
    // filter below lets the pilot off it (when one fails the run is ended).
    if m.sov.extras.den.run.is_some() {
        let live = matches!(duty, Duty::Haul(h) if plan.fuel.iter().any(|(_, x)| *x == h));
        return match (live, ctx.map) {
            (true, Some(map)) => hold_run(c, m, map),
            _ => {
                if let Some(run) = m.sov.extras.den.run.take() {
                    quiet_for(m, run.activity, run.expiry);
                }
                m.sov.post = None;
                Ok(None)
            }
        };
    }
    let Duty::Haul(hub) = duty else {
        return Ok(None);
    };
    let (Some(map), Some(content)) = (ctx.map, ctx.content) else {
        return Ok(None);
    };
    let Some(&(system, _)) = plan.fuel.iter().find(|(_, h)| *h == hub) else {
        return Ok(None);
    };
    let now = clock::instant();
    if m.sov.extras.den.wait_until.is_some_and(|t| now < t) {
        return Ok(None);
    }
    let Some(corp) = c.session().get_i64("corpid") else {
        return Ok(None);
    };
    let Some(g) = Gw::open(c, &mut m.sov.extras) else {
        return Ok(None);
    };
    let mut s = Session { gw: Some(g) };
    m.sov
        .extras
        .den
        .refused
        .retain(|_, at| now.saturating_duration_since(*at) < REFUSE_BACKOFF);
    m.sov
        .extras
        .den
        .quiet
        .retain(|_, until| *until > clock::wall_unix_secs());

    // The skill: no den allowed means no den runner, and no more requests for
    // a while.
    let Some(raw) = s.read(&mut m.sov.extras, gw::DEN_PKG, MAXIMUM, &[]) else {
        return Ok(None);
    };
    let max_dens = decode_first(&raw);
    if max_dens == 0 {
        m.sov.extras.den.wait_until = Some(now + SKILL_RECHECK);
        return Ok(Some(
            "sov den: MISSING the den skill (in the plan)".to_owned(),
        ));
    }
    let Some(raw) = s.read(&mut m.sov.extras, gw::DEN_PKG, ALL_OWNED, &[]) else {
        return Ok(None);
    };
    let mut dens = Vec::new();
    for id in decode_owned(&raw) {
        let Some(raw) = s.read(&mut m.sov.extras, gw::DEN_PKG, AS_OWNER, &id_body(id)) else {
            if s.gw.is_none() {
                return Ok(None);
            }
            continue;
        };
        if let Some(d) = decode_den(id, &raw) {
            dens.push(d);
        }
    }
    let Some(raw) = s.read(&mut m.sov.extras, gw::ACTIVITY_PKG, ACTIVITIES, &[]) else {
        return Ok(None);
    };
    let activities = decode_activities(&raw);
    let Some(raw) = s.read(&mut m.sov.extras, gw::ACTIVITY_PKG, CAPACITY, &[]) else {
        return Ok(None);
    };
    let capacity = decode_first(&raw);
    // The skyhooks only matter while another den is allowed.
    let hooks = if (dens.len() as u64) < max_dens {
        let Some(raw) = s.read(
            &mut m.sov.extras,
            gw::SKYHOOK_PKG,
            HOOKS_BY_CORP,
            &id_body(corp),
        ) else {
            return Ok(None);
        };
        decode_hooks(&raw)
    } else {
        Vec::new()
    };

    // What the pilot carries and flies.
    let docked = c.session().station_id().is_some();
    let guns = crate::sov_bot::kit_of(c, Some(content))?.guns;
    let cargo = c.cargo()?;
    let hangar = if docked {
        c.hangar_items()?
    } else {
        Vec::new()
    };
    let den_type = den_type(content);
    let def = den_type.and_then(|t| eve_server::merc_den::def_of(content, t));
    let infomorph = def.map(|d| d.infomorph_type);
    // A den never holds more than its cap, so the hold's room that counts is
    // at most the cap (half of a huge hold would never be reached).
    let den_cap = def.map_or(i64::MAX, |d| i64::from(d.cargo_cap));
    let volume = |t: i64| eve_server::item::find_in(content, t).map_or(0.0, |t| t.volume);
    let capacity_m3 = hold_capacity(c, content);
    let used: f64 = cargo
        .iter()
        .map(|r| r.quantity.max(1) as f64 * volume(r.type_id))
        .sum();
    let unit = infomorph.map_or(0.0, volume);
    let hold_room = if unit > 0.0 {
        ((((capacity_m3 - used).max(0.0) / unit) + 1e-9).floor() as i64).min(den_cap)
    } else {
        0
    };
    let held_of = |rows: &[ItemRow]| -> i64 {
        rows.iter()
            .filter(|r| Some(r.type_id) == infomorph)
            .map(|r| r.quantity.max(1))
            .sum()
    };
    let infomorphs_held = held_of(&cargo) + held_of(&hangar);
    let den_row = cargo
        .iter()
        .chain(&hangar)
        .find(|r| r.group_id == GROUP_MERCENARY_DEN);
    let den_item = den_row.map(|r| r.item_id);
    let den_item_type = den_row.map(|r| r.type_id);
    let hooks: Vec<HookRow> = hooks
        .into_iter()
        .filter(|h| !m.sov.extras.den.elsewhere.contains(&h.id))
        .collect();
    let held: Vec<i64> = plan.fuel.iter().map(|(s, _)| *s).collect();
    let refused: Vec<i64> = m.sov.extras.den.refused.keys().copied().collect();
    let quiet: Vec<[u8; 16]> = m.sov.extras.den.quiet.keys().copied().collect();
    let act = den_plan(&DenView {
        max_dens,
        dens: &dens,
        activities: &activities,
        capacity,
        hooks: &hooks,
        system,
        held: &held,
        hold_room,
        infomorph,
        infomorphs_held,
        den_item,
        den_item_type,
        den_type,
        guns,
        docked,
        refused: &refused,
        quiet: &quiet,
    });
    if act == DenAct::None {
        m.sov.extras.den.wait_until = Some(now + IDLE_PASS);
        return Ok(None);
    }
    let mut lines: Vec<String> = Vec::new();
    match act {
        DenAct::Want(t) => {
            let (line, bought) = want(c, t)?;
            if !bought {
                m.sov.extras.den.wait_until = Some(now + IDLE_PASS);
            }
            lines.push(line);
        }
        DenAct::Launch { item, skyhook } => {
            lines.push(launch(
                c,
                m,
                map,
                system,
                item,
                skyhook,
                den_item_in_hangar(&hangar, item),
            )?);
        }
        DenAct::Extract { den, qty } => {
            lines.push(extract(c, m, map, system, den, qty)?);
            // The same trip home sells what was taken.
            if c.session().station_id().is_some()
                && let Some(t) = infomorph
            {
                let (line, ok) = sell(c, t)?;
                if !ok {
                    m.sov.extras.den.refused.insert(t, now);
                }
                lines.push(line);
            }
        }
        DenAct::Sell(t) => {
            let (line, ok) = sell(c, t)?;
            if !ok {
                m.sov.extras.den.refused.insert(t, now);
            }
            lines.push(line);
        }
        DenAct::Start { activity } => {
            let line = match s.ask(
                &mut m.sov.extras,
                &format!("{}{START}", gw::ACTIVITY_PKG),
                &start_body(&activity),
            ) {
                Some((200, _)) => "started an activity".to_owned(),
                Some((status, _)) => {
                    quiet_for(m, activity, 0);
                    format!("activity not started ({status})")
                }
                None => "activity not started (no gateway)".to_owned(),
            };
            lines.push(line);
        }
        DenAct::Run {
            site_system,
            activity,
        } => {
            let expiry = activities
                .iter()
                .find(|a| a.id == activity)
                .map_or(0, |a| a.expiry);
            lines.push(run_site(c, m, map, site_system, activity, expiry)?);
        }
        DenAct::None => {}
    }
    Ok(Some(format!("sov den: {}", lines.join("; "))))
}

fn den_item_in_hangar(hangar: &[ItemRow], item: i64) -> bool {
    hangar.iter().any(|r| r.item_id == item)
}

/// The first den type of the type table that the den table has a row for.
fn den_type(content: &eve_server::content::Content) -> Option<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.group_id == GROUP_MERCENARY_DEN)
        .map(|t| t.type_id)
        .find(|t| eve_server::merc_den::def_of(content, *t).is_some())
}

/// The active hull's hold, m3 (0 when the hull is unknown).
fn hold_capacity(c: &mut Conn, content: &eve_server::content::Content) -> f64 {
    let hull = match c.session().station_id() {
        Some(_) => {
            let ship = c.session().ship_id();
            c.hangar_items()
                .ok()
                .and_then(|rows| rows.into_iter().find(|r| Some(r.item_id) == ship))
                .map(|r| r.type_id)
        }
        None => c.world_now().ego_ball().and_then(|b| b.type_id()),
    };
    hull.and_then(|h| eve_server::item::find_in(content, h))
        .map_or(0.0, |t| t.capacity)
}

/// Warp to `ball` and wait for the landing.
fn arrive(c: &mut Conn, ball: i64, range: f64) -> Result<()> {
    crate::sov_bot::warp_near(c, ball, range)
}

/// Whether the park shows `ball`.
fn in_park(c: &mut Conn, ball: i64) -> bool {
    c.world_now().get(ball).is_some()
}

/// A den bought or bid for (docked): the asks at this station, else a want on
/// the builders' board and a standing bid (**[A] B3-6**).
fn want(c: &mut Conn, ty: i64) -> Result<(String, bool)> {
    super::kit::want(ty);
    let wallet = crate::acquire::spendable(c)? * WALLET_SHARE;
    let book = soft(c.market_orders(ty))?;
    if let (Some(book), Some(station)) = (book, c.session().station_id()) {
        let own = crate::capital::own_ids_soft(c)?;
        let book = crate::verbs::Book {
            asks: book
                .asks
                .into_iter()
                .filter(|a| {
                    !a.bid
                        && a.vol_remaining > 0
                        && a.station_id == station
                        && !own.contains(&a.order_id)
                })
                .collect(),
            bids: Vec::new(),
        };
        if !book.asks.is_empty() {
            let got = crate::capital::buy_units(c, &book, ty, 1, wallet)?;
            if got > 0 {
                return Ok((format!("bought {got} den {ty} (in the hangar)"), true));
            }
        }
    }
    let posted = soft(crate::hull_choice::bid_for(c, ty, 1, wallet))?;
    Ok((
        format!(
            "MISSING den {ty} ({})",
            if posted == Some(true) {
                "bid posted"
            } else {
                "bid stands"
            }
        ),
        false,
    ))
}

/// **Launch the den** beside the skyhook: out of the hangar into the hold
/// (docked), to the held system, beside the skyhook, `ship.Drop`, home.
fn launch(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    system: i64,
    item: i64,
    skyhook: i64,
    in_hangar: bool,
) -> Result<String> {
    let now = clock::instant();
    if in_hangar && c.session().station_id().is_some() {
        let loaded = soft(c.load_cargo(item, None))?;
        if loaded.is_none_or(|o| o.refusal().is_some()) {
            // The den does not fit: the type is left alone for a while.
            if let Some(t) = c
                .hangar_items()?
                .iter()
                .find(|r| r.item_id == item)
                .map(|r| r.type_id)
            {
                m.sov.extras.den.refused.insert(t, now);
            }
            return Ok("MISSING a hull whose hold takes the den".to_owned());
        }
    }
    crate::sov_bot::fly_to_system(c, map, system)?;
    // A skyhook the park does not show is not in this system.
    if !c.world().has_state() || !in_park(c, skyhook) {
        m.sov.extras.den.elsewhere.insert(skyhook);
        let home = crate::sov_bot::go_home(c, m, map)?;
        return Ok(format!(
            "skyhook {skyhook} is not in system {system}; {home}"
        ));
    }
    arrive(c, skyhook, LAUNCH_STANDOFF)?;
    let line = match gw::launch(c, item)? {
        Ok(id) => format!("launched den {id} beside skyhook {skyhook}"),
        Err(text) => {
            m.sov.extras.den.refused.insert(skyhook, now);
            format!("launch beside skyhook {skyhook} refused: {text}")
        }
    };
    let home = crate::sov_bot::go_home(c, m, map)?;
    Ok(format!("{line}; {home}"))
}

/// **Empty a den**: to the held system, beside the den, the den window's own
/// component call, home.
fn extract(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    system: i64,
    den: i64,
    qty: i64,
) -> Result<String> {
    let now = clock::instant();
    crate::sov_bot::fly_to_system(c, map, system)?;
    if !c.world().has_state() || !in_park(c, den) {
        m.sov.extras.den.refused.insert(den, now);
        let home = crate::sov_bot::go_home(c, m, map)?;
        return Ok(format!("den {den} is not in system {system}; {home}"));
    }
    arrive(c, den, TRANSFER_RANGE * RANGE_USE)?;
    let out = gw::component_call(
        c,
        den,
        gw::MERCENARY_DEN,
        EXTRACT,
        vec![PyValue::Int(FLAG_CARGO), PyValue::Int(qty)],
    )?;
    let line = match out.refusal() {
        None => format!("took up to {qty} infomorphs from den {den}"),
        Some(why) => {
            m.sov.extras.den.refused.insert(den, now);
            format!("den {den} refused the take: {why}")
        }
    };
    let home = crate::sov_bot::go_home(c, m, map)?;
    Ok(format!("{line}; {home}"))
}

/// **List the infomorphs** (docked): the hold's into the hangar, then every
/// stack at the reservation price (**[A] B3-2**).
fn sell(c: &mut Conn, ty: i64) -> Result<(String, bool)> {
    let Some(station) = c.session().station_id() else {
        return Ok(("sell: not docked".to_owned(), true));
    };
    let in_hold: Vec<ItemRow> = c.cargo()?.into_iter().filter(|r| r.type_id == ty).collect();
    for r in in_hold {
        soft(c.unload(r.item_id, None))?;
    }
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == ty && r.quantity > 0)
        .collect();
    if stacks.is_empty() {
        return Ok(("no infomorphs to list".to_owned(), true));
    }
    let mut prices = crate::pricing::Prices::read(c)?;
    let Some(price) = prices.reservation(c, ty)?.filter(|p| *p > 0.0) else {
        return Ok((format!("MISSING a price for infomorph {ty}; kept"), false));
    };
    let mut listed = 0;
    for r in stacks {
        let line = SellLine {
            station_id: station,
            type_id: ty,
            item_id: r.item_id,
            price,
            quantity: r.quantity,
        };
        if soft(c.sell(
            &[line],
            crate::escort::LIST_DAYS,
            crate::escort::LIST_BROKER_FEE,
        ))?
        .is_some()
        {
            listed += r.quantity;
        }
    }
    Ok((format!("listed {listed} infomorphs of {ty}"), listed > 0))
}

/// **Run an activity's site**: to the system, the Expeditions entry's warp,
/// then the pilot holds the site as a guard post ([`hold_run`] keeps it).
fn run_site(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    system: i64,
    activity: [u8; 16],
    expiry: u64,
) -> Result<String> {
    use crate::site_runner::{ESCALATION_METHOD, ESCALATION_SERVICE, ESCALATION_SUBJECT};
    crate::sov_bot::fly_to_system(c, map, system)?;
    if c.ballpark().is_none() {
        crate::capital::bind(c)?;
    }
    let outcome = c.call(ESCALATION_SERVICE, ESCALATION_METHOD, vec![])?;
    let rows = outcome
        .value()
        .filter(|_| outcome.refusal().is_none())
        .map(|v| crate::site_runner::read_escalations(&v))
        .unwrap_or_default();
    let site = pick_site(
        &rows,
        system,
        eve_server::skyhook::filetime(expiry),
        eve_server::pyutil::filetime_now(),
    );
    let Some(site) = site else {
        quiet_for(m, activity, expiry);
        let home = crate::sov_bot::go_home(c, m, map)?;
        return Ok(format!(
            "activity site: no Expeditions entry in system {system}; {home}"
        ));
    };
    let warp = c.bound_call_kw(
        "CmdWarpToStuff",
        vec![
            PyValue::Bytes(ESCALATION_SUBJECT.to_vec()),
            PyValue::Int(site),
        ],
        vec![(
            PyValue::Real(crate::site_runner::WARP_RANGE),
            PyValue::Bytes(b"minRange".to_vec()),
        )],
    )?;
    if let Some(why) = warp.refusal() {
        quiet_for(m, activity, 0);
        let home = crate::sov_bot::go_home(c, m, map)?;
        return Ok(format!("warp to the site {site} refused: {why}; {home}"));
    }
    // Wait for the landing.
    c.pump(Duration::from_millis(250))?;
    let deadline = clock::instant() + Duration::from_secs(120);
    while c.world_now().ego_ball().is_some_and(|b| b.warp_commanded()) {
        if clock::instant() >= deadline {
            break;
        }
        c.pump(Duration::from_millis(250))?;
    }
    // The sov role's in-space rules fight from here (as a guard's post).
    m.sov.post = Some((system, site));
    m.sov.extras.den.run = Some(Run {
        activity,
        system,
        started: clock::instant(),
        quiet_since: None,
        expiry,
        ship: c.session().ship_id(),
    });
    // The extras pass again at the next sov turn, not a minute on.
    m.sov.extras.next = None;
    Ok(format!(
        "running the activity site {site} in system {system}"
    ))
}

/// Leave `activity` alone until `expiry` (wall unix seconds) when that is in
/// the future, else for [`REFUSE_BACKOFF`].
fn quiet_for(m: &mut Memory, activity: [u8; 16], expiry: u64) {
    let wall = clock::wall_unix_secs();
    let until = if expiry > wall {
        expiry
    } else {
        wall + REFUSE_BACKOFF.as_secs()
    };
    m.sov.extras.den.quiet.insert(activity, until);
}

/// **Hold the site** (each sov turn while a run is on): a line while the site
/// has NPCs or has not yet been quiet for [`SITE_QUIET`]; the post is cleared
/// and the pilot goes home once it has, the activity is gone from the pilot's
/// system, or [`RUN_LIMIT`] has passed.
fn hold_run(c: &mut Conn, m: &mut Memory, map: &crate::travel::Map) -> Result<Option<String>> {
    let now = clock::instant();
    let Some(run) = m.sov.extras.den.run.clone() else {
        return Ok(None);
    };
    let gone = c.session().station_id().is_some()
        || c.session().ship_id() != run.ship
        || c.session().solar_system_id2() != Some(run.system)
        || !c.world().has_state();
    let npc = !gone && c.world_now().balls().any(crate::driver::is_npc);
    let quiet_since = if npc {
        None
    } else {
        run.quiet_since.or(Some(now))
    };
    let over_limit = now.saturating_duration_since(run.started) >= RUN_LIMIT;
    let cleared = gone
        || over_limit
        || quiet_since.is_some_and(|q| now.saturating_duration_since(q) >= SITE_QUIET);
    if !cleared {
        if let Some(r) = m.sov.extras.den.run.as_mut() {
            r.quiet_since = quiet_since;
        }
        m.sov.extras.next = None;
        return Ok(Some("sov den: holding the activity site".to_owned()));
    }
    m.sov.post = None;
    m.sov.extras.den.run = None;
    // The activity is left alone for a while: a completed one is gone, one
    // that is not is not rerun at once.
    // A run that hit the limit is not retried before the activity expires.
    quiet_for(m, run.activity, if over_limit { run.expiry } else { 0 });
    if gone {
        return Ok(Some(
            "sov den: the site run ended (not on the grid)".to_owned(),
        ));
    }
    let home = crate::sov_bot::go_home(c, m, map)?;
    Ok(Some(format!("sov den: site run cleared; {home}")))
}
