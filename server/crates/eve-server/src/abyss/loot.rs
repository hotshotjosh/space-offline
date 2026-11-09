//! **Caches and loot of the abyssal rooms** (issue #5, AD-2).
//!
//! A room holds one Triglavian Bioadaptive Cache beside its waves, and the
//! extraction nodes its dungeon lists. All are NPCs of group
//! [`GROUP_CACHE`] **[V]** (`groupPrecursorCache`, category 11 **[V-data]**):
//! shootable, outside the client's `AbyssTargets` list, so a living cache
//! never holds the room's gate (`abyss::room_clear`). Each leaves its own
//! wreck (`types/market.tsv` `wreck_type_id` **[V-data]**), and the wreck
//! holds what the authored table `content/pve/abyss-loot.tsv` draws for it.
//!
//! ## Where the loot comes from
//!
//! The table is **authored, every rate [A]**: it names a type group, a word
//! of the type's name and a chance, never a type id. [`candidates`] reads the
//! types from the loaded type data, [`roll`] draws from a
//! [`Stream`](crate::seed::Stream) keyed by the world seed and the NPC's id,
//! so a replay drops the same. The roll is a pure function; the world side
//! ([`place`], [`kind_for`], [`on_npc_kill`]) only spawns, finds the wreck and
//! creates the items. `eve-mod-econ` calls [`on_npc_kill`] from its kill-hook
//! row (`econ_abyss_cache_loot`): the owner's table, the core's rule.
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **Every rate, quantity and row of the table.** Falsified by a week of
//!   runs at a tier showing far more or far less (the `why` column).
//! - **The cache stands on the room's `loot_structure_locator`** ([`LOOT_LOCATOR`],
//!   **[I]** from the name) or, with none, **[A]** midway between the entry
//!   and the exit locators. Falsified by a cache inside a rock or outside the
//!   boundary in the client, or the client's objective marker pointing
//!   elsewhere.
//! - **Wave NPCs get their wrecks too** (groups 1982 and 1997 **[V-data]**
//!   carry a `wreck_type_id`), so they can be salvaged; their loot stays
//!   `npc::loot_in`'s. Falsified by a pilot finding Triglavian wrecks wrong;
//!   drop the 1982/1997 half of [`WRECK_GROUPS`].
//! - **A closed pocket takes its wrecks** (`abyss::close_pocket`): a closed
//!   pocket can never be entered again and `alloc_pocket` refuses a system
//!   with a wreck in it. That includes a pilot's own wreck left in the pocket
//!   (one log line says so when it happens). Falsified by the owner's answer
//!   to the abandoned-wreck question, or a pilot who loses a wreck they
//!   meant to come back for.
//! - **The `{grade}` token**: Decayed for tiers 0 and 1, Gravid for 2 and 3,
//!   Unstable for 4 to 6. Falsified by TQ players' drop logs.
//! - **The owner id of a cache** is the wave owner, as the waves'.
//!
//! ## A cache that does not fight
//!
//! Its kind is derived with no weapon (damage, range and speed are zero in
//! its data **[V-data]**), and `npc::brain::decide` leaves an unarmed type
//! alone, so it never locks, fires or moves.

use super::{AbyssState, Pocket, Run, Spots, WAVE_OWNER, play_tier};
use crate::authored::Row;
use crate::combat::FLAG_IN_WRECK;
use crate::content::{Boxed, Origin, Sources, TableSpec};
use crate::dungeon_data::Dungeon;
use crate::item::{Change, Edit, Slot};
use crate::module::KillEvent;
use crate::npc::NpcType;
use crate::seed::{self, Kind, Stream};
use crate::service::Registry;
use crate::site;
use std::collections::BTreeMap;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "pve/abyss-loot.tsv";

/// Its header, copied into `docs/content/schema.md`.
pub const HEADER: &str = "tier\tcache\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\tcopy\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../../content/pve/abyss-loot.tsv");

/// `groupPrecursorCache` **[V]** `C:inventorycommon/const.py:1100`.
pub const GROUP_CACHE: i64 = 2009;

/// `loot_structure_locator` **[V-data]** (group 1973 `groupLocators`).
pub const LOOT_LOCATOR: i64 = 50135;

/// The entity groups whose wreck the table knows **[V-data]**: the abyssal
/// drones and Triglavians the waves spawn (1982, 1997) and the caches
/// ([`GROUP_CACHE`]). The first two are **[A]** (see the header).
pub const WRECK_GROUPS: [i64; 3] = [1982, 1997, GROUP_CACHE];

