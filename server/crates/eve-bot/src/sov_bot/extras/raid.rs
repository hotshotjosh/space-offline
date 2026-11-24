//! **The raiders** (SOVX-RAID, #221 B2): bot alliances steal the unsecured
//! reagents of an enemy's skyhook while its theft window is open, and guard
//! their own silos against the same.
//!
//! **Where it plugs in.** The campaign planner ([`crate::sov_bot::plan`])
//! gains the candidate [`Kind::Raid`] from the theft windows [`read_raids`]
//! reads over the gateway (the two map-filter requests the client's map
//! sends), and [`Kind::Rescue`] from the alarms [`watch`] raises. The
//! planner's own guard rule does the rest: a campaign with a target gives a
//! [`Duty::Guard`] to every pilot with guns, so a raid has guards beside it
//! whenever the alliance has any. The raiders are the [`Duty::Idle`] pilots
//! ([`arm`]).
//!
//! **What a raider does**, every act a client verb: fly to the skyhook's
//! system, warp in at half the link range ([`MAX_LINK_RANGE`]) of the silo
//! that stands while the window is open, call the silo's component
//! `InitiateLink` (as the silo's own context menu does) only when in range
//! and in an allowed hull, hold range until the silo's slim says the link is
//! over, then fly the reagents home: into an own hub's reagent bay when the
//! alliance has one, else onto the market. **Nothing is made**: the hold only
//! gains what the server's link moves out of the victim's skyhook, and the
//! raider's cargo is counted before and after.
//!
//! **The defence** is [`watch`]: a pilot of the alliance that sees a silo of
//! its own with a link running whose ship is not an own alliance's tells the
//! hall ([`crate::sov_bot::SovHall::alarm`]), and the planner makes that the
//! first thing to defend ([`crate::sov_bot::DEFEND_VALUE`]).
//!
//! **[A] B2-1**: the server lets any hull link
//! (`skyhook_theft::initiate_at`, spec L9.3), so the bots' rule is ours: a
//! frigate-class hull ([`RAIDER_GROUPS`]) with [`HOLD_MIN_M3`] free in the
//! hold. Falsified by TQ naming a hull rule (then [`hull_allowed`] reads
//! it), or by raiders that cannot carry what a link moves.
//!
//! **[A] B2-2**: [`RAIDER_GROUPS`] are the inventory groups of the
//! frigate-class hulls a thief flies: frigate 25, rookie 237, assault 324,
//! covert ops 830, interceptor 831, stealth bomber 834, electronic attack 893,
//! expedition 1283, logistics frigate 1527 (no shared const exists in the
//! tree). Falsified by a content table whose frigate groups differ (the arm
//! then says `MISSING an allowed hull`).
//!
//! **[A] B2-3**: [`HOLD_MIN_M3`] is 50 m3. Falsified by links that move
//! nothing for want of room.
//!
//! **[A] B2-4**: [`HOLD_EXTRA`] is 90 s, the wall seconds a raider waits
//! past the link's duration for the slim to say `COMPLETED`. Falsified by a
//! raider that gives up on a link that completes later.
//!
//! **[A] B2-5**: the planner takes a skyhook's owner to be the alliance that
//! holds its system (`stationSvc.GetAllianceSystems`); the map filter names
//! no owner, and a skyhook stays with its corp when sov flips until
//! `AcquireSkyhooks`. So the raider checks again at the silo ([`owner_gate`]):
//! the silo's slim names the corp (`sk.corp`, `skyhook_theft::slim`); our own
//! corp is never linked, and where the skyhook ball's slim names the corp's
//! alliance the own, mate and war tests run on that alliance. A corp with no
//! alliance on the slim falls back to the holder's. Falsified by raids on a
//! blue or own skyhook (in a soak note), or by a skyhook slim that never
//! carries its alliance (then the fallback decides every time).
//!
//! **[A] B2-7**: an alarm ([`watch`]) names a silo as ours when its slim's
//! corp is our corp, or the skyhook ball's or the silo's slim names our
//! alliance. A silo of another corp of our alliance is seen only where the
//! slim carries the alliance. Falsified by a link on such a silo that raises
//! no alarm.
//!
//! **[A] B2-6**: a raider holds its place and does not flee (the guards
//! fight). Falsified by raiders lost in soaks without delivering.

