//! PV-3: Triglavian filament families (`parity-pochven.md` PV-3).
//!
//! A key of group 4087 is a family by its **name** (public game text, matched
//! case-sensitively): `content/pochven/filaments.tsv` says whether the family
//! is inbound or outbound and which set of systems it lands in. A type that
//! matches no row, or two, is not served ([`Table::index_of`]).
//!
//! The landing sets are pure functions of the map ([`landings_by`]): the
//! faction, the gates, the homes, the light-year centres and the Devana set
//! are passed in, so a test uses an invented map. [`landings`] is the
//! Triglavians' own.

use crate::celestial::{Centers, Universe};
use crate::content::{Content, Sources, TableSpec};
use crate::pochven::{self, SystemClass};
use crate::service::Registry;
use std::collections::BTreeSet;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "pochven/filaments.tsv";
/// Its header.
pub const HEADER: &str = "name_has\tdirection\tlands\twhy";
/// The shipped copy.
const SHIPPED: &str = include_str!("../../../../../content/pochven/filaments.tsv");

/// The registration row in `content_tables::TABLES` (coordinator-only).
pub const TABLE: TableSpec = TableSpec {
    name: "pochven_filaments",
    load: boxed_load,
    fixture: boxed_fixture,
};

/// `lightYearDistanceMax` **[V]** (`dogma/const.py:633`, `attributeLightYearDistanceMax`): the reach of a
/// proximity key, in light years.
pub const ATTR_LY_MAX: i64 = 3097;

/// **[A] How far from a Pochven system a Devana key may land, light years**:
/// the Devana set is the k-space within this reach of any Pochven system, since
/// this world has no minor-victory list. Falsified by the owner wanting the old
/// border list (a later content section reading the disrupted gates). Never in
/// a test: a test passes its own reach.
pub const DEVANA_REACH_LY: f64 = 10.0;

/// Which way a key goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

/// Where a family lands.
#[derive(Debug, Clone, PartialEq)]
pub enum Lands {
    /// The Krai whose constellation name contains the word.
    Krai(String),
    Class(SystemClass),
    /// K-space within the type's `lightYearDistanceMax`.
    Proximity,
    Devana,
}

/// One row.
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name_has: String,
    pub dir: Dir,
    pub lands: Lands,
}

fn parse_lands(s: &str) -> Result<Lands, String> {
    match s {
        "proximity" => Ok(Lands::Proximity),
        "devana" => Ok(Lands::Devana),
        "class:home" => Ok(Lands::Class(SystemClass::Home)),
        "class:internal" => Ok(Lands::Class(SystemClass::Internal)),
        "class:border" => Ok(Lands::Class(SystemClass::Border)),
        _ => match s.strip_prefix("krai:") {
            Some(w) if !w.is_empty() => Ok(Lands::Krai(w.to_owned())),
            _ => Err(format!("unknown lands {s:?}")),
        },
    }
}

fn parse_row(r: &crate::authored::Row) -> Result<Family, String> {
    let name_has = r.field(0).to_owned();
    if name_has.is_empty() {
        return Err("name_has is empty".to_owned());
    }
    let dir = match r.field(1) {
        "in" => Dir::In,
        "out" => Dir::Out,
        other => return Err(format!("direction {other:?} is not in or out")),
    };
    Ok(Family {
        name_has,
        dir,
        lands: parse_lands(r.field(2))?,
    })
}

/// Every family, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    rows: Vec<Family>,
}

impl Table {
    pub fn new(rows: Vec<Family>) -> Table {
        Table { rows }
    }

    pub fn rows(&self) -> &[Family] {
        &self.rows
    }

    /// The family at `index`.
    pub fn get(&self, index: usize) -> Option<&Family> {
        self.rows.get(index)
    }