/// One draw's denominator: `chance_ppm` is parts per million.
pub const PPM: u64 = 1_000_000;

/// A domain tag in the loot stream's parts, so this module's draws never meet
/// another loot draw on the same NPC id.
const STREAM_TAG: u64 = 0x0041_4243;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "abyss_loot",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The authored rows, and with a bundle the wreck of each entity type.
fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let rows: Vec<Parsed> = file.parse_rows(parse_row);
    let wrecks = match src.bundle {
        Some(b) => {
            src.note(Origin::Bundle);
            let (name, header) = crate::reprocess::MARKET;
            parse_wrecks(&b.data_lines(name, header)?)?
        }
        None => BTreeMap::new(),
    };
    Ok(Box::new(Table::from_rows(rows, wrecks)))
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// What kind of cache an object is, by the words of its name **[V-data]**:
/// "Triglavian Bioadaptive Cache", "Triglavian Extraction SubNode", "...
/// Node", "... Nexus".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CacheKind {
    Bioadaptive,
    Node,
    SubNode,
    Nexus,
}

impl CacheKind {
    /// The word an authored row names it by.
    fn from_word(w: &str) -> Option<CacheKind> {
        match w {
            "bioadaptive" => Some(CacheKind::Bioadaptive),
            "node" => Some(CacheKind::Node),
            "subnode" => Some(CacheKind::SubNode),
            "nexus" => Some(CacheKind::Nexus),
            _ => None,
        }
    }
}

/// The kind of a cache type by its name, or `None` for any other type
/// (including the other group-2009 caches, which belong to other content).
pub fn cache_kind(name: &str) -> Option<CacheKind> {
    let n = name.to_lowercase();
    // "subnode" before "node": the one contains the other.
    if n.contains("bioadaptive") {
        Some(CacheKind::Bioadaptive)
    } else if n.contains("subnode") || n.contains("sub-node") {
        Some(CacheKind::SubNode)
    } else if n.contains("nexus") {
        Some(CacheKind::Nexus)
    } else if n.contains("node") {
        Some(CacheKind::Node)
    } else {
        None
    }
}

/// What a row's `name_has` asks of a type's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameHas {
    /// `*`.
    Any,
    /// A word, lower-cased; the name must contain it.
    Word(String),
    /// `{tier}`: the key word of the played tier (from the bundle's keys).
    Tier,
    /// `{grade}`: the mutaplasmid grade of the tier **[A]**.
    Grade,
}

/// One authored row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootRow {
    /// `None` is every tier.
    tier: Option<i64>,
    /// `None` is every cache.
    cache: Option<CacheKind>,
    group: i64,
    name_has: NameHas,
    chance_ppm: u64,
    qty: (i64, i64),
    copy: bool,
}

/// The table, with its world switch and the wreck of each entity type.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// False when an `off` row turned the loot off.
    pub enabled: bool,
    rows: Vec<LootRow>,
    /// Entity type -> its wreck type (groups [`WRECK_GROUPS`]) **[V-data]**.
    wrecks: BTreeMap<i64, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Parsed {
    Row(LootRow),
    Off,
}

fn parse_row(r: &Row) -> Result<Parsed, String> {
    if r.field(0) == "off" {
        return Ok(Parsed::Off);
    }
    let tier = match r.field(0) {
        "*" => None,
        _ => {
            let t = r.i64(0)?;
            if !crate::abyss::TIERS.contains(&t) {
                return Err(format!("tier {t} is outside 0..=6"));
            }
            Some(t)
        }
    };
    let cache = match r.field(1) {
        "*" => None,
        w => Some(
            CacheKind::from_word(&w.to_lowercase())
                .ok_or_else(|| format!("cache {w:?} is not bioadaptive, node, subnode or nexus"))?,
        ),
    };
    let group = r.i64(2)?;
    if group < 1 {
        return Err(format!("group {group} is not a type group"));
    }
    let name_has = match r.field(3) {
        "*" => NameHas::Any,
        "{tier}" => NameHas::Tier,
        "{grade}" => NameHas::Grade,
        "" => return Err("name_has is empty".to_owned()),
        w => NameHas::Word(w.to_lowercase()),
    };
    let ppm = r.i64(4)?;
    if !(0..=PPM as i64).contains(&ppm) {
        return Err(format!("chance_ppm {ppm} is outside 0..=1000000"));
    }
    let (lo, hi) = (r.i64(5)?, r.i64(6)?);
    if lo < 1 || hi < lo {
        return Err(format!("quantity {lo}..{hi} is not 1 or more, low to high"));
    }
    let copy = match r.field(7) {
        "0" => false,
        "1" => true,
        c => return Err(format!("copy {c:?} is not 0 or 1")),
    };
    Ok(Parsed::Row(LootRow {
        tier,
        cache,
        group,
        name_has,
        chance_ppm: ppm as u64,
        qty: (lo, hi),
        copy,
    }))
}