use super::gw::{self, Gw, LINK_WITH_SHIP};
use super::steward::{self, add_body, soft};
use crate::brain::Memory;
use crate::clock;
use crate::sov_bot::{
    Ctx, Duty, Kind, Plan, RANGE_USE, RaidView, candidates, fly_to_system, warp_near,
};
use crate::verbs::{ItemRow, SellLine};
use crate::{Conn, Error, Result};
use eve_server::skyhook::{TRANSFER_RANGE, filetime};
use eve_server::skyhook_theft::{
    LINK_DURATION_S, LINKSTATE_COMPLETED, LINKSTATE_IDLE, LINKSTATE_RUNNING, MAX_LINK_RANGE,
};
use eve_server::sov_resources::GROUP_COLONY_REAGENTS;
use std::collections::BTreeMap;
use std::time::Duration;

/// The inventory groups of the hulls a raider flies (**[A] B2-2**).
pub const RAIDER_GROUPS: [i64; 9] = [25, 237, 324, 830, 831, 834, 893, 1283, 1527];
/// The silo's link states as the slim's integers (`linkWithShip.py` **[V]**).
pub const STATE_IDLE: i64 = LINKSTATE_IDLE as i64;
pub const STATE_RUNNING: i64 = LINKSTATE_RUNNING as i64;
pub const STATE_COMPLETED: i64 = LINKSTATE_COMPLETED as i64;
/// Free hold a raider needs to set out (**[A] B2-3**), m3.
pub const HOLD_MIN_M3: f64 = 50.0;
/// Wall seconds past the link's duration the raider waits (**[A] B2-4**).
pub const HOLD_EXTRA: u64 = 90;
/// How long a refused skyhook is left alone.
pub const REFUSE_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// The most systems one planning pass asks the gateway about.
pub const RAID_SYSTEMS_MAX: usize = 6;

const SYSTEMS_WITH: &str = "GetSolarSystemsWithTheftVulnerableSkyhooksRequest";
const IN_SYSTEM: &str = "GetTheftVulnerableSkyhooksInSolarSystemRequest";
const INITIATE: &str = "InitiateLink";
const FUEL_ADD: &str = "AddRequest";

// ---- the pure core -------------------------------------------------------------------------

/// Whether a hull of `group` may raid (**[A] B2-1**).
pub fn hull_allowed(group: i64) -> bool {
    RAIDER_GROUPS.contains(&group)
}

/// What [`may_link`] reads.
#[derive(Debug, Clone, Copy)]
pub struct LinkView {
    /// The raider's hull is allowed ([`hull_allowed`]).
    pub hull_ok: bool,
    /// The gap from the raider to the silo's surface, m; `None`: unknown.
    pub gap: Option<f64>,
    /// The silo's link state from its slim; `None`: no silo on the grid.
    pub silo: Option<i64>,
    /// Free hold, m3.
    pub hold_free: f64,
    /// The window's end and now, `blue` ticks; `None` for the end when the
    /// plan does not know it.
    pub closes: Option<i64>,
    pub now: i64,
    /// The silo's owner passed [`owner_gate`].
    pub owner_ok: bool,
}

/// Seconds beyond a link's duration a window must still have to start one
/// (**[A] B2-8**). Falsified by links that break at the close (too short) or
/// raiders that never start in a two-hour window (too long).
pub const LINK_MARGIN_S: i64 = 120;

/// Whether a window ending at `closes` (ticks) still has room for a whole
/// link at `now` (pure).
pub fn window_allows(closes: i64, now: i64) -> bool {
    closes - now >= (LINK_DURATION_S as i64 + LINK_MARGIN_S) * crate::sov_bot::TICKS
}

