//! **Bots use the HyperNet** (HN-3, issue #203; spec
//! `parity-dailygoals-hypernet.md` §HN-3). One act, [`crate::brain::Act::HyperNet`],
//! docked, every [`RECHECK`], with the verbs a player has (`raffleProxy`):
//!
//! 1. **Claim wins**: `GetMyRaffleHistory`; a finished-undelivered raffle whose
//!    winning ticket is ours gets `AwardItem`.
//! 2. **Offer surplus loot**: the one stack in the hangar here that no fit plan
//!    wants ([`crate::contracts::stock`] keeps the guns and rounds), is not ore,
//!    mineral or PLEX, and is worth [`MIN_ITEM_ISK`] or more, goes up as an offer
//!    (`CreateRaffle`), paid for with HyperCores **the bot holds**.
//! 3. **Buy tickets** with ISK above [`RESERVE`], at most [`SPEND`] of that
//!    surplus a turn, in running offers that are good value and not its own.
//! 4. **Buy cores** from the market only, when step 2 has a prize and no cores.
//!
//! **Nothing from nowhere, no GM.** Every item offered was in the hangar, every
//! core was held or bought from another pilot's ask, every ticket was paid for
//! from the wallet. A refused offer waits [`super::BACKOFF`].
//!
//! The decisions are pure ([`surplus_item`], [`offer_shape`], [`ticket_budget`],
//! [`pick_raffles`]); the wire functions are thin. **Every number is [A]**; the
//! falsifier sits beside each.

use super::{BACKOFF, book_of, soft};
use crate::clock;
use crate::verbs::{FLAG_HANGAR, ItemRow};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::hypernet as hn;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How often a docked bot spends a turn here **[A]**. Falsified by a soak with
/// no bot offer in two hours (shorter), or bots that do nothing else (longer).
pub const RECHECK: Duration = Duration::from_secs(30 * 60);
/// The least an item is worth for the bot to offer it **[A]**. Falsified by
/// offers that never sell out (higher) or loot piling up unsold (lower).
pub const MIN_ITEM_ISK: f64 = 5_000_000.0;
/// The least a ticket costs **[A]**. Falsified by offers that stay at one or
/// two tickets sold for a day (lower) or none ever shaped at 8 (higher).
pub const MIN_TICKET_ISK: f64 = 100_000.0;
/// ISK the bot keeps before it buys a ticket **[A]**. Falsified by bots that
/// cannot afford a fit after a ticket run (higher).
pub const RESERVE: f64 = 50_000_000.0;
/// The share of the surplus above [`RESERVE`] spent on tickets in one turn
/// **[A]**. Falsified by wallets that drain into tickets (lower).
pub const SPEND: f64 = 0.10;
/// The most tickets bought in one offer in one turn **[A]**.
pub const MAX_TICKETS: i64 = 4;
/// The most a HyperCore is worth buying at, per unit **[A]**. Falsified by
/// bots that hold loot and never get cores (higher) or that pay more for the
/// cores than the offer yields (lower).
pub const CORE_MAX_ISK: f64 = 1_000_000.0;
/// Running offers of its own a bot keeps at most **[A]**. Falsified by
/// hangars that still fill with surplus after days (higher).
pub const MAX_OPEN: usize = 2;
/// The client's default ticket pricing and its "good value" line, both
/// `estimate * 1.4` **[V]** (spec §HN-3: `GOOD_VALUE_FACTOR`).
pub const GOOD_VALUE_FACTOR: f64 = 1.4;

/// `categoryAsteroid` and the mineral group **[V-data]**: never offered.
const CATEGORY_ASTEROID: i64 = crate::brain::CATEGORY_ASTEROID;
const GROUP_MINERAL: i64 = 18;

// ---- the state ------------------------------------------------------------------

/// What the bot remembers between turns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub checked: Option<Instant>,
    wait: BTreeMap<&'static str, Instant>,
    /// Offers made, tickets bought, prizes claimed, in all.
    pub offered: u64,
    pub tickets: u64,
    pub claimed: u64,
}

impl State {
    fn backed_off(&self, what: &'static str, now: Instant) -> bool {
        self.wait.get(what).is_some_and(|t| now < *t)
    }

