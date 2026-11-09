//! PV-9: the loot of Pochven's wrecks (`parity-pochven.md` PV-9).
//!
//! A Triglavian (or Drifter) wreck in a Pochven system holds what
//! `content/pochven/loot.tsv` draws for it, and the Triglavian filaments the
//! `p` rows of `content/pve/filament-drops.tsv` give. The table is **authored,
//! every rate `[A]`**: it names a faction, an NPC class, a type group and a
//! word of the type's name, never a type id. [`roll`] and [`filament`] are
//! pure; [`wreck_extras`] is the one world-side function, called by
//! `combat::npc_wreck` (the hot line, in the lane's separate diff).
//!
//! - **The loot is paid in Pochven only** **[A]**: the abyss spawns Triglavian
//!   Damaviks too, and their wrecks keep `npc::loot_in` and the abyss tables.
//!   Pochven is `pochven::in_region_by` (faction and wormhole class 25), so the
//!   Triglavians' one wormhole-space outlier pays nothing, as in #245.
//!   Falsified by the owner wanting abyssal Triglavians to drop survey
//!   databases (drop the Pochven test in [`wreck_extras`]).
//! - **A type pool is the published types of the group whose name contains
//!   the row's word and which the store knows** (`item::find_in`), uniformly.
//! - **Every rate, quantity and row** is `[A]` (the `why` column).
//! - **The type pools are built once per world**, in the [`SLOT`] (world_ext
//!   slot `pochven_loot_pools`, a coordinator line in `world_ext::SLOTS`): one
//!   pool per table row and the Triglavian key candidates. A kill is a lookup.
//!   Content never changes while a world runs.

use crate::authored::Row;
use crate::content::{Boxed, Sources, TableSpec};
use crate::seed::{self, Kind};
use crate::service::Registry;
use crate::world_ext::SlotSpec;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "pochven/loot.tsv";

/// Its header.
pub const HEADER: &str = "faction\tclass\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\twhy";

/// The shipped copy, so the binary always has the defaults.
const SHIPPED: &str = include_str!("../../../../../content/pochven/loot.tsv");

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "pochven_loot",
    load: load_table,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// One draw's denominator: `chance_ppm` is parts per million.
pub const PPM: u64 = 1_000_000;

/// A domain tag in the loot stream's parts, so these draws never meet another
/// loot draw on the same NPC id.
const STREAM_TAG: u64 = 0x0050_564c;

/// The tag of the filament stream, apart from the rows' own streams and from
/// `filament_drops::stream` (which `combat.rs` draws from, on the same id).
const FILAMENT_TAG: u64 = 0x0050_5646;

/// Whose wreck a row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Faction {
    Triglavian,
    Drifter,
}

impl Faction {
    /// The faction an NPC's `faction_id` is, if one of ours.
    pub fn of_id(faction_id: i64) -> Option<Faction> {
        match faction_id {
            super::FACTION_TRIGLAVIAN => Some(Faction::Triglavian),
            super::FACTION_DRIFTERS => Some(Faction::Drifter),
            _ => None,
        }
    }
}

/// One authored row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootRow {
    pub faction: Faction,
    /// Lower-case; `None` is every class.
    pub class: Option<String>,
    /// A type group id.
    pub group: i64,
    /// Lower-case; `None` is every type of the group.
    pub name_has: Option<String>,
    pub chance_ppm: u64,
    pub qty: (i64, i64),
}

/// The table, with its world switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// False when an `off` row turned the loot off.
    pub enabled: bool,
    rows: Vec<LootRow>,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            enabled: true,
            rows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Parsed {
    Row(LootRow),
    Off,
}

