//! **The steward** (SOVX-STEWARD-1, #221): one pilot per hub keeps the held
//! system's planets under skyhooks and the hub's reagent bay stocked.
//!
//! A steward is a [`Duty::Haul`] pilot (the plan's round-robin gives a hub its
//! haulers). One steward per hub holds the claim ([`claim`]); the other
//! haulers carry fuel blocks as before. The steward
//!
//! 1. **wants** one skyhook over every planet of the system that has a yield
//!    and none of ours (a bid on the market and a line on the want board,
//!    [`super::kit::want`], that the builders read), **fetches** one a builder
//!    has put on sale, and **launches** it over the planet (`ship.Drop`, as the
//!    client's Launch for Corporation);
//! 2. **switches on** an own skyhook that stands off (the gateway's
//!    `ActivateRequest`);
//! 3. **extracts** a skyhook's reagents into the hold when the bay holds less
//!    than [`STEWARD_DAYS`] of burn (the component call the skyhook window
//!    sends), and **deposits** them into the hub's bay (`AddRequest`).
//!
//! Every act is a client verb; nothing is made from nowhere. The pure core is
//! [`steward_plan`] over a [`StewardView`]; [`arm`] reads the view, runs the
//! plan and answers a `sov steward:` line, or `Ok(None)` when it did nothing
//! (an answered line skips the pilot's campaign duty for the turn).
//!
//! **[A] B1a-1** [`STEWARD_DAYS`] is 3 (parity-sov section 3). Falsified by
//! soak hubs whose bay runs dry (too low) or skyhooks emptied while the bay
//! overflows (too high).
//!
//! **[A] B1a-2** [`HAUL_MIN`] is a quarter of the skyhook's unsecured cap
//! (read from its definition), else [`HAUL_MIN_FALLBACK`] units. Falsified by
//! notes of trips carrying a handful of units.
//!
//! **[A] B1a-3** [`IDLE_STOCK`]: a reagent no upgrade burns yet is hauled up to
//! this many units (the upgrade table is read by the upgrade lane, not here).
//! Falsified by soak bays filling with a reagent nothing burns.
//!
//! **[A] B1a-4** the steward carries the kit only in the hull it flies; no
//! hull swap. Falsified by soak notes `MISSING a hull whose hold takes ...` on
//! most stewards.
//!
//! **[A] B1a-5** [`STEWARD_TTL`] is 30 min, [`ROLE_BACKOFF`] 6 h,
//! [`DEFS_TTL`] 1 h, [`LAUNCH_STANDOFF`] a fifth of the launch range.
//! Falsified by two stewards on one hub in a soak, a newly promoted director
//! waiting hours, planet definitions gone stale after a content change, or
//! launches refused for distance.
//!
//! **[A] B1a-7** [`REFUSE_BACKOFF`] is 30 min and [`LEARN_RETRY`] 15 min.
//! Falsified by a soak where a refused target is retried every pass (too
//! short) or a fixed cause (a role granted, a window passed) waits an hour
//! (too long).
//!
//! **[A] B1a-6** a steward is any Haul pilot; the one with the role does the
//! role acts. Falsified by alliances whose haulers never hold a role (every
//! launch refused): then `corp_ops` should grant the role.

use super::Mind as ExtrasMind;
use super::gw::{self, Gw};
use crate::brain::Memory;
use crate::clock;
use crate::sov_bot::{Ctx, Duty, Plan, RANGE_USE, WALLET_SHARE};
use crate::verbs::{FLAG_CARGO, ItemRow};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::skyhook::{GROUP_SKYHOOK, LAUNCH_RANGE, Refused, TRANSFER_RANGE};
use eve_server::sov_resources::GROUP_COLONY_REAGENTS;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Days of burn the bay is kept stocked for (**[A] B1a-1**).
pub const STEWARD_DAYS: u64 = 3;
/// A claim on a hub lasts this long without renewal (**[A] B1a-5**).
pub const STEWARD_TTL: Duration = Duration::from_secs(30 * 60);
/// After a role refusal the role acts wait this long (**[A] B1a-5**).
pub const ROLE_BACKOFF: Duration = Duration::from_secs(6 * 3600);
/// How long the planet definitions are kept (**[A] B1a-5**).
pub const DEFS_TTL: Duration = Duration::from_secs(3600);
/// How far from the planet's surface the launch is made (**[A] B1a-5**).
pub const LAUNCH_STANDOFF: f64 = LAUNCH_RANGE / 5.0;
/// After a refusal the target is left alone this long (**[A] B1a-7**).
pub const REFUSE_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// After a scout trip that learned nothing, the next waits this long
/// (**[A] B1a-7**).
pub const LEARN_RETRY: Duration = Duration::from_secs(15 * 60);
/// [`HAUL_MIN`] when the skyhook's definition has no cap (**[A] B1a-2**).
pub const HAUL_MIN_FALLBACK: u64 = 100;
/// Units of an unburned reagent worth hauling to the bay (**[A] B1a-3**).
pub const IDLE_STOCK: u64 = 100;

const PLANET_DEFS: &str = "GetAllDefinitionsRequest";
const HOOKS_BY_CORP: &str = "GetAllByCorporationRequest";
const ACTIVATE: &str = "ActivateRequest";
const FUEL_GET: &str = "GetRequest";
const FUEL_ADD: &str = "AddRequest";
const EXTRACT: &str = "ExtractReagentsToShip";

