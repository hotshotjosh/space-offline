//! Mutated items: mint one, store its rolls, describe it (#69 MUTA-1).
//!
//! A mutated item is a new item (its own id, a singleton) of a mutator's
//! result type, a type flagged `isDynamicType` [V `evetypes/__init__.py:255`].
//! The client asks `dynamicItemService.GetDynamicItemInfo(itemID)` once per
//! item per session and caches the answer for good
//! [V `dogma/dynamicItemSvc.py:20-25`], so a row never changes after it is
//! minted. Its dogma base values are *only* `info.attributes`
//! [V `baseDogmaLocation.py:1875-1877`], so the answer carries the whole
//! attribute set (§1.3 of `parity-mutaplasmids.md`), not just the rolled ones.
//!
//! **The row** is the delta kind [`KIND`] (`dynitem`), key `[item_id]`, value
//! `[source_type, mutator_type, creator, created_at, rolls]`; `rolls` is
//! `attribute:modifier` pairs joined by `;`, ascending by attribute. The
//! *modifiers* are stored, not the values [A]: a range change in a new client
//! build then moves existing items with it. Falsifier: a patch note saying
//! existing mutated items keep their old numbers; then store the values too.
//!
//! [`create`] applies a mutaplasmid (#70 MUTA-2); `/dynamicitem` (GM) mints
//! one without consuming anything.

use crate::content::Content;
use crate::dogma::{Dynamic, Overrides};
use crate::item::{Change, Edit, Item, Slot};
use crate::mutator_data;
use crate::pyutil::{bstr, keyval};
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// The delta kind this module writes.
pub const KIND: &str = "dynitem";

/// One stored mutated item.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub item_id: i64,
    pub source_type: i64,
    pub mutator_type: i64,
    pub creator: i64,
    /// A wall FILETIME.
    pub created_at: i64,
    /// `(attribute, modifier)`, ascending by attribute.
    pub rolls: Vec<(i64, f64)>,
}

impl Row {
    /// The value fields, without the key.
    pub fn to_value(&self) -> Vec<String> {
        let mut rolls = self.rolls.clone();
        rolls.sort_by_key(|&(a, _)| a);
        let rolls: Vec<String> = rolls.iter().map(|(a, m)| format!("{a}:{m}")).collect();
        vec![
            self.source_type.to_string(),
            self.mutator_type.to_string(),
            self.creator.to_string(),
            self.created_at.to_string(),
            rolls.join(";"),
        ]
    }

    /// Lenient: a missing trailing field takes its default and an extra one
    /// is ignored. `None` when the source or mutator is missing or not a
    /// number, or a roll pair does not parse (the row is then left in place).
    pub fn from_value(item_id: i64, v: &[String]) -> Option<Row> {
        let source_type = v.first()?.parse().ok()?;
        let mutator_type = v.get(1)?.parse().ok()?;
        let creator = v.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        let created_at = v.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut rolls = Vec::new();
        for pair in v.get(4).map_or("", String::as_str).split(';') {
            if pair.is_empty() {
                continue;
            }
            let (a, m) = pair.split_once(':')?;
            let m: f64 = m.parse().ok()?;
            if !m.is_finite() {
                return None;
            }
            rolls.push((a.parse().ok()?, m));
        }
        rolls.sort_by_key(|&(a, _)| a);
        Some(Row {
            item_id,
            source_type,
            mutator_type,
            creator,
            created_at,
            rolls,
        })
    }
}

fn key(item_id: i64) -> Vec<String> {
    vec![item_id.to_string()]
}

/// The stored row of an item, if there is one and it reads.
pub fn row(reg: &Registry, item_id: i64) -> Option<Row> {
    let v = reg.deltas().of(KIND).get(&key(item_id))?;
    Row::from_value(item_id, v)
}

/// Store a row (replaces one of the same item).
pub fn put(reg: &mut Registry, row: &Row) {
    reg.deltas_mut().put(KIND, key(row.item_id), row.to_value());
    // A replaced row must not be answered from the old cached roll.
    if let Ok(mut c) = reg.ext::<Cache>().map.lock() {
        c.remove(&row.item_id);
    }
}

/// Whether the item's type is a mutator's result type.
pub fn is_dynamic_item(reg: &Registry, item: &Item) -> bool {
    mutator_data::is_dynamic_type(reg.content(), item.type_id)
}

/// The whole attribute set of a mutated item, ascending by attribute:
/// the source type's rows, then the dynamic type's rows, then the dynamic
/// type's mass, capacity, volume and radius, then each roll
/// (`base * (modifier * (max - min) + min)`, the base being the source's own
/// value, else the attribute's default).
///
/// Layers 2 and 3 are [A]: the TQ answer was not seen. Falsifiers: Show Info
/// on a mutated item made from a meta source showing the source's skill
/// level (layer 2), or showing the source's mass (layer 3).
pub fn attributes(content: &Content, row: &Row, result_type: i64) -> BTreeMap<i64, f64> {
    use crate::dogma::attr;
    let table = crate::typedata::installed_in(content).table();
    let mut out = BTreeMap::new();
    if let Some(s) = table.get(row.source_type) {
        out.extend(s.attributes.iter().copied());
    }
    if let Some(r) = table.get(result_type) {
        out.extend(r.attributes.iter().copied());
        out.insert(attr::MASS, r.mass);
        out.insert(attr::CAPACITY, r.capacity);
        out.insert(attr::VOLUME, r.volume);
        out.insert(attr::RADIUS, r.radius);
    }
    if let Some(m) = mutator_data::in_content(content).get(row.mutator_type) {
        for &(a, modifier) in &row.rolls {
            if let Some(range) = m.range(a) {
                let base = table.attribute(row.source_type, a);
                out.insert(a, mutator_data::value(base, modifier, range));
            }
        }
    }
    out
}

/// The `GetDynamicItemInfo` KeyVal for a stored row: `itemID`, `typeID`,
/// `sourceTypeID`, `mutatorTypeID`, `attributes`, `modifiers`. Keys are Int,
/// values Real [I: the rule the godma rows follow].
pub fn info_value(content: &Content, row: &Row, result_type: i64) -> PyValue {
    // A dict entry is `(value, key)` on the wire.
    let attrs = attributes(content, row, result_type)
        .into_iter()
        .map(|(a, v)| (PyValue::Real(v), PyValue::Int(a)))
        .collect();
    let mods = row
        .rolls
        .iter()
        .map(|&(a, m)| (PyValue::Real(m), PyValue::Int(a)))
        .collect();
    keyval(vec![
        ("itemID", PyValue::Int(row.item_id)),
        ("typeID", PyValue::Int(result_type)),
        ("sourceTypeID", PyValue::Int(row.source_type)),
        ("mutatorTypeID", PyValue::Int(row.mutator_type)),
        ("attributes", PyValue::Dict(attrs)),
        ("modifiers", PyValue::Dict(mods)),
    ])
}

/// What a dynamic item with no row is described as: the first mutator that
/// makes `result_type` and its first published source, no rolls. `None` when
/// the table names no mutator (or no published source) for the type.
pub fn fallback_info(content: &Content, item_id: i64, result_type: i64) -> Option<PyValue> {
    let row = fallback_row(content, item_id, result_type)?;
    Some(info_value(content, &row, result_type))
}

/// The row [`fallback_info`] describes: no rolls.
fn fallback_row(content: &Content, item_id: i64, result_type: i64) -> Option<Row> {
    let mutators = mutator_data::in_content(content);
    let table = crate::typedata::installed_in(content).table();
    let mutator = *mutators.mutators_making(result_type).first()?;
    let source = mutators
        .get(mutator)?
        .mappings
        .iter()
        .find(|&&(s, r)| r == result_type && table.get(s).is_some_and(|t| t.published))?
        .0;
    Some(Row {
        item_id,
        source_type: source,
        mutator_type: mutator,
        creator: 0,
        created_at: 0,
        rolls: Vec::new(),
    })
}