/// Why the owner of a silo may not be raided, if so (pure). `corp` is the
/// silo slim's corp, `alliance` the alliance the skyhook's slim names,
/// `holder` the system's holder (the fallback). `raidable` is the planner's
/// own test ([`crate::sov_bot::raidable`]) on an alliance.
pub fn owner_gate(
    corp: Option<i64>,
    own_corp: i64,
    alliance: Option<i64>,
    holder: Option<i64>,
    raidable: &dyn Fn(i64) -> bool,
) -> std::result::Result<(), &'static str> {
    if corp == Some(own_corp) {
        return Err("the skyhook is our corp's");
    }
    match alliance.or(holder) {
        Some(a) if raidable(a) => Ok(()),
        Some(_) => Err("the skyhook's alliance is not an enemy"),
        None => Err("the skyhook has no known owner"),
    }
}

/// **Whether to call `InitiateLink` now** (pure): in an allowed hull, an
/// owner that may be raided, a silo that is idle, in range, a window with
/// room for the link, room in the hold.
pub fn may_link(v: &LinkView) -> bool {
    v.hull_ok
        && v.owner_ok
        && v.closes.is_none_or(|c| window_allows(c, v.now))
        && v.silo == Some(STATE_IDLE)
        && v.gap.is_some_and(|g| g <= MAX_LINK_RANGE)
        && v.hold_free >= HOLD_MIN_M3
}

/// Whether a link in `state` is over for the raider (pure): completed, or
/// back to idle or disabled; a silo gone is over too (`None`).
pub fn link_over(state: Option<i64>) -> bool {
    !matches!(state, Some(STATE_RUNNING))
}

/// Whether a silo's slim shows a thief of another alliance linking (pure):
/// the link runs and its ship's alliance is not `me`'s. A ship the park does
/// not show counts as a stranger.
pub fn is_theft(link: Option<(i64, Option<i64>)>, linker_alliance: Option<i64>, me: i64) -> bool {
    matches!(link, Some((STATE_RUNNING, _))) && linker_alliance != Some(me)
}

/// Where the reagents go (pure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposal {
    /// Nothing was delivered.
    Nothing,
    /// Into this own hub's reagent bay: `(system, hub)`.
    Deposit(i64, i64),
    /// On the market, at home.
    Sell,
}

/// **What to do with `delivered` units** (pure): an own hub takes them, else
/// they are sold. Never more than the hold gained.
pub fn disposal(delivered: i64, hubs: &[(i64, i64)]) -> Disposal {
    if delivered <= 0 {
        return Disposal::Nothing;
    }
    match hubs.first() {
        Some(&(system, hub)) => Disposal::Deposit(system, hub),
        None => Disposal::Sell,
    }
}

/// The units of `rows` that are reagents (pure).
pub fn reagent_units(rows: &[ItemRow]) -> i64 {
    rows.iter()
        .filter(|r| r.group_id == GROUP_COLONY_REAGENTS)
        .map(|r| r.quantity.max(1))
        .sum()
}

/// A `Timestamp` message `{1 seconds}` as unix seconds.
fn seconds(b: &[u8], field: u32) -> Option<u64> {
    gw::uint(gw::bytes(b, field)?, 1)
}

/// `GetSolarSystemsWithTheftVulnerableSkyhooksResponse`: `{1* id}`.
pub fn decode_systems(payload: &[u8]) -> Vec<i64> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|m| gw::uint(m, 1).and_then(|n| i64::try_from(n).ok()))
        .collect()
}

/// `GetTheftVulnerableSkyhooksInSolarSystemResponse`: `{1* {1 id skyhook, 2
/// id planet, 3 end, 4 start}}` -> `(skyhook, opens, closes)` in unix seconds.
pub fn decode_windows(payload: &[u8]) -> Vec<(i64, u64, u64)> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|m| {
            Some((
                i64::try_from(gw::read_id(m, 1)?).ok()?,
                seconds(m, 4)?,
                seconds(m, 3)?,
            ))
        })
        .collect()
}

/// **What to sell** (pure): of the hangar's reagent stacks `(item, units)`,
/// at most `budget` units in all, the stacks just unloaded first. Never more
/// than the budget, so a stack that was in the hangar before is not sold.
pub fn sell_quota(stacks: &[(i64, i64)], unloaded: &[i64], budget: i64) -> Vec<(i64, i64)> {
    let mut left = budget.max(0);
    let mut out = Vec::new();
    let first = stacks.iter().filter(|(i, _)| unloaded.contains(i));
    let rest = stacks.iter().filter(|(i, _)| !unloaded.contains(i));
    for &(item, qty) in first.chain(rest) {
        let take = qty.min(left);
        if take > 0 {
            out.push((item, take));
            left -= take;
        }
    }
    out
}