    fn back_off(&mut self, what: &'static str, now: Instant) {
        self.wait.insert(what, now + BACKOFF);
    }
}

/// How often a bot with no cores and no dealings spends a turn here **[A]**:
/// it only looks for good-value tickets. Falsified by a soak where such bots
/// never buy a ticket in a day (shorter) or where these looks show in the
/// act log's share (longer).
pub const IDLE_RECHECK: Duration = Duration::from_secs(8 * 3600);

impl State {
    /// The bot has dealings here: it made an offer, bought a ticket or
    /// claimed a prize, and so has something to watch.
    pub fn engaged(&self) -> bool {
        self.offered + self.tickets + self.claimed > 0
    }
}

/// **Is a turn due?** (pure) [`RECHECK`] since the last for a bot that holds
/// cores or has dealings here, [`IDLE_RECHECK`] for any other. A bot that has
/// not been looked at yet is not due: the first look starts its clock, so a
/// thousand bots docking together do not all turn at once.
pub fn due_at(checked: Option<Instant>, now: Instant, busy: bool) -> bool {
    let every = if busy { RECHECK } else { IDLE_RECHECK };
    checked.is_some_and(|t| now.saturating_duration_since(t) >= every)
}

/// The brain's docked hook: a turn when [`due_at`] says so.
pub fn due(
    m: &mut crate::brain::Memory,
    o: &crate::brain::Observation,
) -> Option<Vec<crate::brain::Act>> {
    if m.hypernet.checked.is_none() {
        m.hypernet.checked = Some(o.now);
        return None;
    }
    let busy = m.hypernet.engaged() || o.hangar.iter().any(|r| r.type_id == hn::TOKEN_TYPE_ID);
    if !due_at(m.hypernet.checked, o.now, busy) {
        return None;
    }
    m.hypernet.checked = Some(o.now);
    Some(vec![crate::brain::Act::HyperNet])
}

// ---- pure rules -----------------------------------------------------------------

/// The stack the bot would offer.
#[derive(Debug, Clone, PartialEq)]
pub struct Prize {
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    /// `averagePrice x quantity`.
    pub estimate: f64,
}

/// **The surplus item** (pure): of the stacks in `rows`' hangar at `here`, the
/// most valuable one that `wanted` does not name, is not ore, mineral, PLEX or
/// a HyperCore, and whose estimate (`avg` per unit times quantity) is at least
/// `min_isk`. Lowest item id on a tie.
pub fn surplus_item(
    rows: &[ItemRow],
    here: i64,
    wanted: &dyn Fn(&ItemRow) -> bool,
    avg: &dyn Fn(i64) -> Option<f64>,
    min_isk: f64,
) -> Option<Prize> {
    rows.iter()
        .filter(|r| r.flag_id == FLAG_HANGAR && r.location_id == here)
        .filter(|r| {
            r.category_id != CATEGORY_ASTEROID
                && r.group_id != GROUP_MINERAL
                && r.type_id != hn::PLEX_TYPE_ID
                && r.type_id != hn::TOKEN_TYPE_ID
        })
        .filter(|r| !wanted(r))
        .filter_map(|r| {
            let estimate = avg(r.type_id)? * r.quantity.max(1) as f64;
            (estimate >= min_isk).then(|| Prize {
                item_id: r.item_id,
                type_id: r.type_id,
                quantity: r.quantity.max(1),
                estimate,
            })
        })
        .max_by(|a, b| {
            a.estimate
                .total_cmp(&b.estimate)
                .then(b.item_id.cmp(&a.item_id))
        })
}

/// How an offer is made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub count: i64,
    /// ISK per ticket.
    pub price: i64,
    /// HyperCores the offer burns.
    pub tokens: i64,
}

/// **The offer's shape** (pure): the total is `estimate x` [`GOOD_VALUE_FACTOR`]
/// inside the server's accepted total range (the client's clamp is **[I]**:
/// the range the server enforces); the count is the smallest of the client's
/// ticket-count pool with `total / count >= min_ticket`; the price is
/// `total / count` rounded; the cores are [`hn::tokens_required`] at `plex`.
pub fn offer_shape(estimate: f64, plex: f64, min_ticket: f64) -> Option<Shape> {
    if !estimate.is_finite() || estimate <= 0.0 {
        return None;
    }
    let total = (estimate * GOOD_VALUE_FACTOR).clamp(hn::MIN_TOTAL_PRICE, hn::MAX_TOTAL_PRICE);
    let count = hn::TICKET_COUNTS
        .iter()
        .copied()
        .find(|c| total / *c as f64 >= min_ticket)?;
    let price = (total / count as f64).round() as i64;
    Some(Shape {
        count,
        price,
        tokens: hn::tokens_required((price * count) as f64, plex),
    })
}

