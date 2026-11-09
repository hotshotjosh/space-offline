//! **Filament drops** (FIL-DROP, issue #61): how a filament gets into a
//! closed world from an NPC kill or a hacked site, so the Abyss and the
//! random-jump filaments need no GM.
//!
//! The authored table `content/pve/filament-drops.tsv` gives, per source (an
//! NPC class or a site archetype), kind (abyssal key or random-jump filament)
//! and security band, a chance in parts per million. No type id is authored:
//! [`candidates`] reads the filament types from the loaded type data (abyssal
//! keys by their group, random-jump filaments by `random_jump`'s test), and
//! [`roll`] picks one.
//!
//! - **tier** of an abyssal key follows the system's band
//!   ([`tier_for_band`]) **[A]**; falsified by players finding the tiers they
//!   get useless for the space they rat in. A band with no key of that tier
//!   falls back to the next lower tier that has one.
//! - **weather** is `any`, or a word the key type's name must contain
//!   (case-blind) **[A]**; falsified by key types whose names do not carry
//!   their weather.
//! - **rates** are all **[A]** until the owner decides the filament sources.
//! - **Triglavian keys** (PV-9, group 4087): kinds `trig_in` and `trig_out`
//!   by the key's family direction (`pochven::filaments`); a key whose name
//!   matches no family is not a candidate.
//! - **band** letters are `h l n w`, and `p` (Pochven). Pochven is `Band::Null`
//!   to the celestial map, so [`roll`] (the caller of `combat.rs`) treats a
//!   Pochven kill as null space and never sees a `p` row; [`roll_where`] with
//!   `pochven: true` draws **only** the `p` rows, and with `false` never one.
//!   `p` stands alone in a row (`np` is refused). So an NPC wreck in Pochven
//!   rolls its band rows once, through `roll` (the `n` rows, and the `*` rows,
//!   which match every band), and its `p` rows once, through
//!   `pochven::loot::wreck_extras`.
//! - **off**: a row whose `source` is `off` (the world switch), or
//!   [`Table::enabled`] set false by a world setting, drops nothing.
//!
//! This module is pure: [`roll`] takes a [`Stream`] and the candidates and
//! returns a type id. The two loot paths call it (the maintainer's lines are
//! in the lane report): the NPC wreck's loot (`combat::npc_wreck`) with
//! `Source::Npc` and the NPC's class, and a hacked can's loot
//! (`hacking_loot::on_won`) with `Source::Site` and the site archetype.

use crate::authored::Row;
use crate::celestial::Band;
use crate::content::{Boxed, Content, Sources, TableSpec};
use crate::seed::{self, Stream};

/// The authored file, relative to `--authored`.
pub const FILE: &str = "pve/filament-drops.tsv";

/// Its header, copied into `docs/content/schema.md`.
pub const HEADER: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/pve/filament-drops.tsv");

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "filament_drops",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    Ok(Box::new(Table::from_rows(&file.parse_rows(parse_row))))
}

/// One draw's denominator: `chance_ppm` is parts per million.
pub const PPM: u64 = 1_000_000;

/// A domain tag in the loot stream's parts, so a filament draw never meets
/// another loot draw on the same id.
const STREAM_TAG: u64 = 0x0046_494c;

/// `[A]` the abyssal tier a kill in each band drops: high 1, low 2, null and
/// wormhole 3. Falsified by players finding the tiers they get useless for
/// the space they rat in (for example null-sec ratters who only ever run
/// tier 3, or high-sec ones who cannot use a tier 1 key).
pub fn tier_for_band(band: Band) -> i64 {
    match band {
        Band::High => 1,
        Band::Low => 2,
        Band::Null | Band::Wormhole | Band::Void => 3,
    }
}

/// Where a drop is rolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A killed NPC; the name is its class.
    Npc,
    /// A hacked can; the name is its site's archetype.
    Site,
}

/// What a filament is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An abyssal key.
    Abyss,
    /// A random-jump filament.
    Jump,
    /// A Triglavian key into Pochven (PV-9).
    TrigIn,
    /// A Triglavian key out of Pochven (PV-9).
    TrigOut,
}

/// Where a roll happens: the celestial band and whether the system is
/// Pochven (which the band calls null).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Where {
    pub band: Band,
    pub pochven: bool,
}