// ---- what the steward reads ----------------------------------------------------------

/// One planet definition (`planet.api.GetAllDefinitionsResponse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanetDef {
    pub planet: i64,
    pub power: u64,
    pub workforce: u64,
    /// The reagent type the planet gives, if any.
    pub reagent: Option<i64>,
}

/// One own skyhook (`skyhook.api.GetAllByCorporationResponse`). The record
/// carries no planet: [`Mind::placed`] and the park say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookView {
    pub id: i64,
    pub planet: Option<i64>,
    pub active: bool,
    pub reagent: Option<i64>,
    pub secured: u64,
    pub unsecured: u64,
    /// The unsecured capacity, from the definition entry.
    pub unsecured_cap: Option<u64>,
    /// Seen not to be in this system's park (the record lists the corp's
    /// skyhooks everywhere).
    pub elsewhere: bool,
}

impl HookView {
    /// Units in the skyhook.
    pub fn stock(&self) -> u64 {
        self.secured + self.unsecured
    }
}

/// The least worth a trip from `h` (**[A] B1a-2**).
pub fn haul_min(h: &HookView) -> u64 {
    h.unsecured_cap
        .map_or(HAUL_MIN_FALLBACK, |cap| (cap / 4).max(1))
}

/// Units of burn `STEWARD_DAYS` come to at `per_hour`.
pub fn steward_days_units(per_hour: u64) -> u64 {
    STEWARD_DAYS * 24 * per_hour
}

/// The bay: `(reagent type, amount, per hour)`.
pub type Bay = [(i64, u64, u64)];

/// Everything [`steward_plan`] reads.
pub struct StewardView<'a> {
    /// The planets of the held system, with their definitions (a planet
    /// that has none is not in this list: it gives nothing).
    pub planets: &'a [PlanetDef],
    /// Planets another corporation's skyhook stands over.
    pub skipped: &'a [i64],
    pub hooks: &'a [HookView],
    /// Own skyhooks still building (not switched on yet).
    pub building: &'a [i64],
    pub bay: &'a Bay,
    /// A skyhook item in the hold, if any.
    pub kit_cargo: Option<i64>,
    /// A skyhook item in the station hangar.
    pub kit_hangar: bool,
    /// A skyhook on sale where the pilot can reach it.
    pub ask_known: bool,
    pub skyhook_type: Option<i64>,
    /// Reagents in the hold: `(type, units)`.
    pub cargo_reagents: &'a [(i64, i64)],
    pub may_manage: bool,
    pub hold_free_m3: f64,
    pub item_m3: &'a dyn Fn(i64) -> f64,
    /// Targets refused lately (planets, skyhooks), left alone.
    pub refused: &'a [i64],
    /// The bay refused a deposit lately: no deposit, and no extraction.
    pub deposit_blocked: bool,
    /// The pilot is docked (the fetch and the want are docked acts).
    pub docked: bool,
}

/// One thing the steward does.
#[derive(Debug, Clone, PartialEq)]
pub enum StewardAct {
    /// Post a bid and a want for the kit product.
    Want(i64),
    /// Buy one from an ask in reach, carry it home and into the hold.
    Fetch {
        item_type: i64,
    },
    /// Fly over the planet and launch the item.
    Launch {
        item: i64,
        planet: i64,
    },
    Activate {
        skyhook: i64,
    },
    Extract {
        skyhook: i64,
        qty: i64,
    },
    /// Everything in the hold of this reagent into the bay.
    Deposit {
        reagent: i64,
        qty: i64,
    },
}

impl StewardAct {
    /// Acts done in space (the rest are done docked).
    pub fn in_space(&self) -> bool {
        !matches!(self, StewardAct::Want(_) | StewardAct::Fetch { .. })
    }
}

