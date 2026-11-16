//! **HyperNet** (#203, HN-1): the client's raffle window, `raffleProxy`. A
//! seller escrows an item and burns HyperCores to open an offer; buyers pay
//! ISK for numbered tickets; when the last ticket sells a ticket is drawn,
//! the seller is paid and the winner may claim the item. An offer nobody
//! finishes expires after three days and everything goes back.
//!
//! The wire is `service/hypernet.rs` (arms only); the client is
//! `raffles/**` (3670638, decompiled for the spec,
//! `parity-dailygoals-hypernet.md` §3).
//!
//! **Nothing from nowhere.**
//! - The prize is the seller's own item, moved (not copied) to an escrow
//!   location ([`escrow_slot`]) at creation, to the winner on [`award`], back
//!   to the seller on expiry. It moves exactly once from escrow.
//! - Tickets are the buyers' ISK, debited at once (ref 143, `Flow::Held`).
//!   Held ISK is the sum of the sold tickets of the running offers
//!   ([`held_isk`]). A sell-out pays the seller the total (144) and takes the
//!   5 % fee from the seller (145): the fee is a plain sink (owner ruling
//!   2026-11-07, there is no pool). An expiry refunds each ticket (146).
//! - HyperCores consumed at creation are destroyed and never refunded **[A]**.
//!
//! **State**, delta kind [`KIND`] (`raffle`), every value a string:
//!
//! ```text
//! next              -> next_raffle_id next_running_id
//! r <id>            -> running_id owner location system item type count price
//!                      restriction(0|1) created expires status winner(-1)
//!                      end_date(0) is_copy(0|1|-) sold(-)
//! t <id> <number>   -> owner running_id
//! h <id>            -> number:owner:running_id, one string per ticket
//! ```
//!
//! **Compaction** (#276): a finished or expired offer keeps its sold count in
//! `r` column 15 (`-` while running or in a world from before the column) and
//! folds every `t` row but the winner's into one `h` row, about 20 bytes a
//! ticket in place of about 300. The winner's `t` row stays: [`get`] and
//! [`award`] read it. Every reader sees the same tickets as before
//! (`tickets_of`, `holds`, `sold_count`). [`prune_at`] drops the rows of
//! delivered and expired offers after [`KEEP`].
//!
//! Times are FILETIME on the wall clock, as the client reads them.
//! Subscriptions (who is told what) are a world slot ([`State`]), not
//! persisted; it also caches the soonest expiry so [`tick`] is one
//! comparison while nothing is due.

use crate::damage::BlueTime;
use crate::item::{Edit, Slot, escrow_location};
use crate::model::FLAG_HANGAR;
use crate::notifications::{self, Value, data};
use crate::notify::Push;
use crate::seed::{self, Kind};
use crate::service::{Caller, Registry};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::{ObjectForm, PyValue};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "raffle";

/// `typeRaffleToken`, the HyperCore **[V]** (`inventorycommon/const.py:2597`;
/// `raffles/common/const.py` `TOKEN_TYPE_ID`).
pub const TOKEN_TYPE_ID: i64 = 52_568;
pub use crate::econ_prices::{PLEX_FALLBACK_ISK, PLEX_TYPE_ID};
/// `VALIDATION_TYPE_LIST_ID` and `ALLOWED_SINGLETON_TYPE_LIST_ID` **[V]**.
/// List 133 is a **deny** list: `is_type_valid_for_hypernet` is
/// `is_tradable(type) and type not in list 133` (`raffles/common/util.py`),
/// so PLEX and HyperCores themselves cannot be offered. (The spec read it the
/// other way round; the client code is the authority.) `is_tradable` (the
/// non-tradable and soulbound lists) is not checked here **[A]**; falsifier:
/// a soulbound item that reaches an offer.
pub const VALIDATION_LIST: i64 = 133;
pub const SINGLETON_LIST: i64 = 134;
/// `TICKET_COUNT_POOL`, `MIN_TOTAL_PRICE`, `MAX_TOTAL_PRICE` **[V]**.
pub const TICKET_COUNTS: [i64; 4] = [8, 16, 48, 512];
pub const MIN_TOTAL_PRICE: f64 = 1000.0;
pub const MAX_TOTAL_PRICE: f64 = 10_000_000_000_000.0;
/// `RAFFLE_TAX_PERCENTAGE` and `BROKERS_FEE` **[V]**.
pub const TAX: f64 = 0.05;
pub const BROKERS_FEE: f64 = 0.05;
/// `RAFFLE_DURATION = 3 * DAY` **[V]**, in FILETIME ticks.
pub const DAY: i64 = 864_000_000_000;
pub const DURATION: i64 = 3 * DAY;

/// `RaffleStatus` **[V]** (`raffles/common/const.py`).
pub const RUNNING: i64 = 6;
pub const FINISHED_UNDELIVERED: i64 = 7;
pub const FINISHED_DELIVERED: i64 = 8;
pub const FINISHED_EXPIRED: i64 = 9;

/// Raffle tickets inside `item::escrow_location`'s one band. **[A]**: market
/// uses 10^12 and trade 2·10^12; falsified by a collision test
/// (`the_escrow_band_is_clear_of_the_other_bases`).
pub const ESCROW_RAFFLE_BASE: i64 = 3_000_000_000_000;
/// Who owns an escrowed item (as contracts and the market do).
pub const ESCROW_OWNER: i64 = 1;
/// The first raffle id. **[A]**: the client only compares and links them.
pub const FIRST_RAFFLE_ID: i64 = 1;
/// Raffles a browse page holds at most. **[A]**
pub const GRAB_SIZE: usize = 48;
/// Raffles a history page holds. **[A]**
pub const PAGE: usize = 50;

/// How long a delivered (8) or expired (9) offer stays in the history before
/// [`prune_at`] drops its rows. **[A]**; falsifier: a TQ history page older
/// than 30 days.
pub const KEEP: i64 = 30 * DAY;

/// Mixed into the draw stream so no other loot stream shares it.
const HN_TAG: u64 = 0x4859_5045_524e_4554;

/// Journal refs **[V]** (`ledger.rs` table, `appConst`).
pub const REF_TICKET: i64 = 143;
pub const REF_PAYOUT: i64 = 144;
pub const REF_FEE: i64 = 145;
pub const REF_REFUND: i64 = 146;

/// Notification types **[V]** (`notifications.rs` group 15).
pub const NOTE_CREATED: i64 = 243;
pub const NOTE_EXPIRED: i64 = 244;
pub const NOTE_FINISHED: i64 = 245;

/// `invConst.categoryBlueprint` **[V]** (also the client's browse filter).
const CATEGORY_BLUEPRINT: i64 = 9;

/// The journal's "the system" owner.
const SYSTEM_OWNER: i64 = 1;

// ---------------------------------------------------------------------------
// The slot
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Sub {
    all: bool,
    raffles: BTreeSet<i64>,
}

/// Who listens to what, the ids of the running offers and the soonest expiry
/// among them. Not persisted: the first stage run after a load scans the rows
/// once (`primed`), and every later change reaches the slot through
/// [`note_running`], [`finish`] and [`expire`], never through a rescan.
#[derive(Debug, Default)]
pub struct State {
    subs: BTreeMap<i64, Sub>,
    running: BTreeSet<i64>,
    primed: bool,
    due: i64,
}

#[cfg(test)]
thread_local! {
    /// Full scans of the rows by the stage (test builds only).
    static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Leave the rows of a finished offer as they were (a test snapshots
    /// them, then calls `compact` itself).
    static NO_COMPACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A fixed draw salt for a test.
    static SALT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// A running offer `id` that expires at `expires`: remember it, and bring the
/// slot's soonest expiry forward.
fn note_running(reg: &mut Registry, id: i64, expires: i64) {
    let st = reg.ext_mut::<State>();
    st.running.insert(id);
    st.due = st.due.min(expires);
}

/// The ids of every running offer, by one pass over the rows.
fn scan_running(reg: &Registry) -> BTreeSet<i64> {
    #[cfg(test)]
    SCANS.with(|c| c.set(c.get() + 1));
    ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, false))
        .filter(|r| r.status == RUNNING)
        .map(|r| r.raffle_id)
        .collect()
}

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "hypernet",
    make: |_| Box::new(State::default()) as Boxed,
};

/// `SubscribeToTickets` / `UnSubscribeFromTickets`.
pub fn subscribe_tickets(reg: &mut Registry, who: i64, on: bool) {
    let st = reg.ext_mut::<State>();
    if on {
        st.subs.entry(who).or_default().all = true;
    } else if let Some(s) = st.subs.get_mut(&who) {
        s.all = false;
    }
}

/// `SubscribeToRaffle` / `UnsubscribeFromRaffle`.
pub fn subscribe_raffle(reg: &mut Registry, who: i64, id: i64, on: bool) {
    let st = reg.ext_mut::<State>();
    if on {
        st.subs.entry(who).or_default().raffles.insert(id);
    } else if let Some(s) = st.subs.get_mut(&who) {
        s.raffles.remove(&id);
    }
}

/// Characters to tell about raffle `id` (`all == false`) or about tickets
/// (`all == true`), without a session filter.
fn listeners(reg: &Registry, id: Option<i64>) -> Vec<i64> {
    reg.ext::<State>()
        .subs
        .iter()
        .filter(|(_, s)| match id {
            Some(id) => s.raffles.contains(&id),
            None => s.all,
        })
        .map(|(c, _)| *c)
        .collect()
}

/// Keep characters with a session; a gone one is dropped from the slot.
fn online(reg: &mut Registry, chars: Vec<i64>) -> Vec<i64> {
    let (here, gone): (Vec<i64>, Vec<i64>) = chars
        .into_iter()
        .partition(|c| !reg.sessions().of_character(*c).is_empty());
    if !gone.is_empty() {
        let st = reg.ext_mut::<State>();
        for c in gone {
            st.subs.remove(&c);
        }
    }
    here
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One sold ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRec {
    pub running_id: i64,
    pub raffle_id: i64,
    pub owner_id: i64,
    pub number: i64,
}

/// One raffle, read from its rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Raffle {
    pub raffle_id: i64,
    pub running_id: i64,
    pub owner_id: i64,
    pub location_id: i64,
    pub system_id: i64,
    pub item_id: i64,
    pub type_id: i64,
    pub count: i64,
    pub price: i64,
    /// `0` public, `1` private.
    pub restriction: i64,
    pub created: i64,
    pub expires: i64,
    pub status: i64,
    /// `-1` until drawn.
    pub winner_number: i64,
    pub end_date: i64,
    /// `None` for anything but a blueprint.
    pub is_copy: Option<bool>,
    pub sold: i64,
    /// Filled only by a load with tickets.
    pub tickets: Vec<TicketRec>,
    /// The drawn ticket, loaded with the raffle whenever there is one.
    pub winner: Option<TicketRec>,
}

impl Raffle {
    pub fn total(&self) -> i64 {
        self.count * self.price
    }

    /// The drawn ticket, when tickets are loaded.
    pub fn winning_ticket(&self) -> Option<&TicketRec> {
        self.winner.as_ref()
    }
}