/// One authored row.
#[derive(Debug, Clone, PartialEq)]
pub struct DropRow {
    pub source: Source,
    /// Lower-case; `*` is any.
    pub name: String,
    pub kind: Kind,
    /// `None` is every band.
    pub bands: Option<Vec<Band>>,
    /// The band letters held `p`: the row rolls in Pochven, through
    /// [`roll_where`] with `pochven: true`, and only there.
    pub pochven: bool,
    pub chance_ppm: u64,
    /// Lower-case; `any` is `None`.
    pub weather: Option<String>,
}

/// A filament type a drop may name, read from the type data.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub type_id: i64,
    pub kind: Kind,
    /// An abyssal key's tier; 0 for a jump filament.
    pub tier: i64,
    /// The type's name, lower-case, for the weather match.
    pub name: String,
}

/// The table, with its world switch.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    rows: Vec<DropRow>,
    /// False when a world setting or an `off` row turned the drops off.
    pub enabled: bool,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            rows: Vec::new(),
            enabled: true,
        }
    }
}

impl Table {
    /// Invented rows, for tests: never a shipped rate.
    pub fn fixture() -> Table {
        Table::default()
    }

    /// The table of already-parsed rows; `None` rows (the `on` and `off`
    /// switch rows) are folded in: any `off` disables.
    fn from_rows(rows: &[Parsed]) -> Table {
        let mut t = Table::default();
        for r in rows {
            match r {
                Parsed::Drop(d) => t.rows.push(d.clone()),
                Parsed::Off => t.enabled = false,
                Parsed::On => {}
            }
        }
        t
    }

    /// The table from authored text (tests, and the loader's pure core).
    pub fn parse(text: &str) -> Result<Table, String> {
        let (rows, _) = crate::authored::parse(text, HEADER)?;
        let parsed: Vec<Parsed> = rows.iter().filter_map(|r| parse_row(r).ok()).collect();
        Ok(Table::from_rows(&parsed))
    }