/// `types/market.tsv` data lines: the wreck of every entity of
/// [`WRECK_GROUPS`]. A malformed line is an error naming the file and line.
fn parse_wrecks(lines: &[String]) -> Result<BTreeMap<i64, i64>, String> {
    let (file, _) = crate::reprocess::MARKET;
    let mut out = BTreeMap::new();
    for (n, line) in lines.iter().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        let int = |i: usize| -> Result<Option<i64>, String> {
            match f.get(i).map(|v| v.trim()) {
                Some("") => Ok(None),
                Some(v) => v
                    .parse::<i64>()
                    .map(Some)
                    .map_err(|_| format!("{file}: data line {}: column {i}", n + 1)),
                None => Err(format!("{file}: data line {}: column {i} missing", n + 1)),
            }
        };
        let (Some(type_id), Some(group)) = (int(0)?, int(1)?) else {
            return Err(format!("{file}: data line {}: no type or group", n + 1));
        };
        if !WRECK_GROUPS.contains(&group) {
            continue;
        }
        if let Some(wreck) = int(11)? {
            out.insert(type_id, wreck);
        }
    }
    Ok(out)
}

impl Table {
    /// The table of nothing: for tests, which bring their own rows.
    pub fn fixture() -> Table {
        Table {
            enabled: true,
            rows: Vec::new(),
            wrecks: BTreeMap::new(),
        }
    }

    fn from_rows(parsed: Vec<Parsed>, wrecks: BTreeMap<i64, i64>) -> Table {
        let mut t = Table {
            enabled: true,
            rows: Vec::new(),
            wrecks,
        };
        for p in parsed {
            match p {
                Parsed::Row(r) => t.rows.push(r),
                Parsed::Off => t.enabled = false,
            }
        }
        t
    }

    /// The table from authored text and `types/market.tsv` data lines (tests,
    /// and the loader's pure core). A bad row is an error naming its line.
    pub fn parse(authored: &str, market_lines: &[String]) -> Result<Table, String> {
        let (rows, _) = crate::authored::parse(authored, HEADER)?;
        let mut parsed = Vec::new();
        for r in &rows {
            parsed.push(parse_row(r).map_err(|e| format!("abyss-loot line {}: {e}", r.line))?);
        }
        Ok(Table::from_rows(parsed, parse_wrecks(market_lines)?))
    }

    /// The wreck type an entity type leaves, if the bundle names one.
    pub fn wreck_of(&self, type_id: i64) -> Option<i64> {
        self.wrecks.get(&type_id).copied()
    }
}

// ---------------------------------------------------------------------------
// Pure rules
// ---------------------------------------------------------------------------

/// The mutaplasmid grade word of a played tier **[A]**.
fn grade_word(tier: i64) -> &'static str {
    match tier {
        ..=1 => "decayed",
        2 | 3 => "gravid",
        _ => "unstable",
    }
}

/// The key word of each tier, from `(tier, name)` pairs of the abyssal keys:
/// the first word of the names, the commonest one when a tier's keys differ
/// (a placeholder among them does not win), the lowest word on a tie. Lower
/// case.
pub fn tier_words<'a>(keys: impl IntoIterator<Item = (i64, &'a str)>) -> BTreeMap<i64, String> {
    let mut count: BTreeMap<i64, BTreeMap<String, usize>> = BTreeMap::new();
    for (tier, name) in keys {
        if let Some(w) = name.split_whitespace().next() {
            *count
                .entry(tier)
                .or_default()
                .entry(w.to_lowercase())
                .or_default() += 1;
        }
    }
    count
        .into_iter()
        .filter_map(|(tier, words)| {
            // The map iterates ascending, so a tie keeps the lowest word.
            let best = words.values().copied().max()?;
            let word = words.into_iter().find(|(_, n)| *n == best)?.0;
            Some((tier, word))
        })
        .collect()
}