fn parse_row(r: &Row) -> Result<Parsed, String> {
    let faction = match r.field(0) {
        "off" => return Ok(Parsed::Off),
        "triglavian" => Faction::Triglavian,
        "drifter" => Faction::Drifter,
        f => return Err(format!("faction {f:?} is not triglavian, drifter or off")),
    };
    let class = match r.field(1) {
        "*" => None,
        "" => return Err("class is empty".to_owned()),
        c => Some(c.to_lowercase()),
    };
    let group = r.i64(2)?;
    if group < 1 {
        return Err(format!("group {group} is not a type group"));
    }
    let name_has = match r.field(3) {
        "*" => None,
        "" => return Err("name_has is empty".to_owned()),
        w => Some(w.to_lowercase()),
    };
    let ppm = r.i64(4)?;
    if !(0..=PPM as i64).contains(&ppm) {
        return Err(format!("chance_ppm {ppm} is outside 0..=1000000"));
    }
    let (lo, hi) = (r.i64(5)?, r.i64(6)?);
    if lo < 1 || hi < lo {
        return Err(format!("quantity {lo}..{hi} is not 1 or more, low to high"));
    }
    Ok(Parsed::Row(LootRow {
        faction,
        class,
        group,
        name_has,
        chance_ppm: ppm as u64,
        qty: (lo, hi),
    }))
}

impl Table {
    fn from_parsed(parsed: Vec<Parsed>) -> Table {
        let mut t = Table::default();
        for p in parsed {
            match p {
                Parsed::Row(r) => t.rows.push(r),
                Parsed::Off => t.enabled = false,
            }
        }
        t
    }

    /// The table from authored text (tests, and the loader's pure core). A
    /// bad row is an error naming its line.
    pub fn parse(text: &str) -> Result<Table, String> {
        let (rows, _) = crate::authored::parse(text, HEADER)?;
        let mut parsed = Vec::new();
        for r in &rows {
            parsed.push(parse_row(r).map_err(|e| format!("pochven loot line {}: {e}", r.line))?);
        }
        Ok(Table::from_parsed(parsed))
    }

    pub fn rows(&self) -> &[LootRow] {
        &self.rows
    }
}

fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    Ok(Box::new(Table::from_parsed(file.parse_rows(parse_row))))
}

/// The published types of `row.group` whose name has the row's word,
/// ascending. `types` is `(id, group, name, published)`.
pub fn candidates(row: &LootRow, types: &[(i64, i64, &str, bool)]) -> Vec<i64> {
    let mut out: Vec<i64> = types
        .iter()
        .filter(|(_, group, name, published)| {
            *published
                && *group == row.group
                && row
                    .name_has
                    .as_ref()
                    .is_none_or(|w| name.to_lowercase().contains(w.as_str()))
        })
        .map(|(id, ..)| *id)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// **One wreck's extra loot**, pure: every row of `faction` whose class is
/// `class` (any, or equal case-blind) is drawn once, from its own stream
/// `seed::stream(seed, Kind::Loot, [TAG, npc_id, row])`, so a row never shifts
/// another's draw and a replay drops the same. The three draws (chance, pick,
/// quantity) are always taken. A hit picks one candidate uniformly; a hit with
/// none drops nothing. A table turned off drops nothing. `pools[i]` is row
/// `i`'s candidates ([`pools`]); a row past the end has none.
pub fn roll(
    t: &Table,
    faction: Faction,
    class: &str,
    seed: u64,
    npc_id: i64,
    pools: &[Vec<i64>],
) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    if !t.enabled {
        return out;
    }
    let class = class.to_lowercase();
    for (i, row) in t.rows.iter().enumerate() {
        if row.faction != faction || row.class.as_ref().is_some_and(|c| *c != class) {
            continue;
        }
        let mut s = seed::stream(seed, Kind::Loot, &[STREAM_TAG, npc_id as u64, i as u64]);
        let hit = s.range(0, PPM) < row.chance_ppm;
        let pick = s.next();
        let (lo, hi) = row.qty;
        let qty = lo + s.range(0, (hi - lo) as u64 + 1) as i64;
        if !hit {
            continue;
        }
        let pool = pools.get(i).map_or(&[][..], Vec::as_slice);
        if let Some(type_id) = pool.get((pick % pool.len().max(1) as u64) as usize) {
            out.push((*type_id, qty));
        }
    }
    out
}

