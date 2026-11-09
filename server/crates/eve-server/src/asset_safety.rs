//! **Asset safety** (issue #49, lane ASSET-SAFETY-1): what becomes of the
//! things stored in an Upwell structure that leaves space.
//!
//! The client's own code is the spec **[V]**:
//! `eve/client/script/ui/shared/assets/assetSafetySvc.py` (the seven
//! `structureAssetSafety` calls and the two pushes `OnAssetSafetyCreated`,
//! `OnAssetSafetyDelivered`), `assetSafetyEntry.py` (the row a wrap is shown
//! as, and how its two timers read) and `assetSafetyDeliverWindow.py` (the
//! destinations). The wire half is
//! [`crate::service::structures::safety`]; this file is state and logic.
//!
//! # The model
//!
//! When a structure is unanchored ([`wrap_structure`], called from
//! [`crate::structure_dock::evacuate`]) everything stored in it is put into
//! **one wrap per owner**: a person's, or a corporation's. A wrap is
//!
//! - an item of type [`TYPE_WRAP`] (the client names it through
//!   `assetSafety.GetWrapName`, `uix.py:303`), owned by the owner and located
//!   in the system, like the structure item itself was; and
//! - one `safetywrap` delta row ([`KIND`]): `wrap_item_id -> owner system
//!   origin created name`. `created` is the FILETIME the client calls
//!   `ejectTime`.
//!
//! The things inside keep their owner and flag and have the wrap's id as
//! their location, so nothing else in the store has to learn about wraps and
//! a restart loses nothing (the store and the delta rows are the world file).
//!
//! # Time
//!
//! Both waits count from `created` and come from [`Rules`] (a live world
//! setting), so a change applies to wraps that already exist:
//!
//! - **from `created + safety_manual_secs`** the owner may deliver the wrap to
//!   a station in its system, or a structure there they may dock at
//!   ([`deliver_to`]); earlier is refused ([`Refusal::TooEarly`]);
//! - **at `created + safety_auto_secs`** the world moves it to the nearest
//!   NPC station by itself ([`tick`]).
//!
//! `MoveEjectTimeGM(wrap, days)` (the client's "Move Timer" menu,
//! `assetSafetyEntry.py:MoveEjectTimeGM`) moves `created` back by `days`
//! days: the seam the tests and the client scenario use.
//!
//! # Delivery and the fee
//!
//! A delivery moves each thing to its owner's place at the destination, same
//! flag, destroys the wrap item and removes the row. The fee is
//! `safety_fee_pct` percent of the things' estimated value
//! ([`value_of`], the trading index or the reference price), taken from the
//! owner's wallet (a person's balance, a corporation's Master Wallet) with
//! the asset safety ledger reference ([`REF_ASSET_SAFETY_TAX`]). A wallet that
//! holds less pays what it holds: the delivery is never held back by it.
//!
//! # Where there is none
//!
//! Wormhole space and Pochven have no asset safety (the client's own rule,
//! `assetSafetySvc._IsAssetSafetyDisabledInSystem`): [`applies_in`] is false
//! there and the caller keeps the older harbour move.
//!
//! # [A]s (falsifier in each)
//!
//! - The waits (5 and 30 days) and the fee (15 percent) are the client's GM
//!   menu steps and retail's recovery tax. Falsifier: the owner decision on
//!   the numbers (`Rules::safety_*`).
//! - The fee is taken at delivery, on the whole wrap, and the things come out
//!   of the wrap into the destination hangar then. Retail delivers the wrap
//!   as an item and charges a tax per item taken out
//!   (`invCache.AcceptPossibleRemovalTax`). Falsifier: the owner wants the
//!   wrap delivered whole.
//! - The wrap item is type 60, group 1319, category 29, from the extracted
//!   type names. Falsifier: the client draws the wrap oddly in an inventory.
//! - A pilot's active ship is not wrapped: it goes to the harbour with the
//!   pilot, as before. Falsifier: the owner wants hulls wrapped too.
//! - A corporation's wrap is delivered, and its list shown, to a director or
//!   the CEO. Falsifier: a station manager who cannot see it.
//! - Wrapping takes every item of the owner located in the structure, fitted
//!   service modules and rigs included (they are located in it). Falsifier:
//!   a structure that loses its services to a wrap.
//! - `MovePersonalAssetsToSafety` is open to anyone who owns things at a
//!   structure they are not docked in (retail asks for lost docking rights).
//!   Falsifier: the owner wants it limited.