/// The candidates of one row: published types of its group whose name
/// matches, ascending. `types` is `(id, group, name, published)`.
pub fn candidates(
    row: &LootRow,
    tier: i64,
    types: &[(i64, i64, &str, bool)],
    tier_words: &BTreeMap<i64, String>,
) -> Vec<i64> {
    let want: Option<String> = match &row.name_has {
        NameHas::Any => None,
        NameHas::Word(w) => Some(w.clone()),
        NameHas::Tier => match tier_words.get(&tier) {
            Some(w) => Some(w.clone()),
            None => return Vec::new(),
        },
        NameHas::Grade => Some(grade_word(tier).to_owned()),
    };
    let mut out: Vec<i64> = types
        .iter()
        .filter(|(_, group, name, published)| {
            *published
                && *group == row.group
                && want
                    .as_ref()
                    .is_none_or(|w| name.to_lowercase().contains(w.as_str()))
        })
        .map(|(id, ..)| *id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What one hit leaves: `qty` of `type_id`, or, for a copy, one blueprint
/// copy with `runs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stack {
    pub type_id: i64,
    pub qty: i64,
    pub runs: Option<i64>,
}

/// **One cache's loot**: every row that matches the tier and the cache kind
/// is drawn once, in file order. The three draws of a row (chance, pick,
/// quantity) are always taken, so a row never shifts the draws of the rows
/// after it. A hit picks one candidate uniformly; a hit with none drops
/// nothing. A table turned off drops nothing.
pub fn roll(
    t: &Table,
    tier: i64,
    kind: CacheKind,
    cands: &dyn Fn(&LootRow) -> Vec<i64>,
    s: &mut Stream,
) -> Vec<Stack> {
    let mut out = Vec::new();
    if !t.enabled {
        return out;
    }
    for row in &t.rows {
        if row.tier.is_some_and(|x| x != tier) || row.cache.is_some_and(|c| c != kind) {
            continue;
        }
        let hit = s.range(0, PPM) < row.chance_ppm;
        let pick = s.next();
        let (lo, hi) = row.qty;
        let qty = lo + s.range(0, (hi - lo) as u64 + 1) as i64;
        if !hit {
            continue;
        }
        let pool = cands(row);
        let Some(type_id) = pool.get((pick % pool.len().max(1) as u64) as usize) else {
            continue;
        };
        out.push(if row.copy {
            Stack {
                type_id: *type_id,
                qty: 1,
                runs: Some(qty),
            }
        } else {
            Stack {
                type_id: *type_id,
                qty,
                runs: None,
            }
        });
    }
    out
}

/// Where the room's cache stands **[A]**: on its lowest-id
/// [`LOOT_LOCATOR`], else midway between the entry and the exit.
pub fn cache_spot(d: &Dungeon, spots: Spots) -> [f64; 3] {
    match d
        .objects
        .iter()
        .filter(|o| o.type_id == LOOT_LOCATOR)
        .min_by_key(|o| o.id)
    {
        Some(o) => o.pos,
        None => [
            (spots.entry[0] + spots.exit[0]) / 2.0,
            (spots.entry[1] + spots.exit[1]) / 2.0,
            (spots.entry[2] + spots.exit[2]) / 2.0,
        ],
    }
}

/// The room's own node objects **[V-data]**: every object of [`GROUP_CACHE`]
/// the dungeon lists, in dungeon order, as `(type, position)`.
pub fn nodes(d: &Dungeon, group_of: &dyn Fn(i64) -> Option<i64>) -> Vec<(i64, [f64; 3])> {
    d.objects
        .iter()
        .filter(|o| group_of(o.type_id) == Some(GROUP_CACHE))
        .map(|o| (o.type_id, o.pos))
        .collect()
}

// ---------------------------------------------------------------------------
// The world side
// ---------------------------------------------------------------------------

/// One cache or node of a room: what is to stand there, and whether it was
/// destroyed (a room made again respawns the ones that were not).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheSlot {
    pub type_id: i64,
    pub at: [f64; 3],
    pub kind: CacheKind,
    /// The NPC standing now (0: none yet).
    pub npc: i64,
    pub killed: bool,
}

/// What the kill hook needs to know of a cache NPC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheTag {
    pub run: u64,
    pub room: usize,
    pub tier: i64,
    pub kind: CacheKind,
}