    pub fn rows(&self) -> &[DropRow] {
        &self.rows
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Parsed {
    Drop(DropRow),
    On,
    Off,
}

/// The bands of a row, and whether it holds `p` (Pochven).
fn parse_bands(s: &str) -> Result<(Option<Vec<Band>>, bool), String> {
    if s == "*" {
        return Ok((None, false));
    }
    let mut bands = Vec::new();
    let mut pochven = false;
    // `p` is its own roll (Pochven's `roll_where`); beside a band letter the
    // row would roll twice on a Pochven kill, so it stands alone.
    if s.contains('p') && s != "p" {
        return Err(format!(
            "band {s:?} mixes p with other letters; p stands alone"
        ));
    }
    for c in s.chars() {
        match c {
            'h' => bands.push(Band::High),
            'l' => bands.push(Band::Low),
            'n' => bands.push(Band::Null),
            'w' => bands.push(Band::Wormhole),
            'p' => pochven = true,
            _ => return Err(format!("band {s:?} has {c:?}, not h, l, n, w or p")),
        }
    }
    Ok((Some(bands), pochven))
}

fn parse_row(r: &Row) -> Result<Parsed, String> {
    let source = match r.field(0) {
        "on" => return Ok(Parsed::On),
        "off" => return Ok(Parsed::Off),
        "npc" => Source::Npc,
        "site" => Source::Site,
        s => return Err(format!("source {s:?} is not npc, site, on or off")),
    };
    let kind = match r.field(2) {
        "abyss" => Kind::Abyss,
        "jump" => Kind::Jump,
        "trig_in" => Kind::TrigIn,
        "trig_out" => Kind::TrigOut,
        k => {
            return Err(format!(
                "kind {k:?} is not abyss, jump, trig_in or trig_out"
            ));
        }
    };
    let ppm = r.i64(4)?;
    if !(0..=PPM as i64).contains(&ppm) {
        return Err(format!("chance_ppm {ppm} is outside 0..=1000000"));
    }
    let (bands, pochven) = parse_bands(r.field(3))?;
    Ok(Parsed::Drop(DropRow {
        source,
        name: r.field(1).to_lowercase(),
        kind,
        bands,
        pochven,
        chance_ppm: ppm as u64,
        weather: match r.field(5).to_lowercase().as_str() {
            "any" | "-" | "" => None,
            w => Some(w.to_owned()),
        },
    }))
}

/// The kind of a Triglavian key (group 4087) by its family's direction, or
/// `None` for a name no family claims (or two do).
fn trig_kind(families: &crate::pochven::filaments::Table, name: &str) -> Option<Kind> {
    use crate::pochven::filaments::Dir;
    Some(match families.family_of(name)?.dir {
        Dir::In => Kind::TrigIn,
        Dir::Out => Kind::TrigOut,
    })
}

/// Every filament type in the loaded type data: abyssal keys (their group,
/// with a tier), random-jump filaments and Triglavian keys, by type id.
pub fn candidates(content: &Content) -> Vec<Candidate> {
    let table = crate::typedata::installed_in(content).table();
    let families = crate::pochven::filaments::table_in(content);
    let mut out: Vec<Candidate> = Vec::new();
    for t in table.types().filter(|t| t.published) {
        if t.group_id == crate::random_jump::GROUP_TRIGLAVIAN_JUMP_KEYS {
            if let Some(kind) = families.and_then(|f| trig_kind(f, &t.name)) {
                out.push(Candidate {
                    type_id: t.type_id,
                    kind,
                    tier: 0,
                    name: t.name.to_lowercase(),
                });
            }
        } else if t.group_id == crate::abyss::KEY_GROUP {
            if let Some(tier) = t.attribute(crate::abyss::ATTR_TIER) {
                out.push(Candidate {
                    type_id: t.type_id,
                    kind: Kind::Abyss,
                    tier: tier as i64,
                    name: t.name.to_lowercase(),
                });
            }
        } else if crate::random_jump::filament_type(content, t.type_id).is_some() {
            out.push(Candidate {
                type_id: t.type_id,
                kind: Kind::Jump,
                tier: 0,
                name: t.name.to_lowercase(),
            });
        }
    }
    out.sort_by_key(|c| c.type_id);
    out
}

/// The stream for the drop of one kill or can: `id` is the NPC's or the
/// can's, so a replay draws the same.
pub fn stream(seed: u64, source: Source, id: i64) -> Stream {
    seed::stream(
        seed,
        seed::Kind::Loot,
        &[STREAM_TAG, source as u64, id as u64],
    )
}

/// The filament type, if any, a `source` named `name` in `band` drops: the
/// callers of `combat.rs` and `hacking_loot.rs`, which see a Pochven kill as
/// null space. [`roll_where`] with `pochven: false`. One
/// draw per matching row, in file order; the first hit picks a candidate of
/// its kind (an abyssal key of [`tier_for_band`], or the nearest lower tier
/// that has one; and of its weather), uniformly. A hit with no candidate
/// drops nothing. Draws are always taken, so a table's rows never shift each
/// other's results.
pub fn roll(
    table: &Table,
    source: Source,
    name: &str,
    band: Band,
    candidates: &[Candidate],
    s: &mut Stream,
) -> Option<i64> {
    roll_where(
        table,
        source,
        name,
        Where {
            band,
            pochven: false,
        },
        candidates,
        s,
    )
}

/// [`roll`] told whether the system is Pochven. With `pochven: true` only the
/// `p` rows are drawn; with `false` only the band rows, and never a `p` one.
pub fn roll_where(
    table: &Table,
    source: Source,
    name: &str,
    at: Where,
    candidates: &[Candidate],
    s: &mut Stream,
) -> Option<i64> {
    let band = at.band;
    if !table.enabled {
        return None;
    }
    let name = name.to_lowercase();
    let mut won: Option<i64> = None;
    for row in &table.rows {
        if row.source != source || (row.name != "*" && row.name != name) || !row.rolls_at(at) {
            continue;
        }
        let hit = s.range(0, PPM) < row.chance_ppm;
        let pick = s.next();
        if hit && won.is_none() {
            won = choose(row, band, candidates, pick);
        }
    }
    won
}

impl DropRow {
    /// Does the row roll at `at`? A Pochven roll takes the `p` rows alone.
    fn rolls_at(&self, at: Where) -> bool {
        if at.pochven {
            self.pochven
        } else {
            self.bands.as_ref().is_none_or(|b| b.contains(&at.band))
        }
    }
}

fn choose(row: &DropRow, band: Band, candidates: &[Candidate], pick: u64) -> Option<i64> {
    let fits = |c: &&Candidate| {
        c.kind == row.kind
            && row
                .weather
                .as_ref()
                .is_none_or(|w| row.kind != Kind::Abyss || c.name.contains(w.as_str()))
    };
    let mut pool: Vec<&Candidate> = candidates.iter().filter(fits).collect();
    if row.kind == Kind::Abyss {
        let want = tier_for_band(band);
        let best = pool.iter().map(|c| c.tier).filter(|t| *t <= want).max()?;
        pool.retain(|c| c.tier == best);
    }
    if pool.is_empty() {
        return None;
    }
    Some(pool[(pick % pool.len() as u64) as usize].type_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented rows and types: no shipped rate, no client id.
    const ROWS: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
        npc\tgizmo\tabyss\thl\t1000000\tany\tinvented\n\
        npc\tgizmo\tjump\t*\t0\tany\tinvented\n\
        npc\t*\tjump\tw\t1000000\tany\tinvented\n\
        npc\twidget\tabyss\t*\t1000000\tzephyr\tinvented\n\
        site\tgadget\tjump\tw\t1000000\tany\tinvented\n\
        site\tgadget\tbogus\tw\t5\tany\tinvented\n";

    fn cand(type_id: i64, kind: Kind, tier: i64, name: &str) -> Candidate {
        Candidate {
            type_id,
            kind,
            tier,
            name: name.to_owned(),
        }
    }

    fn pool() -> Vec<Candidate> {
        vec![
            cand(9001, Kind::Abyss, 1, "calm zephyr key"),
            cand(9002, Kind::Abyss, 1, "calm quill key"),
            cand(9003, Kind::Abyss, 3, "wild zephyr key"),
            cand(9101, Kind::Jump, 0, "hop"),
        ]
    }

    fn run(t: &Table, src: Source, name: &str, band: Band, seed: u64) -> Option<i64> {
        roll(t, src, name, band, &pool(), &mut stream(seed, src, 7))
    }

    #[test]
    fn tier_follows_the_band_and_falls_back_lower() {
        let t = Table::parse(ROWS).unwrap();
        // High space wants tier 1: both tier-1 keys are possible, tier 3 never.
        let seen: Vec<_> = (0..40)
            .filter_map(|n| run(&t, Source::Npc, "gizmo", Band::High, n))
            .collect();
        assert!(seen.contains(&9001) && seen.contains(&9002));
        assert!(!seen.contains(&9003));
        // Low wants tier 2: none exists, so tier 1 again.
        assert!(matches!(
            run(&t, Source::Npc, "gizmo", Band::Low, 1),
            Some(9001 | 9002)
        ));
        // A tier-3 band finds tier 3 for the weather-bound widget row.
        assert_eq!(run(&t, Source::Npc, "widget", Band::Null, 1), Some(9003));
    }

    #[test]
    fn band_name_and_source_filter_rows() {
        let t = Table::parse(ROWS).unwrap();
        // gizmo has no row for null space; the any-name jump row hits in a wormhole.
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::Null, 3), None);
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::Wormhole, 3), Some(9101));
        assert!(run(&t, Source::Npc, "GIZMO", Band::High, 3).is_some());
        assert_eq!(
            run(&t, Source::Site, "gadget", Band::Wormhole, 3),
            Some(9101)
        );
        assert_eq!(run(&t, Source::Site, "gadget", Band::High, 3), None);
        assert_eq!(run(&t, Source::Site, "gizmo", Band::High, 3), None);
    }

    #[test]
    fn weather_matches_the_type_name() {
        let t = Table::parse(ROWS).unwrap();
        for n in 0..20 {
            let got = run(&t, Source::Npc, "widget", Band::High, n);
            assert_eq!(got, Some(9001), "only the zephyr key of tier 1");
        }
    }

    #[test]
    fn a_zero_chance_row_and_an_empty_pool_drop_nothing() {
        let t = Table::parse(ROWS).unwrap();
        let mut s = stream(1, Source::Npc, 7);
        assert_eq!(
            roll(&t, Source::Npc, "gizmo", Band::Null, &pool(), &mut s),
            None
        );
        let mut s = stream(1, Source::Npc, 7);
        assert_eq!(
            roll(&t, Source::Npc, "gizmo", Band::High, &[], &mut s),
            None
        );
    }

    #[test]
    fn fixed_seed_is_deterministic_and_chance_is_a_rate() {
        let t = Table::parse(
            "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
             npc\tgizmo\tjump\t*\t250000\tany\tinvented\n",
        )
        .unwrap();
        assert_eq!(
            run(&t, Source::Npc, "gizmo", Band::High, 5),
            run(&t, Source::Npc, "gizmo", Band::High, 5)
        );
        let hits = (0..4000)
            .filter(|n| run(&t, Source::Npc, "gizmo", Band::High, *n).is_some())
            .count();
        assert!((800..1200).contains(&hits), "{hits} of 4000 at 25%");
    }

    #[test]
    fn off_row_and_setting_turn_drops_off() {
        let off = format!("{ROWS}off\t*\t-\t*\t0\t-\tinvented\n");
        let t = Table::parse(&off).unwrap();
        assert!(!t.enabled);
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::High, 1), None);
        let mut t = Table::parse(ROWS).unwrap();
        assert!(run(&t, Source::Npc, "gizmo", Band::High, 1).is_some());
        t.enabled = false;
        assert_eq!(run(&t, Source::Npc, "gizmo", Band::High, 1), None);
    }

    #[test]
    fn bad_rows_are_skipped_not_fatal() {
        let t = Table::parse(ROWS).unwrap();
        assert_eq!(t.rows().len(), 5, "the bogus-kind row is skipped");
        let bad = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
                   npc\ta\tjump\tx\t5\tany\twhy\n\
                   npc\ta\tjump\t*\t2000000\tany\twhy\n";
        assert!(Table::parse(bad).unwrap().rows().is_empty());
    }

    /// Invented Pochven rows: kinds `trig_in` and `trig_out`, bands with `p`.
    const TRIG_ROWS: &str = "source\tname\tkind\tband\tchance_ppm\tweather\twhy\n\
        npc\tgizmo\ttrig_in\thln\t1000000\tany\tinvented\n\
        npc\t*\ttrig_out\tp\t1000000\tany\tinvented\n\
        npc\tgizmo\tjump\tn\t1000000\tany\tinvented\n";

    fn trig_pool() -> Vec<Candidate> {
        vec![
            cand(9201, Kind::TrigIn, 0, "inbound a"),
            cand(9202, Kind::TrigIn, 0, "inbound b"),
            cand(9301, Kind::TrigOut, 0, "outbound a"),
            cand(9101, Kind::Jump, 0, "hop"),
        ]
    }

    fn at(band: Band, pochven: bool) -> Where {
        Where { band, pochven }
    }

    fn run_where(t: &Table, name: &str, w: Where, seed: u64) -> Option<i64> {
        let src = Source::Npc;
        roll_where(t, src, name, w, &trig_pool(), &mut stream(seed, src, 7))
    }

    #[test]
    fn a_p_row_drops_only_in_pochven() {
        let t = Table::parse(TRIG_ROWS).unwrap();
        for seed in 0..20 {
            assert_eq!(
                run_where(&t, "gizmo", at(Band::Null, true), seed),
                Some(9301),
                "the p row is the only one drawn in Pochven"
            );
            assert!(matches!(
                run_where(&t, "gizmo", at(Band::High, false), seed),
                Some(9201 | 9202)
            ));
        }
        // Outside Pochven the trig_out row (p only) never draws.
        let outs = (0..40).filter_map(|n| run_where(&t, "gizmo", at(Band::Null, false), n));
        assert!(
            outs.clone().all(|id| id != 9301),
            "{:?}",
            outs.collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_n_row_never_drops_in_pochven_through_roll_where() {
        let t = Table::parse(TRIG_ROWS).unwrap();
        for seed in 0..20 {
            // The null jump row and the hln trig_in row both cover null space,
            // yet neither draws in Pochven: only the p row does.
            assert_eq!(
                run_where(&t, "gizmo", at(Band::Null, true), seed),
                Some(9301)
            );
        }
        // Through roll (Pochven counted as null), the n rows roll as before.
        let got = roll(
            &t,
            Source::Npc,
            "gizmo",
            Band::Null,
            &trig_pool(),
            &mut stream(1, Source::Npc, 7),
        );
        assert!(matches!(got, Some(9201 | 9202)), "{got:?}");
    }

    #[test]
    fn a_trig_in_draw_picks_an_inbound_type() {
        let t = Table::parse(TRIG_ROWS).unwrap();
        let seen: Vec<i64> = (0..60)
            .filter_map(|n| run_where(&t, "gizmo", at(Band::High, false), n))
            .collect();
        assert!(seen.contains(&9201) && seen.contains(&9202), "{seen:?}");
        assert!(seen.iter().all(|id| matches!(id, 9201 | 9202)), "{seen:?}");
    }

    #[test]
    fn roll_is_unchanged_for_the_existing_kinds() {
        // Pinned outputs of the invented ROWS and pool over seeds 0..24, taken
        // from `roll` as it stood at be9efd6e (before `roll_where`). An invented
        // table, not an [A]: `a`/`b`/`c` are the abyss keys 9001..9003, `j` the
        // jump filament 9101, `.` nothing.
        let t = Table::parse(ROWS).unwrap();
        let line = |src: Source, name: &str, band: Band| -> String {
            (0..24)
                .map(|seed| match run(&t, src, name, band, seed) {
                    None => '.',
                    Some(9001) => 'a',
                    Some(9002) => 'b',
                    Some(9003) => 'c',
                    Some(9101) => 'j',
                    Some(_) => '?',
                })
                .collect()
        };
        let n = Source::Npc;
        assert_eq!(line(n, "gizmo", Band::High), "bababbaaaabbaaabaaaaabab");
        assert_eq!(line(n, "gizmo", Band::Low), "bababbaaaabbaaabaaaaabab");
        assert_eq!(line(n, "gizmo", Band::Null), "........................");
        assert_eq!(line(n, "gizmo", Band::Wormhole), "jjjjjjjjjjjjjjjjjjjjjjjj");
        assert_eq!(line(n, "widget", Band::Null), "cccccccccccccccccccccccc");
        assert_eq!(line(n, "widget", Band::High), "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(
            line(Source::Site, "gadget", Band::Wormhole),
            "jjjjjjjjjjjjjjjjjjjjjjjj"
        );
        // A p row is invisible to roll.
        let t = Table::parse(TRIG_ROWS).unwrap();
        assert_eq!(run(&t, Source::Npc, "other", Band::Wormhole, 1), None);
    }

    #[test]
    fn a_trig_key_is_in_or_out_by_its_family_and_unclaimed_names_are_skipped() {
        let fam = crate::pochven::filaments::Table::fixture();
        assert_eq!(trig_kind(&fam, "Invented Alpha Key"), Some(Kind::TrigIn));
        assert_eq!(trig_kind(&fam, "Invented Reach Key"), Some(Kind::TrigOut));
        assert_eq!(trig_kind(&fam, "Unrelated Key"), None);
    }

    #[test]
    fn the_new_band_and_kind_words_parse_and_bad_ones_do_not() {
        assert_eq!(parse_bands("p").unwrap(), (Some(vec![]), true));
        assert_eq!(parse_bands("*").unwrap(), (None, false));
        assert!(parse_bands("q").is_err());
        for mixed in ["np", "pn", "hlp", "pp"] {
            assert!(parse_bands(mixed).is_err(), "{mixed}: p stands alone");
        }
        let t = Table::parse(TRIG_ROWS).unwrap();
        assert_eq!(t.rows().len(), 3);
        assert_eq!(t.rows()[1].kind, Kind::TrigOut);
        assert!(t.rows()[1].pochven && !t.rows()[0].pochven);
    }

    #[test]
    fn the_shipped_file_parses_clean_and_stays_on() {
        let (rows, skipped) = crate::authored::parse(SHIPPED, HEADER).unwrap();
        assert!(skipped.is_empty());
        let parsed: Vec<_> = rows.iter().map(parse_row).collect();
        assert!(parsed.iter().all(|p| p.is_ok()), "{parsed:?}");
        let t = Table::parse(SHIPPED).unwrap();
        assert!(t.enabled && !t.rows().is_empty());
    }
}