use crate::delta::Deltas;
use crate::item::{Edit, Slot};
use crate::rules::Rules;
use crate::service::Registry;
use crate::structure::Structure;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "safetywrap";

/// One second in FILETIME ticks.
pub const SECOND: i64 = 10_000_000;
/// One day in FILETIME ticks.
pub const DAY: i64 = 86_400 * SECOND;

/// `inventorycommon.const.typeAssetSafetyWrap` **[V]**.
pub const TYPE_WRAP: i64 = 60;
/// The wrap's group and category **[V]** (the extracted type names).
pub const GROUP_WRAP: i64 = 1319;
/// See [`GROUP_WRAP`].
pub const CATEGORY_WRAP: i64 = 29;
/// `appConst.refAssetSafetyTax` **[V]**: "Asset safety recovery tax".
pub const REF_ASSET_SAFETY_TAX: i64 = 123;

/// One asset safety wrap.
#[derive(Debug, Clone, PartialEq)]
pub struct Wrap {
    /// The wrap item's id: the client's `assetWrapID`.
    pub id: i64,
    /// The person or corporation the things belong to.
    pub owner: i64,
    pub system: i64,
    /// The structure it came out of.
    pub origin: i64,
    /// FILETIME: the client's `ejectTime`.
    pub created: i64,
    /// The client's `wrapName`.
    pub name: String,
}

fn key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

impl Wrap {
    fn to_row(&self) -> Vec<String> {
        vec![
            self.owner.to_string(),
            self.system.to_string(),
            self.origin.to_string(),
            self.created.to_string(),
            self.name.clone(),
        ]
    }

    fn from_row(k: &[String], v: &[String]) -> Option<Wrap> {
        Some(Wrap {
            id: k.first()?.parse().ok()?,
            owner: v.first()?.parse().ok()?,
            system: v.get(1)?.parse().ok()?,
            origin: v.get(2)?.parse().ok()?,
            created: v.get(3)?.parse().ok()?,
            name: v.get(4)?.clone(),
        })
    }

    /// When the owner may deliver it.
    pub fn manual_at(&self, rules: &Rules) -> i64 {
        self.created
            .saturating_add(rules.safety_manual_secs.saturating_mul(SECOND))
    }

    /// When the world moves it to an NPC station.
    pub fn auto_at(&self, rules: &Rules) -> i64 {
        self.created
            .saturating_add(rules.safety_auto_secs.saturating_mul(SECOND))
    }
}

/// One wrap, if there is one.
pub fn get(d: &Deltas, id: i64) -> Option<Wrap> {
    let k = key(id);
    Wrap::from_row(&k, d.of(KIND).get(&k)?)
}

/// Every wrap, in id order. A row that does not parse is skipped.
pub fn all(d: &Deltas) -> Vec<Wrap> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| Wrap::from_row(k, v))
        .collect()
}

/// The wraps of one owner.
pub fn of_owner(d: &Deltas, owner: i64) -> Vec<Wrap> {
    all(d).into_iter().filter(|w| w.owner == owner).collect()
}

fn put(d: &mut Deltas, w: &Wrap) {
    d.put(KIND, key(w.id), w.to_row());
}

fn remove(d: &mut Deltas, id: i64) {
    d.del(KIND, &key(id));
}

/// Whether a system has asset safety at all: not wormhole space **[V]**
/// (`assetSafetySvc._IsAssetSafetyDisabledInSystem`).
pub fn applies(system: i64) -> bool {
    !crate::celestial::WORMHOLE_SYSTEMS.contains(&system)
}