/// The kind an abyssal NPC type is spawned as: its own, with the wreck the
/// bundle gives its type, derived once and kept (`loot_kinds`). A type with no
/// wreck in the map, or one the type data cannot derive (the hand fixtures
/// the wave tests use), falls back to `npc::resolve_law` unchanged.
pub(crate) fn kind_for(reg: &mut Registry, type_id: i64) -> Option<&'static NpcType> {
    if let Some(k) = reg.ext::<AbyssState>().loot_kinds.get(&type_id) {
        return Some(*k);
    }
    let wreck = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.wreck_of(type_id));
    if let Some(wreck) = wreck
        && let Some(base) = crate::npc::derive_owned(reg, type_id, WAVE_OWNER)
    {
        let mut made = *base;
        made.wreck_type_id = Some(wreck);
        let k: &'static NpcType = Box::leak(Box::new(made));
        reg.ext_mut::<AbyssState>().loot_kinds.insert(type_id, k);
        return Some(k);
    }
    crate::npc::resolve_law(reg, type_id, WAVE_OWNER)
}

/// The cache type of a room: the lowest-id group-2009 type whose name says
/// "Bioadaptive" **[V-data]**, found by name so no id is written.
fn bioadaptive_type(reg: &Registry) -> Option<i64> {
    crate::typedata::installed_in(reg.content())
        .table()
        .types()
        .filter(|t| {
            t.group_id == GROUP_CACHE && cache_kind(&t.name) == Some(CacheKind::Bioadaptive)
        })
        .map(|t| t.type_id)
        .min()
}

/// What a room is to hold, the first time it is made: its cache, then its
/// nodes in dungeon order.
fn plan(reg: &Registry, p: &Pocket) -> Vec<CacheSlot> {
    let mut out = Vec::new();
    let content = reg.content();
    let Some(d) = content
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&p.dungeon))
    else {
        return out;
    };
    let td = crate::typedata::installed_in(content).table();
    if let Some(type_id) = bioadaptive_type(reg) {
        let spots = Spots {
            entry: p.entry,
            exit: p.exit_at,
        };
        out.push(CacheSlot {
            type_id,
            at: cache_spot(d, spots),
            kind: CacheKind::Bioadaptive,
            npc: 0,
            killed: false,
        });
    }
    for (type_id, at) in nodes(d, &|t| td.get(t).map(|r| r.group_id)) {
        let Some(kind) = td.get(type_id).and_then(|r| cache_kind(&r.name)) else {
            continue;
        };
        out.push(CacheSlot {
            type_id,
            at,
            kind,
            npc: 0,
            killed: false,
        });
    }
    out
}

/// A room made again is told which of its caches have since died: a slot
/// whose NPC is gone from the table was destroyed (the hook says so too, but
/// the core has no hooks). Call before the room's NPCs are sent away.
pub(crate) fn sync(reg: &mut Registry, p: &mut Pocket) {
    for slot in &mut p.caches {
        if !slot.killed && reg.npcs().get(slot.npc).is_none() {
            slot.killed = true;
            reg.ext_mut::<AbyssState>().cache_of.remove(&slot.npc);
        }
    }
}

/// **Put the room's caches in space**, tagged to its site: the first time
/// (nothing planned yet) the cache and the nodes are planned and spawned, then
/// each call stands the ones not destroyed. A cache still standing is only
/// tagged again (a cold sweep dropped its tag); one gone is made again at its
/// spot. One `AddBalls2` to the room for the new ones.
pub(crate) fn place(reg: &mut Registry, run: &Run, p: &mut Pocket) {
    if p.caches.is_empty() {
        p.caches = plan(reg, p);
    }
    let tag = site::Tag {
        site_id: p.site_id,
        dun_object_id: 0,
        entity_group: None,
    };
    let mut made = Vec::new();
    for i in 0..p.caches.len() {
        if p.caches[i].killed {
            continue;
        }
        let slot = p.caches[i].clone();
        let standing = reg
            .npcs()
            .get(slot.npc)
            .is_some_and(|n| n.solar_system_id == p.system);
        let id = if standing {
            slot.npc
        } else {
            let Some(kind) = kind_for(reg, slot.type_id) else {
                continue;
            };
            let Some(id) = crate::npc::spawn(reg, kind, p.system, slot.at) else {
                break;
            };
            made.push(id);
            id
        };
        site::tag(reg, id, tag);
        let s = reg.ext_mut::<AbyssState>();
        s.cache_of.remove(&slot.npc);
        s.cache_of.insert(
            id,
            CacheTag {
                run: run.id,
                room: p.room,
                tier: play_tier(run.tier),
                kind: slot.kind,
            },
        );
        p.caches[i].npc = id;
    }
    crate::npc::announce(reg, p.system, &made, true);
}