// ---- reading the windows -------------------------------------------------------------------

/// **The theft windows in view** (the planner's input): the systems that
/// hold a vulnerable skyhook, those near `home` first, each asked for its
/// windows. Empty without a gateway. The owner is the system's holder
/// (**[A] B2-5**, re-checked at the silo). The server's map filter answers
/// only windows open now (`skyhook::vulnerability`), so a window that has not
/// opened never arises from this data; the planner's `Shut` case is for
/// callers with a schedule.
pub fn read_raids(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    home: i64,
    held: &BTreeMap<i64, i64>,
) -> Vec<RaidView> {
    let Some(mut g) = Gw::open(c, &mut m.sov.extras) else {
        return Vec::new();
    };
    let ty = format!("{}{SYSTEMS_WITH}", gw::SKYHOOK_PKG);
    let Ok((200, payload)) = g.ask(&mut m.sov.extras, &ty, &[]) else {
        return Vec::new();
    };
    let open = decode_systems(&payload);
    let mut out = Vec::new();
    let near: Vec<i64> = candidates(map, home, held)
        .into_iter()
        .map(|(s, _)| s)
        .filter(|s| open.contains(s))
        .take(RAID_SYSTEMS_MAX)
        .collect();
    for system in near {
        let ty = format!("{}{IN_SYSTEM}", gw::SKYHOOK_PKG);
        let body = steward::id_body(system);
        let payload = match g.ask(&mut m.sov.extras, &ty, &body) {
            Ok((200, p)) => p,
            // One system's refusal leaves the others; a lost gateway ends it.
            Ok(_) => continue,
            Err(_) => break,
        };
        for (skyhook, opens, closes) in decode_windows(&payload) {
            out.push(RaidView {
                system,
                skyhook,
                owner: held.get(&system).copied(),
                opens: filetime(opens),
                closes: filetime(closes),
            });
        }
    }
    out
}

// ---- the defence ---------------------------------------------------------------------------

/// **The steward's eyes** ([`super::ARMS`], first; always `Ok(None)`): a
/// pilot of an alliance in space, on a grid with a silo of its own corp (or
/// whose skyhook or silo slim names its alliance, **[A] B2-7**) with a
/// stranger's link running, tells the hall. The planner then sends the
/// guards ([`Kind::Rescue`]).
pub fn watch(
    c: &mut Conn,
    _m: &mut Memory,
    ctx: &Ctx<'_>,
    _plan: &Plan,
    _duty: Duty,
) -> Result<Option<String>> {
    if c.session().station_id().is_some() || !c.world().has_state() {
        return Ok(None);
    }
    let (Some(me), Some(corp), Some(system)) = (
        c.session().get_i64("allianceid"),
        c.session().get_i64("corpid"),
        c.session().solar_system_id2(),
    ) else {
        return Ok(None);
    };
    let world = c.world_now();
    let alarms: Vec<i64> = world
        .balls()
        .filter_map(|b| {
            let s = b.slim?;
            let skyhook = s.silo_skyhook?;
            let hook_alliance = world
                .get(skyhook)
                .and_then(|h| h.slim)
                .and_then(|h| h.alliance_id);
            let own = s.corp_id == Some(corp)
                || s.owner_id == Some(corp)
                || s.alliance_id == Some(me)
                || hook_alliance == Some(me);
            let linker = s
                .link
                .and_then(|(_, ship)| ship)
                .and_then(|ship| world.get(ship))
                .and_then(|t| t.slim)
                .and_then(|s| s.alliance_id);
            (own && is_theft(s.link, linker, me)).then_some(skyhook)
        })
        .collect();
    for skyhook in alarms {
        ctx.hall.alarm(me, system, skyhook, clock::instant());
    }
    Ok(None)
}

// ---- the raider ----------------------------------------------------------------------------

/// A raider's memory (a field of [`super::Mind`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    /// Skyhooks refused, and when.
    pub refused: BTreeMap<i64, std::time::Instant>,
}