/// **ISK to spend on tickets this turn** (pure): `share` of what `wallet`
/// holds above `reserve`.
pub fn ticket_budget(wallet: f64, reserve: f64, share: f64) -> f64 {
    ((wallet - reserve) * share).max(0.0)
}

/// One running offer as `Grab` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub raffle_id: i64,
    pub owner_id: i64,
    pub type_id: i64,
    pub count: i64,
    pub price: i64,
    pub sold: i64,
    pub status: i64,
    /// The drawn ticket's owner, when there is one.
    pub winner: Option<i64>,
}

impl Seen {
    /// ISK for every ticket.
    pub fn total_isk(&self) -> f64 {
        (self.count * self.price) as f64
    }
}

/// **Which tickets to buy** (pure): from running offers that are not `me`'s,
/// have a ticket left and are good value (`total <= avg x factor`, give or
/// take the price's rounding; the unit price stands for the prize **[I]**,
/// the offer does not carry its quantity), best value first, each
/// `min(remaining, budget / price, max_tickets)` tickets while the budget
/// lasts.
pub fn pick_raffles(
    seen: &[Seen],
    me: i64,
    mut budget: f64,
    avg: &dyn Fn(i64) -> Option<f64>,
    factor: f64,
    max_tickets: i64,
) -> Vec<(i64, i64)> {
    let mut good: Vec<(f64, &Seen)> = seen
        .iter()
        .filter(|r| r.owner_id != me && r.status == hn::RUNNING && r.sold < r.count && r.price > 0)
        .filter_map(|r| {
            let est = avg(r.type_id)?;
            (est > 0.0 && r.total_isk() <= est * factor + r.count as f64)
                .then(|| (r.total_isk() / est, r))
        })
        .collect();
    good.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.raffle_id.cmp(&b.1.raffle_id)));
    let mut out = Vec::new();
    for (_, r) in good {
        let n = (r.count - r.sold)
            .min((budget / r.price as f64).floor() as i64)
            .min(max_tickets);
        if n >= 1 {
            budget -= n as f64 * r.price as f64;
            out.push((r.raffle_id, n));
        }
    }
    out
}

// ---- the wire -------------------------------------------------------------------

/// The state tuple of a `Raffle` (a `0x23` object: header `((cls,), state)`).
fn state_of(v: &PyValue) -> Option<&[PyValue]> {
    let PyValue::ObjectEx { header, .. } = v else {
        return None;
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return None;
    };
    match h.get(1)? {
        PyValue::Tuple(s) | PyValue::List(s) => Some(s),
        _ => None,
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Real(r) => Some(*r as i64),
        _ => None,
    }
}

/// A `Ticket`'s `owner_id` (a `0x23` object with a state dict).
fn ticket_owner(v: &PyValue) -> Option<i64> {
    let PyValue::ObjectEx { header, .. } = v else {
        return None;
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return None;
    };
    int(crate::dict_get(h.get(1)?, "owner_id"))
}

/// One `Raffle` as the server marshals it: the 18-tuple state, in
/// `__getstate__` order **[V]** (`eve_server::hypernet::raffle_value`).
pub fn raffle_of(v: &PyValue) -> Option<Seen> {
    let s = state_of(v)?;
    Some(Seen {
        raffle_id: int(s.get(1))?,
        owner_id: int(s.get(2))?,
        type_id: int(s.get(6))?,
        count: int(s.get(7))?,
        price: int(s.get(8))?,
        sold: int(s.get(12))?,
        winner: s.get(14).and_then(ticket_owner),
        status: int(s.get(15))?,
    })
}