/// A pocket closes: its caches are no longer anyone's.
pub(crate) fn forget(reg: &mut Registry, p: &Pocket) {
    let s = reg.ext_mut::<AbyssState>();
    for slot in &p.caches {
        s.cache_of.remove(&slot.npc);
    }
}

/// **The kill hook**: a cache of a room died; its wreck takes the loot
/// [`roll`] draws for it. The plain stacks are created in one step at the
/// wreck, owned as the wreck is, and each blueprint copy alone with its runs
/// (the `hacking_loot::on_won` path). A wreck that is a record only (the
/// store did not know its type) gets nothing, logged once per run.
pub fn on_npc_kill(reg: &mut Registry, ev: &KillEvent) {
    let Some(tag) = reg
        .ext_mut::<AbyssState>()
        .cache_of
        .remove(&ev.victim_character)
    else {
        return;
    };
    if let Some(run) = reg.ext_mut::<AbyssState>().runs.get_mut(&tag.run)
        && let Some(p) = run.pockets.get_mut(tag.room - 1)
    {
        for slot in p.caches.iter_mut().filter(|s| s.npc == ev.victim_character) {
            slot.killed = true;
        }
    }
    let Some(wreck) = reg
        .combat()
        .wrecks()
        .find(|w| w.ship_id == ev.victim_ship)
        .copied()
    else {
        return;
    };
    if reg.store().get(wreck.wreck_id).is_none() {
        if reg.ext_mut::<AbyssState>().warned.insert(tag.run) {
            crate::outln!(
                "[abyss] the wreck of a cache in run {} is a record only; no loot (is the bundle loaded?)",
                tag.run
            );
        }
        return;
    }
    let stacks = drawn(reg, &tag, ev.victim_character);
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    let plain: Vec<Edit> = stacks
        .iter()
        .filter(|x| x.runs.is_none())
        .map(|x| Edit::Create {
            type_id: x.type_id,
            quantity: x.qty,
            to,
        })
        .collect();
    if !plain.is_empty()
        && let Err(e) = reg.apply_items_quiet(&plain)
    {
        crate::outln!(
            "[abyss] the loot of cache {} was refused: {e:?}",
            ev.victim_character
        );
    }
    for x in stacks.iter().filter(|x| x.runs.is_some()) {
        let create = [Edit::Create {
            type_id: x.type_id,
            quantity: 1,
            to,
        }];
        match reg.apply_items_quiet(&create) {
            Ok(changes) => {
                for ch in changes {
                    if let Change::Created { item_id, .. } = ch {
                        reg.set_blueprint_state(crate::industry::BlueprintState {
                            item_id,
                            runs_remaining: x.runs.unwrap_or(1),
                            material_efficiency: 0,
                            time_efficiency: 0,
                        });
                    }
                }
            }
            Err(e) => crate::outln!(
                "[abyss] a copy in the wreck of cache {} was refused: {e:?}",
                ev.victim_character
            ),
        }
    }
}

/// What the table draws for `npc` (a cache of `tag`): the types the store
/// knows, from the loaded type data.
pub(crate) fn drawn(reg: &Registry, tag: &CacheTag, npc: i64) -> Vec<Stack> {
    let content = reg.content();
    let Some(table) = content.try_get::<Table>() else {
        return Vec::new();
    };
    let td = crate::typedata::installed_in(content).table();
    let keys = td
        .types()
        .filter(|t| t.group_id == crate::abyss::KEY_GROUP && t.published)
        .filter_map(|t| {
            t.attribute(crate::abyss::ATTR_TIER)
                .map(|tier| (tier as i64, t.name.as_str()))
        });
    let words = tier_words(keys);
    // Only the groups the rows name, and only types the store can make.
    let wanted: Vec<i64> = table.rows.iter().map(|r| r.group).collect();
    let types: Vec<(i64, i64, &str, bool)> = td
        .types()
        .filter(|t| wanted.contains(&t.group_id))
        .filter(|t| crate::item::find_in(content, t.type_id).is_some())
        .map(|t| (t.type_id, t.group_id, t.name.as_str(), t.published))
        .collect();
    // The run and the room are in the key: NPC ids may restart at boot, and
    // the same id would otherwise drop the same loot after every restart.
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[STREAM_TAG, npc as u64, tag.run, tag.room as u64],
    );
    roll(
        table,
        tag.tier,
        tag.kind,
        &|row| candidates(row, tag.tier, &types, &words),
        &mut s,
    )
}

#[cfg(test)]
mod tests;