/// [`applies`] in the world: Pochven has none either, "as in J-space"
/// (`assetSafetySvc.py:78-79` **[V]**).
pub fn applies_in(reg: &Registry, system: i64) -> bool {
    applies(system) && !crate::pochven::in_pochven(reg, system)
}

/// Why a call was refused. The wire turns each into the client's dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// No such wrap.
    NoWrap,
    /// The caller may not act for the wrap's owner.
    NotYours,
    /// The manual wait has not run out; the seconds left.
    TooEarly(i64),
    /// The destination is not in the wrap's system, or not one the owner may
    /// use.
    BadDestination,
    /// Nothing of the caller's is stored there.
    NothingThere,
    /// The caller is docked there, or the place is gone from the books.
    Docked,
    /// Wormhole space: nothing to put in safety.
    NoSafetyHere,
    /// A GM-only call (`MoveEjectTimeGM`) by someone who is not a GM.
    NotGm,
}

impl Refusal {
    /// The text of the dialog.
    pub fn text(&self) -> String {
        match self {
            Refusal::NoWrap => "That asset safety wrap no longer exists.".to_owned(),
            Refusal::NotYours => "That asset safety wrap is not yours to deliver.".to_owned(),
            Refusal::TooEarly(secs) => format!(
                "This wrap cannot be delivered yet; it can be in {} minutes.",
                (secs + 59) / 60
            ),
            Refusal::BadDestination => {
                "That is not a place this wrap can be delivered to.".to_owned()
            }
            Refusal::NothingThere => "You have no items to put in asset safety there.".to_owned(),
            Refusal::Docked => "You cannot do that while docked there.".to_owned(),
            Refusal::NoSafetyHere => "There is no asset safety in this system.".to_owned(),
            Refusal::NotGm => "Only a GM can do that.".to_owned(),
        }
    }
}