/// The raffles of a `Grab` (a list) or of `GetMyRaffleHistory` (a
/// `(list, page_size)` tuple).
pub fn raffles_of(v: &PyValue) -> Vec<Seen> {
    let list = match v {
        PyValue::Tuple(t) => t.first(),
        other => Some(other),
    };
    match list {
        Some(PyValue::List(rows)) => rows.iter().filter_map(raffle_of).collect(),
        _ => Vec::new(),
    }
}

/// `RaffleCreationData`, as the client builds it **[V]** (the server reads
/// eleven fields; `solar_system_id` is the client's `None`).
fn creation(station: i64, me: i64, prize: &Prize, token: i64, shape: Shape) -> PyValue {
    eve_server::pyutil::new_style(
        "raffles.common.raffle_data.RaffleCreationData",
        vec![
            ("owner_id", PyValue::Int(me)),
            ("owner_location_id", PyValue::Int(station)),
            ("location_id", PyValue::Int(station)),
            ("solar_system_id", PyValue::None),
            ("token_id", PyValue::Int(token)),
            ("token_location_id", PyValue::Int(station)),
            ("item_id", PyValue::Int(prize.item_id)),
            ("type_id", PyValue::Int(prize.type_id)),
            ("ticket_count", PyValue::Int(shape.count)),
            ("ticket_price", PyValue::Int(shape.price)),
            ("restriction_id", PyValue::None),
        ],
    )
}

/// One `raffleProxy` call. A raised exception (`raffles.*`, no `CallRsp`) is
/// `Ok(Err(text))`; only a dead connection is `Err`.
fn verb(
    conn: &mut Conn,
    method: &str,
    args: Vec<PyValue>,
) -> Result<std::result::Result<PyValue, String>> {
    let out = conn.call("raffleProxy", method, args)?;
    if let Some(key) = out.refusal() {
        return Ok(Err(key));
    }
    Ok(out.value().ok_or_else(|| format!("{method} was raised on")))
}

/// The wallet's ISK and the prices `(avg per type, PLEX)`: the trading index.
fn prices(conn: &mut Conn) -> Result<(BTreeMap<i64, f64>, f64)> {
    let avg: BTreeMap<i64, f64> = conn
        .price_index_shared()?
        .iter()
        .filter_map(|(t, _, a)| Some((*t, (*a)?)))
        .collect();
    let plex = avg
        .get(&hn::PLEX_TYPE_ID)
        .copied()
        .unwrap_or(hn::PLEX_FALLBACK_ISK);
    Ok((avg, plex))
}

/// The best HyperCore stack here: its item id and quantity.
fn core_stack(rows: &[ItemRow], here: i64) -> Option<(i64, i64)> {
    rows.iter()
        .filter(|r| {
            r.type_id == hn::TOKEN_TYPE_ID && r.flag_id == FLAG_HANGAR && r.location_id == here
        })
        .max_by_key(|r| r.quantity)
        .map(|r| (r.item_id, r.quantity))
}

/// Step 1: claim what was won.
fn claim(conn: &mut Conn, st: &mut State, me: i64, notes: &mut Vec<String>) -> Result<Vec<Seen>> {
    let v = match verb(conn, "GetMyRaffleHistory", vec![PyValue::None])? {
        Ok(v) => v,
        Err(e) => {
            notes.push(format!("history: {e}"));
            return Ok(Vec::new());
        }
    };
    let mine = raffles_of(&v);
    for r in mine
        .iter()
        .filter(|r| r.status == hn::FINISHED_UNDELIVERED && r.winner == Some(me))
    {
        match verb(conn, "AwardItem", vec![PyValue::Int(r.raffle_id)])? {
            Ok(_) => {
                st.claimed += 1;
                notes.push(format!("claimed the prize of offer {}", r.raffle_id));
            }
            Err(e) => notes.push(format!("claim {}: {e}", r.raffle_id)),
        }
    }
    Ok(mine)
}