fn n(v: &[String], i: usize) -> i64 {
    v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn rkey(id: i64) -> Vec<String> {
    vec!["r".to_owned(), id.to_string()]
}

fn tkey(id: i64, number: i64) -> Vec<String> {
    vec!["t".to_owned(), id.to_string(), number.to_string()]
}

fn parse(id: i64, v: &[String]) -> Raffle {
    Raffle {
        raffle_id: id,
        running_id: n(v, 0),
        owner_id: n(v, 1),
        location_id: n(v, 2),
        system_id: n(v, 3),
        item_id: n(v, 4),
        type_id: n(v, 5),
        count: n(v, 6),
        price: n(v, 7),
        restriction: n(v, 8),
        created: n(v, 9),
        expires: n(v, 10),
        status: n(v, 11),
        winner_number: v.get(12).and_then(|s| s.parse().ok()).unwrap_or(-1),
        end_date: n(v, 13),
        is_copy: match v.get(14).map(String::as_str) {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        },
        sold: 0,
        tickets: Vec::new(),
        winner: None,
    }
}

fn row_of(r: &Raffle) -> Vec<String> {
    vec![
        r.running_id.to_string(),
        r.owner_id.to_string(),
        r.location_id.to_string(),
        r.system_id.to_string(),
        r.item_id.to_string(),
        r.type_id.to_string(),
        r.count.to_string(),
        r.price.to_string(),
        r.restriction.to_string(),
        r.created.to_string(),
        r.expires.to_string(),
        r.status.to_string(),
        r.winner_number.to_string(),
        r.end_date.to_string(),
        match r.is_copy {
            Some(true) => "1",
            Some(false) => "0",
            None => "-",
        }
        .to_owned(),
    ]
}

/// Rows with the key prefix `prefix`, in key order.
fn with_prefix(
    reg: &Registry,
    prefix: Vec<String>,
) -> impl Iterator<Item = (&Vec<String>, &Vec<String>)> {
    let p2 = prefix.clone();
    reg.deltas()
        .of(KIND)
        .range(prefix..)
        .take_while(move |(k, _)| k.starts_with(&p2))
}

fn hkey(id: i64) -> Vec<String> {
    vec!["h".to_owned(), id.to_string()]
}

/// The tickets folded into raffle `id`'s `h` row (every one but the winner's),
/// or `None` while it holds none.
fn compacted(reg: &Registry, id: i64) -> Option<Vec<TicketRec>> {
    let row = reg.deltas().of(KIND).get(&hkey(id))?;
    Some(
        row.iter()
            .filter_map(|s| {
                let mut p = s.split(':').map(|x| x.parse::<i64>());
                Some(TicketRec {
                    number: p.next()?.ok()?,
                    owner_id: p.next()?.ok()?,
                    running_id: p.next()?.ok()?,
                    raffle_id: id,
                })
            })
            .collect(),
    )
}

fn tickets_of(reg: &Registry, id: i64) -> Vec<TicketRec> {
    let mut out: Vec<TicketRec> = with_prefix(reg, vec!["t".to_owned(), id.to_string()])
        .filter_map(|(k, v)| {
            Some(TicketRec {
                running_id: n(v, 1),
                raffle_id: id,
                owner_id: n(v, 0),
                number: k.get(2)?.parse().ok()?,
            })
        })
        .collect();
    out.extend(compacted(reg, id).unwrap_or_default());
    out.sort_by_key(|t| t.number);
    out
}

fn sold_count(reg: &Registry, id: i64) -> i64 {
    if let Some(k) = reg
        .deltas()
        .of(KIND)
        .get(&rkey(id))
        .and_then(|v| v.get(15))
        .and_then(|s| s.parse::<i64>().ok())
    {
        return k;
    }
    with_prefix(reg, vec!["t".to_owned(), id.to_string()]).count() as i64
}

/// One raffle; its tickets only when `with_tickets`.
pub fn get(reg: &Registry, id: i64, with_tickets: bool) -> Option<Raffle> {
    let v = reg.deltas().of(KIND).get(&rkey(id))?;
    let mut r = parse(id, v);
    if r.winner_number >= 0
        && let Some(w) = reg.deltas().of(KIND).get(&tkey(id, r.winner_number))
    {
        r.winner = Some(TicketRec {
            running_id: n(w, 1),
            raffle_id: id,
            owner_id: n(w, 0),
            number: r.winner_number,
        });
    }
    if with_tickets {
        r.tickets = tickets_of(reg, id);
        r.sold = r.tickets.len() as i64;
    } else {
        r.sold = sold_count(reg, id);
    }
    Some(r)
}

/// Write the row; the sold column (15) stays as it was.
fn put_raffle(reg: &mut Registry, r: &Raffle) {
    let sold = reg
        .deltas()
        .of(KIND)
        .get(&rkey(r.raffle_id))
        .and_then(|v| v.get(15).cloned())
        .unwrap_or_else(|| "-".to_owned());
    let mut row = row_of(r);
    row.push(sold);
    reg.deltas_mut().put(KIND, rkey(r.raffle_id), row);
    reg.mark_dirty();
}

/// Fold a finished offer's tickets (#276): the sold count into column 15, every
/// `t` row but the winner's into one `h` row. Never touches a running offer.
/// Idempotent.
fn compact(reg: &mut Registry, id: i64) {
    #[cfg(test)]
    if NO_COMPACT.with(|c| c.get()) {
        return;
    }
    let Some(r) = get(reg, id, true) else {
        return;
    };
    if r.status == RUNNING {
        return;
    }
    let fold: Vec<&TicketRec> = r
        .tickets
        .iter()
        .filter(|t| t.number != r.winner_number)
        .collect();
    for t in &fold {
        reg.deltas_mut().del(KIND, &tkey(id, t.number));
    }
    if fold.is_empty() {
        reg.deltas_mut().del(KIND, &hkey(id));
    } else {
        let row = fold
            .iter()
            .map(|t| format!("{}:{}:{}", t.number, t.owner_id, t.running_id))
            .collect();
        reg.deltas_mut().put(KIND, hkey(id), row);
    }
    let mut row = row_of(&r);
    row.push(r.tickets.len().to_string());
    reg.deltas_mut().put(KIND, rkey(id), row);
    reg.mark_dirty();
}

/// Whether [`prune_at`] drops this offer at `now`.
fn prunable(r: &Raffle, now: i64) -> bool {
    matches!(r.status, FINISHED_DELIVERED | FINISHED_EXPIRED) && r.end_date + KEEP < now
}

/// Drop the `r`, `h` and `t` rows of every delivered (8) or expired (9) offer
/// that ended more than [`KEEP`] before `now`, and its subscriptions. Status 6
/// and 7 keep their rows (the prize is in escrow). Returns the offers dropped.
pub fn prune_at(reg: &mut Registry, now: i64) -> usize {
    let gone: Vec<i64> = ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, false))
        .filter(|r| prunable(r, now))
        .map(|r| r.raffle_id)
        .collect();
    for id in &gone {
        let keys: Vec<Vec<String>> = ["t", "h", "r"]
            .iter()
            .flat_map(|p| with_prefix(reg, vec![(*p).to_owned(), id.to_string()]))
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            reg.deltas_mut().del(KIND, &k);
        }
        for s in reg.ext_mut::<State>().subs.values_mut() {
            s.raffles.remove(id);
        }
    }
    if !gone.is_empty() {
        reg.mark_dirty();
    }
    gone.len()
}

/// The rollover step `hypernet_prune` (a maintainer line in
/// `rollover_steps.rs`): its count is the offers dropped.
pub fn on_rollover(reg: &mut Registry, _d: &crate::rollover::Day) -> usize {
    prune_at(reg, crate::pyutil::filetime_now())
}

/// Every raffle id, ascending.
pub fn ids(reg: &Registry) -> Vec<i64> {
    let mut v: Vec<i64> = with_prefix(reg, vec!["r".to_owned()])
        .filter(|(k, _)| k.len() == 2)
        .filter_map(|(k, _)| k[1].parse().ok())
        .collect();
    v.sort_unstable();
    v
}

fn next_ids(reg: &mut Registry) -> (i64, i64) {
    let key = vec!["next".to_owned()];
    let (a, b) = reg
        .deltas()
        .of(KIND)
        .get(&key)
        .map_or((FIRST_RAFFLE_ID, 1), |v| (n(v, 0), n(v, 1)));
    reg.deltas_mut()
        .put(KIND, key, vec![(a + 1).to_string(), (b + 1).to_string()]);
    (a, b)
}

/// A fresh running id for a ticket, from the same counter.
fn next_running(reg: &mut Registry) -> i64 {
    let key = vec!["next".to_owned()];
    let (a, b) = reg
        .deltas()
        .of(KIND)
        .get(&key)
        .map_or((FIRST_RAFFLE_ID, 1), |v| (n(v, 0), n(v, 1)));
    reg.deltas_mut()
        .put(KIND, key, vec![a.to_string(), (b + 1).to_string()]);
    b
}

/// Where the prize waits.
pub fn escrow_slot(id: i64) -> Slot {
    Slot::new(
        ESCROW_OWNER,
        escrow_location(ESCROW_RAFFLE_BASE + id),
        FLAG_HANGAR,
    )
}

/// ISK in tickets of running offers: held for the draw or the refund.
pub fn held_isk(reg: &Registry) -> f64 {
    ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, false))
        .filter(|r| r.status == RUNNING)
        .map(|r| (r.sold * r.price) as f64)
        .sum()
}

// ---------------------------------------------------------------------------
// Money
// ---------------------------------------------------------------------------

fn cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Credit `who` with `amount` under `ref_id`: balance, one journal row in
/// their own wallet, the balance push.
fn pay(reg: &mut Registry, who: i64, amount: f64, ref_id: i64, reference: i64) {
    let Some(c) = reg.character_mut(who) else {
        crate::outln!(
            "[hypernet] {amount} ISK (ref {ref_id}, raffle {reference}) for missing character {who} is lost"
        );
        return;
    };
    c.balance += amount;
    let balance = c.balance;
    reg.journal_mut().record(
        crate::pyutil::filetime_now(),
        ref_id,
        SYSTEM_OWNER,
        who,
        reference,
        amount,
        balance,
        "",
    );
    reg.mark_dirty();
    reg.schedule(
        Duration::ZERO,
        crate::notify::AccountChange {
            owner_id: who,
            balance,
        }
        .to_owner(),
    );
}

/// `tokens_required` of the client (`raffles/common/util.py`), with true
/// division: `max(1, int(total / max(1, plex / 10) * 0.05))` **[V]**.
pub fn tokens_required(total: f64, plex: f64) -> i64 {
    let per = (plex / 10.0).max(1.0);
    ((total / per * BROKERS_FEE).trunc() as i64).max(1)
}

/// The PLEX price the client formula uses: the trading index, or the
/// fallback.
pub fn plex_price(reg: &Registry) -> f64 {
    crate::econ_prices::average(reg, PLEX_TYPE_ID).unwrap_or(PLEX_FALLBACK_ISK)
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// `RaffleCreationData`, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Creation {
    pub owner_location_id: i64,
    pub location_id: i64,
    pub solar_system_id: i64,
    pub token_id: i64,
    pub item_id: i64,
    pub type_id: i64,
    pub ticket_count: i64,
    pub ticket_price: i64,
    /// `None` public.
    pub restriction_id: Option<i64>,
}

/// Why a create was refused: `raffles.CreateError(name)`, names from the
/// spec's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    ItemOwner,
    ItemType,
    ItemSingleton,
    ItemInventory,
    ItemLocation,
    ItemTriglavianSystem,
    TypeMismatch,
    TokenType,
    TokenOwner,
    TokenLocation,
    TokenInventory,
    TokenAmount,
    TicketPrice,
    TicketCount,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::ItemOwner => "ItemOwnerError",
            Reason::ItemType => "ItemTypeError",
            Reason::ItemSingleton => "ItemSingletonError",
            Reason::ItemInventory => "ItemInventoryError",
            Reason::ItemLocation => "ItemLocationError",
            Reason::ItemTriglavianSystem => "ItemTriglavianSystemError",
            Reason::TypeMismatch => "TypeMismatchError",
            Reason::TokenType => "TokenTypeError",
            Reason::TokenOwner => "TokenOwnerError",
            Reason::TokenLocation => "TokenLocationError",
            Reason::TokenInventory => "TokenInventoryError",
            Reason::TokenAmount => "TokenAmountError",
            Reason::TicketPrice => "TicketPriceError",
            Reason::TicketCount => "TicketCountError",
        }
    }
}

/// `idCheckers.IsStation` **[V]** (`inventorycommon/const.py:59-62`).
pub fn is_station(id: i64) -> bool {
    (60_000_000..=63_999_999).contains(&id)
}

fn in_list(reg: &Registry, list: i64, item: &crate::item::Item) -> Option<bool> {
    reg.content()
        .try_get::<crate::dungeon_data::Table>()?
        .typelists
        .get(&list)
        .map(|l| l.contains(item.type_id, item.group_id, item.category_id))
}

fn log_once_no_list() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        crate::outln!(
            "[hypernet] typelist {VALIDATION_LIST} is not loaded: no type check on offers"
        );
    }
}

/// The offer's solar system. The client sends `solar_system_id=None`
/// (`create/controller.py` `create`) **[V]**, so it is the station's; the
/// request's own value only when the station is not in the table.
fn system_of(reg: &Registry, station: i64, d: &Creation) -> i64 {
    reg.station_ref(station)
        .map_or(d.solar_system_id, |s| s.solar_system_id)
}

/// Validate `d` for `actor` in the client's order. Moves nothing.
fn validate(reg: &Registry, actor: i64, d: &Creation) -> Result<i64, Reason> {
    let item = reg
        .store()
        .get(d.item_id)
        .filter(|i| i.owner_id == actor)
        .ok_or(Reason::ItemOwner)?;
    match in_list(reg, VALIDATION_LIST, item) {
        Some(true) => return Err(Reason::ItemType),
        Some(false) => {}
        None => log_once_no_list(),
    }
    // `_validate_singleton`: a mutated (dynamic) type is allowed, else the
    // type must be in list 134.
    if item.singleton
        && !crate::mutator_data::is_dynamic_type(reg.content(), item.type_id)
        && in_list(reg, SINGLETON_LIST, item) != Some(true)
    {
        return Err(Reason::ItemSingleton);
    }
    if item.flag_id != FLAG_HANGAR {
        return Err(Reason::ItemInventory);
    }
    if !is_station(item.location_id) {
        return Err(Reason::ItemLocation);
    }
    if crate::pochven::in_pochven(reg, system_of(reg, item.location_id, d)) {
        return Err(Reason::ItemTriglavianSystem);
    }
    if d.type_id != item.type_id {
        return Err(Reason::TypeMismatch);
    }
    let token = reg.store().get(d.token_id);
    let token = token
        .filter(|t| t.type_id == TOKEN_TYPE_ID)
        .ok_or(Reason::TokenType)?;
    if token.owner_id != actor {
        return Err(Reason::TokenOwner);
    }
    if !is_station(token.location_id) {
        return Err(Reason::TokenLocation);
    }
    if token.flag_id != FLAG_HANGAR {
        return Err(Reason::TokenInventory);
    }
    let total = (d.ticket_count as f64) * (d.ticket_price as f64);
    let need = tokens_required(total, plex_price(reg));
    if token.quantity < need {
        return Err(Reason::TokenAmount);
    }
    if !(MIN_TOTAL_PRICE..=MAX_TOTAL_PRICE).contains(&total) {
        return Err(Reason::TicketPrice);
    }
    if !TICKET_COUNTS.contains(&d.ticket_count) {
        return Err(Reason::TicketCount);
    }
    Ok(need)
}