    /// The index of the one family whose `name_has` the name contains; `None`
    /// for no match or for two (the caller logs which).
    pub fn index_of(&self, type_name: &str) -> Option<usize> {
        let mut hits = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, f)| type_name.contains(&f.name_has));
        let first = hits.next()?;
        hits.next().is_none().then_some(first.0)
    }

    /// The one family of a type name.
    pub fn family_of(&self, type_name: &str) -> Option<&Family> {
        self.index_of(type_name).and_then(|i| self.rows.get(i))
    }

    /// How many families match the name (0, 1, or more): for the log line.
    pub fn matches(&self, type_name: &str) -> usize {
        self.rows
            .iter()
            .filter(|f| type_name.contains(&f.name_has))
            .count()
    }

    /// Invented families for a test: names `Invented ...`, no client text.
    pub fn fixture() -> Table {
        let text = format!(
            "{HEADER}\n\
             Invented Reach\tout\tproximity\tx\n\
             Invented Far\tout\tdevana\tx\n\
             Invented Alpha\tin\tkrai:Alpha\tx\n\
             Invented Beta\tin\tkrai:Beta\tx\n\
             Invented Edge\tin\tclass:border\tx\n\
             Invented Mid\tin\tclass:internal\tx\n\
             Invented Seat\tin\tclass:home\tx\n"
        );
        let (raw, _) = crate::authored::parse(&text, HEADER).expect("the fixture header");
        Table::new(
            raw.iter()
                .map(|r| parse_row(r).expect("a fixture row"))
                .collect(),
        )
    }
}

fn boxed_load(src: &Sources) -> Result<crate::content::Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_row);
    Ok(Box::new(Table::new(rows)))
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

/// The world's table, if one is registered.
pub fn table_in(content: &Content) -> Option<&Table> {
    content.try_get::<Table>()
}

/// What a landing set is computed from. Pure data: a test builds its own.
pub struct Map<'a> {
    pub universe: &'a Universe,
    /// The faction that makes a system Pochven.
    pub faction: i64,
    /// `(system, far system)` for every gate.
    pub gates: &'a [(i64, i64)],
    pub homes: &'a [i64],
    pub centers: &'a Centers,
    /// The Devana set ([`devana_set_by`]).
    pub devana: &'a [i64],
}

/// The landing set of a family from `origin`, ascending. `ly_max` is the
/// type's reach for a proximity key; a proximity key with none lands nowhere.
pub fn landings_by(f: &Family, origin: i64, ly_max: Option<f64>, m: &Map<'_>) -> Vec<i64> {
    let u = m.universe;
    let mut out: Vec<i64> = match &f.lands {
        Lands::Krai(word) => pochven::systems_by(u, m.faction)
            .into_iter()
            .filter(|&s| {
                u.system(s)
                    .and_then(|sys| u.constellation(sys.constellation_id))
                    .is_some_and(|c| c.name.contains(word.as_str()))
            })
            .collect(),
        Lands::Class(c) => pochven::systems_of_class_with(u, m.faction, m.gates, m.homes, *c),
        Lands::Proximity => match ly_max {
            Some(ly) if ly > 0.0 => {
                let near = pochven::kspace_within_by(u, m.faction, m.centers, origin, ly);
                // `kspace_within_by` falls back to the nearest system outside
                // the reach; a key with a reach does not.
                near.into_iter()
                    .filter(|&s| m.centers.ly(origin, s).is_some_and(|d| d <= ly))
                    .collect()
            }
            _ => Vec::new(),
        },
        Lands::Devana => m.devana.to_vec(),
    };
    out.sort_unstable();
    out.dedup();
    out
}

/// [`landings_by`] for the Triglavians' Pochven.
pub fn landings(f: &Family, origin: i64, ly_max: Option<f64>, m: &Map<'_>) -> Vec<i64> {
    debug_assert_eq!(m.faction, pochven::FACTION_TRIGLAVIAN);
    landings_by(f, origin, ly_max, m)
}

/// **[A]** The Devana set: the k-space systems within `reach_ly` of any system
/// of `faction`'s region, ascending. Pure core: the reach is a parameter.
pub fn devana_set_by(u: &Universe, faction: i64, centers: &Centers, reach_ly: f64) -> Vec<i64> {
    let mut out = BTreeSet::new();
    for p in pochven::systems_by(u, faction) {
        for s in pochven::kspace_within_by(u, faction, centers, p, reach_ly) {
            if centers.ly(p, s).is_some_and(|d| d <= reach_ly) {
                out.insert(s);
            }
        }
    }
    out.into_iter().collect()
}