/// **What the steward does this pass** (pure), in the order it is done: the
/// launch, the switches, the deposits, the extractions, then (docked) the
/// fetch or the want of the next skyhook.
pub fn steward_plan(v: &StewardView<'_>) -> Vec<StewardAct> {
    let mut acts = Vec::new();
    let mut late = Vec::new();
    // Skyhooks of the corp that stand in this system (or may): the ones seen
    // elsewhere are not this plan's.
    let here: Vec<&HookView> = v.hooks.iter().filter(|h| !h.elsewhere).collect();
    // Placed on a planet of this system: the only ones acted on.
    let placed: Vec<&HookView> = here
        .iter()
        .copied()
        .filter(|h| {
            h.planet
                .is_some_and(|p| v.planets.iter().any(|d| d.planet == p))
        })
        .collect();
    // 1. A yield planet without our skyhook (own hooks whose planet is not
    //    known cover as many as they number).
    let unplaced = here.iter().filter(|h| h.planet.is_none()).count();
    let mut lacking: Vec<i64> = v
        .planets
        .iter()
        .map(|p| p.planet)
        .filter(|p| {
            !v.skipped.contains(p)
                && !v.refused.contains(p)
                && !here.iter().any(|h| h.planet == Some(*p))
        })
        .collect();
    lacking.truncate(lacking.len().saturating_sub(unplaced));
    if v.may_manage
        && let Some(&planet) = lacking.first()
    {
        if let Some(item) = v.kit_cargo {
            acts.push(StewardAct::Launch { item, planet });
        } else if v.docked
            && let Some(t) = v.skyhook_type
        {
            if v.kit_hangar || v.ask_known {
                late.push(StewardAct::Fetch { item_type: t });
            } else {
                late.push(StewardAct::Want(t));
            }
        }
    }
    // 2. An own, built, inactive skyhook is switched on.
    if v.may_manage {
        for h in placed
            .iter()
            .filter(|h| !h.active && !v.building.contains(&h.id) && !v.refused.contains(&h.id))
        {
            acts.push(StewardAct::Activate { skyhook: h.id });
        }
    }
    // 3. Reagents in the hold go to the bay first.
    if !v.deposit_blocked {
        for &(reagent, qty) in v.cargo_reagents.iter().filter(|(_, q)| *q > 0) {
            acts.push(StewardAct::Deposit { reagent, qty });
        }
    }
    // 4. A skyhook worth the trip, while the bay holds less than the days;
    //    no more than the bay lacks.
    let mut free = v.hold_free_m3;
    // What each reagent's bay still lacks, lowered by every take planned.
    let mut lacking_of: BTreeMap<i64, u64> = BTreeMap::new();
    for h in placed {
        let Some(reagent) = h.reagent else { continue };
        if v.deposit_blocked
            || v.refused.contains(&h.id)
            || h.stock() < haul_min(h)
            || v.cargo_reagents
                .iter()
                .any(|(t, q)| *t == reagent && *q > 0)
        {
            continue;
        }
        let (amount, per_hour) = v
            .bay
            .iter()
            .find(|(t, _, _)| *t == reagent)
            .map_or((0, 0), |&(_, a, p)| (a, p));
        let want = if per_hour > 0 {
            steward_days_units(per_hour)
        } else {
            IDLE_STOCK
        };
        let lack = *lacking_of
            .entry(reagent)
            .or_insert_with(|| want.saturating_sub(amount));
        if lack == 0 {
            continue;
        }
        let unit = (v.item_m3)(reagent);
        let room = if unit > 0.0 {
            (free / unit + 1e-9).floor() as i64
        } else {
            0
        };
        let qty = room.min(h.stock() as i64).min(lack as i64);
        if qty > 0 {
            free -= qty as f64 * unit;
            if let Some(l) = lacking_of.get_mut(&reagent) {
                *l = l.saturating_sub(qty as u64);
            }
            acts.push(StewardAct::Extract { skyhook: h.id, qty });
        }
    }
    acts.extend(late);
    acts
}

// ---- one steward per hub ---------------------------------------------------------------

/// A hub on one gateway: two servers in one process never collide.
type Key = (Option<SocketAddr>, i64);

static CLAIMS: Mutex<BTreeMap<Key, (i64, Instant)>> = Mutex::new(BTreeMap::new());

fn held_by_other_at(g: &BTreeMap<Key, (i64, Instant)>, key: Key, pilot: i64, now: Instant) -> bool {
    matches!(g.get(&key), Some((holder, at))
        if *holder != pilot && now.saturating_duration_since(*at) < STEWARD_TTL)
}

/// Whether another pilot holds the claim on `hub` at `now` (a look, no take).
pub fn held_by_other(gateway: Option<SocketAddr>, hub: i64, pilot: i64, now: Instant) -> bool {
    let g = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    held_by_other_at(&g, (gateway, hub), pilot, now)
}

/// Whether `pilot` is the steward of `hub` at `now`: the claim is taken when
/// free or lapsed (older than [`STEWARD_TTL`]), and renewed by its holder.
pub fn claim(gateway: Option<SocketAddr>, hub: i64, pilot: i64, now: Instant) -> bool {
    let mut g = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if held_by_other_at(&g, (gateway, hub), pilot, now) {
        return false;
    }
    g.insert((gateway, hub), (pilot, now));
    true
}

/// Let go of the claim on `hub` if `pilot` holds it.
pub fn release(gateway: Option<SocketAddr>, hub: i64, pilot: i64) {
    let mut g = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if g.get(&(gateway, hub)).is_some_and(|(p, _)| *p == pilot) {
        g.remove(&(gateway, hub));
    }
}

// ---- decoders (the server's own field layouts) -------------------------------------------

/// `planet.api.GetAllDefinitionsResponse`: `{1* {1 id(planet), 2 power, 3
/// workforce, 4 {2 definition, 3 id(reagent type)}}}`.
pub fn decode_defs(payload: &[u8]) -> Vec<PlanetDef> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|d| {
            Some(PlanetDef {
                planet: i64::try_from(gw::read_id(d, 1)?).ok()?,
                power: gw::uint(d, 2).unwrap_or(0),
                workforce: gw::uint(d, 3).unwrap_or(0),
                reagent: gw::bytes(d, 4)
                    .and_then(|r| gw::read_id(r, 3))
                    .and_then(|t| i64::try_from(t).ok()),
            })
        })
        .collect()
}