/// Whether the call comes from a server-made source or a session with
/// `ROLE_GML` (as `service/hacking.rs` and `service/war.rs` gate theirs).
fn is_gm(reg: &Registry) -> bool {
    match reg.caller() {
        crate::service::Caller::Internal => true,
        crate::service::Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

// -- who may act -------------------------------------------------------------------

/// Whether `actor` acts for `owner`: it is them, or a corporation they
/// direct (its CEO, or a holder of the Director role).
pub fn acts_for(reg: &Registry, actor: i64, owner: i64) -> bool {
    if actor == owner {
        return true;
    }
    let Some(corp) = reg.corporations().get(owner) else {
        return false;
    };
    if corp.ceo_id == actor {
        return true;
    }
    reg.corporations().membership(actor).is_some_and(|m| {
        m.corporation_id == owner && m.roles & crate::corp_roles::CORP_ROLE_DIRECTOR != 0
    })
}

/// Whether an owner id is a corporation.
fn is_corp(reg: &Registry, owner: i64) -> bool {
    reg.corporations().get(owner).is_some()
}

// -- value and fee -----------------------------------------------------------------

/// The estimated value of what a wrap (or any container) holds: each stack at
/// the trading index, else the reference price, down through containers.
pub fn value_of(reg: &Registry, container: i64) -> f64 {
    fn walk(reg: &Registry, container: i64, depth: u32) -> f64 {
        if depth > 6 {
            return 0.0;
        }
        reg.store()
            .contents_of(container)
            .map(|i| {
                let price = crate::econ_prices::average(reg, i.type_id)
                    .filter(|p| p.is_finite() && *p > 0.0)
                    .unwrap_or(0.0);
                price * i.quantity.max(0) as f64 + walk(reg, i.item_id, depth + 1)
            })
            .sum()
    }
    walk(reg, container, 0)
}

/// The fee on a wrap worth `value`, in whole cents.
pub fn fee_of(value: f64, pct: f64) -> f64 {
    if !(value.is_finite() && pct.is_finite()) || value <= 0.0 || pct <= 0.0 {
        return 0.0;
    }
    (value * pct).round() / 100.0
}

// -- making wraps ------------------------------------------------------------------

/// What [`wrap_structure`] could not wrap: nothing in the books, or nowhere
/// to put it.
#[derive(Debug, Default)]
pub struct Wrapped {
    /// The wraps made.
    pub wraps: Vec<i64>,
}

/// Put what is stored in `s` into one wrap per owner. `keep` names items that
/// stay out (a pilot's active ship, which goes with the pilot). Call it
/// **before** the structure's row goes. In a system with no asset safety
/// nothing is wrapped and the answer is empty.
pub fn wrap_structure(
    reg: &mut Registry,
    s: &Structure,
    now: i64,
    keep: &dyn Fn(i64) -> bool,
) -> Wrapped {
    let mut out = Wrapped::default();
    if !applies_in(reg, s.system) {
        return out;
    }
    let mut by_owner: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
    for i in reg.store().contents_of(s.id) {
        if keep(i.item_id) {
            continue;
        }
        by_owner
            .entry(i.owner_id)
            .or_default()
            .push((i.item_id, i.flag_id));
    }
    for (owner, items) in by_owner {
        let id = crate::service::structures::safety::insert_wrap_item(reg, owner, s.system);
        let edits: Vec<Edit> = items
            .iter()
            .map(|&(item_id, flag)| Edit::Move {
                item_id,
                to: Slot::new(owner, id, flag),
            })
            .collect();
        if let Err(e) = reg.apply_items(&edits) {
            crate::errln!("[safety] wrapping {owner}'s things from {}: {e}", s.id);
            let _ = reg.apply_items(&[Edit::Destroy { item_id: id }]);
            continue;
        }
        let w = Wrap {
            id,
            owner,
            system: s.system,
            origin: s.id,
            created: now,
            name: wrap_name(&s.name),
        };
        put(reg.deltas_mut(), &w);
        reg.mark_dirty();
        crate::service::structures::safety::tell_created(reg, &w);
        out.wraps.push(id);
    }
    out
}

/// The name the Assets tab shows.
fn wrap_name(structure: &str) -> String {
    let n = structure.trim();
    if n.is_empty() {
        "Asset Safety".to_owned()
    } else {
        format!("{n} Asset Safety")
    }
}

/// `MovePersonalAssetsToSafety` / `MoveCorpAssetsToSafety`: the things
/// `owner` has stored at `place` (a structure on the books, in `system`) go
/// into a wrap of their own. `keep` as in [`wrap_structure`].
pub fn wrap_place(
    reg: &mut Registry,
    owner: i64,
    system: i64,
    place: i64,
    now: i64,
    keep: &dyn Fn(i64) -> bool,
) -> Result<i64, Refusal> {
    if !applies_in(reg, system) {
        return Err(Refusal::NoSafetyHere);
    }
    // The client names both ids; neither is trusted. The place must be a
    // structure on the books (not an NPC station, a ship or an office), and
    // the system the wrap is made in is the structure's own.
    let Some(s) = crate::structure::get(reg.deltas(), place) else {
        return Err(Refusal::NothingThere);
    };
    if s.system != system {
        return Err(Refusal::BadDestination);
    }
    let items: Vec<(i64, i64)> = reg
        .store()
        .contents_of(place)
        .filter(|i| i.owner_id == owner && !keep(i.item_id))
        .map(|i| (i.item_id, i.flag_id))
        .collect();
    if items.is_empty() {
        return Err(Refusal::NothingThere);
    }
    let name = s.name;
    let id = crate::service::structures::safety::insert_wrap_item(reg, owner, system);
    let edits: Vec<Edit> = items
        .iter()
        .map(|&(item_id, flag)| Edit::Move {
            item_id,
            to: Slot::new(owner, id, flag),
        })
        .collect();
    if reg.apply_items(&edits).is_err() {
        let _ = reg.apply_items(&[Edit::Destroy { item_id: id }]);
        return Err(Refusal::NothingThere);
    }
    let w = Wrap {
        id,
        owner,
        system,
        origin: place,
        created: now,
        name: wrap_name(&name),
    };
    put(reg.deltas_mut(), &w);
    reg.mark_dirty();
    crate::service::structures::safety::tell_created(reg, &w);
    Ok(id)
}

// -- destinations ------------------------------------------------------------------

/// The NPC station nearest to `system`: in it, else its constellation, its
/// region, else the lowest id. `None` only when no NPC station is known.
pub(crate) fn nearest_station(reg: &Registry, system: i64) -> Option<crate::clones::StationRef> {
    crate::structure_dock::nearest_station(reg, system)
}

/// Where `actor` may deliver a wrap in `system`: the NPC stations there and
/// the structures there they may dock at, as `(id, type)`.
pub fn destinations(reg: &Registry, actor: i64, system: i64) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = reg
        .celestials()
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Station)
        .map(|c| (c.id, c.type_id))
        .collect();
    for s in crate::structure::in_system(reg.deltas(), system) {
        if crate::structure_profile::may_dock(reg, &s, actor) {
            out.push((s.id, s.type_id));
        }
    }
    out
}