/// The world's Devana set, [`DEVANA_REACH_LY`] **[A]**.
pub fn devana_set(reg: &Registry) -> Vec<i64> {
    let content = reg.content();
    let (Some(u), Some(c)) = (content.try_get::<Universe>(), content.try_get::<Centers>()) else {
        return Vec::new();
    };
    devana_set_by(u, pochven::FACTION_TRIGLAVIAN, c, DEVANA_REACH_LY)
}

#[cfg(test)]
mod tests {
    //! Invented map only: faction 599_026, region 19_070_001, constellations
    //! 29_070_00x, systems 30_998_0xx (Pochven) and 30_999_0xx (k-space, where
    //! a light-year distance needs a known-space id).
    use super::*;

    const FACTION: i64 = 599_026;
    const KRAI_A: i64 = 29_070_001;
    const KRAI_B: i64 = 29_070_002;

    fn row(id: i64, krai: i64, faction: Option<i64>, sec: &str, name: &str) -> String {
        let f = faction.map(|f| f.to_string()).unwrap_or_default();
        let class = faction
            .map(|_| pochven::CLASS.to_string())
            .unwrap_or_default();
        format!("{id}\t19070001\t{krai}\t{f}\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t{class}\t0\t\t\t{name}")
    }

    fn universe() -> Universe {
        Universe::parse(
            &[
                row(30_998_001, KRAI_A, Some(FACTION), "-1.0", "Alpha"),
                row(30_998_002, KRAI_A, Some(FACTION), "-1.0", "Beta"),
                row(30_998_003, KRAI_B, Some(FACTION), "-1.0", "Gamma"),
                row(30_999_001, KRAI_B, None, "-0.4", "Near"),
                row(30_999_002, KRAI_B, None, "0.8", "Mid"),
                row(30_999_003, KRAI_B, None, "0.3", "Far"),
            ],
            &[],
            &[
                format!("{KRAI_A}\t19070001\t\t\t\tInvented Krai Aaa"),
                format!("{KRAI_B}\t19070001\t\t\t\tInvented Krai Bbb"),
            ],
        )
        .expect("invented rows parse")
    }

    fn centers() -> Centers {
        let ly = crate::celestial::LIGHTYEAR;
        let lines: Vec<String> = [
            (30_998_001, 0.0),
            (30_998_002, 0.1),
            (30_998_003, 0.2),
            (30_999_001, 1.0),
            (30_999_002, 4.0),
            (30_999_003, 9.0),
        ]
        .iter()
        .map(|(id, n)| format!("s\t{id}\t{}\t0.0\t0.0", n * ly))
        .collect();
        Centers::parse(&lines).expect("invented centres")
    }

    fn gates() -> Vec<(i64, i64)> {
        // 1-2 inside Krai A, 2-3 crossing into Krai B.
        [(30_998_001, 30_998_002), (30_998_002, 30_998_003)]
            .iter()
            .flat_map(|&(a, b)| [(a, b), (b, a)])
            .collect()
    }

    fn fam(lands: Lands) -> Family {
        Family {
            name_has: "x".to_owned(),
            dir: Dir::In,
            lands,
        }
    }

    fn landed(
        lands: Lands,
        origin: i64,
        ly: Option<f64>,
        devana: &[i64],
        homes: &[i64],
    ) -> Vec<i64> {
        let (u, c, g) = (universe(), centers(), gates());
        let map = Map {
            universe: &u,
            faction: FACTION,
            gates: &g,
            homes,
            centers: &c,
            devana,
        };
        landings_by(&fam(lands), origin, ly, &map)
    }

    #[test]
    fn the_shipped_file_parses_every_row_and_no_name_matches_another() {
        let (raw, _) = crate::authored::parse(SHIPPED, HEADER).expect("the shipped header");
        let rows: Vec<Family> = raw.iter().filter_map(|r| parse_row(r).ok()).collect();
        assert!(!rows.is_empty());
        assert_eq!(rows.len(), raw.len(), "every row parses");
        for (i, a) in rows.iter().enumerate() {
            for (j, b) in rows.iter().enumerate().filter(|(j, _)| *j != i) {
                assert!(
                    !a.name_has.contains(&b.name_has),
                    "row {i} would also match row {j}"
                );
            }
        }
    }