/// Steps 2 and 4: an offer of the surplus stack, with the cores held or
/// bought from the market.
#[allow(clippy::too_many_arguments)]
fn offer(
    conn: &mut Conn,
    st: &mut State,
    station: i64,
    me: i64,
    avg: &BTreeMap<i64, f64>,
    plex: f64,
    now: Instant,
    notes: &mut Vec<String>,
) -> Result<()> {
    let rows = conn.hangar_items()?;
    // What no fit plan wants: whole stacks the loot rule hands out.
    let lots = crate::contracts::stock(crate::contracts::Career::Explorer, &rows);
    let wanted = |r: &ItemRow| {
        !lots
            .iter()
            .any(|l| l.item_id == r.item_id && l.quantity == r.quantity)
    };
    let Some(prize) = surplus_item(
        &rows,
        station,
        &wanted,
        &|t| avg.get(&t).copied(),
        MIN_ITEM_ISK,
    ) else {
        return Ok(());
    };
    let Some(shape) = offer_shape(prize.estimate, plex, MIN_TICKET_ISK) else {
        return Ok(());
    };
    let mut cores = core_stack(&rows, station);
    if cores.is_none_or(|(_, q)| q < shape.tokens) {
        // Step 4: cores from the market's asks (players or bots), never a GM.
        let held = cores.map_or(0, |(_, q)| q);
        let book = book_of(conn, hn::TOKEN_TYPE_ID, notes)?;
        let own = crate::capital::own_ids_soft(conn)?;
        let ask = book
            .as_ref()
            .and_then(|b| crate::capital::cheapest_ask(b, station, &own));
        match (book, ask) {
            (Some(book), Some(a)) if a.price <= CORE_MAX_ISK => {
                let wallet = crate::acquire::spendable(conn)?;
                let budget = wallet.min(a.price * (shape.tokens - held) as f64);
                let got = soft(
                    crate::capital::buy_units(
                        conn,
                        &book,
                        hn::TOKEN_TYPE_ID,
                        shape.tokens - held,
                        budget,
                    ),
                    notes,
                    "buy cores",
                )?
                .unwrap_or(0);
                if got > 0 {
                    notes.push(format!("bought {got} HyperCores at <= {:.0}", a.price));
                }
                cores = core_stack(&conn.hangar_items()?, station);
            }
            _ => {}
        }
    }
    let Some((token, _)) = cores.filter(|(_, q)| *q >= shape.tokens) else {
        st.back_off("offer", now);
        notes.push(format!(
            "offer of {} wants {} HyperCores, none to be had",
            prize.type_id, shape.tokens
        ));
        return Ok(());
    };
    match verb(
        conn,
        "CreateRaffle",
        vec![creation(station, me, &prize, token, shape)],
    )? {
        Ok(id) => {
            st.offered += 1;
            notes.push(format!(
                "offered {}x{} as {} tickets at {} (offer {id:?})",
                prize.quantity, prize.type_id, shape.count, shape.price
            ));
        }
        Err(e) => {
            st.back_off("offer", now);
            notes.push(format!("offer refused: {e}"));
        }
    }
    Ok(())
}

/// Step 3: tickets, with ISK above the reserve.
fn buy_tickets(
    conn: &mut Conn,
    st: &mut State,
    me: i64,
    avg: &BTreeMap<i64, f64>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let budget = ticket_budget(crate::acquire::spendable(conn)?, RESERVE, SPEND);
    if budget <= 0.0 {
        return Ok(());
    }
    let Ok(v) = verb(conn, "Grab", vec![])? else {
        return Ok(());
    };
    let picks = pick_raffles(
        &raffles_of(&v),
        me,
        budget,
        &|t| avg.get(&t).copied(),
        GOOD_VALUE_FACTOR,
        MAX_TICKETS,
    );
    for (id, n) in picks {
        match verb(
            conn,
            "BuyRandomTickets",
            vec![PyValue::Int(id), PyValue::Int(n)],
        )? {
            Ok(_) => {
                st.tickets += n as u64;
                notes.push(format!("bought {n} tickets in offer {id}"));
            }
            Err(e) => notes.push(format!("tickets in {id}: {e}")),
        }
    }
    Ok(())
}