/// A silo on the grid, as its slim shows it.
#[derive(Debug, Clone, Copy)]
struct Silo {
    ball: i64,
    state: Option<i64>,
    corp: Option<i64>,
}

/// The silo of `skyhook` on the grid.
fn silo_of(c: &mut Conn, skyhook: i64) -> Option<Silo> {
    c.world_now().balls().find_map(|b| {
        let s = b.slim?;
        (s.silo_skyhook == Some(skyhook)).then_some(Silo {
            ball: b.ball_id,
            state: s.link.map(|l| l.0),
            corp: s.corp_id.or(s.owner_id),
        })
    })
}

fn gap_to(c: &mut Conn, ball: i64) -> Option<f64> {
    let w = c.world_now();
    w.surface_distance(w.ego?, ball)
}

/// The active hull's type.
fn hull_type(c: &mut Conn) -> Option<i64> {
    match c.session().station_id() {
        Some(_) => {
            let ship = c.session().ship_id();
            c.hangar_items()
                .ok()?
                .into_iter()
                .find(|r| Some(r.item_id) == ship)
                .map(|r| r.type_id)
        }
        None => c.world_now().ego_ball().and_then(|b| b.type_id()),
    }
}

/// The alliance the skyhook ball's slim names, if the park shows it.
fn hook_alliance(c: &mut Conn, skyhook: i64) -> Option<i64> {
    c.world_now()
        .get(skyhook)
        .and_then(|b| b.slim)
        .and_then(|s| s.alliance_id)
}