    #[test]
    fn a_bad_row_is_refused_with_what_is_wrong() {
        let parsed = |text: &str| {
            let (raw, _) = crate::authored::parse(&format!("{HEADER}\n{text}\n"), HEADER).unwrap();
            parse_row(&raw[0])
        };
        assert!(parsed("Invented\tsideways\tdevana\tx").is_err());
        assert!(parsed("Invented\tin\tnowhere\tx").is_err());
        assert!(parsed("Invented\tin\tkrai:\tx").is_err());
        assert!(parsed("Invented\tin\tclass:mid\tx").is_err());
        assert_eq!(
            parsed("Invented\tout\tproximity\tx").map(|f| f.lands),
            Ok(Lands::Proximity)
        );
    }

    #[test]
    fn a_name_picks_exactly_one_family() {
        let t = Table::fixture();
        assert_eq!(
            t.family_of("Invented Alpha Key").map(|f| f.dir),
            Some(Dir::In)
        );
        assert_eq!(
            t.family_of("Invented Far Key").map(|f| f.dir),
            Some(Dir::Out)
        );
        assert!(t.family_of("Something Else").is_none());
        assert!(
            t.family_of("Invented Alpha Invented Far Key").is_none(),
            "two"
        );
        assert_eq!(t.matches("Invented Alpha Invented Far Key"), 2);
        assert!(t.family_of("invented alpha key").is_none(), "case matters");
    }

    #[test]
    fn a_krai_family_lands_in_the_krai_whose_name_has_the_word() {
        assert_eq!(
            landed(Lands::Krai("Aaa".into()), 30_999_001, None, &[], &[]),
            vec![30_998_001, 30_998_002]
        );
        assert_eq!(
            landed(Lands::Krai("Bbb".into()), 30_999_001, None, &[], &[]),
            vec![30_998_003]
        );
        assert!(landed(Lands::Krai("Nowhere".into()), 30_999_001, None, &[], &[]).is_empty());
    }

    #[test]
    fn a_class_family_lands_in_that_class_with_the_homes_passed_in() {
        let homes = [30_998_001];
        let class = |c| landed(Lands::Class(c), 30_999_001, None, &[], &homes);
        assert_eq!(class(SystemClass::Home), vec![30_998_001]);
        assert_eq!(class(SystemClass::Border), vec![30_998_002, 30_998_003]);
        assert!(class(SystemClass::Internal).is_empty());
        // No homes: system 1 is Internal.
        assert_eq!(
            landed(
                Lands::Class(SystemClass::Internal),
                30_999_001,
                None,
                &[],
                &[]
            ),
            vec![30_998_001]
        );
    }

    #[test]
    fn a_proximity_family_stays_within_the_reach_and_a_missing_reach_lands_nowhere() {
        let near = |ly| landed(Lands::Proximity, 30_998_001, ly, &[], &[]);
        assert_eq!(near(Some(2.0)), vec![30_999_001]);
        assert_eq!(near(Some(5.0)), vec![30_999_001, 30_999_002]);
        assert_eq!(near(Some(20.0)), vec![30_999_001, 30_999_002, 30_999_003]);
        // Nothing within 0.5 ly: not even the nearest outside it.
        assert!(near(Some(0.5)).is_empty());
        assert!(near(None).is_empty());
    }

    #[test]
    fn the_devana_set_is_the_kspace_within_the_reach_of_any_pochven_system() {
        let (u, c) = (universe(), centers());
        assert_eq!(
            devana_set_by(&u, FACTION, &c, 5.0),
            vec![30_999_001, 30_999_002]
        );
        assert_eq!(devana_set_by(&u, FACTION, &c, 1.5), vec![30_999_001]);
        assert_eq!(
            landed(Lands::Devana, 30_998_001, None, &[30_999_001], &[]),
            vec![30_999_001],
            "the family lands in the set it is given"
        );
    }
}