/// **One turn** (module header). A transport fault is the only error.
pub fn turn(conn: &mut Conn, st: &mut State) -> Result<String> {
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Err(Error::NotSent("hypernet: not docked".to_owned()));
    };
    let now = clock::instant();
    let mut notes: Vec<String> = Vec::new();
    let mine = claim(conn, st, me, &mut notes)?;
    let (avg, plex) = prices(conn)?;
    let open = mine
        .iter()
        .filter(|r| r.owner_id == me && r.status == hn::RUNNING)
        .count();
    if open < MAX_OPEN && !st.backed_off("offer", now) {
        offer(conn, st, station, me, &avg, plex, now, &mut notes)?;
    }
    buy_tickets(conn, st, me, &avg, &mut notes)?;
    Ok(if notes.is_empty() {
        "hypernet: nothing to do".to_owned()
    } else {
        format!("hypernet: {}", notes.join("; "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(item_id: i64, type_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 9_000_001,
            flag_id: FLAG_HANGAR,
            quantity,
            group_id: 990_001,
            category_id: 7,
            singleton: false,
        }
    }

    const HERE: i64 = 9_000_001;

    fn avg_of(t: i64) -> Option<f64> {
        match t {
            990_101 => Some(8_000.0),
            990_102 => Some(40.0),
            990_103 => Some(100.0),
            _ => None,
        }
    }

    #[test]
    fn surplus_item_skips_wanted_ore_and_cheap_items() {
        let min = 5_000.0;
        let none = |_: &ItemRow| false;
        let good = row(1, 990_101, 1);
        let cheap = row(2, 990_102, 10);
        let unpriced = row(3, 990_999, 5);
        let mut ore = row(4, 990_101, 1);
        ore.category_id = CATEGORY_ASTEROID;
        let mut mineral = row(5, 990_101, 1);
        mineral.group_id = GROUP_MINERAL;
        let plex = row(6, hn::PLEX_TYPE_ID, 500);
        let core = row(7, hn::TOKEN_TYPE_ID, 500);
        let mut elsewhere = row(8, 990_101, 1);
        elsewhere.location_id = HERE + 1;
        let mut fitted = row(9, 990_101, 1);
        fitted.flag_id = 5;
        let rows = [
            cheap, unpriced, ore, mineral, plex, core, elsewhere, fitted, good,
        ];
        let got = surplus_item(&rows, HERE, &none, &avg_of, min).expect("the good stack");
        assert_eq!((got.item_id, got.estimate), (1, 8_000.0));
        // The cheap stack counts once its quantity makes it worth the line.
        let rows = [row(2, 990_102, 200), cheap];
        assert_eq!(
            surplus_item(&rows, HERE, &none, &avg_of, min).map(|p| p.item_id),
            Some(2)
        );
        // A wanted stack is never offered.
        let wanted = |r: &ItemRow| r.item_id == 1;
        assert_eq!(surplus_item(&[good], HERE, &wanted, &avg_of, min), None);
        // The dearest wins, the lowest id on a tie.
        let rows = [
            row(12, 990_101, 1),
            row(11, 990_101, 1),
            row(13, 990_103, 1),
        ];
        assert_eq!(
            surplus_item(&rows, HERE, &none, &avg_of, min).map(|p| p.item_id),
            Some(11)
        );
    }

    #[test]
    fn offer_shape_follows_the_clamp_the_count_rule_and_the_cores_formula() {
        // 1_000_000 x 1.4 over the smallest count that keeps a ticket at the
        // floor: 8 tickets at 175_000 (the factor is the client's, [V]).
        let s = offer_shape(1_000_000.0, 100.0, 60_000.0).expect("shaped");
        assert_eq!((s.count, s.price), (8, 175_000));
        assert_eq!(s.tokens, hn::tokens_required(1_400_000.0, 100.0));
        // A floor no pool entry can hold shapes nothing.
        assert_eq!(offer_shape(1_000_000.0, 100.0, 10_000_000.0), None);
        // A total big enough for the floor at 8 keeps 8 (the smallest).
        let s = offer_shape(100_000_000.0, 100.0, 60_000.0).expect("shaped");
        assert_eq!(s.count, 8);
        // The total is clamped to the server's range.
        let big = offer_shape(1.0e15, 100.0, 60_000.0).expect("shaped");
        assert!(((big.price * big.count) as f64) <= hn::MAX_TOTAL_PRICE + 8.0);
        // Nothing shapes from a bad estimate.
        assert_eq!(offer_shape(f64::NAN, 100.0, 1.0), None);
        assert_eq!(offer_shape(-5.0, 100.0, 1.0), None);
    }

    #[test]
    fn ticket_budget_respects_the_reserve_and_the_share() {
        assert_eq!(ticket_budget(100.0, 100.0, 0.5), 0.0);
        assert_eq!(ticket_budget(50.0, 100.0, 0.5), 0.0);
        assert_eq!(ticket_budget(300.0, 100.0, 0.25), 50.0);
    }

    fn seen(id: i64, owner: i64, type_id: i64, count: i64, price: i64, sold: i64) -> Seen {
        Seen {
            raffle_id: id,
            owner_id: owner,
            type_id,
            count,
            price,
            sold,
            status: hn::RUNNING,
            winner: None,
        }
    }

    #[test]
    fn pick_raffles_excludes_bad_value_own_offers_and_full_ones() {
        let me = 7;
        let all = [
            // good value: total 8 x 150 = 1_200 <= 8_000 x 0.2
            seen(1, 5, 990_101, 8, 150, 2),
            // bad value: 8 x 900 = 7_200 over 8_000 x 0.5
            seen(2, 5, 990_101, 8, 900, 0),
            // my own
            seen(3, me, 990_101, 8, 150, 0),
            // sold out
            seen(4, 5, 990_101, 8, 150, 8),
            // unpriced type
            seen(5, 5, 990_999, 8, 1, 0),
        ];
        let got = pick_raffles(&all, me, 10_000.0, &avg_of, 0.5, 4);
        assert_eq!(got, vec![(1, 4)]);
        // The cap, the stock left and the budget each bound the count.
        let got = pick_raffles(&all[..1], me, 10_000.0, &avg_of, 0.5, 9);
        assert_eq!(got, vec![(1, 6)], "6 left");
        let got = pick_raffles(&all[..1], me, 310.0, &avg_of, 0.5, 4);
        assert_eq!(got, vec![(1, 2)], "310 buys two at 150");
        assert!(pick_raffles(&all[..1], me, 100.0, &avg_of, 0.5, 4).is_empty());
        // A drawn offer is not for sale.
        let mut done = seen(6, 5, 990_101, 8, 150, 1);
        done.status = hn::FINISHED_UNDELIVERED;
        assert!(pick_raffles(&[done], me, 1e9, &avg_of, 0.5, 4).is_empty());
    }

    #[test]
    fn a_turn_is_due_by_how_busy_the_bot_is() {
        let t0 = Instant::now();
        assert!(
            !due_at(None, t0, true),
            "the first look only starts the clock"
        );
        assert!(!due_at(
            Some(t0),
            t0 + RECHECK - Duration::from_secs(1),
            true
        ));
        assert!(due_at(Some(t0), t0 + RECHECK, true));
        assert!(!due_at(Some(t0), t0 + RECHECK, false), "idle waits longer");
        assert!(due_at(Some(t0), t0 + IDLE_RECHECK, false));
        let mut st = State::default();
        assert!(!st.engaged());
        st.tickets = 1;
        assert!(st.engaged());
    }

    #[test]
    fn the_raffle_the_server_marshals_reads_back() {
        let r = hn::Raffle {
            raffle_id: 3,
            running_id: 4,
            owner_id: 5,
            location_id: 9_000_001,
            system_id: 1,
            item_id: 8,
            type_id: 990_101,
            count: 8,
            price: 150,
            restriction: 0,
            created: 1,
            expires: 2,
            status: hn::FINISHED_UNDELIVERED,
            winner_number: 0,
            end_date: 2,
            is_copy: None,
            sold: 8,
            tickets: Vec::new(),
            winner: Some(hn::TicketRec {
                running_id: 1,
                raffle_id: 3,
                owner_id: 6,
                number: 0,
            }),
        };
        let got = raffle_of(&hn::raffle_value(&r, false)).expect("reads");
        assert_eq!(
            (got.raffle_id, got.owner_id, got.count, got.price, got.sold),
            (3, 5, 8, 150, 8)
        );
        assert_eq!(
            (got.status, got.winner),
            (hn::FINISHED_UNDELIVERED, Some(6))
        );
        let list = PyValue::Tuple(vec![
            PyValue::List(vec![hn::raffle_value(&r, false)]),
            PyValue::Int(50),
        ]);
        assert_eq!(raffles_of(&list).len(), 1);
    }
}