/// `skyhook.api.GetAllByCorporationResponse`: `{1* {1 id, 2 {1 id(type), 4
/// {1 secured, 2 unsecured}}, 3 active, 4 {1 id(type), 2 {.. 5 unsecured
/// cap}}}}`. The planet is not in the record.
pub fn decode_hooks(payload: &[u8]) -> Vec<HookView> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|r| {
            let id = i64::try_from(gw::read_id(r, 1)?).ok()?;
            let sim = gw::bytes(r, 2);
            let stock = sim.and_then(|s| gw::bytes(s, 4));
            Some(HookView {
                id,
                planet: None,
                active: gw::uint(r, 3) == Some(1),
                reagent: sim
                    .and_then(|s| gw::read_id(s, 1))
                    .and_then(|t| i64::try_from(t).ok()),
                secured: stock.and_then(|s| gw::uint(s, 1)).unwrap_or(0),
                unsecured: stock.and_then(|s| gw::uint(s, 2)).unwrap_or(0),
                unsecured_cap: gw::bytes(r, 4)
                    .and_then(|d| gw::bytes(d, 2))
                    .and_then(|d| gw::uint(d, 5)),
                elsewhere: false,
            })
        })
        .collect()
}

/// `hub.fuel.api.GetResponse`: `{1* {1 id(type), 2 amount, 3 per hour}, 2
/// stamp {1 seconds}}`. Each amount is stated at the stamp, so what the bay
/// burned since (`now_unix` minus the stamp, at the entry's rate) is taken
/// off.
pub fn decode_bay(payload: &[u8], now_unix: u64) -> Vec<(i64, u64, u64)> {
    let since = gw::bytes(payload, 2)
        .and_then(|t| gw::uint(t, 1))
        .map_or(0, |stamp| now_unix.saturating_sub(stamp));
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|e| {
            let per_hour = gw::uint(e, 3).unwrap_or(0);
            let burned = since.saturating_mul(per_hour) / 3600;
            Some((
                i64::try_from(gw::read_id(e, 1)?).ok()?,
                gw::uint(e, 2).unwrap_or(0).saturating_sub(burned),
                per_hour,
            ))
        })
        .collect()
}

/// A request body of one identifier: `{1 id(x)}`.
pub fn id_body(id: i64) -> Vec<u8> {
    let mut b = Vec::new();
    gw::put_bytes(&mut b, 1, &gw::id(id as u64));
    b
}

/// The `AddRequest` body: `{1 id(item), 2 amount, 3 id(hub)}`.
pub fn add_body(item: i64, qty: u64, hub: i64) -> Vec<u8> {
    let mut b = Vec::new();
    gw::put_bytes(&mut b, 1, &gw::id(item as u64));
    gw::put_uint(&mut b, 2, qty);
    gw::put_bytes(&mut b, 3, &gw::id(hub as u64));
    b
}

// ---- what a steward remembers ------------------------------------------------------------

/// A steward's memory (a field of [`super::Mind`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// Role acts wait until then.
    pub no_role_until: Option<Instant>,
    /// Own skyhook -> the planet it stands over (the record has none).
    pub placed: BTreeMap<i64, i64>,
    /// Own skyhooks launched here, and when (they build for a while).
    pub launched: BTreeMap<i64, Instant>,
    /// System -> its planets, learned from the park.
    pub planets: BTreeMap<i64, Vec<i64>>,
    /// Planets a launch was refused for ("already has a skyhook").
    pub skipped: BTreeSet<i64>,
    /// Own skyhooks seen not to be in the held system's park.
    pub elsewhere: BTreeSet<i64>,
    /// Targets (planets, skyhooks, the hub, a kit type) refused, and when.
    pub refused: BTreeMap<i64, Instant>,
    /// A scout trip is not made again before this.
    pub learn_retry: Option<Instant>,
}

type Defs = BTreeMap<Option<SocketAddr>, (Instant, Vec<PlanetDef>)>;

static DEFS: Mutex<Defs> = Mutex::new(BTreeMap::new());

// ---- the arm ---------------------------------------------------------------------------

/// A session that is dropped after the first `MISSING gateway`.
struct Session {
    gw: Option<Gw>,
    lost: bool,
}

impl Session {
    /// One request; `None` once the gateway is gone (no further request is
    /// sent in this pass).
    fn ask(&mut self, mind: &mut ExtrasMind, ty: &str, body: &[u8]) -> Option<(u32, Vec<u8>)> {
        let g = self.gw.as_mut()?;
        match g.ask(mind, ty, body) {
            Ok(r) => Some(r),
            Err(_) => {
                self.gw = None;
                self.lost = true;
                None
            }
        }
    }
}

fn is_role_text(text: &str) -> bool {
    text == Refused::NoRole.text()
}

/// A role refusal: this pilot waits for a role and gives up the claim, so a
/// pilot that has the role can take it.
fn role_refused(st: &mut Mind, gateway: Option<SocketAddr>, hub: i64, who: i64, now: Instant) {
    st.no_role_until = Some(now + ROLE_BACKOFF);
    release(gateway, hub, who);
}

