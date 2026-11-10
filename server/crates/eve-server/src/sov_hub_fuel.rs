//! The sov hub's reagent bay: `hub.fuel.api` `Get` and `Add`, with a burn that
//! runs on the wall clock (SOV-FUEL, #218, lane L5 of
//! `eve-offline-lanes/specs/parity-sov.md`; the wire is `docs/re/sov-gateway.md`
//! §2.2 and §5.3).
//!
//! **What a hub holds.** Colony reagents by type (the bay offers the two the
//! client names, ice and lava; any type of `groupColonyReagents` is taken).
//! One row per hub and reagent in delta kind [`KIND`]: `hub reagent -> amount
//! last_updated per_hour carry`. The amount is *as of* `last_updated`; what is
//! left now is [`amount_at`], `max(0, amount - int(elapsed_hours * per_hour))`,
//! the client's own rule (`sovereignty/fuel/client/data_types.py:47-52`) **[V]**,
//! worked in integers with the `carry` below.
//! Nothing here keeps a countdown: every answer is computed from the stored
//! stamp, so a restart or a late tick changes nothing.
//!
//! **Settling.** A change to the stock (a deposit, a startup cost, a new burn
//! rate) first writes the amount as it is now ([`settle`]), so the burn is
//! continuous across the change. The fraction of a unit already burned is not
//! lost: it is kept as `carry`, in unit-seconds (`0..3600`, a unit is 3600 of
//! them), so `amount_at` is exactly `(carry + elapsed * per_hour) / 3600` in
//! integers and any number of settles burns the same as none **[I]** (the spec's
//! plain "`last_updated = now`" would let three units an hour and a deposit
//! every ten minutes never burn one). `Get` and the tick never write a settle
//! for the same reason.
//!
//! **What `Get` sends.** The client counts from the stamp it is sent
//! (`data_types.py:166-171`), so one stamp covers the whole answer: the oldest
//! stamp of the hub's rows (spec §L5.2), and each amount is stated at that
//! stamp so that the client's own sum equals [`amount_at`] at the moment of the
//! answer.
//!
//! **Who may use the bay.** The pilot's alliance holds the hub's system, the
//! rule of `IsOnLocalSovHubFuelAccessGroup` (`service/sov_hub.rs`); the fuel
//! access group itself is not kept **[A]**.
//!
//! **The upgrades.** `sov_upgrade` sets the burn rates ([`set_burn`]) and spends
//! the stock ([`take`]) through its `FuelBay`. The resimulate slot
//! ([`set_resimulate_hook`]) defaults to `sov_upgrade::resimulate` on first use;
//! it is called with the hub when a stock runs out and when a deposit lands
//! (`add_reply`, `slash`), so a PENDING upgrade can start (spec §1.3 item 3).
//! The fuel-block stand-in in `sov_hub` is untouched:
//! blocks still keep the claim, reagents run upgrades **[A]** (owner Q1).
//!
//! **Registration (coordinator).** `("sovrgt", "sov_hub_fuel")` in
//! `delta_kinds::KINDS`, [`gateway`] in `gateway_ext::CHAIN` above the sov
//! stubs, the `sov_hub_fuel` stage in `tick::STAGES` (every 20, phase 7) with
//! its `tick_budget` rows, `pub mod sov_hub_fuel;` in `lib.rs`, and the
//! `/sov reagents` and `/sov age` words in `service/sov.rs` calling [`slash`].
//!
//! **Clock:** the wall clock, unix seconds ([`crate::sov_proto::unix_now`]);
//! the game clock's scale does not touch the burn (the client extrapolates in
//! real time).

use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, get_uint, put_bytes};
use crate::service::Registry;
use crate::sov_proto::{id, put_uint, read_id, status, timestamp, typed, unix_now};
use std::sync::OnceLock;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovrgt";

const PKG: &str = "eve_public.sovereignty.hub.fuel.api.";

/// What a reagent bay row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stock {
    /// Units as of `last_updated`.
    pub amount: u64,
    /// Unix seconds.
    pub last_updated: u64,
    /// Units burned per hour.
    pub per_hour: u32,
    /// The part of the next unit already burned, in unit-seconds (`0..3600`).
    pub carry: u32,
}

/// Unit-seconds in one unit: the burn is per hour.
const UNIT: u128 = 3600;

fn key(hub: i64, reagent: i64) -> Vec<String> {
    vec![hub.to_string(), reagent.to_string()]
}

fn parse(v: &[String]) -> Option<Stock> {
    Some(Stock {
        amount: v.first()?.parse().ok()?,
        last_updated: v.get(1)?.parse().ok()?,
        per_hour: v.get(2)?.parse().ok()?,
        // Rows written before the carry existed have none.
        carry: v.get(3).and_then(|c| c.parse().ok()).unwrap_or(0),
    })
}

/// The stored row; an empty stock (amount 0, never burning) when there is none.
pub fn stock(d: &Deltas, hub: i64, reagent: i64) -> Stock {
    d.of(KIND)
        .get(&key(hub, reagent))
        .and_then(|v| parse(v))
        .unwrap_or_default()
}

/// Every reagent a hub has a row for, in type order.
pub fn stocks(d: &Deltas, hub: i64) -> Vec<(i64, Stock)> {
    let mut out: Vec<(i64, Stock)> = d
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let h: i64 = k.first()?.parse().ok()?;
            let r: i64 = k.get(1)?.parse().ok()?;
            (h == hub).then(|| parse(v).map(|s| (r, s)))?
        })
        .collect();
    out.sort_by_key(|(r, _)| *r);
    out
}

fn put(d: &mut Deltas, hub: i64, reagent: i64, s: Stock) {
    let k = key(hub, reagent);
    if s.amount == 0 && s.per_hour == 0 {
        d.del(KIND, &k);
        return;
    }
    d.put(
        KIND,
        k,
        vec![
            s.amount.to_string(),
            s.last_updated.to_string(),
            s.per_hour.to_string(),
            s.carry.to_string(),
        ],
    );
}

/// Units burned `elapsed` seconds after the stamp, with `carry` already
/// counted: `int(elapsed_h * per_hour)` **[V]**, in integers.
fn burned(elapsed: u64, per_hour: u32, carry: u32) -> u128 {
    (u128::from(carry) + u128::from(elapsed) * u128::from(per_hour)) / UNIT
}

/// What is left at `now`: `max(0, amount - int(elapsed_h * per_hour))` **[V]**.
pub fn amount_at(s: &Stock, now: u64) -> u64 {
    let b = burned(now.saturating_sub(s.last_updated), s.per_hour, s.carry);
    u64::try_from(u128::from(s.amount).saturating_sub(b)).unwrap_or(0)
}