/// The type a dynamic item was made from: its stored row's source, else the
/// source the no-row fallback names (as [`fallback_info`] does). `None` for an
/// item whose type is not a mutator's result type, or when no source is known.
/// Repair prices the item from it (#263): a dynamic type carries no price.
pub fn source_type_of(w: &Registry, item_id: i64, type_id: i64) -> Option<i64> {
    if !mutator_data::is_dynamic_type(w.content(), type_id) {
        return None;
    }
    match row(w, item_id) {
        Some(r) => Some(r.source_type),
        None => Some(fallback_row(w.content(), item_id, type_id)?.source_type),
    }
}

/// The process cache of override maps by item id (#71 MUTA-3): item ids are
/// never reused (the allocator is persisted), and [`put`] drops an item's entry
/// when its row is replaced, so an entry never goes stale. Only row-backed items are kept; a
/// dynamic item with no row is answered by [`fallback_row`] each time, so a
/// row that arrives later is seen. Derived state, nothing persists.
#[derive(Debug, Default)]
pub struct Cache {
    map: Mutex<HashMap<i64, Arc<Overrides>>>,
}

/// How many items' overrides are kept; beyond it the cache is emptied (one
/// recomputation each).
const CACHE_CAP: usize = 65_536;

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const CACHE: SlotSpec = SlotSpec {
    name: "dynamic_item_cache",
    make: |_| Box::new(Cache::default()) as Boxed,
};

/// The overrides of an item by id and type, cached; `None` for an item whose
/// type is not a mutator's result type.
pub fn overrides_of(w: &Registry, item_id: i64, type_id: i64) -> Option<Arc<Overrides>> {
    if !mutator_data::is_dynamic_type(w.content(), type_id) {
        return None;
    }
    overrides_checked(w, item_id, type_id)
}

/// [`overrides_of`] for a type already known to be dynamic.
fn overrides_checked(w: &Registry, item_id: i64, type_id: i64) -> Option<Arc<Overrides>> {
    let cache = &w.ext::<Cache>().map;
    if let Ok(c) = cache.lock()
        && let Some(o) = c.get(&item_id)
    {
        return Some(Arc::clone(o));
    }
    let content = w.content();
    let (r, stored) = match row(w, item_id) {
        Some(r) => (r, true),
        None => (fallback_row(content, item_id, type_id)?, false),
    };
    let o = Arc::new(Overrides::new(
        attributes(content, &r, type_id).into_iter().collect(),
    ));
    if stored && let Ok(mut c) = cache.lock() {
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(item_id, Arc::clone(&o));
    }
    Some(o)
}

/// [`overrides_of`] for an item.
pub fn overrides(w: &Registry, item: &Item) -> Option<Arc<Overrides>> {
    overrides_of(w, item.item_id, item.type_id)
}

/// The dynamic ones among a ship's fitted modules (`modules` ascending by item
/// id, as `dogma::module_inputs` builds them), with their overrides. One set
/// lookup per module; a ship with no mutated module gets an empty `Vec`, which
/// does not allocate.
pub fn dynamic_of(w: &Registry, modules: &[crate::dogma::ModuleInput]) -> Dynamic {
    let mutators = mutator_data::in_content(w.content());
    modules
        .iter()
        .filter(|m| mutators.is_dynamic(m.type_id))
        .filter_map(|m| Some((m.item_id, overrides_checked(w, m.item_id, m.type_id)?)))
        .collect()
}

/// Every fitted module of `ship_id` that is a dynamic item, with its
/// overrides. The same filter as the sheet's module list (singleton, a
/// fitting flag, not a loaded charge).
pub fn fitted(w: &Registry, ship_id: i64) -> Dynamic {
    let mutators = mutator_data::in_content(w.content());
    let mut v: Dynamic = w
        .store()
        .contents_of(ship_id)
        .filter(|i| {
            i.singleton
                && crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
                && mutators.is_dynamic(i.type_id)
        })
        .filter_map(|i| Some((i.item_id, overrides_checked(w, i.item_id, i.type_id)?)))
        .collect();
    v.sort_by_key(|&(id, _)| id);
    v
}

const USAGE: &str = "usage: /dynamicitem <source type> <mutator type> [roll 0..1]";

/// GM: `/dynamicitem <source type> <mutator type> [roll]`. Docked only. One
/// item of the pair's result type lands in the caller's station hangar,
/// assembled, with every range of the mutator rolled at `roll` (default 0.5,
/// a design choice for the GM command, not a client claim).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let text = |s: &str| Ok(bstr(s));
    let (Some(a_source), Some(a_mutator)) = (args.first(), args.get(1)) else {
        return text(USAGE);
    };
    let (Ok(source), Ok(mutator)) = (a_source.parse::<i64>(), a_mutator.parse::<i64>()) else {
        return text(USAGE);
    };
    let roll = match args.get(2) {
        None => 0.5,
        Some(w) => match w.parse::<f64>() {
            Ok(r) if (0.0..=1.0).contains(&r) => r,
            _ => return text("roll must be a number from 0 to 1"),
        },
    };
    let Some(actor) = actor else {
        return text("no character to mint for");
    };
    let Some(station) = reg.docked_station(actor) else {
        return text("dock first: a mutated item is minted into the station hangar");
    };
    let content = reg.content();
    let mutators = mutator_data::in_content(content);
    let Some(m) = mutators.get(mutator) else {
        return text(&format!("type {mutator} is not a mutaplasmid"));
    };
    let published = crate::typedata::installed_in(content)
        .table()
        .get(source)
        .is_some_and(|t| t.published);
    let Some(result) = mutators.result_for(mutator, source).filter(|_| published) else {
        return text(&format!(
            "mutaplasmid {mutator} does not take a published source of type {source}"
        ));
    };
    let rolls: Vec<(i64, f64)> = m.ranges.iter().map(|r| (r.attribute_id, roll)).collect();
    let made = reg.apply_items(&[Edit::CreateAssembled {
        type_id: result,
        to: Slot::new(actor, station, crate::model::FLAG_HANGAR),
    }]);
    let item_id = match made.as_deref() {
        Ok([Change::Created { item_id, .. }]) => *item_id,
        Ok(other) => return text(&format!("unexpected result {other:?}")),
        Err(e) => return text(&format!("could not mint type {result}: {e:?}")),
    };
    put(
        reg,
        &Row {
            item_id,
            source_type: source,
            mutator_type: mutator,
            creator: actor,
            created_at: crate::pyutil::filetime_now(),
            rolls,
        },
    );
    text(&format!("minted dynamic item {item_id} ({result})"))
}

/// `flagShipHangar` [V `inventorycommon/const.py:141`]: the ship's own hangar
/// is not "the pilot's location" for a mutaplasmid (`IsInPilotLocation`,
/// `baseCheckers.py:549-559`).
pub const FLAG_SHIP_HANGAR: i64 = 90;

/// Salt of the roll stream, so it never shares a stream with `seed::Kind`'s.
const MUTATION_SALT: u64 = 0x6d75_7461_706c_6173;

/// Why [`create`] refused. Each is a `CustomNotify` text in the arm **[A]**
/// (falsifier: an empty or garbled dialog, or a non-UserError traceback).
#[derive(Debug, PartialEq)]
pub enum CreateError {
    NoCharacter,
    NotFound,
    NotYours,
    Fitted,
    NotHere,
    NotAMutator,
    NotApplicable,
    Unpublished,
    Store(crate::item::EditError),
}

impl CreateError {
    /// The text the player reads.
    pub fn text(&self) -> &'static str {
        match self {
            CreateError::NoCharacter => "There is no character to apply that for.",
            CreateError::NotFound => "That item does not exist.",
            CreateError::NotYours => "You can only use items that you own.",
            CreateError::Fitted => "Remove the item from your ship's fitting first.",
            CreateError::NotHere => "Both items must be in your hangar or your ship.",
            CreateError::NotAMutator => "That item is not a mutaplasmid.",
            CreateError::NotApplicable => "That mutaplasmid cannot be applied to this item.",
            CreateError::Unpublished => "That item cannot be mutated.",
            CreateError::Store(_) => "The server could not apply the mutaplasmid.",
        }
    }
}