/// **The steward arm** ([`super::ARMS`]): `Ok(None)` for a pilot that is not
/// the steward of its hub, or whose pass found nothing to do.
pub fn arm(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    duty: Duty,
) -> Result<Option<String>> {
    let Duty::Haul(hub) = duty else {
        return Ok(None);
    };
    let (Some(map), Some(content)) = (ctx.map, ctx.content) else {
        return Ok(None);
    };
    let Some(&(system, _)) = plan.fuel.iter().find(|(_, h)| *h == hub) else {
        return Ok(None);
    };
    let Some(who) = c.session().char_id() else {
        return Ok(None);
    };
    let now = clock::instant();
    let gateway = gw::gateway_of(c);
    // A pilot that waits for a role does not hold the claim.
    if m.sov.extras.steward.no_role_until.is_some_and(|t| now < t) {
        release(gateway, hub, who);
        return Ok(None);
    }
    if held_by_other(gateway, hub, who, now) {
        return Ok(None);
    }
    let Some(corp) = c.session().get_i64("corpid") else {
        return Ok(None);
    };
    let Some(g) = Gw::open(c, &mut m.sov.extras) else {
        return Ok(None);
    };
    let mut s = Session {
        gw: Some(g),
        lost: false,
    };
    let mut lines: Vec<String> = Vec::new();

    // The reads; the claim is taken once they have all been answered.
    let Some(defs) = read_defs(&mut s, &mut m.sov.extras, gateway, now) else {
        return Ok(None);
    };
    let Some((200, hooks_raw)) = s.ask(
        &mut m.sov.extras,
        &format!("{}{HOOKS_BY_CORP}", gw::SKYHOOK_PKG),
        &id_body(corp),
    ) else {
        return Ok(None);
    };
    let Some((200, bay_raw)) = s.ask(
        &mut m.sov.extras,
        &format!("{}{FUEL_GET}", gw::FUEL_PKG),
        &id_body(hub),
    ) else {
        return Ok(None);
    };
    if !claim(gateway, hub, who, now) {
        return Ok(None);
    }
    let mut hooks = decode_hooks(&hooks_raw);
    let bay = decode_bay(&bay_raw, clock::wall_unix_secs());
    {
        let st = &mut m.sov.extras.steward;
        st.refused
            .retain(|_, at| now.saturating_duration_since(*at) < REFUSE_BACKOFF);
        for h in &mut hooks {
            h.planet = st.placed.get(&h.id).copied();
            h.elsewhere = st.elsewhere.contains(&h.id);
        }
    }

    // The planets of the system and the place of every own skyhook come from
    // the park: a scout trip when something is unknown (not again for a
    // while when it learned nothing).
    let known = m.sov.extras.steward.planets.contains_key(&system);
    let unresolved = hooks.iter().any(|h| h.planet.is_none() && !h.elsewhere);
    let may_scout = m.sov.extras.steward.learn_retry.is_none_or(|t| now >= t);
    if (!known || unresolved) && may_scout {
        m.sov.extras.steward.learn_retry = Some(now + LEARN_RETRY);
        let here =
            c.session().solar_system_id2() == Some(system) && c.session().station_id().is_none();
        if !here {
            fly_to_system_noting(c, map, system)?;
        }
        if learn(c, &mut m.sov.extras.steward, system, &defs, &hooks) {
            m.sov.extras.steward.learn_retry = None;
        }
        let home = crate::sov_bot::go_home(c, m, map)?;
        let n = m
            .sov
            .extras
            .steward
            .planets
            .get(&system)
            .map_or(0, Vec::len);
        return Ok(Some(format!(
            "sov steward: scouted system {system}, {n} planets; {home}"
        )));
    }
    if !known {
        return Ok(None);
    }
    let st = &m.sov.extras.steward;
    let planets: Vec<PlanetDef> = defs
        .iter()
        .filter(|d| {
            st.planets
                .get(&system)
                .is_some_and(|ps| ps.contains(&d.planet))
        })
        .copied()
        .collect();
    let skipped: Vec<i64> = st.skipped.iter().copied().collect();
    let refused: Vec<i64> = st.refused.keys().copied().collect();
    let deposit_blocked = st.refused.contains_key(&hub);
    let building: Vec<i64> = st
        .launched
        .iter()
        .filter(|(_, at)| {
            clock::since(**at) < 2 * Duration::from_secs(eve_server::skyhook::ACTIVATION_S)
        })
        .map(|(id, _)| *id)
        .collect();
    let may_manage = true; // a pilot waiting for a role returned above

    let docked = c.session().station_id().is_some();
    let cargo = c.cargo()?;
    let hangar = if docked {
        c.hangar_items()?
    } else {
        Vec::new()
    };
    let skyhook_type = skyhook_type(content);
    let kit_cargo = cargo
        .iter()
        .find(|r| r.group_id == GROUP_SKYHOOK)
        .map(|r| r.item_id);
    let kit_hangar = hangar.iter().any(|r| r.group_id == GROUP_SKYHOOK);
    let cargo_reagents = reagents_in(&cargo);
    let volume = |t: i64| eve_server::item::find_in(content, t).map_or(0.0, |t| t.volume);
    let capacity = hold_capacity(c, content);
    let used: f64 = cargo
        .iter()
        .map(|r| r.quantity.max(1) as f64 * volume(r.type_id))
        .sum();
    let hold = (capacity - used).max(0.0);
    // Whether a skyhook is on sale in reach (only read when it matters).
    let lacks_planet = planets.iter().any(|p| {
        !skipped.contains(&p.planet)
            && !refused.contains(&p.planet)
            && !hooks
                .iter()
                .any(|h| !h.elsewhere && h.planet == Some(p.planet))
    });
    let needs_kit = may_manage && kit_cargo.is_none() && !kit_hangar && docked && lacks_planet;
    let ask_known = match (needs_kit, skyhook_type) {
        (true, Some(t)) => reachable_book(c, map, t)?.is_some(),
        _ => false,
    };
    let mut acts = steward_plan(&StewardView {
        planets: &planets,
        skipped: &skipped,
        hooks: &hooks,
        building: &building,
        bay: &bay,
        kit_cargo,
        kit_hangar,
        ask_known,
        skyhook_type,
        cargo_reagents: &cargo_reagents,
        may_manage,
        hold_free_m3: hold,
        item_m3: &volume,
        refused: &refused,
        deposit_blocked,
        docked,
    });
    // A skyhook the hull's hold cannot take is neither wanted nor bought.
    if let Some(t) = skyhook_type
        && volume(t) > capacity
        && acts
            .iter()
            .any(|a| matches!(a, StewardAct::Want(_) | StewardAct::Fetch { .. }))
    {
        acts.retain(|a| a.in_space());
        let st = &mut m.sov.extras.steward;
        if let std::collections::btree_map::Entry::Vacant(e) = st.refused.entry(t) {
            e.insert(now);
            lines.push(format!("MISSING a hull whose hold takes {} m3", volume(t)));
            if acts.is_empty() {
                return Ok(Some(format!("sov steward: {}", lines.join("; "))));
            }
        }
    }
    if acts.is_empty() {
        return Ok(None);
    }

    // The acts in space first, then home, then the docked ones.
    let space: Vec<&StewardAct> = acts.iter().filter(|a| a.in_space()).collect();
    if !space.is_empty() {
        fly_to_system_noting(c, map, system)?;
        learn(c, &mut m.sov.extras.steward, system, &defs, &hooks);
        for act in &space {
            let line = do_space(c, m, &mut s, hub, act, gateway)?;
            lines.push(line);
            // A role refusal ends the trip.
            if m.sov
                .extras
                .steward
                .no_role_until
                .is_some_and(|t| clock::instant() < t)
            {
                break;
            }
        }
        // Reagents taken are put into the bay on the same trip.
        if space
            .iter()
            .any(|a| matches!(a, StewardAct::Extract { .. }))
        {
            let cargo = c.cargo()?;
            for (reagent, qty) in reagents_in(&cargo) {
                lines.push(do_space(
                    c,
                    m,
                    &mut s,
                    hub,
                    &StewardAct::Deposit { reagent, qty },
                    gateway,
                )?);
            }
        }
        lines.push(crate::sov_bot::go_home(c, m, map)?);
    }
    for act in acts.iter().filter(|a| !a.in_space()) {
        lines.push(do_docked(c, map, &mut m.sov.extras.steward, act)?);
    }
    if s.lost {
        lines.push("MISSING gateway".to_owned());
    }
    Ok(Some(format!("sov steward: {}", lines.join("; "))))
}