/// The first second at which the stock reads 0, or `None` for a stock that is
/// not burning or already empty.
pub fn exhausted_at(s: &Stock) -> Option<u64> {
    if s.per_hour == 0 || s.amount == 0 {
        return None;
    }
    // The least t with carry + t * per >= amount * 3600.
    let need = (u128::from(s.amount) * UNIT).saturating_sub(u128::from(s.carry));
    let secs = need.div_ceil(u128::from(s.per_hour));
    Some(s.last_updated.saturating_add(u64::try_from(secs).ok()?))
}

/// Write the amount as it is at `now` and return the new row. The fraction of
/// a unit already burned goes into `carry` (see the module header); an empty or
/// non-burning stock is stamped `now` with none.
pub fn settle(d: &mut Deltas, hub: i64, reagent: i64, now: u64) -> Stock {
    let s = stock(d, hub, reagent);
    let left = amount_at(&s, now);
    let carry = if left == 0 || s.per_hour == 0 {
        0
    } else {
        let total = u128::from(s.carry)
            + u128::from(now.saturating_sub(s.last_updated)) * u128::from(s.per_hour);
        (total % UNIT) as u32
    };
    let out = Stock {
        amount: left,
        // A stamp in the future (aged the other way) is left alone.
        last_updated: now.max(s.last_updated),
        per_hour: s.per_hour,
        carry,
    };
    if out != s || !d.of(KIND).contains_key(&key(hub, reagent)) {
        put(d, hub, reagent, out);
    }
    out
}

/// Change a reagent's burn rate at `now`: settle first, so the amount is
/// continuous, then burn at `per_hour` from `now`.
pub fn set_burn(d: &mut Deltas, hub: i64, reagent: i64, per_hour: u32, now: u64) {
    let s = settle(d, hub, reagent, now);
    put(
        d,
        hub,
        reagent,
        Stock {
            last_updated: now,
            per_hour,
            // A paused stock burns nothing, so it keeps no part of a unit.
            carry: if per_hour == 0 { 0 } else { s.carry },
            ..s
        },
    );
}

/// Spend `n` units (an upgrade's startup cost). `false`, and nothing written,
/// when less than `n` is left at `now`.
pub fn take(d: &mut Deltas, hub: i64, reagent: i64, n: u64, now: u64) -> bool {
    if amount_at(&stock(d, hub, reagent), now) < n {
        return false;
    }
    let s = settle(d, hub, reagent, now);
    put(
        d,
        hub,
        reagent,
        Stock {
            amount: s.amount - n,
            ..s
        },
    );
    true
}

/// Whether `n` more units fit at `now` under the wire's width (`u32::MAX`).
pub fn fits(d: &Deltas, hub: i64, reagent: i64, n: u64, now: u64) -> bool {
    amount_at(&stock(d, hub, reagent), now)
        .checked_add(n)
        .is_some_and(|t| t <= u64::from(u32::MAX))
}

/// Deposit `n` units at `now` (saturating at `u32::MAX`, the wire's width;
/// callers that must not lose a unit check [`fits`] first).
pub fn add(d: &mut Deltas, hub: i64, reagent: i64, n: u64, now: u64) {
    let s = settle(d, hub, reagent, now);
    put(
        d,
        hub,
        reagent,
        Stock {
            amount: s.amount.saturating_add(n).min(u64::from(u32::MAX)),
            ..s
        },
    );
}

// ---------------------------------------------------------------------------
// The re-simulation slot
// ---------------------------------------------------------------------------

/// What L6 (`sov_upgrade`) registers: re-run the hub's upgrade states.
pub type Resimulate = fn(&mut Registry, i64);

static RESIMULATE: OnceLock<Resimulate> = OnceLock::new();

/// Fill the slot once; `false` when it was already filled.
pub fn set_resimulate_hook(f: Resimulate) -> bool {
    RESIMULATE.set(f).is_ok()
}

/// The function in the slot: the upgrades' own re-simulation unless
/// [`set_resimulate_hook`] filled it first.
pub fn resimulate_hook() -> Resimulate {
    // The slot defaults to the upgrades' own re-simulation (SOV-UPG-1 wiring),
    // so no start-up call is needed; `set_resimulate_hook` still wins when it
    // is called before the first use.
    *RESIMULATE.get_or_init(|| crate::sov_upgrade::resimulate)
}

/// Ask the hub's upgrades to re-simulate.
fn resimulate(reg: &mut Registry, hub: i64) {
    resimulate_hook()(reg, hub);
}

// ---------------------------------------------------------------------------
// The tick
// ---------------------------------------------------------------------------

/// Hubs with a stock that has run dry by `now`: each such stock is written as
/// 0 (so it is reported once) and its hub named. Writes nothing else; a stock
/// that is merely burning is not touched.
pub fn run_dry(d: &mut Deltas, now: u64) -> Vec<i64> {
    let dry: Vec<(i64, i64)> = d
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let s = parse(v)?;
            (exhausted_at(&s)? <= now)
                .then(|| Some((k.first()?.parse().ok()?, k.get(1)?.parse().ok()?)))?
        })
        .collect();
    let mut hubs = Vec::new();
    for (hub, reagent) in dry {
        settle(d, hub, reagent, now);
        if !hubs.contains(&hub) {
            hubs.push(hub);
        }
    }
    hubs
}

/// [`tick`] at an explicit time (tests).
pub fn tick_at(reg: &mut Registry, now: u64) {
    tick_with(reg, now, &mut resimulate);
}

/// [`tick_at`] telling `told` instead of the slot (so a test does not share
/// the process-wide slot with other tests).
fn tick_with(reg: &mut Registry, now: u64, told: &mut dyn FnMut(&mut Registry, i64)) {
    if reg.deltas().of(KIND).is_empty() {
        return;
    }
    // Look before taking the write: most ticks find nothing dry.
    let any = reg.deltas().of(KIND).values().any(|v| {
        parse(v)
            .and_then(|s| exhausted_at(&s))
            .is_some_and(|t| t <= now)
    });
    if !any {
        return;
    }
    for hub in run_dry(reg.deltas_mut(), now) {
        told(reg, hub);
    }
}