// -- delivering --------------------------------------------------------------------

/// What a delivery did.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivered {
    pub owner: i64,
    pub to: i64,
    /// What was taken from the wallet.
    pub fee: f64,
    /// Items moved.
    pub moved: usize,
}

/// Move a wrap's things to `dest`, take the fee, destroy the wrap.
/// No checks: [`deliver_to`] and [`tick`] make them.
fn deliver(reg: &mut Registry, w: &Wrap, dest: i64, now: i64) -> Delivered {
    let items: Vec<(i64, i64)> = reg
        .store()
        .contents_of(w.id)
        .map(|i| (i.item_id, i.flag_id))
        .collect();
    let fee_due = fee_of(value_of(reg, w.id), reg.config().rules.safety_fee_pct);
    let edits: Vec<Edit> = items
        .iter()
        .map(|&(item_id, flag)| Edit::Move {
            item_id,
            to: Slot::new(w.owner, dest, flag),
        })
        .collect();
    let mut moved = items.len();
    if let Err(e) = reg.apply_items(&edits) {
        crate::errln!("[safety] delivering wrap {} to {dest}: {e}", w.id);
        moved = 0;
    }
    if moved > 0 || items.is_empty() {
        let fee = take_fee(reg, w, fee_due, now);
        let _ = reg.apply_items(&[Edit::Destroy { item_id: w.id }]);
        remove(reg.deltas_mut(), w.id);
        reg.mark_dirty();
        crate::service::structures::safety::tell_delivered(reg, w.owner);
        Delivered {
            owner: w.owner,
            to: dest,
            fee,
            moved,
        }
    } else {
        Delivered {
            owner: w.owner,
            to: dest,
            fee: 0.0,
            moved: 0,
        }
    }
}

/// Take up to `due` from the owner's wallet as the asset safety recovery tax.
/// Answers what was taken.
fn take_fee(reg: &mut Registry, w: &Wrap, due: f64, now: i64) -> f64 {
    if due <= 0.0 {
        return 0.0;
    }
    let sink = crate::war::CONCORD_OWNER;
    if is_corp(reg, w.owner) {
        let pay = due.min(crate::war::balance_of(reg, w.owner));
        if pay > 0.0 && crate::war::pay(reg, w.owner, pay, REF_ASSET_SAFETY_TAX, sink, w.id) {
            return pay;
        }
        return 0.0;
    }
    let Some(c) = reg.character_mut(w.owner) else {
        return 0.0;
    };
    let pay = due.min(c.balance.max(0.0));
    if pay <= 0.0 {
        return 0.0;
    }
    c.balance -= pay;
    let balance = c.balance;
    reg.journal_mut().record(
        now,
        REF_ASSET_SAFETY_TAX,
        w.owner,
        sink,
        w.id,
        -pay,
        balance,
        "",
    );
    reg.mark_dirty();
    reg.schedule(
        std::time::Duration::ZERO,
        crate::notify::Addressed {
            to: Some(vec![w.owner]),
            inner: Box::new(crate::notify::AccountChange {
                owner_id: w.owner,
                balance,
            }),
        },
    );
    pay
}