fn skyhook_type(content: &eve_server::content::Content) -> Option<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .find(|t| t.group_id == GROUP_SKYHOOK)
        .map(|t| t.type_id)
}

/// The reagent stacks of `rows`, summed per type.
fn reagents_in(rows: &[ItemRow]) -> Vec<(i64, i64)> {
    let mut by: BTreeMap<i64, i64> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.group_id == GROUP_COLONY_REAGENTS) {
        *by.entry(r.type_id).or_default() += r.quantity.max(1);
    }
    by.into_iter().collect()
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

/// The planet definitions: the process's per gateway, kept [`DEFS_TTL`].
fn read_defs(
    s: &mut Session,
    mind: &mut ExtrasMind,
    gateway: Option<SocketAddr>,
    now: Instant,
) -> Option<Vec<PlanetDef>> {
    {
        let g = DEFS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, defs)) = g.get(&gateway)
            && now.saturating_duration_since(*at) < DEFS_TTL
        {
            return Some(defs.clone());
        }
    }
    let (status, payload) = s.ask(mind, &format!("{}{PLANET_DEFS}", gw::PLANET_PKG), &[])?;
    if status != 200 {
        return None;
    }
    let defs = decode_defs(&payload);
    DEFS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(gateway, (now, defs.clone()));
    Some(defs)
}

/// Fly to `system`; a refusal on the way is the error the turn turns into a
/// note.
fn fly_to_system_noting(c: &mut Conn, map: &crate::travel::Map, system: i64) -> Result<()> {
    crate::sov_bot::fly_to_system(c, map, system)
}