/// **The Triglavian key a Pochven wreck may hold**, pure: the `p` rows of the
/// filament table, drawn once through `filament_drops::roll_where` from the
/// stream `[FILAMENT_TAG, npc_id]`.
pub fn filament(
    table: &crate::filament_drops::Table,
    candidates: &[crate::filament_drops::Candidate],
    seed: u64,
    npc_id: i64,
    class: &str,
) -> Option<i64> {
    let mut s = seed::stream(seed, Kind::Loot, &[FILAMENT_TAG, npc_id as u64]);
    crate::filament_drops::roll_where(
        table,
        crate::filament_drops::Source::Npc,
        class,
        crate::filament_drops::Where {
            band: crate::celestial::Band::Null,
            pochven: true,
        },
        candidates,
        &mut s,
    )
}

/// One candidate list per table row, in row order: [`candidates`] of each.
pub fn pools(t: &Table, types: &[(i64, i64, &str, bool)]) -> Vec<Vec<i64>> {
    t.rows.iter().map(|r| candidates(r, types)).collect()
}

/// Whether the faction's wrecks hold Triglavian keys (the Triglavians and
/// EDENCOM, per the spec's text; Drifters do not).
fn drops_keys(faction_id: i64) -> bool {
    matches!(
        faction_id,
        super::FACTION_TRIGLAVIAN | super::FACTION_EDENCOM
    )
}

/// Is `system` Pochven for this table: the Triglavian faction **and** wormhole
/// class 25 (`pochven::in_region_by`), not the faction-only test.
pub fn in_loot_region(u: &crate::celestial::Universe, system: i64) -> bool {
    super::in_region_by(u, super::FACTION_TRIGLAVIAN, system)
}

/// What a world holds for [`extras`], built once ([`SLOT`]).
#[derive(Debug, Default)]
pub struct Pools {
    /// [`pools`] of the world's table, row by row; empty with no table.
    pub rows: Vec<Vec<i64>>,
    /// The Triglavian key candidates (in and out) the store knows.
    pub keys: Vec<crate::filament_drops::Candidate>,
}

/// The pure build of [`Pools`]: `known` is the store's test for a type.
pub fn build_pools(
    t: Option<&Table>,
    types: &[(i64, i64, &str, bool)],
    filaments: Vec<crate::filament_drops::Candidate>,
    known: &dyn Fn(i64) -> bool,
) -> Pools {
    use crate::filament_drops::Kind as K;
    let mut rows = t.map(|t| pools(t, types)).unwrap_or_default();
    for pool in &mut rows {
        pool.retain(|id| known(*id));
    }
    let keys = filaments
        .into_iter()
        .filter(|c| matches!(c.kind, K::TrigIn | K::TrigOut) && known(c.type_id))
        .collect();
    Pools { rows, keys }
}

/// The slot, for `world_ext::SLOTS` (coordinator line, after `FILAMENT_SLOT`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "pochven_loot_pools",
    make: |content| {
        // A content with no type data (a bare test world) has no pools.
        if content.try_get::<crate::typedata::Installed>().is_none() {
            return Box::new(Pools::default());
        }
        let table = content.try_get::<Table>();
        let groups: Vec<i64> = table
            .map(|t| t.rows.iter().map(|r| r.group).collect())
            .unwrap_or_default();
        let owned: Vec<(i64, i64, String, bool)> = crate::typedata::installed_in(content)
            .table()
            .types()
            .filter(|t| groups.contains(&t.group_id))
            .map(|t| (t.type_id, t.group_id, t.name.clone(), t.published))
            .collect();
        let types: Vec<(i64, i64, &str, bool)> = owned
            .iter()
            .map(|(i, g, n, p)| (*i, *g, n.as_str(), *p))
            .collect();
        Box::new(build_pools(
            table,
            &types,
            crate::filament_drops::candidates(content),
            &|id| crate::item::find_in(content, id).is_some(),
        ))
    },
};