/// **The raider's arm** ([`super::ARMS`]): an [`Duty::Idle`] pilot of an
/// alliance whose plan is a raid. `Ok(None)` for every other pilot or plan.
/// Reagents already in the hold (a delivery of an earlier link that was not
/// disposed of) are disposed of first.
pub fn arm(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    duty: Duty,
) -> Result<Option<String>> {
    if duty != Duty::Idle {
        return Ok(None);
    }
    let (Some(map), Some(content)) = (ctx.map, ctx.content) else {
        return Ok(None);
    };
    // Stranded reagents are real server deliveries: they go first, whatever
    // the plan says now.
    let stranded = reagent_units(&c.cargo()?);
    if stranded > 0 {
        let line = dispose(c, m, map, stranded, &plan.fuel)?;
        return Ok(Some(format!("sov raid: stranded {line}")));
    }
    let Some(t) = plan.target.as_ref().filter(|t| t.kind == Kind::Raid) else {
        return Ok(None);
    };
    let now = clock::instant();
    m.sov
        .extras
        .raid
        .refused
        .retain(|_, at| clock::since(*at) < REFUSE_BACKOFF);
    if m.sov.extras.raid.refused.contains_key(&t.hub_id) {
        return Ok(None);
    }
    // A hull or a hold that cannot raid is the pilot's to change: no backoff
    // on the skyhook.
    let hull_ok = hull_type(c)
        .and_then(|h| eve_server::item::find_in(content, h))
        .is_some_and(|t| hull_allowed(t.group_id));
    if !hull_ok {
        return Ok(Some(
            "sov raid: MISSING an allowed hull (a frigate-class hull)".to_owned(),
        ));
    }
    let capacity = steward::hold_capacity(c, content);
    let used: f64 = c
        .cargo()?
        .iter()
        .map(|r| {
            r.quantity.max(1) as f64
                * eve_server::item::find_in(content, r.type_id).map_or(0.0, |t| t.volume)
        })
        .sum();
    let hold_free = (capacity - used).max(0.0);
    if hold_free < HOLD_MIN_M3 {
        return Ok(Some(
            "sov raid: MISSING room in the hold for what a link moves".to_owned(),
        ));
    }

    fly_to_system(c, map, t.system)?;
    // Before the window the silo is not there: wait beside the skyhook.
    let Some(seen) = silo_of(c, t.hub_id) else {
        if c.world().get(t.hub_id).is_some() {
            warp_near(c, t.hub_id, MAX_LINK_RANGE * RANGE_USE)?;
        }
        return Ok(Some(format!(
            "sov raid: no silo at skyhook {} yet, waiting",
            t.hub_id
        )));
    };
    warp_near(c, seen.ball, MAX_LINK_RANGE * RANGE_USE)?;
    let silo = silo_of(c, t.hub_id).unwrap_or(seen);
    // A silo already linked is not a refusal: the pilot stands by (the
    // guards fight) and the next pass looks again.
    if silo.state == Some(STATE_RUNNING) {
        return Ok(Some(format!(
            "sov raid: skyhook {} is already linked, standing by",
            t.hub_id
        )));
    }
    // The skyhook's own corp, not the system's holder, decides (B2-5).
    let (me, own_corp) = (
        c.session().get_i64("allianceid").unwrap_or(0),
        c.session().get_i64("corpid").unwrap_or(0),
    );
    let alliance = hook_alliance(c, t.hub_id);
    let relation = |a: i64| match ctx.society {
        Some(h) => h.lock().coalition_relation(me, a),
        None => ctx.relation.map_or(0.0, |f| f(me, a)),
    };
    let bloc_enemy = |a: i64| ctx.society.is_some_and(|h| h.lock().bloc_enemy(me, a));
    let bloc_mate = |a: i64| ctx.society.is_some_and(|h| h.lock().bloc_mate(me, a));
    let gate = owner_gate(silo.corp, own_corp, alliance, t.holder, &|a| {
        crate::sov_bot::raidable(me, a, &relation, &bloc_enemy, &bloc_mate)
    });
    let closes = match t.phase {
        crate::sov_bot::Phase::Open { closes } => Some(closes),
        _ => None,
    };
    let view = LinkView {
        hull_ok,
        gap: gap_to(c, silo.ball),
        silo: silo.state,
        hold_free,
        closes,
        now: eve_server::pyutil::filetime_now(),
        owner_ok: gate.is_ok(),
    };
    if !may_link(&view) {
        m.sov.extras.raid.refused.insert(t.hub_id, now);
        return Ok(Some(format!(
            "sov raid: not linking skyhook {} ({}; silo {:?}, gap {:?})",
            t.hub_id,
            gate.err().unwrap_or("window, range or state"),
            view.silo,
            view.gap
        )));
    }
    let out = gw::component_call(c, silo.ball, LINK_WITH_SHIP, INITIATE, vec![])?;
    if let Some(why) = out.refusal() {
        m.sov.extras.raid.refused.insert(t.hub_id, now);
        return Ok(Some(format!(
            "sov raid: skyhook {} refused the link: {why}",
            t.hub_id
        )));
    }
    let mut lines = vec![format!(
        "linking silo {} of skyhook {}",
        silo.ball, t.hub_id
    )];

    // Hold range until the silo says the link is over. A refusal inside the
    // wait still leaves the hold to be disposed of.
    match hold_link(c, t.hub_id, silo.ball) {
        Ok(l) => lines.extend(l),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => lines.push(format!("hold ended: {e}")),
    }

    // What the hold gained is what the server's link moved; nothing else.
    let delivered = reagent_units(&c.cargo()?);
    lines.push(format!("{delivered} reagents in the hold"));
    if delivered > 0 {
        lines.push(dispose(c, m, map, delivered, &plan.fuel)?);
    }
    Ok(Some(format!("sov raid: {}", lines.join("; "))))
}

/// Wait out the link beside the silo: until the slim says completed or over,
/// the silo is gone, or the wall deadline passes.
fn hold_link(c: &mut Conn, skyhook: i64, silo: i64) -> Result<Vec<String>> {
    let deadline = clock::wall_unix_secs() + LINK_DURATION_S + HOLD_EXTRA;
    loop {
        c.pump(Duration::from_secs(3))?;
        if clock::wall_unix_secs() >= deadline {
            return Ok(vec!["link wait over".to_owned()]);
        }
        match silo_of(c, skyhook).map(|s| s.state) {
            Some(Some(s)) if s == STATE_COMPLETED => {
                return Ok(vec!["link completed".to_owned()]);
            }
            Some(s) if !link_over(s) => {
                if gap_to(c, silo).is_none_or(|g| g > MAX_LINK_RANGE * 0.9) {
                    warp_near(c, silo, MAX_LINK_RANGE * RANGE_USE)?;
                }
            }
            Some(_) => return Ok(vec!["link ended".to_owned()]),
            None => return Ok(vec!["silo gone".to_owned()]),
        }
    }
}