/// **Learn the park** (in the system): which defined planets are here, which
/// planet each own skyhook in the park stands over (the nearest one), and
/// which own skyhooks are not here at all. `false` when the pilot is not in
/// the system's park yet.
fn learn(c: &mut Conn, st: &mut Mind, system: i64, defs: &[PlanetDef], hooks: &[HookView]) -> bool {
    if c.session().solar_system_id2() != Some(system) || !c.world().has_state() {
        return false;
    }
    let world = c.world_now();
    let planets: Vec<(i64, [f64; 3])> = defs
        .iter()
        .filter_map(|d| world.get(d.planet).map(|b| (d.planet, b.motion.position)))
        .collect();
    // A system with no defined planet is known to have none.
    for h in hooks {
        let Some(ball) = world.get(h.id) else {
            if !st.placed.contains_key(&h.id) {
                st.elsewhere.insert(h.id);
            }
            continue;
        };
        st.elsewhere.remove(&h.id);
        if st.placed.contains_key(&h.id) {
            continue;
        }
        let near = planets
            .iter()
            .map(|(p, at)| {
                (
                    *p,
                    eve_server::space_state::distance(ball.motion.position, *at),
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((p, _)) = near {
            st.placed.insert(h.id, p);
        }
    }
    st.planets
        .insert(system, planets.iter().map(|(p, _)| *p).collect());
    true
}

/// Warp to `ball` and wait for the landing (a planet: its warp-in point is
/// far above the surface, so no follow is made).
fn arrive(c: &mut Conn, ball: i64, range: f64) -> Result<()> {
    let out = c.warp_to_item(ball, range)?;
    c.pump(Duration::from_millis(250))?;
    if out.refusal().is_none() {
        let deadline = clock::instant() + Duration::from_secs(120);
        while c.world_now().ego_ball().is_some_and(|b| b.warp_commanded()) {
            if clock::instant() >= deadline {
                return Err(Error::Refused("sov steward: still warping".into()));
            }
            c.pump(Duration::from_millis(250))?;
        }
    }
    Ok(())
}

fn do_space(
    c: &mut Conn,
    m: &mut Memory,
    s: &mut Session,
    hub: i64,
    act: &StewardAct,
    gateway: Option<SocketAddr>,
) -> Result<String> {
    let now = clock::instant();
    let who = c.session().char_id().unwrap_or(0);
    // A gateway act is not worth the flight once the session is gone.
    if s.gw.is_none()
        && matches!(
            act,
            StewardAct::Activate { .. } | StewardAct::Deposit { .. }
        )
    {
        return Ok("no gateway, act skipped".to_owned());
    }
    match act {
        StewardAct::Launch { item, planet } => {
            arrive(c, *planet, LAUNCH_STANDOFF)?;
            Ok(match gw::launch(c, *item)? {
                Ok(id) => {
                    let st = &mut m.sov.extras.steward;
                    st.placed.insert(id, *planet);
                    st.launched.insert(id, now);
                    format!("launched skyhook {id} over planet {planet}")
                }
                Err(text) => {
                    let st = &mut m.sov.extras.steward;
                    st.refused.insert(*planet, now);
                    if is_role_text(&text) {
                        role_refused(st, gateway, hub, who, now);
                    } else if text == Refused::HasSkyhook.text() {
                        st.skipped.insert(*planet);
                    }
                    format!("launch over planet {planet} refused: {text}")
                }
            })
        }
        StewardAct::Activate { skyhook } => {
            let ty = format!("{}{ACTIVATE}", gw::SKYHOOK_PKG);
            Ok(match s.ask(&mut m.sov.extras, &ty, &id_body(*skyhook)) {
                Some((200, _)) => format!("switched skyhook {skyhook} on"),
                Some((status, _)) => {
                    let st = &mut m.sov.extras.steward;
                    st.refused.insert(*skyhook, now);
                    // A role refusal only when the skyhook stands here and
                    // has had time to build (an unknown launch time counts as not built).
                    let built = st.launched.get(skyhook).is_some_and(|at| {
                        clock::since(*at)
                            >= 2 * Duration::from_secs(eve_server::skyhook::ACTIVATION_S)
                    });
                    let here = st.placed.contains_key(skyhook) && !st.elsewhere.contains(skyhook);
                    if status == 403 && here && built {
                        role_refused(st, gateway, hub, who, now);
                    }
                    format!("skyhook {skyhook} not switched on ({status})")
                }
                None => format!("skyhook {skyhook} not switched on (no gateway)"),
            })
        }
        StewardAct::Extract { skyhook, qty } => {
            crate::sov_bot::warp_near(c, *skyhook, TRANSFER_RANGE * RANGE_USE)?;
            let out = gw::component_call(
                c,
                *skyhook,
                gw::ORBITAL_SKYHOOK,
                EXTRACT,
                vec![PyValue::Int(FLAG_CARGO), PyValue::Int(*qty)],
            )?;
            Ok(match out.refusal() {
                None => format!("took up to {qty} reagents from skyhook {skyhook}"),
                Some(why) => {
                    m.sov.extras.steward.refused.insert(*skyhook, now);
                    format!("skyhook {skyhook} refused the take: {why}")
                }
            })
        }
        StewardAct::Deposit { reagent, .. } => {
            if m.sov.extras.steward.refused.contains_key(&hub) {
                return Ok("deposit held back (the bay refused lately)".to_owned());
            }
            crate::sov_bot::warp_near(c, hub, TRANSFER_RANGE * RANGE_USE)?;
            let stacks: Vec<ItemRow> = c
                .cargo()?
                .into_iter()
                .filter(|r| r.type_id == *reagent && r.group_id == GROUP_COLONY_REAGENTS)
                .collect();
            let mut put = 0u64;
            let mut notes = Vec::new();
            for r in stacks {
                let qty = r.quantity.max(1) as u64;
                let ty = format!("{}{FUEL_ADD}", gw::FUEL_PKG);
                match s.ask(&mut m.sov.extras, &ty, &add_body(r.item_id, qty, hub)) {
                    Some((200, _)) => put += qty,
                    Some((status, _)) => {
                        m.sov.extras.steward.refused.insert(hub, now);
                        notes.push(format!("{status}"));
                        break;
                    }
                    None => {
                        notes.push("no gateway".to_owned());
                        break;
                    }
                }
            }
            Ok(if notes.is_empty() {
                format!("deposited {put} of reagent {reagent} into hub {hub}")
            } else {
                format!(
                    "deposited {put} of reagent {reagent} into hub {hub} (refused: {})",
                    notes.join(", ")
                )
            })
        }
        StewardAct::Want(_) | StewardAct::Fetch { .. } => Ok(String::new()),
    }
}

fn do_docked(
    c: &mut Conn,
    map: &crate::travel::Map,
    st: &mut Mind,
    act: &StewardAct,
) -> Result<String> {
    match act {
        StewardAct::Want(product) => want(c, *product),
        StewardAct::Fetch { item_type } => fetch(c, map, st, *item_type),
        _ => Ok(String::new()),
    }
}

/// The want on the board and a standing bid for the skyhook (docked).
fn want(c: &mut Conn, product: i64) -> Result<String> {
    super::kit::want(product);
    let wallet = crate::acquire::spendable(c)? * WALLET_SHARE;
    let posted = soft(crate::hull_choice::bid_for(c, product, 1, wallet))?;
    Ok(format!(
        "MISSING skyhook {product} ({})",
        if posted == Some(true) {
            "bid posted"
        } else {
            "bid stands"
        }
    ))
}

pub(super) fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// The book of `ty` without the pilot's own orders, when it holds an ask in
/// reach: at the docked station, or at a seller [`crate::travel::sources`]
/// names within the wallet's share.
pub(super) fn reachable_book(
    c: &mut Conn,
    map: &crate::travel::Map,
    ty: i64,
) -> Result<Option<crate::verbs::Book>> {
    let Some(station) = c.session().station_id() else {
        return Ok(None);
    };
    let Some(book) = soft(c.market_orders(ty))? else {
        return Ok(None);
    };
    let own = crate::capital::own_ids_soft(c)?;
    let book = crate::verbs::Book {
        asks: book
            .asks
            .into_iter()
            .filter(|a| !a.bid && a.vol_remaining > 0 && !own.contains(&a.order_id))
            .collect(),
        bids: Vec::new(),
    };
    if book.asks.iter().any(|a| a.station_id == station) {
        return Ok(Some(book));
    }
    let cap = crate::acquire::spendable(c)? * WALLET_SHARE;
    let Some(system) = map.system_of(station) else {
        return Ok(None);
    };
    let trips = crate::travel::Trips {
        map,
        floor: crate::sov_bot::SOV_FLOOR,
    };
    Ok(
        (!crate::travel::sources(&book, trips, system, station, Some(cap), 1).is_empty())
            .then_some(book),
    )
}

/// **Fetch one skyhook** (docked): from the hangar, else the asks here, else
/// the nearest seller in reach (a trip home again), then into the hold. The
/// want goes on the board; a bid is rested only when nothing is on sale.
fn fetch(c: &mut Conn, map: &crate::travel::Map, st: &mut Mind, ty: i64) -> Result<String> {
    let Some(station) = c.session().station_id() else {
        return Ok("fetch: not docked".to_owned());
    };
    super::kit::want(ty);
    let mut notes: Vec<String> = Vec::new();
    let owned = |c: &mut Conn| -> Result<Option<ItemRow>> {
        Ok(c.hangar_items()?
            .into_iter()
            .find(|r| r.type_id == ty && r.group_id == GROUP_SKYHOOK))
    };
    if owned(c)?.is_none()
        && let Some(book) = reachable_book(c, map, ty)?
    {
        let budget = crate::acquire::spendable(c)? * WALLET_SHARE;
        if book.asks.iter().any(|a| a.station_id == station) {
            let got = crate::capital::buy_units(c, &book, ty, 1, budget)?;
            notes.push(format!("bought {got} skyhook here"));
        } else if let Some(system) = map.system_of(station) {
            let trips = crate::travel::Trips {
                map,
                floor: crate::sov_bot::SOV_FLOOR,
            };
            if let Some(src) =
                crate::travel::sources(&book, trips, system, station, Some(budget), 1)
                    .into_iter()
                    .next()
            {
                match soft(crate::travel::fetch(
                    c,
                    trips,
                    &src,
                    ty,
                    1,
                    Some(budget),
                    crate::travel::Haul::Cargo,
                ))? {
                    Some(f) => notes.push(f.summary()),
                    None => {
                        // A failed trip backs off.
                        st.refused.insert(ty, clock::instant());
                        notes.push("the trip failed".to_owned());
                    }
                }
            }
        }
    }
    let Some(row) = owned(c)? else {
        // Nothing to carry: the bid stands for a builder's ask.
        return want(c, ty);
    };
    let loaded = c.load_cargo(
        row.item_id,
        (!row.singleton && row.quantity > 1).then_some(1),
    );
    Ok(match loaded {
        Ok(out) => match out.refusal() {
            None => format!("skyhook {} in the hold ({})", row.item_id, notes.join("; ")),
            Some(why) => format!("MISSING a hull whose hold takes the skyhook ({why})"),
        },
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => format!("MISSING a hull whose hold takes the skyhook ({e})"),
    })
}