/// `MoveSafetyWrapToStructure(wrapID, solarSystemID, destinationID)`: the
/// owner delivers a wrap, after the manual wait, to a station in its system
/// or a structure there they may dock at.
pub fn deliver_to(
    reg: &mut Registry,
    actor: i64,
    wrap: i64,
    dest: i64,
    now: i64,
) -> Result<Delivered, Refusal> {
    let w = get(reg.deltas(), wrap).ok_or(Refusal::NoWrap)?;
    if !acts_for(reg, actor, w.owner) {
        return Err(Refusal::NotYours);
    }
    let at = w.manual_at(&reg.config().rules);
    if now < at {
        return Err(Refusal::TooEarly((at - now) / SECOND));
    }
    if !destinations(reg, actor, w.system)
        .iter()
        .any(|&(id, _)| id == dest)
    {
        return Err(Refusal::BadDestination);
    }
    Ok(deliver(reg, &w, dest, now))
}

/// `MoveEjectTimeGM(wrapID, days)`: the wrap was made `days` days earlier.
pub fn move_eject_time(
    reg: &mut Registry,
    actor: i64,
    wrap: i64,
    days: i64,
) -> Result<(), Refusal> {
    if !is_gm(reg) {
        return Err(Refusal::NotGm);
    }
    let mut w = get(reg.deltas(), wrap).ok_or(Refusal::NoWrap)?;
    if !acts_for(reg, actor, w.owner) {
        return Err(Refusal::NotYours);
    }
    w.created = w.created.saturating_sub(days.saturating_mul(DAY));
    put(reg.deltas_mut(), &w);
    reg.mark_dirty();
    Ok(())
}

/// How long a wrap whose automatic delivery failed waits before the next try
/// (FILETIME ticks), so one stuck wrap does not log on every cadence.
const RETRY: i64 = 60 * SECOND;

/// When a wrap whose delivery failed may be tried again, by `(world birth,
/// wrap id)`: wrap ids are minted from the same base in every world, so a
/// process that hosts several worlds must not let one's failure delay
/// another's wrap.
static NEXT_TRY: std::sync::Mutex<BTreeMap<(i64, i64), i64>> =
    std::sync::Mutex::new(BTreeMap::new());

/// The retry key of wrap `id` in `reg`'s world.
fn try_key(reg: &Registry, id: i64) -> (i64, i64) {
    (reg.deltas().world().map_or(0, |w| w.birth), id)
}

/// The tick: every wrap whose long wait has run out moves to the nearest NPC
/// station. With no NPC station known it stays, and is tried again; so is one
/// whose move failed, after [`RETRY`]. `ft` is the stage's FILETIME.
pub fn tick(reg: &mut Registry, _: std::time::Instant, ft: crate::damage::BlueTime) {
    if reg.deltas().of(KIND).is_empty() {
        return;
    }
    let now = ft;
    let rules = reg.config().rules;
    for w in all(reg.deltas()) {
        if now < w.auto_at(&rules) {
            continue;
        }
        let key = try_key(reg, w.id);
        if NEXT_TRY
            .lock()
            .ok()
            .and_then(|m| m.get(&key).copied())
            .is_some_and(|t| now < t)
        {
            continue;
        }
        let Some(to) = nearest_station(reg, w.system) else {
            continue;
        };
        let d = deliver(reg, &w, to.station_id, now);
        if get(reg.deltas(), w.id).is_some() {
            if let Ok(mut m) = NEXT_TRY.lock() {
                m.insert(key, now.saturating_add(RETRY));
            }
            continue;
        }
        if let Ok(mut m) = NEXT_TRY.lock() {
            m.remove(&key);
        }
        crate::errln!(
            "[safety] wrap {} of {} moved itself to {} ({} items, fee {})",
            w.id,
            w.owner,
            to.station_id,
            d.moved,
            d.fee
        );
    }
}

#[cfg(test)]
pub(crate) mod tests;