/// The tables and pools [`extras`] reads, and the world's seed.
pub struct Inputs<'a> {
    pub loot: Option<&'a Table>,
    pub drops: Option<&'a crate::filament_drops::Table>,
    pub pools: &'a Pools,
    pub seed: u64,
}

/// **The extra stacks of one NPC's wreck**, pure: nothing outside Pochven
/// (`in_pochven`); the loot table's rows for a Triglavian or Drifter NPC (none
/// when the table is off or absent, none for any other faction); and, for a
/// Triglavian or EDENCOM NPC, one Triglavian key from the filament table's `p`
/// rows. An unlisted NPC (`faction_id` of neither) gets nothing from the loot.
pub fn extras(
    inp: &Inputs,
    faction_id: i64,
    class: &str,
    in_pochven: bool,
    npc_id: i64,
) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    if !in_pochven {
        return out;
    }
    let class = class.to_lowercase();
    if let (Some(faction), Some(table)) = (Faction::of_id(faction_id), inp.loot)
        && table.enabled
    {
        out = roll(table, faction, &class, inp.seed, npc_id, &inp.pools.rows);
    }
    if drops_keys(faction_id)
        && let Some(drops) = inp.drops.filter(|t| t.enabled)
        && let Some(key) = filament(drops, &inp.pools.keys, inp.seed, npc_id, &class)
    {
        out.push((key, 1));
    }
    out
}