/// The clock stage (every 20 beats, phase 7): a hub whose reagent ran out
/// asks its upgrades to re-simulate. State is keyed on stored stamps, so a late
/// run does what an early one would.
pub fn tick(reg: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    tick_at(reg, unix_now());
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// The colony-reagent types the typedata knows, in type order.
fn reagent_types(reg: &Registry) -> Vec<i64> {
    let mut v: Vec<i64> = reg
        .content()
        .try_get::<crate::typedata::Installed>()
        .map(|t| {
            t.table()
                .types()
                .filter(|r| r.group_id == crate::sov_resources::GROUP_COLONY_REAGENTS)
                .map(|r| r.type_id)
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v
}

/// The `GetResponse` for `hub` at `now`: one entry for each reagent type with
/// stock or burn, and for each of `pad` (the bay pads both types with 0
/// itself, `reagentBay/controllers.py:63-64`, but a row of 0 is honest).
///
/// The client counts from the one stamp it is sent, so the stamp is the oldest
/// stored one (`now` when the hub has no row) and each amount is stated at it:
/// the amount now plus what the client's own rule burns between the stamp and
/// `now`. The client cannot be told the kept fraction of a unit, so its sum can
/// differ from [`amount_at`] by one; the amount is stated so that it differs
/// on the low side, never showing a unit that is gone.
fn get_bytes_for(d: &Deltas, hub: i64, pad: &[i64], now: u64) -> Vec<u8> {
    let mut rows = stocks(d, hub);
    let stamp = rows
        .iter()
        .map(|(_, s)| s.last_updated)
        .min()
        .map_or(now, |t| t.min(now));
    for &t in pad {
        if !rows.iter().any(|(r, _)| *r == t) {
            rows.push((t, Stock::default()));
        }
    }
    rows.sort_by_key(|(r, _)| *r);
    let mut out = Vec::new();
    for (r, s) in &rows {
        let left = amount_at(s, now);
        let sent = if left == 0 {
            0
        } else {
            // What the client's sum has burned by `now` (k), and the part of
            // a unit we have burned by `now` (r): when its part is the smaller
            // the client would overshoot after a while, so state one less.
            let k = u128::from(now - stamp) * u128::from(s.per_hour);
            let r = (u128::from(s.carry)
                + u128::from(now.saturating_sub(s.last_updated)) * u128::from(s.per_hour))
                % UNIT;
            let under = u128::from(s.per_hour > 0 && k % UNIT < r);
            u64::try_from(u128::from(left) + k / UNIT - under).unwrap_or(u64::MAX)
        };
        let mut m = Vec::new();
        put_bytes(&mut m, 1, &id(*r as u64));
        put_uint(&mut m, 2, sent.min(u64::from(u32::MAX)));
        put_uint(&mut m, 3, u64::from(s.per_hour));
        put_bytes(&mut out, 1, &m);
    }
    put_bytes(&mut out, 2, &timestamp(stamp));
    out
}

/// 403 unless `character`'s alliance holds the hub's system.
fn access(reg: &Registry, hub: &crate::sov_hub::Hub, character: i64) -> bool {
    let holder = crate::sov::claim(reg.deltas(), hub.system_id)
        .filter(|c| c.hub_id == hub.hub_id)
        .and_then(|c| c.holder());
    holder.is_some() && reg.character(character).and_then(|c| c.alliance_id) == holder
}

/// `AddRequest` for `character`, under the write lock. Checks in the order of
/// the spec: hub (404), access (403), item (404 gone, 403 not a reagent),
/// range (403), amount (400).
fn add_reply(reg: &mut Registry, body: &[u8], character: i64) -> Reply {
    let Some(hub_id) = read_id(body, 3).and_then(|h| i64::try_from(h).ok()) else {
        return status(404);
    };
    let Some(hub) = crate::sov_hub::by_id(reg.deltas(), hub_id) else {
        return status(404);
    };
    if !access(reg, &hub, character) {
        return status(403);
    }
    let item_id = read_id(body, 1).and_then(|i| i64::try_from(i).ok());
    let Some(c) = reg.character(character).cloned() else {
        return status(404);
    };
    let item = item_id
        .and_then(|i| reg.store().get(i))
        .filter(|i| {
            i.owner_id == character
                && i.location_id == c.ship_id
                && crate::sov_hub::FUEL_SOURCE_FLAGS.contains(&i.flag_id)
        })
        .cloned();
    let Some(item) = item else {
        return status(404);
    };
    if item.group_id != crate::sov_resources::GROUP_COLONY_REAGENTS {
        return status(403);
    }
    reg.advance_character(character);
    let here = reg
        .roster()
        .get(character)
        .copied()
        .filter(|p| p.ship_id == c.ship_id && p.solar_system_id == hub.system_id);
    let Some(here) = here else {
        return status(403);
    };
    if here.motion.warp_active() {
        return status(403);
    }
    let hull = crate::destiny::hull::of_in(reg.content(), c.ship_type_id);
    let gap = crate::space_state::distance(here.motion.position, hub.position)
        - crate::sov_hub::hub_radius(reg.content())
        - f64::from(hull.radius);
    if gap > crate::sov_hub::TRANSFER_RANGE {
        return status(403);
    }
    let amount = get_uint(body, 2).unwrap_or(0);
    if amount == 0 || i64::try_from(amount).map_or(true, |a| a > item.quantity) {
        return status(400);
    }
    // Refuse before the hold is touched: the wire holds a u32 of them.
    if !fits(reg.deltas(), hub_id, item.type_id, amount, unix_now()) {
        return status(400);
    }
    let edit = if i64::try_from(amount) == Ok(item.quantity) {
        crate::item::Edit::Destroy {
            item_id: item.item_id,
        }
    } else {
        crate::item::Edit::Consume {
            item_id: item.item_id,
            quantity: amount as i64,
        }
    };
    if crate::sov_proto::apply_items_on_clock(reg, character, &[edit]).is_err() {
        return status(404);
    }
    add(reg.deltas_mut(), hub_id, item.type_id, amount, unix_now());
    resimulate(reg, hub_id);
    typed(&format!("{PKG}AddRequest"), Vec::new())
}

/// The gateway arm (`gateway_ext::CHAIN`): `hub.fuel.api` `Get` and `Add`.
/// `None` for every other type. On a [`Ground::Local`] (no characters, no
/// items) `Get` lists the stored rows of a hub that has any and `Add` is left
/// to the stub's honest 403 -- a deviation from the spec's 404, which would
/// break `sov_gw_stub`'s chain test.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    match (rest, g) {
        ("GetRequest", Ground::Server(w)) => {
            let reg = crate::service::world_read_timed(w, "sov_hub_fuel_get");
            let Some(hub_id) = read_id(body, 1).and_then(|h| i64::try_from(h).ok()) else {
                return Some(status(404));
            };
            let Some(hub) = crate::sov_hub::by_id(reg.deltas(), hub_id) else {
                return Some(status(404));
            };
            if !access(&reg, &hub, character) {
                return Some(status(403));
            }
            let pad = reagent_types(&reg);
            Some(typed(
                ty,
                get_bytes_for(reg.deltas(), hub_id, &pad, unix_now()),
            ))
        }
        ("GetRequest", Ground::Local(_)) => {
            let hub_id = read_id(body, 1).and_then(|h| i64::try_from(h).ok())?;
            Some(g.read(|d| {
                if crate::sov_hub::by_id(d, hub_id).is_none() && stocks(d, hub_id).is_empty() {
                    return status(404);
                }
                typed(ty, get_bytes_for(d, hub_id, &[], unix_now()))
            }))
        }
        ("AddRequest", Ground::Server(w)) => {
            let mut reg = crate::service::world_write_timed(w, "sov_hub_fuel_add");
            Some(add_reply(&mut reg, body, character))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// GM: /sov reagents, /sov age fuel
// ---------------------------------------------------------------------------

/// The `/sov` words [`slash`] answers (`service/sov.rs` hands them over).
pub const SLASH_WORDS: &[&str] = &["reagents"];

/// Whether `args` (after `/sov`) is ours: `reagents ...` or `age fuel ...`
/// (not any other `age`).
pub fn owns(args: &[&str]) -> bool {
    match args {
        [word, ..] if SLASH_WORDS.contains(word) => true,
        ["age", "fuel", ..] => true,
        _ => false,
    }
}

/// The usage text of this module's commands (`service/sov.rs`'s `USAGE` carries the same words).
pub const USAGE: &str =
    "/sov reagents <systemID|here> <typeID|ice|lava> <n> | /sov age fuel <systemID|here> <hours>";

fn system_of(reg: &Registry, actor: Option<i64>, word: &str) -> Result<i64, String> {
    if word == "here" {
        return actor
            .and_then(|a| reg.session_system(a))
            .ok_or_else(|| "you are not in a system".to_owned());
    }
    word.parse::<i64>()
        .map_err(|_| format!("'{word}' is not a system id"))
}

/// A reagent type by id, or `ice` / `lava` by the name of a type in the
/// colony-reagent group.
fn reagent_of(reg: &Registry, word: &str) -> Result<i64, String> {
    let inst = reg.content().try_get::<crate::typedata::Installed>();
    let in_group = |t: i64| {
        inst.and_then(|i| i.table().get(t))
            .is_some_and(|r| r.group_id == crate::sov_resources::GROUP_COLONY_REAGENTS)
    };
    if let Ok(t) = word.parse::<i64>() {
        return if in_group(t) {
            Ok(t)
        } else {
            Err(format!("type {t} is not a colony reagent"))
        };
    }
    let word = word.to_ascii_lowercase();
    if word != "ice" && word != "lava" {
        return Err(format!("'{word}' is not a type id, ice or lava"));
    }
    reagent_types(reg)
        .into_iter()
        .find(|t| {
            inst.and_then(|i| i.table().get(*t))
                .is_some_and(|r| r.name.to_ascii_lowercase().contains(&word))
        })
        .ok_or_else(|| format!("no colony reagent named '{word}' in the typedata"))
}

/// `/sov reagents <here|system> <type|ice|lava> <n>` adds stock;
/// `/sov age fuel <here|system> <hours>` moves every stamp of the hub back by
/// `hours`, as if that long had passed (the burn follows, the stored amounts
/// do not change).
pub fn slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let go = |reg: &mut Registry| -> Result<String, String> {
        match args {
            ["reagents", system, ty, n] => {
                let system = system_of(reg, actor, system)?;
                let hub = crate::sov_hub::hub(reg.deltas(), system)
                    .ok_or_else(|| format!("system {system} has no Sovereignty Hub"))?;
                let t = reagent_of(reg, ty)?;
                let n: u64 = n.parse().map_err(|_| format!("'{n}' is not a number"))?;
                if !fits(reg.deltas(), hub.hub_id, t, n, unix_now()) {
                    return Err(format!("{n} more would pass the {} cap", u32::MAX));
                }
                add(reg.deltas_mut(), hub.hub_id, t, n, unix_now());
                resimulate(reg, hub.hub_id);
                let s = stock(reg.deltas(), hub.hub_id, t);
                Ok(format!(
                    "hub {} holds {} of reagent {t} ({} an hour)",
                    hub.hub_id,
                    amount_at(&s, unix_now()),
                    s.per_hour
                ))
            }
            ["age", "fuel", system, hours] => {
                let system = system_of(reg, actor, system)?;
                let hub = crate::sov_hub::hub(reg.deltas(), system)
                    .ok_or_else(|| format!("system {system} has no Sovereignty Hub"))?;
                let hours: u64 = hours
                    .parse()
                    .map_err(|_| format!("'{hours}' is not a whole number of hours"))?;
                let back = hours.saturating_mul(3600);
                let rows = stocks(reg.deltas(), hub.hub_id);
                for (r, s) in &rows {
                    put(
                        reg.deltas_mut(),
                        hub.hub_id,
                        *r,
                        Stock {
                            last_updated: s.last_updated.saturating_sub(back),
                            ..*s
                        },
                    );
                }
                resimulate(reg, hub.hub_id);
                Ok(format!(
                    "hub {}: {} reagent stock(s) aged {hours} h",
                    hub.hub_id,
                    rows.len()
                ))
            }
            _ => Err("usage".to_owned()),
        }
    };
    match go(reg) {
        Ok(t) => t,
        Err(e) => format!("usage: {USAGE} -- {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{get_bytes, put_bytes};
    use crate::sov_hub::fixture::*;
    use std::sync::{Arc, RwLock};

    // Invented ids.
    const HUB: i64 = 8_900_100;
    const ICE: i64 = 9_101;
    const LAVA: i64 = 9_102;
    const T0: u64 = 1_000_000;
    const H: u64 = 3600;

    fn st(amount: u64, per_hour: u32) -> Stock {
        Stock {
            amount,
            last_updated: T0,
            per_hour,
            carry: 0,
        }
    }

    #[test]
    fn amount_at_truncates_every_time_it_is_asked() {
        let s = st(10, 3);
        assert_eq!(amount_at(&s, T0), 10, "0 h");
        assert_eq!(amount_at(&s, T0 + H / 2), 9, "0.5 h burns int(1.5) = 1");
        assert_eq!(amount_at(&s, T0 + H), 7, "1 h burns 3");
        assert_eq!(amount_at(&s, T0 + 3 * H / 2), 6, "1.5 h burns int(4.5) = 4");
        assert_eq!(amount_at(&s, T0 + 100 * H), 0, "never below 0");
        assert_eq!(
            amount_at(&s, T0 - 5),
            10,
            "a stamp in the future burns nothing"
        );
        assert_eq!(amount_at(&st(10, 0), T0 + 100 * H), 10, "no burn, no loss");
    }

    #[test]
    fn exhausted_at_is_the_first_second_the_stock_reads_zero() {
        for (amount, per_hour) in [(10, 3), (7, 7), (1, 1), (100, 9), (5, 1000)] {
            let s = st(amount, per_hour);
            let t = exhausted_at(&s).expect("a burning stock runs out");
            assert_eq!(amount_at(&s, t), 0, "{amount}/{per_hour} is empty at {t}");
            assert!(
                amount_at(&s, t - 1) > 0,
                "{amount}/{per_hour} not before {t}"
            );
        }
        assert_eq!(exhausted_at(&st(10, 0)), None);
        assert_eq!(exhausted_at(&st(0, 5)), None, "already empty");
    }

    #[test]
    fn set_burn_settles_first_so_the_amount_is_continuous() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 100, T0);
        set_burn(&mut d, HUB, ICE, 10, T0);
        // Two hours at 10 an hour, then a new rate of 1: the 20 burned stay burned.
        let before = amount_at(&stock(&d, HUB, ICE), T0 + 2 * H);
        assert_eq!(before, 80);
        set_burn(&mut d, HUB, ICE, 1, T0 + 2 * H);
        let s = stock(&d, HUB, ICE);
        assert_eq!((s.amount, s.per_hour, s.last_updated), (80, 1, T0 + 2 * H));
        assert_eq!(
            amount_at(&s, T0 + 3 * H),
            79,
            "one more hour at the new rate"
        );
        // A rate set on an empty hub row writes a row that burns nothing yet.
        set_burn(&mut d, HUB, LAVA, 4, T0);
        assert_eq!(stock(&d, HUB, LAVA).amount, 0);
        assert_eq!(stock(&d, HUB, LAVA).per_hour, 4);
    }

    #[test]
    fn a_deposit_does_not_stop_a_slow_burn() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 50, T0);
        set_burn(&mut d, HUB, ICE, 3, T0);
        // A unit in the bank every ten minutes: six deposits, one hour.
        for k in 1..=6 {
            add(&mut d, HUB, ICE, 1, T0 + k * H / 6);
        }
        assert_eq!(
            amount_at(&stock(&d, HUB, ICE), T0 + H),
            50 + 6 - 3,
            "an hour of 3 an hour burned 3 although it was settled six times"
        );
    }

    #[test]
    fn take_spends_only_what_is_there_and_moves_nothing_when_short() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 10, T0);
        set_burn(&mut d, HUB, ICE, 2, T0);
        let before = d.clone();
        assert!(
            !take(&mut d, HUB, ICE, 9, T0 + H),
            "8 are left after an hour"
        );
        assert_eq!(d.of(KIND), before.of(KIND), "a refused take writes nothing");
        assert!(take(&mut d, HUB, ICE, 8, T0 + H));
        assert_eq!(amount_at(&stock(&d, HUB, ICE), T0 + H), 0);
        assert_eq!(stock(&d, HUB, ICE).per_hour, 2, "the rate stays");
    }

    #[test]
    fn add_saturates_at_the_wires_width() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, u64::from(u32::MAX), T0);
        add(&mut d, HUB, ICE, 5, T0);
        assert_eq!(stock(&d, HUB, ICE).amount, u64::from(u32::MAX));
    }

    #[test]
    fn an_empty_non_burning_row_is_not_kept() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 3, T0);
        assert!(take(&mut d, HUB, ICE, 3, T0));
        assert!(d.of(KIND).is_empty());
    }

    #[test]
    fn run_dry_names_a_hub_once_and_writes_the_zero() {
        let mut d = Deltas::default();
        for r in [ICE, LAVA] {
            add(&mut d, HUB, r, 4, T0);
            set_burn(&mut d, HUB, r, 4, T0);
        }
        add(&mut d, HUB + 1, ICE, 4, T0);
        set_burn(&mut d, HUB + 1, ICE, 1, T0);
        assert!(run_dry(&mut d, T0 + H / 2).is_empty(), "still burning");
        let dry = run_dry(&mut d, T0 + H);
        assert_eq!(dry, vec![HUB], "both of the hub's reagents, named once");
        assert_eq!(stock(&d, HUB, ICE).amount, 0);
        assert_eq!(
            stock(&d, HUB + 1, ICE).amount,
            4,
            "the slow one is not touched"
        );
        assert!(run_dry(&mut d, T0 + 2 * H).is_empty(), "reported once");
    }

    // -- the wire, on a registry ---------------------------------------------

    fn fuel_content() -> crate::content::Content {
        let mut c = content();
        let text = format!(
            "{}type\t{}\t{}\t{}\t1\t1\t10.0\t0.0\t1000.0\t{HUB_TEST_RADIUS}\t0\t\t\tInvented Hub Row\n\
             type\t{ICE}\t{g}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Ice Reagent\n\
             type\t{LAVA}\t{g}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Lava Reagent\n",
            crate::typedata::FIXTURE,
            crate::sov::TYPE_INFRASTRUCTURE_HUB,
            crate::sov_hub::GROUP_INFRASTRUCTURE_HUB,
            crate::sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
            g = crate::sov_resources::GROUP_COLONY_REAGENTS,
        );
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// The three reagent stacks of `world`.
    struct Stacks {
        alice: i64,
        bob: i64,
        dave: i64,
    }

    /// Alice's alliance holds a launched hub; she sits beside it with 100 ice
    /// in the hold. Bob (same alliance) is 200 km away with a stack, Dave
    /// (the rival alliance) has a stack of his own.
    fn world() -> (Registry, i64, Stacks) {
        let (reg, a, _d) = crate::sov_hub::fixture::world();
        let mut reg = reg.with_content(Arc::new(fuel_content()));
        for who in [ALICE, BOB, DAVE] {
            reg.apply_items(&[crate::item::Edit::Create {
                type_id: ICE,
                quantity: 100,
                to: crate::item::Slot::new(who, ship(who), crate::sov_hub::FLAG_CARGO),
            }])
            .expect("a reagent stack");
        }
        let stack_of = |reg: &Registry, who: i64| {
            reg.store()
                .items()
                .iter()
                .find(|i| i.type_id == ICE && i.owner_id == who)
                .map(|i| i.item_id)
                .expect("the stack")
        };
        let stacks = Stacks {
            alice: stack_of(&reg, ALICE),
            bob: stack_of(&reg, BOB),
            dave: stack_of(&reg, DAVE),
        };
        let hub = launched(&mut reg);
        let exec = crate::alliance::Alliances::load(reg.deltas())
            .get(a)
            .and_then(|x| x.executor_corp_id)
            .expect("an executor");
        crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((a, exec)), 1);
        assert_eq!(
            crate::sov::claim(reg.deltas(), SYSTEM).map(|c| c.hub_id),
            Some(hub),
            "the claim is on the launched hub"
        );
        let spot = fuel_spot(&reg);
        place_at(&mut reg, ALICE, spot);
        (reg, hub, stacks)
    }

    fn msg_add(item: i64, amount: u64, hub: i64) -> Vec<u8> {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &id(item as u64));
        put_uint(&mut b, 2, amount);
        put_bytes(&mut b, 3, &id(hub as u64));
        b
    }

    fn msg_get(hub: i64) -> Vec<u8> {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &id(hub as u64));
        b
    }

    fn ask(g: &Ground, ty: &str, body: &[u8], who: i64) -> Reply {
        gateway(&format!("{PKG}{ty}"), body, who, g).expect("this arm owns the type")
    }

    fn ground(reg: Registry) -> (Ground, Arc<RwLock<Registry>>) {
        let w = Arc::new(RwLock::new(reg));
        (Ground::Server(w.clone()), w)
    }

    fn qty(w: &Arc<RwLock<Registry>>, item: i64) -> Option<i64> {
        w.read().unwrap().store().get(item).map(|i| i.quantity)
    }

    /// Every length-delimited `field` of `body`, in order (the gateway reads
    /// only the first).
    fn repeated(body: &[u8], field: u64) -> Vec<Vec<u8>> {
        fn varint(b: &[u8], p: &mut usize) -> u64 {
            let (mut v, mut s) = (0u64, 0);
            loop {
                let x = b[*p];
                *p += 1;
                v |= u64::from(x & 0x7f) << s;
                if x & 0x80 == 0 {
                    return v;
                }
                s += 7;
            }
        }
        let (mut out, mut p) = (Vec::new(), 0);
        while p < body.len() {
            let tag = varint(body, &mut p);
            match tag & 7 {
                0 => {
                    varint(body, &mut p);
                }
                2 => {
                    let n = varint(body, &mut p) as usize;
                    if tag >> 3 == field {
                        out.push(body[p..p + n].to_vec());
                    }
                    p += n;
                }
                w => panic!("wire type {w}"),
            }
        }
        out
    }

    /// `(type, amount, per_hour)` of every entry of a `GetResponse`.
    fn entries(r: &Reply) -> Vec<(u64, u64, u64)> {
        let body = &r.payload.as_ref().expect("a payload").1;
        repeated(body, 1)
            .iter()
            .map(|m| {
                (
                    read_id(m, 1).expect("a type"),
                    get_uint(m, 2).unwrap_or(0),
                    get_uint(m, 3).unwrap_or(0),
                )
            })
            .collect()
    }

    #[test]
    fn half_a_stack_leaves_the_rest_and_the_hub_gets_the_half() {
        let (reg, hub, k) = world();
        let (g, w) = ground(reg);
        let r = ask(&g, "AddRequest", &msg_add(k.alice, 40, hub), ALICE);
        assert_eq!(r.status, 200);
        assert_eq!(
            r.payload.as_ref().map(|p| p.0.as_str()),
            Some("eve_public.sovereignty.hub.fuel.api.AddResponse")
        );
        assert_eq!(qty(&w, k.alice), Some(60), "the hold keeps 60");
        let got = ask(&g, "GetRequest", &msg_get(hub), ALICE);
        let ice = entries(&got)
            .into_iter()
            .find(|e| e.0 == ICE as u64)
            .expect("ice");
        assert_eq!(ice.1, 40, "the bay reads what was deposited");
    }

    /// The hold's `OnItemChange` leaves on the clock's queue; `pending`
    /// drains only after the client's next call, which left the stack
    /// reading 100 for about 20 s after an `AddRequest` of 40.
    #[test]
    fn a_deposit_pushes_the_hold_change_on_the_clock_not_on_pending() {
        let (mut reg, hub, k) = world();
        let earlier = reg.take_pending_raw().len();
        let queued = reg.scheduled_len();
        let r = add_reply(&mut reg, &msg_add(k.alice, 40, hub), ALICE);
        assert_eq!(r.status, 200);
        let pending = reg.take_pending_raw();
        assert!(
            pending.is_empty(),
            "{} push(es) wait for a call ({earlier} were there before)",
            pending.len()
        );
        assert!(reg.scheduled_len() > queued, "nothing was scheduled");
        let due = reg.take_due(std::time::Instant::now());
        let names: Vec<&str> = due.iter().map(|p| p.to_push().0).collect();
        assert!(names.contains(&"OnItemChange"), "{names:?}");
        assert!(
            due.iter().all(|p| p.audience() == Some(vec![ALICE])),
            "every push goes to the pilot alone"
        );
    }

    #[test]
    fn a_whole_stack_removes_the_item() {
        let (reg, hub, k) = world();
        let (g, w) = ground(reg);
        assert_eq!(
            ask(&g, "AddRequest", &msg_add(k.alice, 100, hub), ALICE).status,
            200
        );
        assert_eq!(qty(&w, k.alice), None, "the stack is gone");
        let r = w.read().unwrap();
        assert_eq!(amount_at(&stock(r.deltas(), hub, ICE), unix_now()), 100);
    }

    #[test]
    fn a_stack_out_of_range_is_refused_and_nothing_moves() {
        let (mut reg, hub, k) = world();
        let far = launch_spot(&reg);
        place_at(&mut reg, ALICE, far);
        let (g, w) = ground(reg);
        let r = ask(&g, "AddRequest", &msg_add(k.alice, 10, hub), ALICE);
        assert_eq!((r.status, r.payload), (403, None));
        assert_eq!(qty(&w, k.alice), Some(100));
        assert!(w.read().unwrap().deltas().of(KIND).is_empty());
    }

    #[test]
    fn the_refusals_have_their_own_status_and_move_nothing() {
        let (reg, hub, k) = world();
        let (g, w) = ground(reg);
        let st =
            |item, amount, hub, who| ask(&g, "AddRequest", &msg_add(item, amount, hub), who).status;
        assert_eq!(st(k.alice, 10, hub + 7, ALICE), 404, "no such hub");
        assert_eq!(st(k.alice, 10, hub, DAVE), 403, "the rival alliance");
        assert_eq!(st(k.alice, 10, hub, CAROL), 403, "no alliance");
        assert_eq!(st(k.alice + 5000, 10, hub, ALICE), 404, "no such item");
        assert_eq!(st(k.dave, 10, hub, ALICE), 404, "someone else's item");
        assert_eq!(
            st(ALICE_FUEL, 10, hub, ALICE),
            403,
            "a fuel block is no reagent"
        );
        assert_eq!(st(k.alice, 101, hub, ALICE), 400, "more than the stack");
        assert_eq!(st(k.alice, 0, hub, ALICE), 400, "nothing");
        assert_eq!(st(k.bob, 10, hub, ALICE), 404, "a stack in Bob's ship");
        assert_eq!(qty(&w, k.alice), Some(100));
        assert_eq!(qty(&w, ALICE_FUEL), Some(ALICE_BLOCKS));
        assert!(
            w.read().unwrap().deltas().of(KIND).is_empty(),
            "no stock written"
        );
    }

    #[test]
    fn get_always_lists_both_reagent_types_and_the_burn() {
        let (reg, hub, _k) = world();
        let (g, w) = ground(reg);
        let r = ask(&g, "GetRequest", &msg_get(hub), ALICE);
        assert_eq!(r.status, 200);
        let e = entries(&r);
        assert_eq!(e, vec![(ICE as u64, 0, 0), (LAVA as u64, 0, 0)]);
        // With stock and a burn: the amount is as of now, the rate as stored.
        let now = unix_now();
        {
            let mut reg = w.write().unwrap();
            add(reg.deltas_mut(), hub, ICE, 50, now - 2 * H);
            set_burn(reg.deltas_mut(), hub, ICE, 5, now - 2 * H);
        }
        let r = ask(&g, "GetRequest", &msg_get(hub), ALICE);
        let ice = entries(&r).into_iter().find(|e| e.0 == ICE as u64).unwrap();
        let body = &r.payload.as_ref().unwrap().1;
        assert_eq!(ice.2, 5);
        assert_eq!(client_sees(body, ICE, now), 40, "two hours at 5 an hour");
        let stamp = get_uint(&get_bytes(body, 2).unwrap(), 1).unwrap();
        assert_eq!(
            stamp,
            now - 2 * H,
            "the stored stamp, which the amounts are stated at"
        );
    }

    #[test]
    fn get_is_refused_for_an_outsider_and_a_missing_hub_and_writes_nothing() {
        let (reg, hub, _k) = world();
        let (g, w) = ground(reg);
        assert_eq!(ask(&g, "GetRequest", &msg_get(hub), DAVE).status, 403);
        assert_eq!(ask(&g, "GetRequest", &msg_get(hub + 7), ALICE).status, 404);
        assert_eq!(ask(&g, "GetRequest", &[], ALICE).status, 404);
        assert!(w.read().unwrap().deltas().of(KIND).is_empty());
    }

    #[test]
    fn the_arm_owns_only_its_two_types() {
        let g = Ground::local();
        for ty in [
            "hub.api.GetResourcesRequest",
            "hub.fuel.api.ListRequest",
            "skyhook.api.GetRequest",
        ] {
            assert_eq!(
                gateway(&format!("eve_public.sovereignty.{ty}"), &[], 1, &g),
                None,
                "{ty}"
            );
        }
        // On the bare ground Add is left to the stub, Get answers from the rows.
        assert_eq!(gateway(&format!("{PKG}AddRequest"), &[], 1, &g), None);
        assert_eq!(
            gateway(&format!("{PKG}GetRequest"), &msg_get(HUB), 1, &g).map(|r| r.status),
            Some(404)
        );
        if let Ground::Local(d) = &g {
            let mut d = d.lock().unwrap();
            add(&mut d, HUB, ICE, 9, unix_now());
        }
        let r = gateway(&format!("{PKG}GetRequest"), &msg_get(HUB), 1, &g).unwrap();
        assert_eq!(entries(&r).first().map(|e| e.1), Some(9));
    }

    #[test]
    fn the_tick_tells_a_dry_hub_once() {
        // Its own sink, not the process-wide slot other tests could fill.
        const MINE: i64 = 8_900_777;
        let (mut reg, _, _) = world();
        add(reg.deltas_mut(), MINE, ICE, 2, T0);
        set_burn(reg.deltas_mut(), MINE, ICE, 2, T0);
        let mut seen = Vec::new();
        tick_with(&mut reg, T0 + H / 2, &mut |_, h| seen.push(h));
        assert!(seen.is_empty(), "not dry yet");
        tick_with(&mut reg, T0 + H, &mut |_, h| seen.push(h));
        assert_eq!(seen, vec![MINE], "dry: told once");
        tick_with(&mut reg, T0 + 2 * H, &mut |_, h| seen.push(h));
        assert_eq!(seen, vec![MINE], "and not again");
    }

    #[test]
    fn a_settle_loses_no_burn_at_a_high_rate() {
        // Round 1 lost burn when a settle moved the stamp by whole seconds:
        // 5000 an hour settled every second burned too little (the case that
        // failed on round 1). 7200 an hour, an exact 2 a second, always held.
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 1_000_000, T0);
        set_burn(&mut d, HUB, ICE, 7_200, T0);
        let never = amount_at(&stock(&d, HUB, ICE), T0 + 1000);
        assert_eq!(never, 1_000_000 - 2_000);
        // Settle every second, and at odd rates every 7 s: the same as none.
        for (rate, step) in [
            (7_200u32, 1u64),
            (5_000, 1),
            (5_000, 7),
            (3, 600),
            (1, 3599),
        ] {
            let mut a = Deltas::default();
            let mut b = Deltas::default();
            for d in [&mut a, &mut b] {
                add(d, HUB, ICE, 10_000_000, T0);
                set_burn(d, HUB, ICE, rate, T0);
            }
            let end = T0 + 20_000;
            let mut t = T0;
            while t < end {
                t += step;
                settle(&mut a, HUB, ICE, t.min(end));
            }
            assert_eq!(
                amount_at(&stock(&a, HUB, ICE), end),
                amount_at(&stock(&b, HUB, ICE), end),
                "rate {rate}, settled every {step} s"
            );
        }
    }

    #[test]
    fn exhausted_at_agrees_with_amount_at_with_a_carry() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 5, T0);
        set_burn(&mut d, HUB, ICE, 7, T0);
        settle(&mut d, HUB, ICE, T0 + 1000);
        let s = stock(&d, HUB, ICE);
        assert_ne!(s.carry, 0, "a fraction was kept");
        let t = exhausted_at(&s).unwrap();
        assert_eq!(amount_at(&s, t), 0);
        assert!(amount_at(&s, t - 1) > 0);
    }

    /// What the client shows at `at`, counting from the stamp in the answer
    /// (`data_types.py:166-171`): `amount - int(elapsed_h * per_hour)`.
    fn client_sees(body: &[u8], reagent: i64, at: u64) -> u64 {
        let stamp = get_uint(&get_bytes(body, 2).unwrap(), 1).unwrap();
        let m = repeated(body, 1)
            .into_iter()
            .find(|m| read_id(m, 1) == Some(reagent as u64))
            .unwrap();
        let (amount, per) = (get_uint(&m, 2).unwrap_or(0), get_uint(&m, 3).unwrap_or(0));
        amount.saturating_sub(((at - stamp) as f64 / 3600.0 * per as f64) as u64)
    }

    #[test]
    fn get_keeps_the_kept_fraction_so_the_client_does_not_show_a_unit_too_many() {
        let mut d = Deltas::default();
        // 10 units at 3 an hour, settled 50 minutes in: 2 whole units gone and
        // half a unit kept. Lava settles later, so the stamps differ.
        add(&mut d, HUB, ICE, 10, T0);
        set_burn(&mut d, HUB, ICE, 3, T0);
        settle(&mut d, HUB, ICE, T0 + 3000);
        add(&mut d, HUB, LAVA, 20, T0 + 3300);
        set_burn(&mut d, HUB, LAVA, 6, T0 + 3300);
        let now = T0 + 5400;
        let body = get_bytes_for(&d, HUB, &[], now);
        let stamp = get_uint(&get_bytes(&body, 2).unwrap(), 1).unwrap();
        assert_eq!(stamp, T0 + 3000, "the oldest stored stamp is sent");
        for at in (0..40).map(|k| now + k * 300) {
            for r in [ICE, LAVA] {
                let (seen, ours) = (client_sees(&body, r, at), amount_at(&stock(&d, HUB, r), at));
                assert!(
                    seen <= ours && ours - seen <= 1,
                    "reagent {r} at +{} s: client {seen}, server {ours}",
                    at - T0
                );
            }
        }
    }

    #[test]
    fn a_paused_stock_is_sent_as_the_amount_it_has() {
        let mut d = Deltas::default();
        add(&mut d, HUB, ICE, 10, T0);
        set_burn(&mut d, HUB, ICE, 3, T0);
        // 50 minutes at 3 an hour: 2 whole units gone, half a unit burned.
        set_burn(&mut d, HUB, ICE, 0, T0 + 3000);
        let ours = amount_at(&stock(&d, HUB, ICE), T0 + 5000);
        assert_eq!(ours, 8);
        let body = get_bytes_for(&d, HUB, &[], T0 + 5000);
        assert_eq!(client_sees(&body, ICE, T0 + 5000), ours);
        assert_eq!(client_sees(&body, ICE, T0 + 90_000), ours, "and it stays");
    }

    #[test]
    fn a_deposit_that_would_pass_the_cap_is_refused_before_the_hold_is_touched() {
        let (mut reg, hub, k) = world();
        add(
            reg.deltas_mut(),
            hub,
            ICE,
            u64::from(u32::MAX) - 5,
            unix_now(),
        );
        let (g, w) = ground(reg);
        let r = ask(&g, "AddRequest", &msg_add(k.alice, 10, hub), ALICE);
        assert_eq!(r.status, 400, "10 more would pass the cap");
        assert_eq!(qty(&w, k.alice), Some(100), "the hold is untouched");
        let r = ask(&g, "AddRequest", &msg_add(k.alice, 5, hub), ALICE);
        assert_eq!(r.status, 200, "exactly to the cap is fine");
        assert_eq!(qty(&w, k.alice), Some(95));
        let reg = w.read().unwrap();
        assert_eq!(
            amount_at(&stock(reg.deltas(), hub, ICE), unix_now()),
            u64::from(u32::MAX)
        );
    }

    #[test]
    fn the_slash_refuses_a_stock_past_the_cap_too() {
        let (mut reg, hub, _k) = world();
        add(reg.deltas_mut(), hub, ICE, u64::from(u32::MAX), unix_now());
        let out = slash(&mut reg, Some(ALICE), &["reagents", "here", "ice", "1"]);
        assert!(out.starts_with("usage") && out.contains("cap"), "{out}");
    }

    #[test]
    fn only_age_fuel_is_ours_not_any_age() {
        assert!(owns(&["reagents", "here", "ice", "1"]));
        assert!(owns(&["age", "fuel", "here", "2"]));
        assert!(!owns(&["age"]));
        assert!(!owns(&["age", "skills", "here"]));
        assert!(!owns(&["info"]));
        assert!(!owns(&[]));
    }

    #[test]
    fn slash_adds_stock_and_ages_the_stamps() {
        let (mut reg, hub, _k) = world();
        let out = slash(&mut reg, Some(ALICE), &["reagents", "here", "ice", "70"]);
        assert!(out.contains("70"), "{out}");
        assert_eq!(amount_at(&stock(reg.deltas(), hub, ICE), unix_now()), 70);
        set_burn(reg.deltas_mut(), hub, ICE, 10, unix_now());
        let out = slash(&mut reg, Some(ALICE), &["age", "fuel", "here", "2"]);
        assert!(out.contains("aged 2 h"), "{out}");
        let s = stock(reg.deltas(), hub, ICE);
        assert_eq!(s.amount, 70, "the stored amount is untouched");
        assert!(
            amount_at(&s, unix_now()) <= 50,
            "two hours at 10 an hour burned"
        );
        // Lava by name, a number that is no type, a bad number, a hub-less system.
        assert!(slash(&mut reg, Some(ALICE), &["reagents", "here", "lava", "5"]).contains("5"));
        assert!(
            slash(&mut reg, Some(ALICE), &["reagents", "here", "123", "5"]).starts_with("usage")
        );
        assert!(
            slash(&mut reg, Some(ALICE), &["reagents", "here", "ice", "x"]).starts_with("usage")
        );
        assert!(
            slash(
                &mut reg,
                Some(ALICE),
                &["reagents", &HIGH.to_string(), "ice", "5"]
            )
            .starts_with("usage")
        );
        assert_eq!(stock(reg.deltas(), hub, ICE).amount, 70);
    }
}