/// Open an offer: consume the HyperCores, escrow the item, write the row.
pub fn create(reg: &mut Registry, actor: i64, d: &Creation) -> Result<i64, Reason> {
    let need = validate(reg, actor, d)?;
    let item = reg
        .store()
        .get(d.item_id)
        .cloned()
        .ok_or(Reason::ItemOwner)?;
    let id = {
        let key = vec!["next".to_owned()];
        reg.deltas()
            .of(KIND)
            .get(&key)
            .map_or(FIRST_RAFFLE_ID, |v| n(v, 0))
    };
    let edits = [
        Edit::Consume {
            item_id: d.token_id,
            quantity: need,
        },
        Edit::Move {
            item_id: d.item_id,
            to: escrow_slot(id),
        },
    ];
    // The same stack as prize and token conflicts here; refused as a token
    // shortfall (nothing has moved, `apply_items` is all or none).
    reg.apply_items(&edits).map_err(|_| Reason::TokenAmount)?;
    let (id, running) = next_ids(reg);
    debug_assert!(
        crate::item::ESCROW_BASE + ESCROW_RAFFLE_BASE + id < crate::ess::ESS_ID_BASE,
        "a raffle escrow location reached the ess band"
    );
    let now = crate::pyutil::filetime_now();
    let is_copy = (item.category_id == CATEGORY_BLUEPRINT).then(|| {
        reg.jobs()
            .blueprints()
            .get(item.item_id)
            .is_some_and(|s| s.runs_remaining != -1)
    });
    let r = Raffle {
        raffle_id: id,
        running_id: running,
        owner_id: actor,
        location_id: item.location_id,
        system_id: system_of(reg, item.location_id, d),
        item_id: item.item_id,
        type_id: item.type_id,
        count: d.ticket_count,
        price: d.ticket_price,
        restriction: i64::from(d.restriction_id.is_some()),
        created: now,
        expires: now + DURATION,
        status: RUNNING,
        winner_number: -1,
        end_date: 0,
        is_copy,
        sold: 0,
        tickets: Vec::new(),
        winner: None,
    };
    put_raffle(reg, &r);
    note_running(reg, id, r.expires);
    notifications::notify(reg, actor, NOTE_CREATED, note_data(&r));
    // The create controller compares against the id the call returns, so the
    // push leaves after the answer **[A]**.
    reg.push_later(
        Duration::from_millis(500),
        Created {
            to: vec![actor],
            raffle: r,
        },
    );
    Ok(id)
}

fn note_data(r: &Raffle) -> notifications::Data {
    data([
        ("type_id", Value::Int(r.type_id)),
        ("raffle_id", Value::Int(r.raffle_id)),
        ("ticket_price", Value::Int(r.price)),
        ("ticket_count", Value::Int(r.count)),
        ("owner_id", Value::Int(r.owner_id)),
        ("location_id", Value::Int(r.location_id)),
    ])
}

// ---------------------------------------------------------------------------
// Buy, draw
// ---------------------------------------------------------------------------

/// Which ticket(s) to buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Number(i64),
    Random(i64),
}

/// Why a buy was refused (`raffles.TicketUnavailableError`,
/// `raffles.NotEnoughISKError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyError {
    TicketUnavailable,
    NotEnoughIsk,
}

/// Buy tickets. The owner may buy **[A]** (no client guard; falsifier: the
/// owner's answer). A sell-out finishes the offer at once.
pub fn buy(reg: &mut Registry, actor: i64, id: i64, pick: Pick) -> Result<(), BuyError> {
    let now = crate::pyutil::filetime_now();
    let r = get(reg, id, true)
        .filter(|r| r.status == RUNNING && r.expires > now)
        .ok_or(BuyError::TicketUnavailable)?;
    let taken: BTreeSet<i64> = r.tickets.iter().map(|t| t.number).collect();
    let numbers: Vec<i64> = match pick {
        Pick::Number(x) => {
            if !(0..r.count).contains(&x) || taken.contains(&x) {
                return Err(BuyError::TicketUnavailable);
            }
            vec![x]
        }
        Pick::Random(k) => {
            let mut free: Vec<i64> = (0..r.count).filter(|x| !taken.contains(x)).collect();
            if k <= 0 || k as usize > free.len() {
                return Err(BuyError::TicketUnavailable);
            }
            let k = k as usize;
            let mut s = seed::stream(
                reg.world().seed,
                Kind::Loot,
                &[HN_TAG, id as u64, r.sold as u64],
            );
            for i in 0..k {
                let j = i + s.range(0, (free.len() - i) as u64) as usize;
                free.swap(i, j);
            }
            let mut picked = free[..k].to_vec();
            picked.sort_unstable();
            picked
        }
    };
    let cost = (r.price as f64) * (numbers.len() as f64);
    if reg.character(actor).is_none_or(|c| c.balance < cost) {
        return Err(BuyError::NotEnoughIsk);
    }
    for num in &numbers {
        reg.charge(actor, r.price as f64, REF_TICKET, r.owner_id, id);
        let running = next_running(reg);
        reg.deltas_mut().put(
            KIND,
            tkey(id, *num),
            vec![actor.to_string(), running.to_string()],
        );
    }
    reg.mark_dirty();
    let sold = r.sold + numbers.len() as i64;
    if sold >= r.count {
        finish(reg, id);
    } else if let Some(now) = get(reg, id, true) {
        tell_update(reg, &now);
    }
    Ok(())
}

fn tell_update(reg: &mut Registry, r: &Raffle) {
    let a = listeners(reg, Some(r.raffle_id));
    let a = online(reg, a);
    if !a.is_empty() {
        reg.schedule(
            Duration::ZERO,
            Updated {
                to: a,
                raffle: r.clone(),
            },
        );
    }
    let t = listeners(reg, None);
    let t = online(reg, t);
    if !t.is_empty() {
        reg.schedule(
            Duration::ZERO,
            TicketsUpdated {
                to: t,
                counts: vec![(r.raffle_id, r.sold)],
            },
        );
    }
}

/// Everyone with a stake: the owner and each distinct ticket holder.
fn stakeholders(r: &Raffle) -> Vec<i64> {
    let mut v: BTreeSet<i64> = r.tickets.iter().map(|t| t.owner_id).collect();
    v.insert(r.owner_id);
    v.into_iter().collect()
}

/// The draw: pick the winning number, pay the seller `total - fee`, status 7.
fn finish(reg: &mut Registry, id: i64) {
    let Some(mut r) = get(reg, id, true) else {
        return;
    };
    reg.ext_mut::<State>().running.remove(&id);
    // The salt is fresh process entropy, drawn now and never stored or sent:
    // before the last ticket sells, nothing fixes the winner, so the world
    // seed, the id and the count are not enough to buy the winning ticket.
    let salt = draw_salt();
    r.winner_number = draw_number(reg.world().seed, id, r.count, salt);
    r.winner = r
        .tickets
        .iter()
        .find(|t| t.number == r.winner_number)
        .cloned();
    r.status = FINISHED_UNDELIVERED;
    r.end_date = crate::pyutil::filetime_now();
    put_raffle(reg, &r);
    let total = r.total() as f64;
    let tax = cents(total * TAX);
    pay(reg, r.owner_id, total, REF_PAYOUT, id);
    reg.charge(r.owner_id, tax, REF_FEE, SYSTEM_OWNER, id);
    tell_finished(reg, &r, NOTE_FINISHED);
    compact(reg, id);
}

/// Fresh entropy for one draw (a test may fix it).
fn draw_salt() -> u64 {
    #[cfg(test)]
    if let Some(s) = SALT.with(|c| c.get()) {
        return s;
    }
    seed::draw_world_seed()
}

/// The winning number of offer `id` with `count` tickets.
fn draw_number(world_seed: u64, id: i64, count: i64, salt: u64) -> i64 {
    seed::stream(
        world_seed,
        Kind::Loot,
        &[HN_TAG, id as u64, count as u64, salt],
    )
    .range(0, count as u64) as i64
}

fn tell_finished(reg: &mut Registry, r: &Raffle, note: i64) {
    let who = stakeholders(r);
    for c in &who {
        notifications::notify(reg, *c, note, note_data(r));
    }
    reg.schedule(
        Duration::ZERO,
        Finished {
            to: who,
            raffle_id: r.raffle_id,
            ticket: r.winning_ticket().cloned(),
        },
    );
    tell_update(reg, r);
}

/// `AwardItem` was refused (`raffles.RafflesError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused;

/// `AwardItem`: the winner takes the prize. `Err` for anyone else, an
/// offer not yet drawn or already delivered.
pub fn award(reg: &mut Registry, actor: i64, id: i64) -> Result<(), Refused> {
    let mut r = get(reg, id, true).ok_or(Refused)?;
    let winner = r.winning_ticket().map(|t| t.owner_id);
    if r.status != FINISHED_UNDELIVERED || winner != Some(actor) {
        return Err(Refused);
    }
    reg.apply_items(&[Edit::Move {
        item_id: r.item_id,
        to: Slot::new(actor, r.location_id, FLAG_HANGAR),
    }])
    .map_err(|_| Refused)?;
    r.status = FINISHED_DELIVERED;
    put_raffle(reg, &r);
    Ok(())
}

// ---------------------------------------------------------------------------
// Expiry
// ---------------------------------------------------------------------------

/// Refund every ticket (146), return the item, status 9. The HyperCores stay
/// spent **[A]**.
fn expire(reg: &mut Registry, id: i64, now: BlueTime) {
    let Some(mut r) = get(reg, id, true) else {
        return;
    };
    reg.ext_mut::<State>().running.remove(&id);
    if r.status != RUNNING {
        return;
    }
    if reg
        .apply_items(&[Edit::Move {
            item_id: r.item_id,
            to: Slot::new(r.owner_id, r.location_id, FLAG_HANGAR),
        }])
        .is_err()
    {
        crate::outln!(
            "[hypernet] raffle {id}: the prize {} could not go home",
            r.item_id
        );
    }
    for t in &r.tickets {
        pay(reg, t.owner_id, r.price as f64, REF_REFUND, id);
    }
    r.status = FINISHED_EXPIRED;
    r.end_date = now;
    put_raffle(reg, &r);
    // No winner: a Finished push with no ticket.
    let who = stakeholders(&r);
    for c in &who {
        notifications::notify(reg, *c, NOTE_EXPIRED, note_data(&r));
    }
    reg.schedule(
        Duration::ZERO,
        Finished {
            to: who,
            raffle_id: id,
            ticket: None,
        },
    );
    tell_update(reg, &r);
    compact(reg, id);
}

/// The clock stage `hypernet` (every 4 ticks): expire what is due. One
/// comparison while nothing is.
pub fn tick(reg: &mut Registry, _: std::time::Instant, now: BlueTime) {
    tick_at(reg, now);
}

/// [`tick`] at `now`. The first run after a load scans the rows once; after
/// that a run costs one comparison until `due`, and then only the running ids.
pub fn tick_at(reg: &mut Registry, now: BlueTime) {
    let (primed, due) = {
        let st = reg.ext::<State>();
        (st.primed, st.due)
    };
    if primed && due > now {
        return;
    }
    if !primed {
        let found = scan_running(reg);
        let st = reg.ext_mut::<State>();
        st.running = found;
        st.primed = true;
    }
    let held: Vec<i64> = reg.ext::<State>().running.iter().copied().collect();
    let mut next = i64::MAX;
    for id in held {
        match get(reg, id, false) {
            Some(r) if r.status == RUNNING && r.expires <= now => expire(reg, id, now),
            Some(r) if r.status == RUNNING => next = next.min(r.expires),
            _ => {
                reg.ext_mut::<State>().running.remove(&id);
            }
        }
    }
    reg.ext_mut::<State>().due = next;
}

// ---------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------

/// `Grab` / `FilteredGrab` filters and constraints.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub type_id: Option<i64>,
    pub group_id: Option<i64>,
    pub category_id: Option<i64>,
    pub solar_system_id: Option<i64>,
    /// Ignored: the item rows carry no meta group **[A]** (falsifier: a
    /// browse filter by meta level that must narrow the list).
    pub meta_group_id: Option<i64>,
    /// `BlueprintType` **[V]**: 0 all, 1 original, 2 copy.
    pub blueprint_type: Option<i64>,
    pub min_ticket_price: Option<i64>,
    pub max_ticket_price: Option<i64>,
    pub min_ticket_count: Option<i64>,
    pub max_ticket_count: Option<i64>,
}

fn matches(reg: &Registry, r: &Raffle, f: &Filter) -> bool {
    let (group, cat) = reg
        .store()
        .get(r.item_id)
        .map_or((0, 0), |i| (i.group_id, i.category_id));
    f.type_id.is_none_or(|t| t == r.type_id)
        && f.group_id.is_none_or(|g| g == group)
        && f.category_id.is_none_or(|c| c == cat)
        && f.solar_system_id.is_none_or(|s| s == r.system_id)
        && match f.blueprint_type {
            Some(1) => r.is_copy == Some(false),
            Some(2) => r.is_copy == Some(true),
            _ => true,
        }
        && f.min_ticket_price.is_none_or(|p| r.price >= p)
        && f.max_ticket_price.is_none_or(|p| r.price <= p)
        && f.min_ticket_count.is_none_or(|c| r.count >= c)
        && f.max_ticket_count.is_none_or(|c| r.count <= c)
}