/// **The extra stacks of an NPC's wreck**, `(type, quantity)`: [`extras`] on
/// the world's tables and cached [`Pools`]. Empty outside a Pochven system and
/// for an NPC type the NPC table does not know. Called by `combat::npc_wreck`.
pub fn wreck_extras(
    w: &Registry,
    npc_type_id: i64,
    npc_id: i64,
    solar_system_id: i64,
) -> Vec<(i64, i64)> {
    let content = w.content();
    let in_pochven = content
        .try_get::<crate::celestial::Universe>()
        .is_some_and(|u| in_loot_region(u, solar_system_id));
    if !in_pochven {
        return Vec::new();
    }
    let Some(row) = content
        .try_get::<crate::npc_data::Table>()
        .and_then(|t| t.types.get(&npc_type_id))
    else {
        return Vec::new();
    };
    let inp = Inputs {
        loot: content.try_get::<Table>(),
        drops: content.try_get::<crate::filament_drops::Table>(),
        pools: w.ext::<Pools>(),
        seed: w.world().seed,
    };
    extras(&inp, row.faction_id, &row.class, true, npc_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented rows, groups and types: no shipped rate, no client id.
    const ROWS: &str = "faction\tclass\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\twhy\n\
        triglavian\t*\t990001\t*\t1000000\t1\t2\tinvented\n\
        triglavian\tbattleship\t990001\t*\t1000000\t30\t60\tinvented\n\
        triglavian\t*\t990002\tAlpha\t1000000\t5\t20\tinvented\n\
        drifter\t*\t990003\t*\t1000000\t1\t3\tinvented\n\
        triglavian\t*\t990002\tBeta\t0\t1\t1\tinvented\n";

    fn types() -> Vec<(i64, i64, &'static str, bool)> {
        vec![
            (9001, 990_001, "Invented Database X", true),
            (9002, 990_001, "Invented Database Y", true),
            (9003, 990_001, "Invented Database Hidden", false),
            (9011, 990_002, "Invented Alpha Dust", true),
            (9012, 990_002, "Invented Alpha Grit", true),
            (9013, 990_002, "Invented Beta Dust", true),
            (9021, 990_003, "Invented Sleeper Thing", true),
        ]
    }

    fn run(t: &Table, f: Faction, class: &str, id: i64) -> Vec<(i64, i64)> {
        roll(t, f, class, 42, id, &pools(t, &types()))
    }

    #[test]
    fn a_triglavian_wreck_rolls_only_triglavian_rows() {
        let t = Table::parse(ROWS).unwrap();
        let got = run(&t, Faction::Triglavian, "frigate", 5);
        assert!(
            got.iter().all(|(id, _)| (9001..=9013).contains(id)),
            "{got:?}"
        );
        assert!(
            got.iter().all(|(id, _)| *id != 9021),
            "no Drifter stack: {got:?}"
        );
        // Two sure rows hit (the any-class group-990001 one and the Alpha one);
        // the chance-0 Beta row and the battleship row do not.
        assert_eq!(got.len(), 2, "{got:?}");
        let d = run(&t, Faction::Drifter, "frigate", 5);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, 9021);
    }

    #[test]
    fn a_battleship_row_needs_a_battleship() {
        let t = Table::parse(ROWS).unwrap();
        let big = |class: &str| {
            (0..10).any(|id| {
                run(&t, Faction::Triglavian, class, id)
                    .iter()
                    .any(|(ty, q)| matches!(ty, 9001 | 9002) && *q >= 30)
            })
        };
        assert!(
            big("Battleship"),
            "the 30-60 row pays a battleship (any case)"
        );
        assert!(!big("cruiser"));
        assert!(!big("frigate"));
    }

    #[test]
    fn the_pick_is_among_the_group_and_the_name_part() {
        let t = Table::parse(ROWS).unwrap();
        let mut seen_group = std::collections::BTreeSet::new();
        let mut seen_alpha = std::collections::BTreeSet::new();
        for id in 0..80 {
            for (ty, q) in run(&t, Faction::Triglavian, "frigate", id) {
                match ty {
                    9001 | 9002 => {
                        assert!((1..=2).contains(&q));
                        seen_group.insert(ty);
                    }
                    9011 | 9012 => {
                        assert!((5..=20).contains(&q));
                        seen_alpha.insert(ty);
                    }
                    other => panic!("{other} is neither in the group nor Alpha, nor published"),
                }
            }
        }
        assert_eq!(seen_group.len(), 2, "both published types are reachable");
        assert_eq!(seen_alpha.len(), 2, "both Alpha types are reachable");
    }

    #[test]
    fn a_hit_with_an_empty_pool_drops_nothing_and_a_same_seed_drops_the_same() {
        let t = Table::parse(ROWS).unwrap();
        let none = roll(&t, Faction::Triglavian, "frigate", 1, 1, &[]);
        assert!(none.is_empty());
        assert_eq!(
            run(&t, Faction::Triglavian, "frigate", 9),
            run(&t, Faction::Triglavian, "frigate", 9)
        );
    }

    #[test]
    fn a_chance_is_a_rate() {
        let t = Table::parse(
            "faction\tclass\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\twhy\n\
             triglavian\t*\t990001\t*\t250000\t1\t1\tinvented\n",
        )
        .unwrap();
        let hits = (0..4000)
            .filter(|id| !run(&t, Faction::Triglavian, "frigate", *id).is_empty())
            .count();
        assert!((800..1200).contains(&hits), "{hits} of 4000 at 25%");
    }

    #[test]
    fn off_turns_it_off() {
        let off = format!("{ROWS}off\t*\t1\t*\t0\t1\t1\tinvented\n");
        let t = Table::parse(&off).unwrap();
        assert!(!t.enabled);
        assert!(run(&t, Faction::Triglavian, "frigate", 5).is_empty());
        assert!(
            !run(
                &Table::parse(ROWS).unwrap(),
                Faction::Triglavian,
                "frigate",
                5
            )
            .is_empty()
        );
    }

    #[test]
    fn bad_rows_are_errors_with_their_line() {
        let head = "faction\tclass\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\twhy\n";
        for bad in [
            "pirate\t*\t1\t*\t5\t1\t1\twhy",
            "triglavian\t*\t0\t*\t5\t1\t1\twhy",
            "triglavian\t*\t1\t*\t2000000\t1\t1\twhy",
            "triglavian\t*\t1\t*\t5\t3\t2\twhy",
        ] {
            let e = Table::parse(&format!("{head}{bad}\n")).unwrap_err();
            assert!(e.contains("line"), "{e}");
        }
    }

    #[test]
    fn faction_ids_map_to_the_words() {
        assert_eq!(
            Faction::of_id(super::super::FACTION_TRIGLAVIAN),
            Some(Faction::Triglavian)
        );
        assert_eq!(
            Faction::of_id(super::super::FACTION_DRIFTERS),
            Some(Faction::Drifter)
        );
        assert_eq!(Faction::of_id(super::super::FACTION_EDENCOM), None);
        assert!(drops_keys(super::super::FACTION_EDENCOM));
        assert!(!drops_keys(super::super::FACTION_DRIFTERS));
    }

    #[test]
    fn the_filament_comes_from_the_p_rows_of_the_pochven_roll() {
        use crate::filament_drops::{Candidate, Kind as K, Table as Drops};
        let cand = |type_id, kind| Candidate {
            type_id,
            kind,
            tier: 0,
            name: "invented".to_owned(),
        };
        let pool = [cand(9301, K::TrigOut), cand(9201, K::TrigIn)];
        let drops = Drops::parse(
            "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
             npc\t*\ttrig_out\tp\t1000000\tany\tinvented\n\
             npc\t*\ttrig_in\tn\t1000000\tany\tinvented\n",
        )
        .unwrap();
        for id in 0..20 {
            assert_eq!(filament(&drops, &pool, 7, id, "frigate"), Some(9301));
        }
        let off = Drops::parse(
            "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
             off\t*\t-\t*\t0\t-\tinvented\n",
        )
        .unwrap();
        assert_eq!(filament(&off, &pool, 7, 1, "frigate"), None);
    }

    // -- the pure core (extras) and the cached pools -------------------------

    use crate::filament_drops::{Candidate, Kind as K, Table as Drops};

    const TRIG: i64 = super::super::FACTION_TRIGLAVIAN;
    const EDEN: i64 = super::super::FACTION_EDENCOM;
    const DRIFT: i64 = super::super::FACTION_DRIFTERS;

    fn key(type_id: i64, kind: K) -> Candidate {
        Candidate {
            type_id,
            kind,
            tier: 0,
            name: "invented".to_owned(),
        }
    }

    fn drops() -> Drops {
        Drops::parse(
            "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
             npc\t*\ttrig_out\tp\t1000000\tany\tinvented\n",
        )
        .unwrap()
    }

    fn built(t: &Table) -> Pools {
        let known = |id: i64| id != 9002;
        build_pools(
            Some(t),
            &types(),
            vec![
                key(9301, K::TrigOut),
                key(9302, K::TrigIn),
                key(9101, K::Jump),
            ],
            &known,
        )
    }

    fn go(
        loot: Option<&Table>,
        drops: Option<&Drops>,
        pools: &Pools,
        faction: i64,
        class: &str,
        pochven: bool,
    ) -> Vec<(i64, i64)> {
        let inp = Inputs {
            loot,
            drops,
            pools,
            seed: 42,
        };
        extras(&inp, faction, class, pochven, 5)
    }

    #[test]
    fn the_pools_are_built_once_and_hold_only_known_types_and_trig_keys() {
        let t = Table::parse(ROWS).unwrap();
        let p = built(&t);
        assert_eq!(p.rows.len(), t.rows().len(), "one pool per row");
        assert_eq!(p.rows[0], vec![9001], "9002 is not in the store");
        let ids: Vec<i64> = p.keys.iter().map(|c| c.type_id).collect();
        assert_eq!(ids, vec![9301, 9302], "the jump filament is not a key");
        // Many kills read the same pools; a kill draws from them without a rebuild.
        let d = drops();
        for npc in 0..30 {
            let inp = Inputs {
                loot: Some(&t),
                drops: Some(&d),
                pools: &p,
                seed: 1,
            };
            let got = extras(&inp, TRIG, "frigate", true, npc);
            assert!(
                got.iter().all(|(id, _)| *id != 9002 && *id != 9101),
                "{got:?}"
            );
            assert!(got.contains(&(9301, 1)), "the p row always hits: {got:?}");
        }
        assert_eq!(Pools::default().rows.len(), 0);
    }

    #[test]
    fn an_abyssal_system_pays_nothing() {
        let t = Table::parse(ROWS).unwrap();
        let d = drops();
        let p = built(&t);
        assert!(go(Some(&t), Some(&d), &p, TRIG, "frigate", false).is_empty());
    }

    #[test]
    fn a_pochven_triglavian_gets_loot_and_a_key() {
        let t = Table::parse(ROWS).unwrap();
        let d = drops();
        let p = built(&t);
        let got = go(Some(&t), Some(&d), &p, TRIG, "frigate", true);
        assert!(got.contains(&(9301, 1)), "{got:?}");
        assert!(got.len() >= 2, "loot rows and the key: {got:?}");
    }

    #[test]
    fn edencom_gets_a_key_and_no_loot() {
        let t = Table::parse(ROWS).unwrap();
        let d = drops();
        let p = built(&t);
        assert_eq!(
            go(Some(&t), Some(&d), &p, EDEN, "frigate", true),
            vec![(9301, 1)]
        );
    }

    #[test]
    fn a_drifter_gets_loot_and_no_key() {
        let t = Table::parse(ROWS).unwrap();
        let d = drops();
        let p = built(&t);
        let got = go(Some(&t), Some(&d), &p, DRIFT, "frigate", true);
        assert_eq!(got, vec![(9021, got[0].1)], "{got:?}");
        assert!(got.iter().all(|(id, _)| *id != 9301));
    }

    #[test]
    fn an_unlisted_npc_gets_nothing() {
        let t = Table::parse(ROWS).unwrap();
        let d = drops();
        let p = built(&t);
        assert!(go(Some(&t), Some(&d), &p, 9_999_001, "frigate", true).is_empty());
    }

    #[test]
    fn a_table_that_is_off_or_absent_pays_no_loot_and_the_drops_switch_is_separate() {
        let t = Table::parse(&format!("{ROWS}off\t*\t1\t*\t0\t1\t1\tinvented\n")).unwrap();
        let d = drops();
        let p = built(&t);
        // Loot off: only the key remains.
        assert_eq!(
            go(Some(&t), Some(&d), &p, TRIG, "frigate", true),
            vec![(9301, 1)]
        );
        // No loot table: the same.
        assert_eq!(
            go(None, Some(&d), &p, TRIG, "frigate", true),
            vec![(9301, 1)]
        );
        // Drops off or absent, loot off: nothing.
        let mut off = drops();
        off.enabled = false;
        assert!(go(Some(&t), Some(&off), &p, TRIG, "frigate", true).is_empty());
        assert!(go(Some(&t), None, &p, TRIG, "frigate", true).is_empty());
    }

    #[test]
    fn the_wormhole_space_outlier_is_not_pochven() {
        let row = |id: i64, class: i64| {
            format!(
                "{id}\t19070001\t29070001\t{TRIG}\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t{class}\t0\t\t\tInvented{id}"
            )
        };
        let u = crate::celestial::Universe::parse(
            &[
                row(39_070_001, super::super::CLASS),
                row(31_070_001, super::super::CLASS + 1),
            ],
            &[],
            &[],
        )
        .unwrap();
        assert!(in_loot_region(&u, 39_070_001));
        assert!(
            !in_loot_region(&u, 31_070_001),
            "faction-only would say yes"
        );
        assert!(!in_loot_region(&u, 1));
    }

    #[test]
    fn the_shipped_file_parses_clean_and_is_on() {
        let t = Table::parse(SHIPPED).unwrap();
        assert!(t.enabled && !t.rows().is_empty());
        assert!(t.rows().iter().any(|r| r.faction == Faction::Triglavian));
        assert!(t.rows().iter().any(|r| r.faction == Faction::Drifter));
        // A class-bound Drifter row names its type: a bare group is a wide pool.
        assert!(
            t.rows()
                .iter()
                .filter(|r| r.faction == Faction::Drifter && r.class.is_some())
                .all(|r| r.name_has.is_some()),
            "a class-bound Drifter row needs a name_has"
        );
    }
}