/// The roll of one attribute: uniform in `[0, 1)` **[A]** (falsifier: a TQ
/// sample with a visibly non-uniform spread), from the world seed and the
/// **result's** item id. Never the input ids: a stack keeps its id while
/// `Consume` takes from it, so two crafts from the same two stacks would roll
/// the same and a good roll could be repeated at will. The store never reuses
/// an item id, across restarts too (`Store::restore`), so every craft draws
/// its own roll, and nothing predicts it without the seed.
pub fn roll(world_seed: u64, item_id: i64, attribute_id: i64) -> f64 {
    let z = crate::seed::mix(
        world_seed,
        &[MUTATION_SALT, item_id as u64, attribute_id as u64],
    );
    // The top 53 bits, as `combat::survives` does.
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Whether `item` is in the pilot's location [V `itemCheckers.py:1082-1106`,
/// `baseCheckers.py:549-559` `IsInPilotLocation`]: the active ship with any
/// flag other than the ship hangar (docked or in space), or, docked, the
/// station (any flag) or a container one level inside the hangar. A corp
/// office is not modelled **[A]** (falsifier: a corp member mutating from an
/// office hangar). The client's third arm, an item in the solar system itself,
/// is not accepted: a mutaplasmid is never floating in space here **[A]**.
/// Also **[A]**: a container inside the ship's cargo and an opened jetcan in
/// space are refused although the client offers them [V `itemCheckers.py:1050-1057`];
/// falsifier: a pilot crafting from a can in cargo or a jetcan in space.
fn in_pilot_location(reg: &Registry, actor: i64, item: &Item) -> bool {
    let active = reg
        .character(actor)
        .and_then(|c| crate::fitting::active_ship_of(reg.store(), c))
        .map(|(id, _)| id);
    if Some(item.location_id) == active {
        return item.flag_id != FLAG_SHIP_HANGAR;
    }
    let Some(station) = reg.docked_station(actor) else {
        return false;
    };
    if item.location_id == station {
        return true;
    }
    // One level inside the hangar; a ship other than the active one is no
    // container here **[A]** (falsifier: the client offering craft for an
    // item in another hull's cargo).
    reg.store().get(item.location_id).is_some_and(|c| {
        c.owner_id == actor
            && c.location_id == station
            && c.category_id != crate::model::CATEGORY_SHIP
    })
}

/// `CreateDynamicItem(mutatorItemID, sourceItemID)`: consume one of each, mint
/// the result at the mutator's address, store its rolls. Returns the new item
/// id. Nothing changes on `Err`.
pub fn create(
    reg: &mut Registry,
    actor: Option<i64>,
    mutator_id: i64,
    source_id: i64,
) -> Result<i64, CreateError> {
    let actor = actor.ok_or(CreateError::NoCharacter)?;
    if mutator_id == source_id {
        return Err(CreateError::NotFound);
    }
    let get = |id: i64| reg.store().get(id).cloned().ok_or(CreateError::NotFound);
    let (mutator, source) = (get(mutator_id)?, get(source_id)?);
    if mutator.owner_id != actor || source.owner_id != actor {
        return Err(CreateError::NotYours);
    }
    if crate::item::is_ship_fitting_flag(mutator.flag_id)
        || crate::item::is_ship_fitting_flag(source.flag_id)
    {
        return Err(CreateError::Fitted);
    }
    if !in_pilot_location(reg, actor, &mutator) || !in_pilot_location(reg, actor, &source) {
        return Err(CreateError::NotHere);
    }
    let content = reg.content();
    let mutators = mutator_data::in_content(content);
    let m = mutators
        .get(mutator.type_id)
        .ok_or(CreateError::NotAMutator)?
        .clone();
    let result = mutators
        .result_for(mutator.type_id, source.type_id)
        .ok_or(CreateError::NotApplicable)?;
    let source_row = crate::typedata::installed_in(content)
        .table()
        .get(source.type_id)
        .cloned();
    if !source_row.as_ref().is_some_and(|t| t.published) {
        return Err(CreateError::Unpublished);
    }
    let take = |i: &Item| {
        if i.quantity > 1 {
            Edit::Consume {
                item_id: i.item_id,
                quantity: 1,
            }
        } else {
            Edit::Destroy { item_id: i.item_id }
        }
    };
    let made = reg
        .apply_items(&[
            take(&mutator),
            take(&source),
            Edit::CreateAssembled {
                type_id: result,
                to: Slot::new(actor, mutator.location_id, mutator.flag_id),
            },
        ])
        .map_err(CreateError::Store)?;
    // The plan above has run and consumed the inputs; a missing `Created` is
    // a broken `apply_items`, not a refusal the caller could take back.
    let new_id = made.iter().find_map(|c| match c {
        Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    });
    debug_assert!(new_id.is_some(), "apply_items made no item");
    let new_id = new_id.expect("apply_items answers the created item");
    let seed = reg.world().seed;
    let rolls = m
        .ranges
        .iter()
        .map(|r| (r.attribute_id, roll(seed, new_id, r.attribute_id)))
        .collect();
    put(
        reg,
        &Row {
            item_id: new_id,
            source_type: source.type_id,
            mutator_type: mutator.type_id,
            creator: actor,
            created_at: crate::pyutil::filetime_now(),
            rolls,
        },
    );
    Ok(new_id)
}

/// Rollover: delete every readable row whose item is no longer in the store
/// (destroyed with a ship, reprocessed). An item in contract escrow or a wreck
/// is still an item. Returns the count.
pub fn on_rollover(reg: &mut Registry, _day: &crate::rollover::Day) -> usize {
    let gone: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let id: i64 = k.first()?.parse().ok()?;
            Row::from_value(id, v)?;
            reg.store().get(id).is_none().then(|| k.clone())
        })
        .collect();
    for k in &gone {
        reg.deltas_mut().del(KIND, k);
    }
    gone.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dogma::attr;
    use crate::pyutil::dict_get;
    use crate::service::{Request, ServiceError};

    // Invented ids (the 981_xxx_xxx band); no client data.
    const MUT: i64 = 981_000_001;
    const SRC: i64 = 981_100_001;
    /// Sorts before `SRC`, and is unpublished.
    const SRC_HIDDEN: i64 = 981_099_999;
    const SRC_OTHER: i64 = 981_100_002;
    const RES: i64 = 981_200_001;
    const ATTR_P: i64 = 981_300_001;
    const ATTR_Q: i64 = 981_300_002;
    const ATTR_PLAIN: i64 = 981_300_003;
    const Q_DEFAULT: f64 = 4.0;

    fn content() -> Content {
        let rows = format!(
            "{}attribute\t{ATTR_Q}\t{Q_DEFAULT}\tinventedQ\n\
             type\t{SRC}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\
             {cpu}=40.0;{power}=10.0;{ATTR_P}=100.0;{ATTR_PLAIN}=3.0;{skill}=11.0\tInvented Source\n\
             type\t{SRC_HIDDEN}\t55\t7\t0\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t{cpu}=1.0\tInvented Hidden Source\n\
             type\t{RES}\t55\t7\t1\t1\t7.0\t2.0\t600.0\t30.0\t0\t\t{skill}=22.0\tInvented Result\n\
             type\t{SRC_OTHER}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\tInvented Other\n",
            crate::typedata::FIXTURE,
            cpu = attr::CPU,
            power = attr::POWER,
            skill = attr::REQUIRED_SKILLS[0],
        );
        let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
        let mutators = mutator_data::Mutators::parse(
            &[
                format!("{MUT}\t{}\t0.8\t1.2\t0", attr::CPU),
                format!("{MUT}\t{ATTR_P}\t1.2\t0.8\t1"),
                format!("{MUT}\t{ATTR_Q}\t0.5\t1.5\t"),
            ],
            &[
                format!("{MUT}\t{RES}\t{SRC_HIDDEN}"),
                format!("{MUT}\t{RES}\t{SRC}"),
            ],
        )
        .expect("the test mutators parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c.insert("mutators", mutators);
        c
    }

    /// A registry on `content()`; the first pilot is docked.
    fn world() -> (Registry, i64, i64) {
        let reg = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(content()));
        let me = reg.config().characters[0].character_id;
        let station = reg.docked_station(me).expect("docked");
        (reg, me, station)
    }

    fn row_of(item_id: i64, rolls: Vec<(i64, f64)>) -> Row {
        Row {
            item_id,
            source_type: SRC,
            mutator_type: MUT,
            creator: 7,
            created_at: 133_000_000_000_000_000,
            rolls,
        }
    }

    fn mint(reg: &mut Registry, me: i64, args: &[&str]) -> (String, Option<i64>) {
        let PyValue::Bytes(b) = slash(reg, Some(me), args).expect("answers") else {
            panic!("a text answer")
        };
        let text = String::from_utf8(b).expect("utf8");
        let id = text
            .strip_prefix("minted dynamic item ")
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok());
        (text, id)
    }

    fn call(
        reg: &mut Registry,
        me: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "dynamicItemService",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(me),
        };
        crate::service::dynamic_item::dispatch(reg, &req)
    }

    fn field<'a>(info: &'a PyValue, name: &[u8]) -> &'a PyValue {
        let PyValue::Instance { state, .. } = info else {
            panic!("not a KeyVal")
        };
        dict_get(state, name).expect("field present")
    }

    /// A marshal dict of Int keys and Real values, as `(key, value)`.
    fn int_real(v: &PyValue) -> BTreeMap<i64, f64> {
        let PyValue::Dict(entries) = v else {
            panic!("not a dict")
        };
        entries
            .iter()
            .map(|(val, key)| match (key, val) {
                (PyValue::Int(k), PyValue::Real(r)) => (*k, *r),
                other => panic!("key Int and value Real wanted: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn row_round_trips() {
        let a = row_of(5, vec![(3, 0.25), (9, 0.1 + 0.2)]);
        assert_eq!(Row::from_value(5, &a.to_value()), Some(a.clone()));
        assert_eq!(
            Row::from_value(5, &a.to_value()).expect("reads").rolls[1]
                .1
                .to_bits(),
            (0.1_f64 + 0.2).to_bits(),
            "bit-exact"
        );
        let none = row_of(6, Vec::new());
        assert_eq!(none.to_value()[4], "");
        assert_eq!(Row::from_value(6, &none.to_value()), Some(none));
        // Written ascending whatever order they were given.
        let v = row_of(7, vec![(9, 1.0), (3, 0.5)]).to_value();
        assert_eq!(v[4], "3:0.5;9:1");
    }

    #[test]
    fn a_short_row_reads_with_defaults_and_a_bad_one_is_skipped() {
        let s = |f: &[&str]| f.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let short = Row::from_value(1, &s(&["10", "20"])).expect("two fields are enough");
        assert_eq!(
            (short.creator, short.created_at, short.rolls.len()),
            (0, 0, 0)
        );
        let extra = Row::from_value(1, &s(&["10", "20", "3", "4", "5:0.5", "future"]))
            .expect("extra ignored");
        assert_eq!(extra.rolls, vec![(5, 0.5)]);
        assert_eq!(Row::from_value(1, &s(&["10"])), None, "no mutator");
        assert_eq!(Row::from_value(1, &s(&["x", "20"])), None);
        assert_eq!(
            Row::from_value(1, &s(&["10", "20", "3", "4", "5:oops"])),
            None
        );
        assert_eq!(
            Row::from_value(1, &s(&["10", "20", "3", "4", "5:NaN"])),
            None
        );
        // A row that does not parse stays in place and answers as no row.
        let (mut reg, _, _) = world();
        reg.deltas_mut().put(KIND, key(77), s(&["x", "y"]));
        assert_eq!(row(&reg, 77), None);
        assert!(reg.deltas().of(KIND).contains_key(&key(77)));
    }

    #[test]
    fn attributes_keep_source_values_and_roll_the_ranges() {
        let c = content();
        let r = row_of(
            1,
            vec![(attr::CPU, 1.0), (ATTR_P, 0.25), (ATTR_Q, 0.5), (999, 0.5)],
        );
        let a = attributes(&c, &r, RES);
        assert_eq!(a[&ATTR_PLAIN], 3.0, "a non-rolled source attribute stays");
        assert_eq!(a[&attr::POWER], 10.0);
        let m = mutator_data::in_content(&c).get(MUT).expect("mutator");
        let want = |attr_id: i64, base: f64, modifier: f64| {
            mutator_data::value(base, modifier, m.range(attr_id).expect("range"))
        };
        assert_eq!(a[&attr::CPU], want(attr::CPU, 40.0, 1.0));
        assert_eq!(a[&ATTR_P], want(ATTR_P, 100.0, 0.25));
        assert_eq!(
            a[&ATTR_Q],
            want(ATTR_Q, Q_DEFAULT, 0.5),
            "rolled from the default"
        );
        assert!(!a.contains_key(&999), "a roll with no range is ignored");
        // Without the mutator table the rolls are skipped and it still answers.
        let mut bare = content();
        bare.insert("mutators", mutator_data::Mutators::default());
        let b = attributes(&bare, &r, RES);
        assert_eq!(b[&attr::CPU], 40.0);
        assert!(!b.contains_key(&ATTR_Q));
    }

    #[test]
    fn info_value_has_the_six_fields_and_real_values() {
        let c = content();
        let r = row_of(42, vec![(attr::CPU, 0.5), (ATTR_P, 1.0)]);
        let info = info_value(&c, &r, RES);
        assert_eq!(field(&info, b"itemID"), &PyValue::Int(42));
        assert_eq!(field(&info, b"typeID"), &PyValue::Int(RES));
        assert_eq!(field(&info, b"sourceTypeID"), &PyValue::Int(SRC));
        assert_eq!(field(&info, b"mutatorTypeID"), &PyValue::Int(MUT));
        let attrs = int_real(field(&info, b"attributes"));
        assert_eq!(attrs, attributes(&c, &r, RES));
        let mods = int_real(field(&info, b"modifiers"));
        assert_eq!(mods, BTreeMap::from([(attr::CPU, 0.5), (ATTR_P, 1.0)]));
    }

    #[test]
    fn slash_mints_one_assembled_item_with_a_row_and_refuses_a_bad_pair() {
        let (mut reg, me, station) = world();
        let src = SRC.to_string();
        let mutator = MUT.to_string();
        let (text, id) = mint(&mut reg, me, &[&src, &mutator, "1.0"]);
        let id = id.unwrap_or_else(|| panic!("minted: {text}"));
        let it = reg.store().get(id).expect("the item").clone();
        assert_eq!(
            (
                it.type_id,
                it.singleton,
                it.quantity,
                it.owner_id,
                it.location_id,
                it.flag_id
            ),
            (RES, true, 1, me, station, crate::model::FLAG_HANGAR)
        );
        let r = row(&reg, id).expect("a row");
        assert_eq!((r.source_type, r.mutator_type, r.creator), (SRC, MUT, me));
        assert!(r.created_at > 0);
        assert_eq!(r.rolls.len(), 3, "every range of the mutator");
        assert!(r.rolls.iter().all(|&(_, m)| m == 1.0));
        // The default roll is the middle.
        let (_, id2) = mint(&mut reg, me, &[&src, &mutator]);
        assert!(
            row(&reg, id2.expect("minted"))
                .expect("row")
                .rolls
                .iter()
                .all(|&(_, m)| m == 0.5)
        );
        let before = reg.store().items().len();
        let hidden = SRC_HIDDEN.to_string();
        let other = SRC_OTHER.to_string();
        let res = RES.to_string();
        for args in [
            vec![&src[..]],
            vec![&src[..], "1"],
            vec![&src[..], "oops"],
            vec![&src[..], &mutator[..], "1.5"],
            vec![&src[..], &mutator[..], "-0.1"],
            vec![&src[..], &mutator[..], "NaN"],
            vec![&hidden[..], &mutator[..]],
            vec![&other[..], &mutator[..]],
            vec![&src[..], &res[..]],
        ] {
            let (text, id) = mint(&mut reg, me, &args);
            assert!(id.is_none(), "{args:?} must refuse: {text}");
        }
        assert_eq!(reg.store().items().len(), before, "a refusal makes nothing");
        assert_eq!(reg.deltas().of(KIND).len(), 2);
    }

    #[test]
    fn get_dynamic_item_info_answers_any_owner_and_refuses_a_plain_item() {
        let (mut reg, me, station) = world();
        let (_, id) = mint(&mut reg, me, &[&SRC.to_string(), &MUT.to_string(), "0.0"]);
        let id = id.expect("minted");
        // A stranger asks.
        let info =
            call(&mut reg, me + 1, "GetDynamicItemInfo", &[PyValue::Int(id)]).expect("answers");
        assert_eq!(field(&info, b"sourceTypeID"), &PyValue::Int(SRC));
        let attrs = int_real(field(&info, b"attributes"));
        assert_eq!(attrs[&attr::CPU], 40.0 * 0.8, "modifier 0 is the min");
        // A plain item, an unknown id, a missing argument.
        reg.apply_items(&[Edit::Create {
            type_id: SRC,
            quantity: 1,
            to: Slot::new(me, station, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
        let plain = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == SRC)
            .expect("plain")
            .item_id;
        for arg in [vec![PyValue::Int(plain)], vec![PyValue::Int(-5)], vec![]] {
            assert!(
                matches!(
                    call(&mut reg, me, "GetDynamicItemInfo", &arg),
                    Err(ServiceError::Raise(_))
                ),
                "{arg:?}"
            );
        }
        assert!(matches!(
            call(
                &mut reg,
                me,
                "CreateDynamicItem",
                &[PyValue::Int(1), PyValue::Int(2)]
            ),
            Err(ServiceError::Raise(_))
        ));
        assert!(matches!(
            call(&mut reg, me, "Nope", &[]),
            Err(ServiceError::NoSuchMethod { .. })
        ));
    }

    #[test]
    fn an_item_with_a_row_answers_even_with_an_empty_mutator_table() {
        let mut bare = content();
        bare.insert("mutators", mutator_data::Mutators::default());
        let mut reg = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(bare));
        let me = reg.config().characters[0].character_id;
        let station = reg.docked_station(me).expect("docked");
        reg.apply_items(&[Edit::CreateAssembled {
            type_id: RES,
            to: Slot::new(me, station, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
        let id = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == RES)
            .expect("it")
            .item_id;
        assert!(call(&mut reg, me, "GetDynamicItemInfo", &[PyValue::Int(id)]).is_err());
        put(&mut reg, &row_of(id, vec![(attr::CPU, 0.5)]));
        let info = call(&mut reg, me, "GetDynamicItemInfo", &[PyValue::Int(id)]).expect("answers");
        assert_eq!(field(&info, b"mutatorTypeID"), &PyValue::Int(MUT));
    }

    #[test]
    fn a_dynamic_item_with_no_row_gets_the_fallback_never_none() {
        let (mut reg, me, station) = world();
        // Born with no row: made through the edit, not the command.
        reg.apply_items(&[Edit::CreateAssembled {
            type_id: RES,
            to: Slot::new(me, station, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
        let id = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == RES)
            .expect("it")
            .item_id;
        let info = call(&mut reg, me, "GetDynamicItemInfo", &[PyValue::Int(id)]).expect("fallback");
        assert_eq!(field(&info, b"mutatorTypeID"), &PyValue::Int(MUT));
        assert_eq!(
            field(&info, b"sourceTypeID"),
            &PyValue::Int(SRC),
            "the first published source"
        );
        assert_eq!(int_real(field(&info, b"modifiers")).len(), 0);
        assert_eq!(int_real(field(&info, b"attributes"))[&attr::CPU], 40.0);
    }

    #[test]
    fn fallback_names_a_mutator_that_makes_the_type() {
        let c = content();
        let info = fallback_info(&c, 9, RES).expect("a mutator makes it");
        assert_eq!(field(&info, b"itemID"), &PyValue::Int(9));
        assert_eq!(field(&info, b"mutatorTypeID"), &PyValue::Int(MUT));
        assert!(
            fallback_info(&c, 9, SRC).is_none(),
            "no mutator makes a source"
        );
    }

    #[test]
    fn the_sweep_drops_rows_of_destroyed_items_only() {
        let (mut reg, me, _) = world();
        let (s, m) = (SRC.to_string(), MUT.to_string());
        let keep = mint(&mut reg, me, &[&s, &m]).1.expect("a");
        let gone = mint(&mut reg, me, &[&s, &m]).1.expect("b");
        reg.apply_items(&[Edit::Destroy { item_id: gone }])
            .expect("destroyed");
        // A row nobody can read stays, even for an item that is not there.
        reg.deltas_mut().put(KIND, key(88), vec!["x".to_owned()]);
        let day = crate::rollover::Day {
            day: 1,
            days: 1,
            now: Default::default(),
        };
        assert_eq!(on_rollover(&mut reg, &day), 1);
        assert!(row(&reg, keep).is_some());
        assert!(row(&reg, gone).is_none());
        assert!(reg.deltas().of(KIND).contains_key(&key(88)));
        assert_eq!(on_rollover(&mut reg, &day), 0);
    }

    #[test]
    fn the_dynitem_kind_survives_the_registry() {
        let (mut reg, _, _) = world();
        let r = row_of(31_990_001, vec![(attr::CPU, 0.1 + 0.2)]);
        put(&mut reg, &r);
        let snap = reg.to_snapshot();
        let mut fresh = Registry::new(crate::model::SandboxConfig::default());
        fresh.apply_snapshot(snap);
        assert_eq!(row(&fresh, 31_990_001), Some(r));
    }
}

/// MUTA-3 (#71): a fitted mutated module reads its rolls. Invented ids
/// (the 981_4xx_xxx band) and invented numbers; no client data.
#[cfg(test)]
mod fitted_tests {
    use super::*;
    use crate::dogma::attr;
    use crate::fitting::Rack;

    const MUT: i64 = 981_400_001;
    const SRC: i64 = 981_410_001;
    /// A source that fits only a group no test hull is in.
    const SRC_RESTRICTED: i64 = 981_410_002;
    const RES: i64 = 981_420_001;
    const HULL: i64 = 990_000_001;
    /// Not a group of the hull above.
    const OTHER_GROUP: i64 = 981_430_001;
    const SRC_CPU: f64 = 40.0;
    const SRC_DURATION: f64 = 2500.0;

    fn world() -> (Registry, i64, i64, i64) {
        let rows = format!(
            "{}type\t{SRC}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t12,16\t\
             {cpu}={SRC_CPU};{power}=10.0;{dur}={SRC_DURATION}\tInvented Source\n\
             type\t{SRC_RESTRICTED}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t12,16\t\
             {cpu}=5.0;1298={OTHER_GROUP}.0\tInvented Restricted\n\
             type\t{RES}\t55\t7\t1\t1\t7.0\t2.0\t600.0\t30.0\t0\t12,16\t\t\
             Invented Result\n",
            crate::typedata::FIXTURE,
            cpu = attr::CPU,
            power = attr::POWER,
            dur = crate::modules::attr::DURATION,
        );
        let table = crate::typedata::Table::parse(&rows).expect("rows parse");
        let mutators = mutator_data::Mutators::parse(
            &[format!("{MUT}\t{}\t0.8\t1.2\t0", attr::CPU)],
            &[
                format!("{MUT}\t{RES}\t{SRC}"),
                format!("{MUT}\t{RES}\t{SRC_RESTRICTED}"),
            ],
        )
        .expect("mutators parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c.insert("mutators", mutators);
        let mut reg =
            Registry::new(crate::model::SandboxConfig::default()).with_content(Arc::new(c));
        let me = reg.config().characters[0].character_id;
        let station = reg.docked_station(me).expect("docked");
        let made = reg
            .apply_items(&[Edit::CreateAssembled {
                type_id: HULL,
                to: Slot::new(me, station, crate::model::FLAG_HANGAR),
            }])
            .expect("hull");
        let Change::Created { item_id: ship, .. } = made[0] else {
            panic!("created")
        };
        reg.character_mut(me).expect("me").ship_id = ship;
        reg.character_mut(me).expect("me").ship_type_id = HULL;
        (reg, me, station, ship)
    }

    fn mint(reg: &mut Registry, me: i64, src: i64, roll: &str) -> i64 {
        let (s, m) = (src.to_string(), MUT.to_string());
        let PyValue::Bytes(b) = slash(reg, Some(me), &[&s, &m, roll]).expect("answers") else {
            panic!("text")
        };
        String::from_utf8(b)
            .expect("utf8")
            .strip_prefix("minted dynamic item ")
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("minted")
    }

    fn fit(reg: &mut Registry, me: i64, ship: i64, item: i64, index: i64) {
        reg.apply_items(&[Edit::Move {
            item_id: item,
            to: Slot::new(me, ship, Rack::High.first_flag() + index),
        }])
        .expect("fitted");
    }

    fn row_cpu(reg: &Registry, item: i64) -> f64 {
        let r = row(reg, item).expect("row");
        attributes(reg.content(), &r, RES)[&attr::CPU]
    }

    #[test]
    fn a_fitted_module_reads_its_roll_in_the_sheet() {
        let (mut reg, me, _, ship) = world();
        let hi = mint(&mut reg, me, SRC, "1.0");
        let lo = mint(&mut reg, me, SRC, "0.0");
        let (hi_cpu, lo_cpu) = (row_cpu(&reg, hi), row_cpu(&reg, lo));
        assert!(
            hi_cpu > 0.0 && hi_cpu != lo_cpu,
            "the rolls differ and are not 0"
        );
        fit(&mut reg, me, ship, hi, 0);
        let s = crate::dogma::sheet(&reg, ship).expect("a sheet");
        assert_eq!(s.module(hi, attr::CPU), Some(hi_cpu));
        // An unrolled attribute is the source's, not the dynamic type's none.
        assert_eq!(s.module(hi, attr::POWER), Some(10.0));
        // Swap the module: the sheet follows (the cache key sees the roll).
        reg.apply_items(&[Edit::Move {
            item_id: hi,
            to: Slot::new(
                me,
                reg.docked_station(me).expect("docked"),
                crate::model::FLAG_HANGAR,
            ),
        }])
        .expect("unfit");
        fit(&mut reg, me, ship, lo, 0);
        let s2 = crate::dogma::sheet(&reg, ship).expect("a sheet");
        assert_eq!(s2.module(lo, attr::CPU), Some(lo_cpu));
    }

    #[test]
    fn fitted_lists_only_dynamic_modules_and_caches_them() {
        let (mut reg, me, _, ship) = world();
        assert!(fitted(&reg, ship).is_empty());
        let a = mint(&mut reg, me, SRC, "0.5");
        fit(&mut reg, me, ship, a, 0);
        let list = fitted(&reg, ship);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, a);
        let again = fitted(&reg, ship);
        assert!(
            Arc::ptr_eq(&list[0].1, &again[0].1),
            "served from the cache"
        );
        assert_eq!(list[0].1.get(attr::CPU), Some(row_cpu(&reg, a)));
        // The sheet is reused while nothing changed.
        let s1 = crate::dogma::sheet(&reg, ship).expect("sheet");
        let s2 = crate::dogma::sheet(&reg, ship).expect("sheet");
        assert!(Arc::ptr_eq(&s1, &s2));
    }

    #[test]
    fn module_info_serves_the_overridden_duration() {
        let (mut reg, me, _, ship) = world();
        let a = mint(&mut reg, me, SRC, "1.0");
        fit(&mut reg, me, ship, a, 0);
        let c = reg.character(me).expect("me").clone();
        let item = reg.item(a).expect("item").clone();
        let info = crate::modules::module_info(&reg, &c, &item);
        let text = format!("{info:?}");
        // The list carries the roll's cpu and the source's cycle.
        let cpu = format!("Real({:?}), Int({})", row_cpu(&reg, a), attr::CPU);
        let dur = format!(
            "Real({:?}), Int({})",
            SRC_DURATION,
            crate::modules::attr::DURATION
        );
        assert!(text.contains(&cpu), "cpu {cpu} missing in {text}");
        assert!(text.contains(&dur), "duration {dur} missing in {text}");
    }

    #[test]
    fn the_fit_check_reads_the_rolled_hull_restriction() {
        let (mut reg, me, _, ship) = world();
        let plain = mint(&mut reg, me, SRC, "0.5");
        let tied = mint(&mut reg, me, SRC_RESTRICTED, "0.5");
        let c = reg.character(me).cloned();
        let go = |m: i64| {
            crate::fitting::plan_fit_for(&reg, c.as_ref(), ship, m, crate::fitting::FLAG_AUTOFIT)
        };
        assert!(go(plain).is_ok(), "an unrestricted source fits");
        assert!(go(tied).is_err(), "the source's group restriction holds");
    }

    /// Replacing a stored row drops the cached roll, so the next read sees it.
    #[test]
    fn put_drops_the_cached_overrides() {
        let (mut reg, me, _, _) = world();
        let a = mint(&mut reg, me, SRC, "0.0");
        let item = reg.item(a).expect("item").clone();
        let before = overrides(&reg, &item).expect("rolled");
        let mut r = row(&reg, a).expect("row");
        for roll in &mut r.rolls {
            roll.1 = 1.0 - roll.1;
        }
        assert!(!r.rolls.is_empty(), "precondition: the roll has entries");
        put(&mut reg, &r);
        let after = overrides(&reg, &item).expect("rolled");
        assert_ne!(before, after);
    }

    /// The loads a hull without a sheet reports come from the roll too.
    #[test]
    fn the_fallback_load_reads_the_roll() {
        let (mut reg, me, _, ship) = world();
        let a = mint(&mut reg, me, SRC, "1.0");
        fit(&mut reg, me, ship, a, 0);
        reg.modules_mut().set_online(a, true);
        let (cpu, power) = crate::fitstate::unmodified_load(&reg, ship);
        assert_eq!(cpu, row_cpu(&reg, a));
        assert_eq!(power, 10.0);
    }

    #[test]
    fn a_plain_item_has_no_overrides() {
        let (reg, _, _, ship) = world();
        let hull = reg.item(ship).expect("hull");
        assert!(overrides(&reg, hull).is_none());
    }
}

#[cfg(test)]
mod create_tests {
    //! Applying a mutaplasmid (#70 MUTA-2). Invented ids (the 981_xxx_xxx
    //! band) and invented ranges; no client data, no `[A]` value pinned: the
    //! uniform roll is only checked to stay inside `[0, 1)`.
    use super::*;
    use crate::dogma::attr;

    const MUT: i64 = 981_000_001;
    const MUT_DRONE: i64 = 981_000_002;
    const MUT_GHOST: i64 = 981_000_003;
    const SRC: i64 = 981_100_001;
    const SRC_HIDDEN: i64 = 981_100_002;
    const SRC_OTHER: i64 = 981_100_003;
    const SRC_DRONE: i64 = 981_100_004;
    const RES: i64 = 981_200_001;
    const RES_DRONE: i64 = 981_200_002;
    /// In the mutator table but not in the type table: the plan then fails.
    const RES_GHOST: i64 = 981_200_003;
    const ATTR_P: i64 = 981_300_001;
    const ATTR_Q: i64 = 981_300_002;
    const Q_DEFAULT: f64 = 4.0;
    const SRC_CPU: f64 = 40.0;
    const SRC_P: f64 = 100.0;
    const STRANGER: i64 = 981_999_001;
    const OTHER_PLACE: i64 = 981_999_002;

    fn content() -> Content {
        let rows = format!(
            "{}attribute\t{ATTR_Q}\t{Q_DEFAULT}\tinventedQ\n\
             type\t{MUT}\t1964\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Mutator\n\
             type\t{MUT_DRONE}\t1964\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Drone Mutator\n\
             type\t{MUT_GHOST}\t1964\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Ghost Mutator\n\
             type\t{SRC}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\
             {cpu}={SRC_CPU};{ATTR_P}={SRC_P}\tInvented Source\n\
             type\t{SRC_HIDDEN}\t55\t7\t0\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\tInvented Hidden\n\
             type\t{SRC_OTHER}\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\tInvented Other\n\
             type\t{SRC_DRONE}\t100\t18\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t\t\tInvented Drone Source\n\
             type\t{RES}\t55\t7\t1\t1\t7.0\t2.0\t600.0\t30.0\t0\t\t\tInvented Result\n\
             type\t{RES_DRONE}\t100\t18\t1\t1\t7.0\t2.0\t600.0\t30.0\t0\t\t\tInvented Drone Result\n",
            crate::typedata::FIXTURE,
            cpu = attr::CPU,
        );
        let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
        let mutators = mutator_data::Mutators::parse(
            &[
                format!("{MUT}\t{}\t0.8\t1.2\t0", attr::CPU),
                format!("{MUT}\t{ATTR_P}\t1.2\t0.8\t1"),
                format!("{MUT}\t{ATTR_Q}\t0.5\t1.5\t"),
                format!("{MUT_DRONE}\t{}\t0.8\t1.2\t0", attr::CPU),
                format!("{MUT_GHOST}\t{}\t0.8\t1.2\t0", attr::CPU),
            ],
            &[
                format!("{MUT}\t{RES}\t{SRC}"),
                format!("{MUT}\t{RES}\t{SRC_HIDDEN}"),
                format!("{MUT_DRONE}\t{RES_DRONE}\t{SRC_DRONE}"),
                format!("{MUT_GHOST}\t{RES_GHOST}\t{SRC}"),
            ],
        )
        .expect("the test mutators parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c.insert("mutators", mutators);
        c
    }

    /// A registry on `content()`; the first pilot is docked.
    fn world() -> (Registry, i64, i64) {
        let reg =
            Registry::new(crate::model::SandboxConfig::default()).with_content(Arc::new(content()));
        let me = reg.config().characters[0].character_id;
        let station = reg.docked_station(me).expect("docked");
        (reg, me, station)
    }

    fn give(reg: &mut Registry, type_id: i64, quantity: i64, to: Slot) -> i64 {
        let made = reg
            .apply_items(&[Edit::Create {
                type_id,
                quantity,
                to,
            }])
            .expect("created");
        made[0].item_id()
    }

    fn hangar(me: i64, station: i64) -> Slot {
        Slot::new(me, station, crate::model::FLAG_HANGAR)
    }

    fn count(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    #[test]
    fn create_consumes_one_of_each_and_makes_one_assembled_result() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 3, hangar(me, station));
        let s = give(&mut reg, SRC, 3, hangar(me, station));
        let id = create(&mut reg, Some(me), m, s).expect("applied");
        assert_eq!(reg.store().get(m).expect("stack stays").quantity, 2);
        assert_eq!(reg.store().get(s).expect("stack stays").quantity, 2);
        let it = reg.store().get(id).expect("the result").clone();
        assert_eq!(
            (it.type_id, it.singleton, it.quantity, it.owner_id),
            (RES, true, 1, me)
        );
        assert_eq!(count(&reg, RES), 1);
        let r = row(&reg, id).expect("a row");
        assert_eq!((r.source_type, r.mutator_type, r.creator), (SRC, MUT, me));
        assert!(r.created_at > 0);
        // Stacks of one are used up.
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 1, hangar(me, station));
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        create(&mut reg, Some(me), m, s).expect("applied");
        assert!(reg.store().get(m).is_none(), "the mutaplasmid is gone");
        assert!(reg.store().get(s).is_none(), "the source is gone");
        assert_eq!(count(&reg, RES), 1);
    }

    #[test]
    fn create_makes_a_mutated_drone() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT_DRONE, 1, hangar(me, station));
        let s = give(&mut reg, SRC_DRONE, 1, hangar(me, station));
        let id = create(&mut reg, Some(me), m, s).expect("a mutated drone is made (#224)");
        let it = reg.store().get(id).expect("the result").clone();
        assert_eq!(
            (it.type_id, it.singleton, it.quantity, it.owner_id),
            (RES_DRONE, true, 1, me)
        );
        assert_eq!(
            crate::item::find_in(reg.content(), it.type_id).map(|t| t.category_id),
            Some(crate::typedata::CATEGORY_DRONE)
        );
        assert_eq!(
            (it.location_id, it.flag_id),
            (station, crate::model::FLAG_HANGAR)
        );
        let r = row(&reg, id).expect("a row");
        let mu = mutator_data::in_content(reg.content())
            .get(MUT_DRONE)
            .expect("mut");
        assert_eq!(r.rolls.len(), mu.ranges.len(), "one roll per range");
        assert_eq!((r.source_type, r.mutator_type), (SRC_DRONE, MUT_DRONE));
    }

    #[test]
    fn create_puts_the_result_at_the_mutators_address() {
        let (mut reg, me, station) = world();
        let (ship, _) = crate::fitting::active_ship_of(reg.store(), reg.character(me).expect("me"))
            .expect("a ship");
        let m = give(
            &mut reg,
            MUT,
            1,
            Slot::new(me, ship, crate::fitting::FLAG_CARGO),
        );
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        let id = create(&mut reg, Some(me), m, s).expect("applied");
        let it = reg.store().get(id).expect("result");
        assert_eq!(
            (it.location_id, it.flag_id),
            (ship, crate::fitting::FLAG_CARGO)
        );
    }

    #[test]
    fn create_refuses() {
        let (mut reg, me, station) = world();
        let (ship, _) = crate::fitting::active_ship_of(reg.store(), reg.character(me).expect("me"))
            .expect("a ship");
        let m = give(&mut reg, MUT, 2, hangar(me, station));
        let s = give(&mut reg, SRC, 2, hangar(me, station));
        let mg = give(&mut reg, MUT_GHOST, 1, hangar(me, station));
        let s_hidden = give(&mut reg, SRC_HIDDEN, 1, hangar(me, station));
        let s_other = give(&mut reg, SRC_OTHER, 1, hangar(me, station));
        let s_fitted = give(&mut reg, SRC, 1, Slot::new(me, ship, 27));
        let s_stranger = give(&mut reg, SRC, 1, Slot::new(STRANGER, station, 4));
        let s_hold = give(&mut reg, SRC, 1, Slot::new(me, ship, FLAG_SHIP_HANGAR));
        let s_away = give(&mut reg, SRC, 1, Slot::new(me, OTHER_PLACE, 4));
        let items = reg.store().items().len();
        let rows = reg.deltas().of(KIND).len();
        let cases: Vec<(&str, Option<i64>, i64, i64, CreateError)> = vec![
            ("no character", None, m, s, CreateError::NoCharacter),
            (
                "missing source",
                Some(me),
                m,
                981_888_888,
                CreateError::NotFound,
            ),
            (
                "missing mutator",
                Some(me),
                981_888_888,
                s,
                CreateError::NotFound,
            ),
            ("same id twice", Some(me), m, m, CreateError::NotFound),
            (
                "other owner",
                Some(me),
                m,
                s_stranger,
                CreateError::NotYours,
            ),
            ("fitted source", Some(me), m, s_fitted, CreateError::Fitted),
            (
                "ship hangar flag",
                Some(me),
                m,
                s_hold,
                CreateError::NotHere,
            ),
            ("another place", Some(me), m, s_away, CreateError::NotHere),
            ("not a mutator", Some(me), s, s, CreateError::NotFound),
            (
                "source as mutator",
                Some(me),
                s,
                m,
                CreateError::NotAMutator,
            ),
            (
                "no mapping",
                Some(me),
                m,
                s_other,
                CreateError::NotApplicable,
            ),
            (
                "unpublished source",
                Some(me),
                m,
                s_hidden,
                CreateError::Unpublished,
            ),
            (
                "plan refused",
                Some(me),
                mg,
                s,
                CreateError::Store(crate::item::EditError::UnknownType(RES_GHOST)),
            ),
        ];
        for (what, actor, mutator, source, want) in cases {
            assert_eq!(
                create(&mut reg, actor, mutator, source),
                Err(want),
                "{what}"
            );
            assert_eq!(reg.store().items().len(), items, "{what}: no item changed");
            assert_eq!(reg.store().get(m).expect("m").quantity, 2, "{what}");
            assert_eq!(reg.deltas().of(KIND).len(), rows, "{what}: no row written");
        }
    }

    #[test]
    fn the_rolls_are_in_range_and_repeatable() {
        let seed = 0x1234_5678_9abc_def0;
        for i in 0..1000i64 {
            let id = 981_500_000 + i * 7;
            let r = roll(seed, id, ATTR_P);
            assert!((0.0..1.0).contains(&r), "{r}");
            assert_eq!(r, roll(seed, id, ATTR_P), "repeatable");
        }
        assert_ne!(roll(seed, 1, ATTR_P), roll(seed, 1, ATTR_Q));
        assert_ne!(roll(seed, 1, ATTR_P), roll(seed ^ 1, 1, ATTR_P));
        assert_ne!(roll(seed, 1, ATTR_P), roll(seed, 2, ATTR_P));
    }

    #[test]
    fn stored_values_follow_the_formula() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 1, hangar(me, station));
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        let seed = reg.world().seed;
        let id = create(&mut reg, Some(me), m, s).expect("applied");
        let r = row(&reg, id).expect("row");
        let attrs = attributes(reg.content(), &r, RES);
        let mu = mutator_data::in_content(reg.content())
            .get(MUT)
            .expect("mut");
        assert_eq!(r.rolls.len(), mu.ranges.len(), "every range rolled");
        for (a, base) in [(attr::CPU, SRC_CPU), (ATTR_P, SRC_P), (ATTR_Q, Q_DEFAULT)] {
            let range = mu.range(a).expect("range");
            let m_a = roll(seed, id, a);
            let want = mutator_data::value(base, m_a, range);
            assert_eq!(attrs.get(&a).copied(), Some(want), "attribute {a}");
        }
    }

    #[test]
    fn a_failed_plan_writes_no_row() {
        let (mut reg, me, station) = world();
        let mg = give(&mut reg, MUT_GHOST, 1, hangar(me, station));
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        let rows = reg.deltas().of(KIND).len();
        assert!(matches!(
            create(&mut reg, Some(me), mg, s),
            Err(CreateError::Store(_))
        ));
        assert_eq!(reg.deltas().of(KIND).len(), rows);
        assert!(reg.store().get(mg).is_some() && reg.store().get(s).is_some());
    }

    #[test]
    fn create_pushes_the_inventory_changes() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 3, hangar(me, station));
        let s = give(&mut reg, SRC, 3, hangar(me, station));
        let _ = reg.take_pending();
        let id = create(&mut reg, Some(me), m, s).expect("applied");
        let pushes: Vec<String> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p)
            .filter(|p| p.to_push().0 == "OnItemChange")
            .map(|p| format!("{:?}", p.to_push().1))
            .collect();
        for item in [m, s, id] {
            assert!(
                pushes.iter().any(|p| p.contains(&item.to_string())),
                "no OnItemChange for {item}: {pushes:?}"
            );
        }
    }

    #[test]
    fn the_ledger_books_the_mint_and_the_two_consumed() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 1, hangar(me, station));
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        let before = crate::econ_ledger::goods_today(&reg);
        create(&mut reg, Some(me), m, s).expect("applied");
        let after = crate::econ_ledger::goods_today(&reg);
        let class = crate::econ_ledger::goods_class(7).expect("module class");
        let (m0, d0) = before.0.get(class).copied().unwrap_or((0, 0));
        let (m1, d1) = after.0.get(class).copied().unwrap_or((0, 0));
        // The module-class source is destroyed and the module-class result
        // minted: they net to nothing for one type each, so both show.
        assert_eq!((m1 - m0, d1 - d0), (1, 1), "{before:?} -> {after:?}");
        let (tm0, td0) = before.total();
        let (tm1, td1) = after.total();
        assert_eq!((tm1 - tm0, td1 - td0), (1, 2), "mutaplasmid counted too");
    }

    #[test]
    fn in_space_the_active_ship_is_the_pilots_location() {
        let (mut reg, me, station) = world();
        let (ship, _) = crate::fitting::active_ship_of(reg.store(), reg.character(me).expect("me"))
            .expect("a ship");
        crate::destiny::on_ballpark_bind(&mut reg, Some(me));
        let _ = reg.take_pending();
        assert!(reg.docked_station(me).is_none(), "the pilot is in space");
        let cargo = Slot::new(me, ship, crate::fitting::FLAG_CARGO);
        let m = give(&mut reg, MUT, 2, cargo);
        let s = give(&mut reg, SRC, 2, cargo);
        let held = give(&mut reg, SRC, 1, Slot::new(me, ship, FLAG_SHIP_HANGAR));
        let away = give(&mut reg, SRC, 1, hangar(me, station));
        assert_eq!(
            create(&mut reg, Some(me), m, held),
            Err(CreateError::NotHere),
            "the ship-hangar flag is refused in space too"
        );
        assert_eq!(
            create(&mut reg, Some(me), m, away),
            Err(CreateError::NotHere),
            "a station hangar is out of reach in space"
        );
        let id = create(&mut reg, Some(me), m, s).expect("cargo of the active ship");
        let it = reg.store().get(id).expect("result");
        assert_eq!(
            (it.location_id, it.flag_id),
            (ship, crate::fitting::FLAG_CARGO)
        );
    }

    #[test]
    fn two_crafts_from_the_same_stacks_roll_differently() {
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 3, hangar(me, station));
        let s = give(&mut reg, SRC, 3, hangar(me, station));
        let a = create(&mut reg, Some(me), m, s).expect("first");
        let b = create(&mut reg, Some(me), m, s).expect("second");
        assert_eq!(
            reg.store().get(m).expect("stack").item_id,
            m,
            "the stack kept its id"
        );
        let (ra, rb) = (
            row(&reg, a).expect("a").rolls,
            row(&reg, b).expect("b").rolls,
        );
        assert_ne!(ra, rb, "the same two stacks rolled the same");
    }

    #[test]
    fn the_arm_answers_the_new_id_and_refuses_with_a_notify() {
        use crate::service::Request;
        let (mut reg, me, station) = world();
        let m = give(&mut reg, MUT, 1, hangar(me, station));
        let s = give(&mut reg, SRC, 1, hangar(me, station));
        let call = |reg: &mut Registry, a: i64, b: i64| {
            let args = [PyValue::Int(a), PyValue::Int(b)];
            let req = Request {
                service: "dynamicItemService",
                method: "CreateDynamicItem",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(me),
            };
            crate::service::dynamic_item::dispatch(reg, &req)
        };
        assert!(call(&mut reg, m, 981_888_888).is_err(), "a refusal raises");
        assert!(reg.store().get(m).is_some(), "and consumes nothing");
        let Ok(PyValue::Int(id)) = call(&mut reg, m, s) else {
            panic!("the new item id")
        };
        assert_eq!(reg.store().get(id).expect("result").type_id, RES);
    }
}