/// `raffles/common/raffle_score.py` **[V]**, without the price-quality term
/// (the raffle has no `estimated_price` here, and the client's `try` skips it
/// then). `lerp(a, b, t) = a + clamp(t) * (b - a)`.
pub fn raffle_score(elapsed: f64, sold_fraction: f64) -> f64 {
    let e = elapsed.min(1.0);
    let etime = 1.0 - e;
    let a = sold_fraction * sold_fraction;
    let lerp = a + e.clamp(0.0, 1.0) * (-e - a);
    let mut s = 0.5 + 0.5 * lerp;
    s += 0.5 * sold_fraction * etime;
    s *= etime + 1.0 + sold_fraction;
    s.powf(1.5)
}

fn score_of(r: &Raffle, now: i64) -> f64 {
    let span = (r.expires - r.created) as f64;
    let elapsed = if span > 0.0 {
        (now - r.created) as f64 / span
    } else {
        1.0
    };
    raffle_score(elapsed, r.sold as f64 / r.count.max(1) as f64)
}

/// The browse list: running, public, matching, best score first.
pub fn grab(reg: &Registry, _actor: i64, f: &Filter, size: usize) -> Vec<Raffle> {
    let now = crate::pyutil::filetime_now();
    let mut v: Vec<(f64, Raffle)> = ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, false))
        .filter(|r| r.status == RUNNING && r.restriction == 0 && matches(reg, r, f))
        .map(|r| (score_of(&r, now), r))
        .collect();
    v.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.running_id.cmp(&a.1.running_id))
    });
    let cap = if size == 0 { GRAB_SIZE } else { size };
    v.into_iter().take(cap).map(|(_, r)| r).collect()
}

fn holds(reg: &Registry, id: i64, who: i64) -> bool {
    with_prefix(reg, vec!["t".to_owned(), id.to_string()]).any(|(_, v)| n(v, 0) == who)
        || compacted(reg, id).is_some_and(|c| c.iter().any(|t| t.owner_id == who))
}

/// The actor's raffles as owner or ticket holder, newest first, a page.
pub fn history(reg: &Registry, actor: i64, before: Option<i64>) -> (Vec<Raffle>, i64) {
    history_page(reg, actor, before, PAGE)
}

/// [`history`] with a page size.
pub fn history_page(
    reg: &Registry,
    actor: i64,
    before: Option<i64>,
    page: usize,
) -> (Vec<Raffle>, i64) {
    let mut v: Vec<Raffle> = ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, false))
        .filter(|r| before.is_none_or(|b| r.running_id < b))
        .filter(|r| r.owner_id == actor || holds(reg, r.raffle_id, actor))
        .collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.running_id));
    v.truncate(page);
    (v, page as i64)
}

/// `GetMyActiveTickets`: the actor's tickets in running offers.
pub fn my_active_tickets(reg: &Registry, actor: i64) -> Vec<TicketRec> {
    ids(reg)
        .into_iter()
        .filter_map(|id| get(reg, id, true))
        .filter(|r| r.status == RUNNING)
        .flat_map(|r| r.tickets)
        .filter(|t| t.owner_id == actor)
        .collect()
}

/// `GetActiveHistoricPrices`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub historic_count: i64,
    pub historic: Option<(f64, f64, f64)>,
    pub active_count: i64,
    pub active: Option<(f64, f64, f64)>,
}

fn summarize(totals: &[f64]) -> Option<(f64, f64, f64)> {
    if totals.is_empty() {
        return None;
    }
    let max = totals.iter().copied().fold(f64::MIN, f64::max);
    let min = totals.iter().copied().fold(f64::MAX, f64::min);
    Some((max, min, totals.iter().sum::<f64>() / totals.len() as f64))
}

/// Total prices of running (`active`) and finished-for-a-winner
/// (`historic`, status 7 and 8) offers of one type.
pub fn stats(reg: &Registry, type_id: i64) -> Stats {
    let mut active = Vec::new();
    let mut historic = Vec::new();
    for r in ids(reg).into_iter().filter_map(|id| get(reg, id, false)) {
        if r.type_id != type_id {
            continue;
        }
        match r.status {
            RUNNING => active.push(r.total() as f64),
            FINISHED_UNDELIVERED | FINISHED_DELIVERED => historic.push(r.total() as f64),
            _ => {}
        }
    }
    Stats {
        historic_count: historic.len() as i64,
        historic: summarize(&historic),
        active_count: active.len() as i64,
        active: summarize(&active),
    }
}

/// `GetCreatedParticipated`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharStats {
    pub raffles_participated: i64,
    pub raffles_won: i64,
    pub finished_delivered: i64,
    pub finished_undelivered: i64,
    pub finished_expired: i64,
    pub created_running: i64,
}