/// **Dispose of `units` reagents in the hold** ([`disposal`]): an own hub's
/// bay, else the market; at most `units` are sold.
fn dispose(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    units: i64,
    hubs: &[(i64, i64)],
) -> Result<String> {
    match disposal(units, hubs) {
        Disposal::Nothing => Ok(String::new()),
        Disposal::Deposit(system, hub) => deposit(c, m, map, system, hub, units),
        Disposal::Sell => sell_home(c, m, map, units),
    }
}

/// **Put the hold's reagents into an own hub's bay** (the steward's
/// `AddRequest`), then home; whatever the bay refuses is sold at home.
fn deposit(
    c: &mut Conn,
    m: &mut Memory,
    map: &crate::travel::Map,
    system: i64,
    hub: i64,
    units: i64,
) -> Result<String> {
    fly_to_system(c, map, system)?;
    if c.world().get(hub).is_none() {
        let sold = sell_home(c, m, map, units)?;
        return Ok(format!("hub {hub} not on the grid; {sold}"));
    }
    warp_near(c, hub, TRANSFER_RANGE * RANGE_USE)?;
    let stacks: Vec<ItemRow> = c
        .cargo()?
        .into_iter()
        .filter(|r| r.group_id == GROUP_COLONY_REAGENTS)
        .collect();
    let mut put = 0u64;
    if let Some(mut g) = Gw::open(c, &mut m.sov.extras) {
        for r in stacks {
            let qty = r.quantity.max(1) as u64;
            let ty = format!("{}{FUEL_ADD}", gw::FUEL_PKG);
            match g.ask(&mut m.sov.extras, &ty, &add_body(r.item_id, qty, hub)) {
                Ok((200, _)) => put += qty,
                _ => break,
            }
        }
    }
    let left = reagent_units(&c.cargo()?);
    let rest = sell_home(c, m, map, left.min(units))?;
    Ok(format!("deposited {put} into hub {hub}; {rest}"))
}

/// **Home, and list the reagents just unloaded** at the reservation price
/// (the escort's listing days and broker fee): at most `units` in all, never
/// another stack the hangar already held.
fn sell_home(c: &mut Conn, m: &mut Memory, map: &crate::travel::Map, units: i64) -> Result<String> {
    let left = reagent_units(&c.cargo()?);
    let home = crate::sov_bot::go_home(c, m, map)?;
    if left == 0 {
        return Ok(home);
    }
    let Some(station) = c.session().station_id() else {
        return Ok(format!("{home}; not docked, reagents kept"));
    };
    let held: Vec<ItemRow> = c
        .cargo()?
        .into_iter()
        .filter(|r| r.group_id == GROUP_COLONY_REAGENTS)
        .collect();
    let mut unloaded = Vec::new();
    for r in held {
        if soft(c.unload(r.item_id, None))?.is_some() {
            unloaded.push(r.item_id);
        }
    }
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| r.group_id == GROUP_COLONY_REAGENTS && r.quantity > 0)
        .collect();
    let quota = sell_quota(
        &stacks
            .iter()
            .map(|r| (r.item_id, r.quantity))
            .collect::<Vec<_>>(),
        &unloaded,
        units.min(left),
    );
    let mut prices = crate::pricing::Prices::read(c)?;
    let mut listed = 0;
    for (item, qty) in quota {
        let Some(r) = stacks.iter().find(|r| r.item_id == item) else {
            continue;
        };
        let Some(price) = prices.reservation(c, r.type_id)?.filter(|p| *p > 0.0) else {
            continue;
        };
        let line = SellLine {
            station_id: station,
            type_id: r.type_id,
            item_id: r.item_id,
            price,
            quantity: qty,
        };
        match c.sell(
            &[line],
            crate::escort::LIST_DAYS,
            crate::escort::LIST_BROKER_FEE,
        ) {
            Ok(()) => listed += qty,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => {}
        }
    }
    Ok(format!("{home}; listed {listed} reagents"))
}