/// What the actor has made and joined **[I]** (key meaning from the names).
pub fn char_stats(reg: &Registry, actor: i64) -> CharStats {
    let mut s = CharStats::default();
    for r in ids(reg).into_iter().filter_map(|id| get(reg, id, true)) {
        if r.tickets.iter().any(|t| t.owner_id == actor) {
            s.raffles_participated += 1;
        }
        if r.winning_ticket().is_some_and(|t| t.owner_id == actor) {
            s.raffles_won += 1;
        }
        if r.owner_id == actor {
            match r.status {
                RUNNING => s.created_running += 1,
                FINISHED_UNDELIVERED => s.finished_undelivered += 1,
                FINISHED_DELIVERED => s.finished_delivered += 1,
                FINISHED_EXPIRED => s.finished_expired += 1,
                _ => {}
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Wire values
// ---------------------------------------------------------------------------

/// The `Ticket` object: no `__setstate__`, so `new_style`'s dict is right
/// **[V]** (`raffles/common/ticket.py`).
pub fn ticket_value(t: &TicketRec) -> PyValue {
    crate::pyutil::new_style(
        "raffles.common.ticket.Ticket",
        vec![
            ("running_id", PyValue::Int(t.running_id)),
            ("raffle_id", PyValue::Int(t.raffle_id)),
            ("owner_id", PyValue::Int(t.owner_id)),
            ("number", PyValue::Int(t.number)),
        ],
    )
}

/// The `Raffle` object. **Not `pyutil::new_style`**: `Raffle` has
/// `__getstate__`/`__setstate__` with an 18-tuple state **[V]**
/// (`raffles/common/raffle.py`), and the reader calls `__setstate__` with
/// the header's second element, so that element must be the tuple, not the
/// attribute dict `new_style` builds. `primed` fills `sold_tickets` with a
/// list of `(running_id, raffle_id, owner_id, number)`, else `None`.
pub fn raffle_value(r: &Raffle, primed: bool) -> PyValue {
    debug_assert!(eve_rowset::is_whitelisted(b"raffles.common.raffle.Raffle"));
    let opt_time = |t: i64| {
        if t == 0 {
            PyValue::None
        } else {
            PyValue::Int(t)
        }
    };
    let sold = if primed {
        PyValue::List(
            r.tickets
                .iter()
                .map(|t| {
                    PyValue::Tuple(vec![
                        PyValue::Int(t.running_id),
                        PyValue::Int(t.raffle_id),
                        PyValue::Int(t.owner_id),
                        PyValue::Int(t.number),
                    ])
                })
                .collect(),
        )
    } else {
        PyValue::None
    };
    let meta = match r.is_copy {
        Some(c) => crate::pyutil::pydict(vec![("is_copy", PyValue::Bool(c))]),
        None => PyValue::None,
    };
    let state = vec![
        PyValue::Int(r.running_id),
        PyValue::Int(r.raffle_id),
        PyValue::Int(r.owner_id),
        PyValue::Int(r.location_id),
        PyValue::Int(r.system_id),
        PyValue::Int(r.item_id),
        PyValue::Int(r.type_id),
        PyValue::Int(r.count),
        PyValue::Int(r.price),
        if r.restriction == 0 {
            PyValue::None
        } else {
            PyValue::Int(r.restriction)
        },
        PyValue::Int(r.created),
        PyValue::Int(r.expires),
        PyValue::Int(r.sold),
        sold,
        r.winning_ticket().map_or(PyValue::None, ticket_value),
        PyValue::Int(r.status),
        opt_time(r.end_date),
        meta,
    ];
    PyValue::ObjectEx {
        form: ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(
                b"raffles.common.raffle.Raffle".to_vec(),
            )]),
            PyValue::Tuple(state),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

/// A `raffles.<name>` exception for `ServiceError::Raise`. `CreateError`
/// carries the reason, the others nothing.
pub fn error_value(name: &str, reason: Option<&str>) -> PyValue {
    debug_assert!(eve_rowset::is_whitelisted(
        format!("raffles.{name}").as_bytes()
    ));
    let args: Vec<PyValue> = reason.map(crate::pyutil::bstr).into_iter().collect();
    PyValue::ObjectEx {
        form: ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(format!("raffles.{name}").into_bytes()),
            PyValue::Tuple(args),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnRaffleCreatedServer(raffle_id, Raffle)`, to the owner.
#[derive(Debug)]
pub struct Created {
    pub to: Vec<i64>,
    pub raffle: Raffle,
}

impl Push for Created {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRaffleCreatedServer",
            vec![
                PyValue::Int(self.raffle.raffle_id),
                raffle_value(&self.raffle, true),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnRaffleUpdatedServer(raffle_id, Raffle)`, to the raffle's subscribers.
#[derive(Debug)]
pub struct Updated {
    pub to: Vec<i64>,
    pub raffle: Raffle,
}

impl Push for Updated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRaffleUpdatedServer",
            vec![
                PyValue::Int(self.raffle.raffle_id),
                raffle_value(&self.raffle, true),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnTicketsUpdatedServer({raffle_id: sold_count})`: a dict, the client
/// iterates it **[V]**.
#[derive(Debug)]
pub struct TicketsUpdated {
    pub to: Vec<i64>,
    pub counts: Vec<(i64, i64)>,
}

impl Push for TicketsUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnTicketsUpdatedServer",
            vec![crate::pyutil::dict_kv(
                self.counts
                    .iter()
                    .map(|(id, c)| (PyValue::Int(*id), PyValue::Int(*c))),
            )],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnRaffleFinishedServer(raffle_id, Ticket or None)`.
#[derive(Debug)]
pub struct Finished {
    pub to: Vec<i64>,
    pub raffle_id: i64,
    pub ticket: Option<TicketRec>,
}

impl Push for Finished {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRaffleFinishedServer",
            vec![
                PyValue::Int(self.raffle_id),
                self.ticket.as_ref().map_or(PyValue::None, ticket_value),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

// ---------------------------------------------------------------------------
// GM
// ---------------------------------------------------------------------------

/// `/hypernet` (a `slash::COMMANDS` row): list the open raffles, or
/// `/hypernet expire <id>` to set the raffle's expiry to now (the next stage
/// run expires it; this moves nothing). GM role only.
pub fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let gm = match reg.caller() {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    };
    if !gm {
        return Ok(crate::pyutil::bstr("/hypernet is for GMs"));
    }
    let text = match args {
        [] => {
            let open: Vec<String> = ids(reg)
                .into_iter()
                .filter_map(|id| get(reg, id, false))
                .filter(|r| r.status == RUNNING)
                .map(|r| {
                    format!(
                        "{} type {} owner {} {}/{} x {} expires {}",
                        r.raffle_id, r.type_id, r.owner_id, r.sold, r.count, r.price, r.expires
                    )
                })
                .collect();
            if open.is_empty() {
                "no open raffles".to_owned()
            } else {
                open.join("\n")
            }
        }
        ["expire", id] => match id.parse::<i64>().ok().and_then(|id| get(reg, id, false)) {
            Some(mut r) if r.status == RUNNING => {
                r.expires = crate::pyutil::filetime_now();
                let id = r.raffle_id;
                put_raffle(reg, &r);
                note_running(reg, id, r.expires);
                format!("raffle {id} expires at the next hypernet stage run")
            }
            Some(_) => "that raffle is not running".to_owned(),
            None => "no such raffle".to_owned(),
        },
        _ => "usage: /hypernet | /hypernet expire <id>".to_owned(),
    };
    Ok(crate::pyutil::bstr(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::model::{Character, SandboxConfig};
    use crate::pyutil::dict_get;
    use crate::service::{Request, ServiceError};
    use std::sync::Arc;

    const SELLER: i64 = 1_001;
    const BUYER: i64 = 1_002;
    const BUYER2: i64 = 1_003;
    const STATION: i64 = 60_000_001;
    const SYSTEM: i64 = 39_999_001;
    const TRIG_SYSTEM: i64 = 39_999_002;
    const PRIZE_T: i64 = 70_001;
    const BAD_T: i64 = 70_003;
    const SINGLE_T: i64 = 70_004;
    const SINGLE_OK_T: i64 = 70_005;
    const PRIZE: i64 = 150_000_001;
    const TOKENS: i64 = 150_000_002;
    const WALLET: f64 = 1_000_000_000.0;

    fn item(id: i64, ty: i64, owner: i64, loc: i64, flag: i64, qty: i64, single: bool) -> Item {
        Item {
            item_id: id,
            type_id: ty,
            owner_id: owner,
            location_id: loc,
            flag_id: flag,
            quantity: qty,
            group_id: 7_000 + ty % 10,
            category_id: 7,
            singleton: single,
        }
    }

    /// Extra prizes for tests that open several offers: 150_000_031 and up.
    fn extra(n: i64) -> i64 {
        150_000_030 + n
    }

    fn world() -> Registry {
        let mut content = crate::content::Content::fixture();
        let mut dungeons = crate::dungeon_data::Table::fixture();
        dungeons.typelists.insert(
            VALIDATION_LIST,
            crate::dungeon_data::TypeList {
                // A deny list: the offered types are everything else.
                inc_types: vec![BAD_T, TOKEN_TYPE_ID],
                ..Default::default()
            },
        );
        dungeons.typelists.insert(
            SINGLETON_LIST,
            crate::dungeon_data::TypeList {
                inc_types: vec![SINGLE_OK_T],
                ..Default::default()
            },
        );
        content.insert("dungeons", dungeons);
        // One Triglavian system, by the code constant; the row format is the
        // map loader's (invented ids).
        let row = format!(
            "{TRIG_SYSTEM}\t19070001\t29070001\t{}\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t25\t0\t\t\tInvented",
            crate::combat::FACTION_TRIGLAVIAN
        );
        content.insert(
            "universe",
            crate::celestial::Universe::parse(&[row], &[], &[]).expect("invented row"),
        );
        let ch = |id: i64| Character {
            character_id: id,
            name: format!("Pilot {id}"),
            ship_id: id + 10_000_000,
            balance: WALLET,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(SELLER), ch(BUYER), ch(BUYER2)],
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(content));
        let mut items = reg.store().items().to_vec();
        items.extend([
            item(PRIZE, PRIZE_T, SELLER, STATION, FLAG_HANGAR, 1, false),
            item(
                TOKENS,
                TOKEN_TYPE_ID,
                SELLER,
                STATION,
                FLAG_HANGAR,
                200,
                false,
            ),
            // Refusal fixtures.
            item(150_000_010, PRIZE_T, BUYER, STATION, FLAG_HANGAR, 1, false),
            item(150_000_011, BAD_T, SELLER, STATION, FLAG_HANGAR, 1, false),
            item(150_000_012, SINGLE_T, SELLER, STATION, FLAG_HANGAR, 1, true),
            item(150_000_013, PRIZE_T, SELLER, STATION, 5, 1, false),
            item(150_000_014, PRIZE_T, SELLER, 5_000, FLAG_HANGAR, 1, false),
            item(
                150_000_015,
                SINGLE_OK_T,
                SELLER,
                STATION,
                FLAG_HANGAR,
                1,
                true,
            ),
            item(
                150_000_020,
                TOKEN_TYPE_ID,
                BUYER,
                STATION,
                FLAG_HANGAR,
                200,
                false,
            ),
            item(
                150_000_021,
                TOKEN_TYPE_ID,
                SELLER,
                5_000,
                FLAG_HANGAR,
                200,
                false,
            ),
            item(150_000_022, TOKEN_TYPE_ID, SELLER, STATION, 5, 200, false),
        ]);
        for n in 1..=4 {
            items.push(item(
                extra(n),
                PRIZE_T,
                SELLER,
                STATION,
                FLAG_HANGAR,
                1,
                false,
            ));
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 160_000_000,
            ..crate::store::Snapshot::default()
        });
        reg
    }

    fn good() -> Creation {
        Creation {
            owner_location_id: STATION,
            location_id: STATION,
            solar_system_id: SYSTEM,
            token_id: TOKENS,
            item_id: PRIZE,
            type_id: PRIZE_T,
            ticket_count: 8,
            ticket_price: 1_000_000,
            restriction_id: None,
        }
    }

    fn bal(reg: &Registry, who: i64) -> f64 {
        reg.character(who).expect("a pilot").balance
    }

    fn wallets(reg: &Registry) -> f64 {
        [SELLER, BUYER, BUYER2].iter().map(|c| bal(reg, *c)).sum()
    }

    fn at(reg: &Registry, id: i64) -> Item {
        reg.store().get(id).expect("the item").clone()
    }

    fn notes(reg: &Registry, who: i64, ty: i64) -> usize {
        notifications::of(reg.deltas(), who)
            .iter()
            .filter(|n| n.type_id == ty)
            .count()
    }

    /// One open offer of `count` tickets at `price`, from `PRIZE`.
    fn open(reg: &mut Registry, count: i64, price: i64) -> i64 {
        let d = Creation {
            ticket_count: count,
            ticket_price: price,
            ..good()
        };
        create(reg, SELLER, &d).expect("a good offer")
    }

    fn journal_rows(reg: &Registry, who: i64, ty: i64) -> Vec<f64> {
        reg.journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == ty && e.wallet() == who)
            .map(|e| e.amount)
            .collect()
    }

    #[test]
    fn tokens_required_matches_the_client_formula() {
        // total / (plex / 10) * 0.05, truncated, at least one.
        assert_eq!(
            tokens_required(100.0, 5.0),
            5,
            "a plex under 10 floors the unit at 1"
        );
        assert_eq!(tokens_required(1000.0, 1_000_000.0), 1, "never below one");
        // 10e9 / 350_000 * 0.05 = 1428.57...: truncated, not rounded.
        assert_eq!(tokens_required(10_000_000_000.0, 3_500_000.0), 1428);
        // True division, not integer division: 3 / 2 * 0.05 * 100 is 7.5 -> 7.
        assert_eq!(tokens_required(300.0, 20.0), 7);
    }

    #[test]
    fn every_create_refusal_returns_its_reason_and_moves_nothing() {
        let bad = |f: &dyn Fn(&mut Creation)| {
            let mut d = good();
            f(&mut d);
            d
        };
        // The TokenAmount case below needs more cores than the stack holds.
        assert!(
            tokens_required(good().ticket_count as f64 * 1e10, plex_price(&world())) > 200,
            "the case needs more cores than the stack"
        );
        let cases: Vec<(Creation, Reason)> = vec![
            (bad(&|d| d.item_id = 150_000_010), Reason::ItemOwner),
            (bad(&|d| d.item_id = 9), Reason::ItemOwner),
            (bad(&|d| d.item_id = 150_000_011), Reason::ItemType),
            (bad(&|d| d.item_id = 150_000_012), Reason::ItemSingleton),
            (bad(&|d| d.item_id = 150_000_013), Reason::ItemInventory),
            (bad(&|d| d.item_id = 150_000_014), Reason::ItemLocation),
            (
                bad(&|d| d.solar_system_id = TRIG_SYSTEM),
                Reason::ItemTriglavianSystem,
            ),
            (bad(&|d| d.type_id = 70_099), Reason::TypeMismatch),
            (bad(&|d| d.token_id = PRIZE), Reason::TokenType),
            (bad(&|d| d.token_id = 150_000_020), Reason::TokenOwner),
            (bad(&|d| d.token_id = 150_000_021), Reason::TokenLocation),
            (bad(&|d| d.token_id = 150_000_022), Reason::TokenInventory),
            (
                bad(&|d| d.ticket_price = 10_000_000_000),
                Reason::TokenAmount,
            ),
            (bad(&|d| d.ticket_price = 100), Reason::TicketPrice),
            (bad(&|d| d.ticket_count = 10), Reason::TicketCount),
        ];
        for (d, want) in cases {
            let mut reg = world();
            let items = reg.store().items().to_vec();
            assert_eq!(create(&mut reg, SELLER, &d), Err(want), "{d:?}");
            assert_eq!(
                reg.store().items(),
                items.as_slice(),
                "{want:?} moved an item"
            );
            assert!(reg.deltas().of(KIND).is_empty(), "{want:?} wrote a row");
        }
        // Neither HyperCores nor the denied type can be the prize.
        for t in [TOKEN_TYPE_ID, BAD_T] {
            let mut reg = world();
            let d = Creation {
                item_id: if t == BAD_T { 150_000_011 } else { TOKENS },
                type_id: t,
                ..good()
            };
            assert_eq!(create(&mut reg, SELLER, &d), Err(Reason::ItemType));
        }
        // A singleton that the allowed list names is accepted.
        let mut reg = world();
        let d = Creation {
            item_id: 150_000_015,
            type_id: SINGLE_OK_T,
            ..good()
        };
        assert_eq!(create(&mut reg, SELLER, &d), Ok(FIRST_RAFFLE_ID));
    }

    #[test]
    fn a_good_create_burns_exactly_the_tokens_and_escrows_the_item() {
        let mut reg = world();
        // The burn follows the client formula at whatever PLEX price the world
        // has; no price is pinned here.
        let id = open(&mut reg, 8, 40_000_000);
        assert_eq!(id, FIRST_RAFFLE_ID);
        let need = tokens_required(320_000_000.0, plex_price(&reg));
        assert!(need >= 1);
        assert_eq!(at(&reg, TOKENS).quantity, 200 - need);
        let prize = at(&reg, PRIZE);
        assert_eq!(prize.slot(), escrow_slot(id), "the prize sits in escrow");
        assert_eq!(prize.quantity, 1, "it moved whole, once");
        let r = get(&reg, id, true).expect("the row");
        assert_eq!(
            (r.owner_id, r.status, r.count, r.price, r.sold),
            (SELLER, RUNNING, 8, 40_000_000, 0)
        );
        assert_eq!(r.expires - r.created, DURATION);
        assert_eq!(ids(&reg), vec![id]);
        assert_eq!(notes(&reg, SELLER, NOTE_CREATED), 1);
        assert_eq!(
            reg.deltas()
                .of(KIND)
                .iter()
                .filter(|(k, _)| k[0] == "r")
                .count(),
            1
        );
        // The prize cannot be offered again: it is no longer the seller's.
        assert_eq!(create(&mut reg, SELLER, &good()), Err(Reason::ItemOwner));
    }

    fn state_of(v: &PyValue) -> &Vec<PyValue> {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("an object, got {v:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a header tuple")
        };
        let PyValue::Tuple(state) = &parts[1] else {
            panic!("the state is a tuple, not a dict (trap 7)")
        };
        state
    }

    #[test]
    fn the_raffle_object_has_eighteen_state_entries_in_getstate_order() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        let r = get(&reg, id, true).unwrap();
        assert!(eve_rowset::is_whitelisted(b"raffles.common.raffle.Raffle"));
        assert!(eve_rowset::is_whitelisted(b"raffles.common.ticket.Ticket"));
        let un = raffle_value(&r, false);
        let st = state_of(&un);
        assert_eq!(st.len(), 18);
        assert_eq!(st[1], PyValue::Int(id));
        assert_eq!(st[2], PyValue::Int(SELLER));
        assert_eq!(st[3], PyValue::Int(STATION));
        assert_eq!(st[4], PyValue::Int(SYSTEM));
        assert_eq!(st[5], PyValue::Int(PRIZE));
        assert_eq!(st[6], PyValue::Int(PRIZE_T));
        assert_eq!(
            (&st[7], &st[8]),
            (&PyValue::Int(8), &PyValue::Int(1_000_000))
        );
        assert_eq!(st[9], PyValue::None, "public");
        assert_eq!(st[12], PyValue::Int(0));
        assert_eq!(st[13], PyValue::None, "unprimed sold_tickets is None");
        assert_eq!(st[14], PyValue::None);
        assert_eq!(st[15], PyValue::Int(RUNNING));
        assert_eq!(st[16], PyValue::None);
        assert_eq!(st[17], PyValue::None);
        buy(&mut reg, BUYER, id, Pick::Number(3)).unwrap();
        let r = get(&reg, id, true).unwrap();
        let pr = raffle_value(&r, true);
        let st = state_of(&pr);
        assert_eq!(st[12], PyValue::Int(1));
        let PyValue::List(sold) = &st[13] else {
            panic!("primed sold_tickets is a list")
        };
        assert_eq!(sold.len(), 1);
        let PyValue::Tuple(t) = &sold[0] else {
            panic!()
        };
        assert_eq!(t.len(), 4);
        assert_eq!(
            (&t[1], &t[2], &t[3]),
            (&PyValue::Int(id), &PyValue::Int(BUYER), &PyValue::Int(3))
        );
    }

    #[test]
    fn a_blueprint_copy_carries_its_flag_in_meta_data() {
        let mut r = Raffle {
            raffle_id: 1,
            running_id: 1,
            owner_id: 1,
            location_id: 1,
            system_id: 1,
            item_id: 1,
            type_id: 1,
            count: 8,
            price: 1,
            restriction: 1,
            created: 1,
            expires: 2,
            status: RUNNING,
            winner_number: -1,
            end_date: 0,
            is_copy: Some(true),
            sold: 0,
            tickets: vec![],
            winner: None,
        };
        let v = raffle_value(&r, false);
        let st = state_of(&v);
        assert_eq!(dict_get(&st[17], b"is_copy"), Some(&PyValue::Bool(true)));
        assert_eq!(st[9], PyValue::Int(1), "private");
        r.is_copy = None;
        assert_eq!(state_of(&raffle_value(&r, false))[17], PyValue::None);
    }

    #[test]
    fn a_taken_number_a_bad_number_and_a_short_wallet_buy_nothing() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        buy(&mut reg, BUYER, id, Pick::Number(0)).unwrap();
        let before = wallets(&reg);
        let rows = reg.deltas().of(KIND).len();
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Number(0)),
            Err(BuyError::TicketUnavailable)
        );
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Number(8)),
            Err(BuyError::TicketUnavailable)
        );
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Number(-1)),
            Err(BuyError::TicketUnavailable)
        );
        assert_eq!(
            buy(&mut reg, BUYER2, 99, Pick::Number(1)),
            Err(BuyError::TicketUnavailable)
        );
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Random(8)),
            Err(BuyError::TicketUnavailable)
        );
        reg.character_mut(BUYER2).unwrap().balance = 999_999.0;
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Number(1)),
            Err(BuyError::NotEnoughIsk)
        );
        assert_eq!(
            buy(&mut reg, BUYER2, id, Pick::Random(3)),
            Err(BuyError::NotEnoughIsk)
        );
        assert_eq!(bal(&reg, BUYER2), 999_999.0, "nothing charged");
        assert_eq!(wallets(&reg), before - (WALLET - 999_999.0));
        assert_eq!(reg.deltas().of(KIND).len(), rows, "no ticket row written");
    }

    /// A sell-out in a fresh world: BUYER takes 0..3 by number, BUYER2 four
    /// at random. Returns the world, the id and ISK before the buys.
    fn sold_out() -> (Registry, i64, f64) {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        let before = wallets(&reg);
        for n in 0..4 {
            buy(&mut reg, BUYER, id, Pick::Number(n)).unwrap();
        }
        // Money is held, not lost, while the offer runs.
        assert_eq!(held_isk(&reg), 4_000_000.0);
        assert_eq!(wallets(&reg) + held_isk(&reg), before);
        buy(&mut reg, BUYER2, id, Pick::Random(4)).unwrap();
        (reg, id, before)
    }

    #[test]
    fn a_sell_out_pays_the_seller_ninety_five_percent_and_conserves_isk() {
        let (reg, id, before) = sold_out();
        let r = get(&reg, id, true).unwrap();
        assert_eq!(r.status, FINISHED_UNDELIVERED);
        assert_eq!(r.sold, 8);
        let total = 8_000_000.0;
        let fee = total * TAX;
        assert_eq!(bal(&reg, SELLER), WALLET + total * 0.95);
        assert_eq!(bal(&reg, BUYER), WALLET - 4_000_000.0);
        assert_eq!(bal(&reg, BUYER2), WALLET - 4_000_000.0);
        // ISK in == ISK out + fee: wallets before - after is exactly the fee,
        // and nothing is left held.
        assert_eq!(held_isk(&reg), 0.0);
        assert_eq!(before - wallets(&reg), fee);
        // The seller's journal: the payout, then the fee.
        assert_eq!(journal_rows(&reg, SELLER, REF_PAYOUT), vec![total]);
        assert_eq!(journal_rows(&reg, SELLER, REF_FEE), vec![-fee]);
        assert_eq!(journal_rows(&reg, BUYER, REF_TICKET).len(), 4);
        assert!(r.winner_number >= 0 && r.winner_number < 8);
        let w = r.winning_ticket().expect("a winning ticket");
        assert!([BUYER, BUYER2].contains(&w.owner_id));
        assert_eq!(notes(&reg, SELLER, NOTE_FINISHED), 1);
        assert_eq!(notes(&reg, BUYER, NOTE_FINISHED), 1);
        assert_eq!(notes(&reg, BUYER2, NOTE_FINISHED), 1);
    }

    /// A sell-out of `count` tickets of the extra prize `n`, drawn with
    /// `salt`; returns the winning number.
    fn drawn_with(reg: &mut Registry, n: i64, count: i64, salt: u64) -> (i64, i64) {
        SALT.with(|c| c.set(Some(salt)));
        let d = Creation {
            item_id: extra(n),
            ticket_count: count,
            ..good()
        };
        let id = create(reg, SELLER, &d).unwrap();
        for k in 0..count {
            buy(reg, BUYER, id, Pick::Number(k)).unwrap();
        }
        SALT.with(|c| c.set(None));
        (id, get(reg, id, false).unwrap().winner_number)
    }

    #[test]
    fn the_draw_is_deterministic_for_a_fixed_salt() {
        let (mut a, mut b) = (world(), world());
        let wa = drawn_with(&mut a, 1, 16, 7);
        let wb = drawn_with(&mut b, 1, 16, 7);
        assert_eq!(wa, wb);
        assert_eq!(wa.1, draw_number(a.world().seed, wa.0, 16, 7));
    }

    #[test]
    fn the_winner_cannot_be_predicted_from_the_seed_the_id_and_the_count() {
        // What a player holding the world seed could compute before the
        // sell-out: the draw without any salt.
        let mut reg = world();
        let seed = reg.world().seed;
        let id = FIRST_RAFFLE_ID;
        let predicted = draw_number(seed, id, 16, 0);
        // Pick a salt whose draw differs from the prediction (such salts are
        // all but one in sixteen), then sell out with it: the winner is the
        // salted draw, not the prediction.
        let salt = (1..64u64)
            .find(|s| draw_number(seed, id, 16, *s) != predicted)
            .expect("a salt that changes the draw");
        let (got_id, w) = drawn_with(&mut reg, 1, 16, salt);
        assert_eq!(got_id, id);
        assert_ne!(w, predicted, "the draw ignored its salt");
        // And with fresh entropy the same offer's winner is not a function of
        // the inputs: the salts differ between calls.
        assert_ne!(seed::draw_world_seed(), seed::draw_world_seed());
    }

    #[test]
    fn the_draw_spreads_over_the_numbers_across_offers() {
        // Sixteen-ticket offers sold out by BUYER, one salt each: the winning
        // number varies with the salt and stays in range.
        let mut reg = world();
        let mut seen = BTreeSet::new();
        for n in 1..=4 {
            let (_, w) = drawn_with(&mut reg, n, 16, n as u64 * 0x9e37_79b9);
            assert!((0..16).contains(&w));
            seen.insert(w);
        }
        assert!(seen.len() > 1, "four draws, one number: {seen:?}");
    }

    #[test]
    fn real_entropy_draws_stay_in_range() {
        let mut reg = world();
        for n in 1..=4 {
            let d = Creation {
                item_id: extra(n),
                ticket_count: 16,
                ..good()
            };
            let id = create(&mut reg, SELLER, &d).unwrap();
            for k in 0..16 {
                buy(&mut reg, BUYER, id, Pick::Number(k)).unwrap();
            }
            let r = get(&reg, id, true).unwrap();
            assert!((0..16).contains(&r.winner_number));
            assert!(r.winning_ticket().is_some());
        }
    }

    #[test]
    fn random_tickets_are_free_distinct_numbers() {
        let mut reg = world();
        let id = open(&mut reg, 16, 1_000_000);
        buy(&mut reg, BUYER, id, Pick::Number(5)).unwrap();
        buy(&mut reg, BUYER2, id, Pick::Random(10)).unwrap();
        let r = get(&reg, id, true).unwrap();
        let nums: BTreeSet<i64> = r.tickets.iter().map(|t| t.number).collect();
        assert_eq!(nums.len(), 11, "no number twice");
        assert!(nums.iter().all(|x| (0..16).contains(x)));
        assert_eq!(
            r.tickets.iter().filter(|t| t.owner_id == BUYER2).count(),
            10
        );
    }

    #[test]
    fn award_goes_to_the_winner_only_and_the_prize_moves_once() {
        let (mut reg, id, _) = sold_out();
        assert_eq!(
            at(&reg, PRIZE).slot(),
            escrow_slot(id),
            "held until claimed"
        );
        let winner = get(&reg, id, true)
            .unwrap()
            .winning_ticket()
            .unwrap()
            .owner_id;
        let loser = if winner == BUYER { BUYER2 } else { BUYER };
        assert_eq!(award(&mut reg, loser, id), Err(Refused));
        assert_eq!(
            award(&mut reg, SELLER, id),
            Err(Refused),
            "not the seller either"
        );
        assert_eq!(
            at(&reg, PRIZE).slot(),
            escrow_slot(id),
            "a refusal moves nothing"
        );
        assert_eq!(award(&mut reg, winner, id), Ok(()));
        assert_eq!(
            at(&reg, PRIZE).slot(),
            Slot::new(winner, STATION, FLAG_HANGAR)
        );
        assert_eq!(get(&reg, id, false).unwrap().status, FINISHED_DELIVERED);
        assert_eq!(award(&mut reg, winner, id), Err(Refused), "once");
        assert_eq!(
            reg.store()
                .items()
                .iter()
                .filter(|i| i.type_id == PRIZE_T && i.item_id == PRIZE)
                .count(),
            1
        );
    }

    #[test]
    fn a_finished_raffle_carries_its_winning_ticket_even_unprimed() {
        let (reg, id, _) = sold_out();
        let (page, _) = history(&reg, SELLER, None);
        let r = page.iter().find(|r| r.raffle_id == id).unwrap();
        assert!(r.tickets.is_empty(), "not primed");
        let w = r.winning_ticket().expect("the winner rides with the row");
        assert_eq!(w.number, r.winner_number);
        let st = raffle_value(r, false);
        assert!(matches!(state_of(&st)[14], PyValue::ObjectEx { .. }));
    }

    #[test]
    fn award_before_the_draw_is_refused() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        buy(&mut reg, BUYER, id, Pick::Number(0)).unwrap();
        assert_eq!(award(&mut reg, BUYER, id), Err(Refused));
    }

    #[test]
    fn an_expired_offer_refunds_every_ticket_returns_the_prize_and_keeps_the_cores_spent() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        let after_create = wallets(&reg);
        let cores = at(&reg, TOKENS).quantity;
        assert_eq!(
            cores,
            200 - tokens_required(8_000_000.0, plex_price(&reg)),
            "the offer burned its cores"
        );
        buy(&mut reg, BUYER, id, Pick::Random(2)).unwrap();
        buy(&mut reg, BUYER2, id, Pick::Number(7)).unwrap();
        let expires = get(&reg, id, false).unwrap().expires;
        // Not yet: nothing happens a tick early.
        tick_at(&mut reg, expires - 1);
        assert_eq!(get(&reg, id, false).unwrap().status, RUNNING);
        assert_eq!(held_isk(&reg), 3_000_000.0);
        tick_at(&mut reg, expires);
        let r = get(&reg, id, false).unwrap();
        assert_eq!(r.status, FINISHED_EXPIRED);
        assert_eq!(r.end_date, expires);
        assert_eq!(
            wallets(&reg),
            after_create,
            "every ticket refunded, ISK conserved"
        );
        assert_eq!(bal(&reg, BUYER), WALLET);
        assert_eq!(bal(&reg, BUYER2), WALLET);
        assert_eq!(held_isk(&reg), 0.0);
        assert_eq!(
            at(&reg, PRIZE).slot(),
            Slot::new(SELLER, STATION, FLAG_HANGAR)
        );
        assert_eq!(
            at(&reg, TOKENS).quantity,
            cores,
            "HyperCores are not refunded"
        );
        assert_eq!(
            journal_rows(&reg, BUYER, REF_REFUND),
            vec![1_000_000.0, 1_000_000.0]
        );
        assert_eq!(notes(&reg, SELLER, NOTE_EXPIRED), 1);
        assert_eq!(notes(&reg, BUYER, NOTE_EXPIRED), 1);
        assert_eq!(notes(&reg, BUYER2, NOTE_EXPIRED), 1);
        // A second run refunds nothing more.
        tick_at(&mut reg, expires + 1);
        assert_eq!(wallets(&reg), after_create);
        assert_eq!(
            buy(&mut reg, BUYER, id, Pick::Number(0)),
            Err(BuyError::TicketUnavailable)
        );
    }

    fn scans() -> usize {
        SCANS.with(|c| c.get())
    }

    #[test]
    fn an_idle_tick_is_one_comparison_and_a_new_offer_is_noticed() {
        let mut reg = world();
        tick_at(&mut reg, 1);
        assert!(reg.ext::<State>().primed);
        assert_eq!(reg.ext::<State>().due, i64::MAX, "nothing running");
        let id = open(&mut reg, 8, 1_000_000);
        let expires = get(&reg, id, false).unwrap().expires;
        assert_eq!(
            reg.ext::<State>().due,
            expires,
            "create lowers the soonest expiry"
        );
        assert_eq!(
            reg.ext::<State>()
                .running
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![id]
        );
        // An offer made after the slot was primed expires on time, found
        // without any rescan.
        let before = scans();
        tick_at(&mut reg, expires);
        assert_eq!(scans(), before);
        assert_eq!(get(&reg, id, false).unwrap().status, FINISHED_EXPIRED);
        assert!(reg.ext::<State>().running.is_empty());
        assert_eq!(reg.ext::<State>().due, i64::MAX);
    }

    #[test]
    fn a_buy_does_not_make_the_next_tick_scan_every_row() {
        let mut reg = world();
        let a = open(&mut reg, 8, 1_000_000);
        let d = Creation {
            item_id: extra(1),
            ..good()
        };
        let b = create(&mut reg, SELLER, &d).unwrap();
        let t0 = crate::pyutil::filetime_now();
        let base = scans();
        tick_at(&mut reg, t0);
        assert_eq!(scans(), base + 1, "the first run primes the slot");
        let mut now = t0;
        for k in 0..8 {
            buy(&mut reg, BUYER, a, Pick::Number(k)).unwrap();
            if k < 4 {
                buy(&mut reg, BUYER2, b, Pick::Number(k)).unwrap();
            }
            now += 1;
            tick_at(&mut reg, now);
        }
        assert_eq!(get(&reg, a, false).unwrap().status, FINISHED_UNDELIVERED);
        assert_eq!(scans(), base + 1, "a buy and a sell-out never rescan");
        assert_eq!(
            reg.ext::<State>()
                .running
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![b],
            "a sold-out offer left the running set"
        );
    }

    #[test]
    fn a_restart_rebuilds_the_slot_from_the_rows_and_expires_on_time() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        buy(&mut reg, BUYER, id, Pick::Random(2)).unwrap();
        buy(&mut reg, BUYER2, id, Pick::Number(7)).unwrap();
        let expires = get(&reg, id, false).unwrap().expires;
        let wallets_before = wallets(&reg);
        let mut again = world();
        again.apply_snapshot(reg.to_snapshot());
        drop(reg);
        assert_eq!(get(&again, id, true).unwrap().sold, 3);
        assert!(!again.ext::<State>().primed, "the slot is not persisted");
        let before = scans();
        tick_at(&mut again, expires - 1);
        assert_eq!(scans(), before + 1, "one scan after a load");
        assert_eq!(get(&again, id, false).unwrap().status, RUNNING);
        tick_at(&mut again, expires);
        assert_eq!(get(&again, id, false).unwrap().status, FINISHED_EXPIRED);
        assert_eq!(scans(), before + 1);
        assert_eq!(wallets(&again), wallets_before + 3_000_000.0);
        assert_eq!(bal(&again, BUYER), WALLET);
        assert_eq!(bal(&again, BUYER2), WALLET);
        assert_eq!(
            at(&again, PRIZE).slot(),
            Slot::new(SELLER, STATION, FLAG_HANGAR)
        );
    }

    #[test]
    fn a_ticket_cannot_be_bought_after_the_deadline_before_the_stage_runs() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        let mut r = get(&reg, id, false).unwrap();
        r.expires = crate::pyutil::filetime_now() - 1;
        put_raffle(&mut reg, &r);
        assert_eq!(
            buy(&mut reg, BUYER, id, Pick::Number(0)),
            Err(BuyError::TicketUnavailable)
        );
        assert_eq!(bal(&reg, BUYER), WALLET);
    }

    #[test]
    fn the_gm_expire_command_only_brings_the_deadline_forward() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        buy(&mut reg, BUYER, id, Pick::Number(0)).unwrap();
        tick_at(&mut reg, 1);
        let out = crate::hypernet::slash(&mut reg, Some(SELLER), &["expire", &id.to_string()]);
        assert!(matches!(out, Ok(PyValue::Bytes(_))));
        assert_eq!(
            get(&reg, id, false).unwrap().status,
            RUNNING,
            "it moves nothing itself"
        );
        assert_eq!(at(&reg, PRIZE).slot(), escrow_slot(id));
        assert!(
            reg.ext::<State>().due <= crate::pyutil::filetime_now(),
            "the command lowers the slot's due"
        );
        // Primed before the command, so only the lowered due can bring it.
        tick_at(&mut reg, crate::pyutil::filetime_now() + 1);
        assert_eq!(get(&reg, id, false).unwrap().status, FINISHED_EXPIRED);
        assert_eq!(bal(&reg, BUYER), WALLET);
    }

    #[test]
    fn grab_excludes_private_and_finished_and_applies_constraints() {
        let mut reg = world();
        let a = open(&mut reg, 8, 1_000_000);
        let mk = |reg: &mut Registry, prize: i64, count: i64, restriction: Option<i64>| {
            let d = Creation {
                item_id: prize,
                ticket_count: count,
                ticket_price: 1_000_000,
                restriction_id: restriction,
                ..good()
            };
            create(reg, SELLER, &d).expect("offer")
        };
        let private = mk(&mut reg, extra(1), 8, Some(1));
        let big = mk(&mut reg, extra(2), 16, None);
        let all: Vec<i64> = grab(&reg, BUYER, &Filter::default(), 0)
            .iter()
            .map(|r| r.raffle_id)
            .collect();
        assert!(all.contains(&a) && all.contains(&big));
        assert!(!all.contains(&private), "a private offer is never browsed");
        let f = Filter {
            min_ticket_count: Some(16),
            ..Filter::default()
        };
        let got: Vec<i64> = grab(&reg, BUYER, &f, 0)
            .iter()
            .map(|r| r.raffle_id)
            .collect();
        assert_eq!(got, vec![big]);
        let f = Filter {
            type_id: Some(70_099),
            ..Filter::default()
        };
        assert!(grab(&reg, BUYER, &f, 0).is_empty());
        assert_eq!(
            grab(&reg, BUYER, &Filter::default(), 1).len(),
            1,
            "size caps"
        );
        // Sell `a` out: a finished offer is gone from the browse list.
        for n in 0..8 {
            buy(&mut reg, BUYER, a, Pick::Number(n)).unwrap();
        }
        let after: Vec<i64> = grab(&reg, BUYER, &Filter::default(), 0)
            .iter()
            .map(|r| r.raffle_id)
            .collect();
        assert_eq!(after, vec![big]);
    }

    #[test]
    fn the_score_favours_a_fuller_and_a_fresher_offer() {
        assert!(raffle_score(0.1, 0.9) > raffle_score(0.1, 0.1));
        assert!(raffle_score(0.1, 0.5) > raffle_score(0.9, 0.5));
        assert!(
            raffle_score(2.0, 0.5) == raffle_score(1.0, 0.5),
            "elapsed is capped at 1"
        );
    }

    #[test]
    fn history_pages_newest_first_for_owner_and_ticket_holder() {
        let mut reg = world();
        let a = open(&mut reg, 8, 1_000_000);
        let mk = |reg: &mut Registry, prize: i64| {
            let d = Creation {
                item_id: prize,
                ..good()
            };
            create(reg, SELLER, &d).expect("offer")
        };
        let b = mk(&mut reg, extra(1));
        let c = mk(&mut reg, extra(2));
        buy(&mut reg, BUYER, a, Pick::Number(0)).unwrap();
        let (page, size) = history_page(&reg, SELLER, None, 2);
        assert_eq!(size, 2);
        assert_eq!(
            page.iter().map(|r| r.raffle_id).collect::<Vec<_>>(),
            vec![c, b]
        );
        let before = page.iter().map(|r| r.running_id).min();
        let (rest, _) = history_page(&reg, SELLER, before, 2);
        assert_eq!(
            rest.iter().map(|r| r.raffle_id).collect::<Vec<_>>(),
            vec![a]
        );
        let (mine, _) = history(&reg, BUYER, None);
        assert_eq!(
            mine.iter().map(|r| r.raffle_id).collect::<Vec<_>>(),
            vec![a],
            "a ticket holder sees it"
        );
        assert!(history(&reg, BUYER2, None).0.is_empty());
        assert_eq!(history(&reg, SELLER, None).1, PAGE as i64);
    }

    #[test]
    fn stats_and_character_stats_count_what_happened() {
        let mut reg = world();
        let a = open(&mut reg, 8, 1_000_000);
        let s = stats(&reg, PRIZE_T);
        assert_eq!((s.active_count, s.historic_count), (1, 0));
        assert_eq!(s.active, Some((8_000_000.0, 8_000_000.0, 8_000_000.0)));
        assert_eq!(s.historic, None);
        assert_eq!(char_stats(&reg, SELLER).created_running, 1);
        for n in 0..8 {
            buy(&mut reg, BUYER, a, Pick::Number(n)).unwrap();
        }
        let s = stats(&reg, PRIZE_T);
        assert_eq!((s.active_count, s.historic_count), (0, 1));
        let cs = char_stats(&reg, BUYER);
        assert_eq!((cs.raffles_participated, cs.raffles_won), (1, 1));
        assert_eq!(char_stats(&reg, SELLER).finished_undelivered, 1);
        award(&mut reg, BUYER, a).unwrap();
        assert_eq!(char_stats(&reg, SELLER).finished_delivered, 1);
    }

    #[test]
    fn the_escrow_band_is_clear_of_the_other_bases() {
        const { assert!(ESCROW_RAFFLE_BASE >= crate::trade::TICKET_BASE + 1_000_000_000_000) };
        // market 1e12 + order, trade 2e12 + n
        const { assert!(ESCROW_RAFFLE_BASE > 2 * 1_000_000_000_000) };
        for id in [1, 1_000_000, 999_999_999] {
            let s = escrow_slot(id);
            assert!(crate::item::is_escrow(s.location_id));
            assert_eq!(
                s.location_id,
                crate::item::ESCROW_BASE + ESCROW_RAFFLE_BASE + id
            );
            assert_eq!(s.owner_id, ESCROW_OWNER);
        }
    }

    #[test]
    fn the_session_carries_a_country_that_is_not_restricted() {
        let st = PyValue::Dict(crate::session::Session::default().initial_state());
        assert_eq!(
            dict_get(&st, b"countryCode"),
            Some(&PyValue::Bytes(crate::session::COUNTRY.as_bytes().to_vec()))
        );
        assert_ne!(crate::session::COUNTRY, "KR", "RESTRICTED_COUNTRIES");
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: Vec<PyValue>,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "raffleProxy",
            method,
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        };
        reg.dispatch(&req)
    }

    fn raised(r: Result<PyValue, ServiceError>) -> (String, Vec<PyValue>) {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
            panic!("a raised exception, got {r:?}")
        };
        let PyValue::Tuple(p) = *header else { panic!() };
        let PyValue::Global(g) = &p[0] else { panic!() };
        let PyValue::Tuple(args) = &p[1] else {
            panic!()
        };
        (String::from_utf8(g.clone()).unwrap(), args.clone())
    }

    #[test]
    fn the_wire_creates_buys_and_raises_the_clients_exception_classes() {
        let mut reg = world();
        assert_eq!(
            call(&mut reg, SELLER, "AmIBanned", vec![]).unwrap(),
            PyValue::Bool(false)
        );
        let data = |item: i64| {
            crate::pyutil::new_style(
                "raffles.common.raffle_data.RaffleCreationData",
                vec![
                    ("owner_id", PyValue::Int(SELLER)),
                    ("owner_location_id", PyValue::Int(STATION)),
                    ("location_id", PyValue::Int(STATION)),
                    ("solar_system_id", PyValue::Int(SYSTEM)),
                    ("token_id", PyValue::Int(TOKENS)),
                    ("token_location_id", PyValue::Int(STATION)),
                    ("item_id", PyValue::Int(item)),
                    ("type_id", PyValue::Int(PRIZE_T)),
                    ("ticket_count", PyValue::Int(8)),
                    ("ticket_price", PyValue::Int(1_000_000)),
                    ("restriction_id", PyValue::None),
                ],
            )
        };
        // A refusal is raffles.CreateError(reason), not a UserError (trap 6).
        let (class, args) = raised(call(
            &mut reg,
            SELLER,
            "CreateRaffle",
            vec![data(150_000_010)],
        ));
        assert_eq!(class, "raffles.CreateError");
        assert_eq!(args, vec![PyValue::Bytes(b"ItemOwnerError".to_vec())]);
        let id = match call(&mut reg, SELLER, "CreateRaffle", vec![data(PRIZE)]).unwrap() {
            PyValue::Int(n) => n,
            other => panic!("an id, got {other:?}"),
        };
        let got = call(&mut reg, BUYER, "GetRaffle", vec![PyValue::Int(id)]).unwrap();
        assert!(
            matches!(state_of(&got)[13], PyValue::List(_)),
            "GetRaffle primes the tickets"
        );
        call(
            &mut reg,
            BUYER,
            "BuyTicket",
            vec![PyValue::Int(id), PyValue::Int(0)],
        )
        .unwrap();
        let (class, args) = raised(call(
            &mut reg,
            BUYER2,
            "BuyTicket",
            vec![PyValue::Int(id), PyValue::Int(0)],
        ));
        assert_eq!(
            (class.as_str(), args.len()),
            ("raffles.TicketUnavailableError", 0)
        );
        reg.character_mut(BUYER2).unwrap().balance = 1.0;
        let (class, _) = raised(call(
            &mut reg,
            BUYER2,
            "BuyRandomTickets",
            vec![PyValue::Int(id), PyValue::Int(2)],
        ));
        assert_eq!(class, "raffles.NotEnoughISKError");
        let (class, _) = raised(call(&mut reg, BUYER2, "GetRaffle", vec![PyValue::Int(99)]));
        assert_eq!(class, "raffles.RaffleNotFoundError");
        let (class, _) = raised(call(&mut reg, BUYER2, "AwardItem", vec![PyValue::Int(id)]));
        assert_eq!(class, "raffles.RafflesError");
        let PyValue::Tuple(h) = call(&mut reg, BUYER, "GetMyRaffleHistory", vec![]).unwrap() else {
            panic!("(page, size)")
        };
        assert_eq!(h[1], PyValue::Int(PAGE as i64));
        let stats = call(
            &mut reg,
            BUYER,
            "GetActiveHistoricPrices",
            vec![PyValue::Int(PRIZE_T)],
        )
        .unwrap();
        for k in [
            "historic_count",
            "historic_max",
            "historic_min",
            "historic_average",
            "active_count",
            "active_max",
            "active_min",
            "active_average",
        ] {
            assert!(dict_get(&stats, k.as_bytes()).is_some(), "{k}");
        }
        assert_eq!(dict_get(&stats, b"active_count"), Some(&PyValue::Int(1)));
        assert_eq!(dict_get(&stats, b"historic_max"), Some(&PyValue::None));
        let cp = call(&mut reg, BUYER, "GetCreatedParticipated", vec![]).unwrap();
        assert_eq!(
            dict_get(&cp, b"raffles_participated"),
            Some(&PyValue::Int(1))
        );
        let PyValue::List(g) = call(&mut reg, BUYER, "Grab", vec![]).unwrap() else {
            panic!()
        };
        assert_eq!(g.len(), 1);
        assert_eq!(
            state_of(&g[0])[13],
            PyValue::None,
            "browse rows are unprimed"
        );
    }

    #[test]
    fn subscriptions_choose_who_is_told() {
        let mut reg = world();
        subscribe_tickets(&mut reg, BUYER, true);
        subscribe_raffle(&mut reg, BUYER2, 7, true);
        assert_eq!(listeners(&reg, None), vec![BUYER]);
        assert_eq!(listeners(&reg, Some(7)), vec![BUYER2]);
        assert!(listeners(&reg, Some(8)).is_empty());
        subscribe_tickets(&mut reg, BUYER, false);
        subscribe_raffle(&mut reg, BUYER2, 7, false);
        assert!(listeners(&reg, None).is_empty() && listeners(&reg, Some(7)).is_empty());
        // Nobody here has a session: pushing drops them.
        subscribe_tickets(&mut reg, BUYER, true);
        assert!(online(&mut reg, vec![BUYER]).is_empty());
        assert!(listeners(&reg, None).is_empty());
    }

    #[test]
    fn the_pushes_have_the_shapes_the_client_unpacks() {
        let t = TicketRec {
            running_id: 4,
            raffle_id: 2,
            owner_id: 3,
            number: 1,
        };
        let (ev, args) = TicketsUpdated {
            to: vec![1],
            counts: vec![(2, 5)],
        }
        .to_push();
        assert_eq!(ev, "OnTicketsUpdatedServer");
        let PyValue::Dict(d) = &args[0] else {
            panic!("a dict (.iteritems)")
        };
        assert_eq!(
            d,
            &vec![(PyValue::Int(5), PyValue::Int(2))],
            "stored value-then-key"
        );
        let (ev, args) = Finished {
            to: vec![1],
            raffle_id: 2,
            ticket: None,
        }
        .to_push();
        assert_eq!(
            (ev, args),
            (
                "OnRaffleFinishedServer",
                vec![PyValue::Int(2), PyValue::None]
            )
        );
        let (_, args) = Finished {
            to: vec![1],
            raffle_id: 2,
            ticket: Some(t.clone()),
        }
        .to_push();
        assert_eq!(args[1], ticket_value(&t));
        assert_eq!(
            Finished {
                to: vec![9],
                raffle_id: 2,
                ticket: None
            }
            .audience(),
            Some(vec![9])
        );
    }

    #[test]
    fn every_value_conserves_over_a_mixed_run() {
        // One offer sells out, one expires: wallets + held + fees == start.
        let mut reg = world();
        let start = wallets(&reg);
        let a = open(&mut reg, 8, 1_000_000);
        let d = Creation {
            item_id: extra(1),
            ..good()
        };
        let b = create(&mut reg, SELLER, &d).unwrap();
        for n in 0..8 {
            buy(&mut reg, BUYER, a, Pick::Number(n)).unwrap();
        }
        buy(&mut reg, BUYER2, b, Pick::Random(3)).unwrap();
        let fee = 8_000_000.0 * TAX;
        assert_eq!(wallets(&reg) + held_isk(&reg) + fee, start);
        let due = get(&reg, b, false).unwrap().expires;
        tick_at(&mut reg, due);
        assert_eq!(wallets(&reg) + held_isk(&reg) + fee, start);
        // Both prizes accounted for exactly once.
        assert_eq!(at(&reg, PRIZE).slot(), escrow_slot(a));
        assert_eq!(
            at(&reg, extra(1)).slot(),
            Slot::new(SELLER, STATION, FLAG_HANGAR)
        );
    }

    // ---- compaction and prune (#276) ----

    fn rows_t(reg: &Registry, id: i64) -> usize {
        with_prefix(reg, vec!["t".to_owned(), id.to_string()]).count()
    }

    fn col15(reg: &Registry, id: i64) -> Option<String> {
        reg.deltas()
            .of(KIND)
            .get(&rkey(id))
            .and_then(|v| v.get(15).cloned())
    }

    /// Everything a client can ask about, for every pilot, as text.
    fn snapshot(reg: &Registry, id: i64) -> Vec<String> {
        let mut out = Vec::new();
        for who in [SELLER, BUYER, BUYER2] {
            out.push(format!("{:?}", history(reg, who, None)));
            out.push(format!("{:?}", char_stats(reg, who)));
        }
        out.push(format!("{:?}", stats(reg, PRIZE_T)));
        for with in [true, false] {
            let r = get(reg, id, with).expect("the offer");
            out.push(format!("{r:?}"));
            for primed in [true, false] {
                out.push(format!("{:?}", raffle_value(&r, primed)));
            }
        }
        out
    }

    /// `sold_out` with the rows left as the draw wrote them.
    fn sold_out_uncompacted() -> (Registry, i64) {
        NO_COMPACT.with(|c| c.set(true));
        let (reg, id, _) = sold_out();
        NO_COMPACT.with(|c| c.set(false));
        (reg, id)
    }

    #[test]
    fn a_compacted_raffle_answers_history_stats_and_char_stats_as_before() {
        let (mut reg, id) = sold_out_uncompacted();
        assert_eq!(rows_t(&reg, id), 8);
        assert_eq!(col15(&reg, id).as_deref(), Some("-"));
        let before = snapshot(&reg, id);
        compact(&mut reg, id);
        assert_eq!(rows_t(&reg, id), 1, "only the winner's row stays");
        assert_eq!(col15(&reg, id).as_deref(), Some("8"));
        assert_eq!(compacted(&reg, id).map(|c| c.len()), Some(7));
        assert_eq!(snapshot(&reg, id), before);
        compact(&mut reg, id);
        assert_eq!(
            snapshot(&reg, id),
            before,
            "compacting twice changes nothing"
        );
        assert!(holds(&reg, id, BUYER) && holds(&reg, id, BUYER2));
        assert!(!holds(&reg, id, SELLER));
    }

    #[test]
    fn compaction_keeps_the_winner_row_and_award_still_works() {
        let (mut reg, id, _) = sold_out();
        let r = get(&reg, id, true).unwrap();
        let w = r.winning_ticket().unwrap().clone();
        assert_eq!(rows_t(&reg, id), 1);
        assert!(
            reg.deltas()
                .of(KIND)
                .contains_key(&tkey(id, r.winner_number))
        );
        assert_eq!(r.tickets.len(), 8);
        assert_eq!(award(&mut reg, w.owner_id, id), Ok(()));
        let r = get(&reg, id, false).unwrap();
        assert_eq!((r.status, r.sold), (FINISHED_DELIVERED, 8));
        assert_eq!(col15(&reg, id).as_deref(), Some("8"), "the award keeps it");
    }

    #[test]
    fn an_expired_raffle_compacts_after_its_refunds() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        let after_create = wallets(&reg);
        buy(&mut reg, BUYER, id, Pick::Random(2)).unwrap();
        buy(&mut reg, BUYER2, id, Pick::Number(7)).unwrap();
        let tickets = get(&reg, id, true).unwrap().tickets;
        let expires = get(&reg, id, false).unwrap().expires;
        tick_at(&mut reg, expires);
        assert_eq!(wallets(&reg), after_create, "every ticket refunded");
        assert_eq!(rows_t(&reg, id), 0);
        assert_eq!(col15(&reg, id).as_deref(), Some("3"));
        let r = get(&reg, id, true).unwrap();
        assert_eq!((r.status, r.sold), (FINISHED_EXPIRED, 3));
        assert_eq!(r.tickets, tickets);
        assert!(holds(&reg, id, BUYER2));
        assert_eq!(history(&reg, BUYER, None).0.len(), 1);
    }

    #[test]
    fn a_running_raffle_is_never_compacted() {
        let mut reg = world();
        let id = open(&mut reg, 8, 1_000_000);
        for n in 0..3 {
            buy(&mut reg, BUYER, id, Pick::Number(n)).unwrap();
        }
        compact(&mut reg, id);
        assert_eq!(rows_t(&reg, id), 3);
        assert!(compacted(&reg, id).is_none());
        assert_eq!(col15(&reg, id).as_deref(), Some("-"));
        assert_eq!(get(&reg, id, false).unwrap().sold, 3);
    }

    #[test]
    fn an_old_world_without_column_15_still_loads() {
        let (mut reg, id) = sold_out_uncompacted();
        let before = snapshot(&reg, id);
        // A row as the world wrote it before the column existed.
        let mut row = reg.deltas().of(KIND).get(&rkey(id)).unwrap().clone();
        row.truncate(15);
        reg.deltas_mut().put(KIND, rkey(id), row);
        assert_eq!(col15(&reg, id), None);
        assert_eq!(snapshot(&reg, id), before);
        assert_eq!(get(&reg, id, false).unwrap().sold, 8);
        // compact still works on such a row and answers the same.
        compact(&mut reg, id);
        assert_eq!(col15(&reg, id).as_deref(), Some("8"));
        assert_eq!(snapshot(&reg, id), before);
    }

    /// Set the end date of offer `id`.
    fn end_at(reg: &mut Registry, id: i64, end: i64) {
        let mut r = get(reg, id, false).unwrap();
        r.end_date = end;
        put_raffle(reg, &r);
    }

    /// One offer of prize `prize` that sells out with BUYER's tickets.
    fn sold_prize(reg: &mut Registry, prize: i64) -> i64 {
        let d = Creation {
            item_id: prize,
            ticket_price: 1_000,
            ..good()
        };
        let id = create(reg, SELLER, &d).expect("offer");
        for n in 0..8 {
            buy(reg, BUYER, id, Pick::Number(n)).unwrap();
        }
        id
    }

    /// Delivered `a`, expired-old `b`, expired-young `c`, undelivered `d`,
    /// running `e`, all but `c` and `e` ended at `w`.
    fn mixed(w: i64) -> (Registry, [i64; 5]) {
        let mut reg = world();
        let a = sold_prize(&mut reg, extra(1));
        award(&mut reg, BUYER, a).ok();
        let winner = get(&reg, a, true)
            .unwrap()
            .winning_ticket()
            .unwrap()
            .owner_id;
        assert_eq!(winner, BUYER);
        assert_eq!(get(&reg, a, false).unwrap().status, FINISHED_DELIVERED);
        let mk = |reg: &mut Registry, prize: i64| {
            let d = Creation {
                item_id: prize,
                ..good()
            };
            create(reg, SELLER, &d).expect("offer")
        };
        let b = mk(&mut reg, extra(2));
        buy(&mut reg, BUYER2, b, Pick::Number(1)).unwrap();
        expire(&mut reg, b, w);
        let c = mk(&mut reg, extra(3));
        expire(&mut reg, c, w + 3 * KEEP);
        let d = sold_prize(&mut reg, extra(4));
        let e = mk(&mut reg, PRIZE);
        buy(&mut reg, BUYER, e, Pick::Number(0)).unwrap();
        end_at(&mut reg, a, w);
        end_at(&mut reg, d, w);
        (reg, [a, b, c, d, e])
    }

    fn rows_of(reg: &Registry, id: i64) -> usize {
        ["r", "h", "t"]
            .iter()
            .map(|p| with_prefix(reg, vec![(*p).to_owned(), id.to_string()]).count())
            .sum()
    }

    #[test]
    fn prune_removes_only_old_8_and_9() {
        let w = 5 * KEEP;
        let (mut reg, [a, b, c, d, e]) = mixed(w);
        subscribe_raffle(&mut reg, BUYER, a, true);
        subscribe_raffle(&mut reg, BUYER, e, true);
        assert_eq!(prune_at(&mut reg, w + KEEP), 0, "not yet, at the edge");
        assert_eq!(prune_at(&mut reg, w + KEEP + 1), 2);
        assert_eq!((rows_of(&reg, a), rows_of(&reg, b)), (0, 0));
        for id in [c, d, e] {
            assert!(rows_of(&reg, id) > 0, "{id} stays");
        }
        assert_eq!(ids(&reg), vec![c, d, e]);
        let subs = &reg.ext::<State>().subs[&BUYER].raffles;
        assert_eq!(subs.iter().copied().collect::<Vec<_>>(), vec![e]);
        assert_eq!(prune_at(&mut reg, w + KEEP + 1), 0, "twice does nothing");
    }

    #[test]
    fn prune_never_touches_6_or_7() {
        let w = 5 * KEEP;
        let (mut reg, [_, _, c, d, e]) = mixed(w);
        let before = (rows_of(&reg, d), rows_of(&reg, e));
        // Long after everything: every 8 and 9 goes; 6 and 7 stay.
        assert_eq!(prune_at(&mut reg, w + 100 * KEEP), 3);
        assert_eq!((rows_of(&reg, d), rows_of(&reg, e)), before);
        assert_eq!(rows_of(&reg, c), 0);
        assert_eq!(get(&reg, d, false).unwrap().status, FINISHED_UNDELIVERED);
        assert_eq!(get(&reg, e, false).unwrap().status, RUNNING);
    }

    #[test]
    fn the_rollover_step_prunes_by_the_wall_clock() {
        let (mut reg, [a, ..]) = mixed(1);
        let day = crate::rollover::Day {
            day: 1,
            days: 1,
            now: 0,
        };
        assert_eq!(on_rollover(&mut reg, &day), 3);
        assert_eq!(rows_of(&reg, a), 0);
    }
}
