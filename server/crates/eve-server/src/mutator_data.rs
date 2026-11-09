//! The client's mutaplasmid tables (#222 MUTA-DATA).
//!
//! Pure data and no behaviour: the lanes that roll and create dynamic items
//! read it. The files are `mutators/ranges.tsv` and `mutators/mappings.tsv`
//! (`docs/content/schema.md`), written by `tools/content_mutators.py` from the
//! client's `dynamicitemattributes.fsdbinary`. A bundle without the files
//! (made before format 1.7) loads as an empty [`Mutators`]: no mutaplasmid
//! then does anything.
//!
//! A range is a factor of the source type's attribute value: the rolled value
//! is `base * (modifier * (max - min) + min)` with `modifier` in `[0, 1]`
//! [V `dynamicitemattributes/__init__.py`]. A range may have `min > max`; the
//! formula still holds, so [`Range`] keeps both as the client ships them.

use crate::content::{Boxed, Bundle, Content, Sources, TableSpec};
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "mutators",
    load: load_table,
    fixture: || Box::new(Mutators::default()) as Boxed,
};

/// The bundle's rows, or none (a pre-1.7 bundle, or no bundle).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Mutators::from_bundle(b)?,
        None => Mutators::default(),
    }))
}

/// The files, and their header lines copied from `docs/content/schema.md`.
pub const RANGES: (&str, &str) = (
    "mutators/ranges.tsv",
    "#mutator_type_id\tattribute_id\tmin\tmax\thigh_is_good",
);
pub const MAPPINGS: (&str, &str) = (
    "mutators/mappings.tsv",
    "#mutator_type_id\tresult_type_id\tsource_type_id",
);

/// One attribute's roll window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub attribute_id: i64,
    pub min: f64,
    pub max: f64,
    /// `None` where the client's row has no `highIsGood`.
    pub high_is_good: Option<bool>,
}

/// One mutator: its ranges and its `(source, result)` pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mutator {
    pub type_id: i64,
    /// Ascending by attribute.
    pub ranges: Vec<Range>,
    /// `(source type, result type)`, ascending by source.
    pub mappings: Vec<(i64, i64)>,
}

impl Mutator {
    /// The window for one attribute.
    pub fn range(&self, attribute_id: i64) -> Option<&Range> {
        self.ranges.iter().find(|r| r.attribute_id == attribute_id)
    }
}

/// The tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mutators {
    by_type: BTreeMap<i64, Mutator>,
    /// Every result type of every mutator: the dynamic item types.
    results: BTreeSet<i64>,
    /// `result type -> mutators making it`, ascending, each once.
    by_result: BTreeMap<i64, Vec<i64>>,
}

impl Mutators {
    /// The files from a checked bundle; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Mutators, String> {
        Mutators::parse(
            &b.data_lines(RANGES.0, RANGES.1)?,
            &b.data_lines(MAPPINGS.0, MAPPINGS.1)?,
        )
    }

    /// From each file's data lines. A short row, a bad number, a non-finite
    /// bound, a `high_is_good` other than empty, `0` or `1`, a repeated key,
    /// a mapping whose mutator has no range row, or a source one mutator maps
    /// to two results is an error naming the line.
    pub fn parse<S: AsRef<str>>(ranges: &[S], mappings: &[S]) -> Result<Mutators, String> {
        let mut t = Mutators::default();
        for (i, line) in ranges.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    RANGES.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [m, a, min, max, high] = f[..] else {
                return Err(bad("want 5 columns"));
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad("not an integer"));
            let num = |s: &str| {
                s.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| bad("not a finite number"))
            };
            let r = Range {
                attribute_id: int(a)?,
                min: num(min)?,
                max: num(max)?,
                high_is_good: match high {
                    "" => None,
                    "1" => Some(true),
                    "0" => Some(false),
                    _ => return Err(bad("high_is_good is not 0, 1 or empty")),
                },
            };
            let id = int(m)?;
            let e = t.by_type.entry(id).or_insert_with(|| Mutator {
                type_id: id,
                ..Mutator::default()
            });
            if e.range(r.attribute_id).is_some() {
                return Err(bad("range twice"));
            }
            e.ranges.push(r);
        }
        for m in t.by_type.values_mut() {
            m.ranges.sort_by_key(|r| r.attribute_id);
        }
        for (i, line) in mappings.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    MAPPINGS.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [m, res, src] = f[..] else {
                return Err(bad("want 3 columns"));
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad("not an integer"));
            let (id, res, src) = (int(m)?, int(res)?, int(src)?);
            let e = t
                .by_type
                .get_mut(&id)
                .ok_or_else(|| bad("mapping before its mutator's ranges"))?;
            if e.mappings.iter().any(|&(s, _)| s == src) {
                return Err(bad("source twice"));
            }
            e.mappings.push((src, res));
            t.results.insert(res);
            let making = t.by_result.entry(res).or_default();
            if !making.contains(&id) {
                making.push(id);
            }
        }
        for m in t.by_type.values_mut() {
            m.mappings.sort_unstable();
        }
        for v in t.by_result.values_mut() {
            v.sort_unstable();
        }
        Ok(t)
    }

    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_type.len()
    }

    pub fn mutators(&self) -> impl Iterator<Item = &Mutator> {
        self.by_type.values()
    }

    /// One mutator's rows.
    pub fn get(&self, mutator: i64) -> Option<&Mutator> {
        self.by_type.get(&mutator)
    }

    pub fn is_mutator(&self, type_id: i64) -> bool {
        self.by_type.contains_key(&type_id)
    }

    /// Whether the type is some mutator's result (a dynamic item type).
    pub fn is_dynamic(&self, type_id: i64) -> bool {
        self.results.contains(&type_id)
    }

    /// The result type of `mutator` applied to `source`, if it applies
    /// (the client's `GetResultType`). Whether the source is published is
    /// the caller's rule (`TypeRow::published`).
    pub fn result_for(&self, mutator: i64, source: i64) -> Option<i64> {
        let m = self.by_type.get(&mutator)?;
        m.mappings
            .binary_search_by_key(&source, |&(s, _)| s)
            .ok()
            .map(|i| m.mappings[i].1)
    }

    /// The mutators that make `result`, ascending (the client's
    /// `GetMutatorTypesByResultTypes`).
    pub fn mutators_making(&self, result: i64) -> &[i64] {
        self.by_result.get(&result).map_or(&[], Vec::as_slice)
    }
}

/// `CalcualteDynamicAttribute` [V `dynamicitemattributes/__init__.py:9-14`]:
/// the rolled value at `modifier` in `[0, 1]`.
pub fn value(base: f64, modifier: f64, r: &Range) -> f64 {
    base * (modifier * (r.max - r.min) + r.min)
}

/// Whether a higher modifier is better for this attribute: `(max > min)`
/// equals `high_is_good`; `None` when the range has no `high_is_good`.
pub fn modifier_is_good_up(r: &Range) -> Option<bool> {
    r.high_is_good.map(|h| (r.max > r.min) == h)
}

/// The table in a loaded content set.
pub fn in_content(c: &Content) -> &Mutators {
    c.get::<Mutators>()
}

/// Whether `type_id` is a mutator's result type.
pub fn is_dynamic_type(c: &Content, type_id: i64) -> bool {
    in_content(c).is_dynamic(type_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids (the 981_xxx_xxx band); no client data.
    const MUT_A: i64 = 981_000_001;
    const MUT_B: i64 = 981_000_002;
    const SRC_1: i64 = 981_100_001;
    const SRC_2: i64 = 981_100_002;
    const RES_X: i64 = 981_200_001;
    const ATTR_P: i64 = 981_300_001;
    const ATTR_Q: i64 = 981_300_002;

    fn ranges() -> Vec<String> {
        vec![
            format!("{MUT_A}\t{ATTR_P}\t0.5\t2.0\t1"),
            format!("{MUT_A}\t{ATTR_Q}\t1.25\t0.75\t0"),
            format!("{MUT_B}\t{ATTR_Q}\t0.5\t1.5\t"),
        ]
    }

    fn mappings() -> Vec<String> {
        vec![
            format!("{MUT_A}\t{RES_X}\t{SRC_1}"),
            format!("{MUT_A}\t{RES_X}\t{SRC_2}"),
        ]
    }

    #[test]
    fn ranges_and_mappings_parse() {
        let t = Mutators::parse(&ranges(), &mappings()).unwrap();
        assert_eq!(t.len(), 2);
        let a = t.get(MUT_A).unwrap();
        assert_eq!(a.ranges.len(), 2);
        assert_eq!(a.ranges[0].attribute_id, ATTR_P);
        assert_eq!(a.ranges[0].high_is_good, Some(true));
        // A reversed window is kept as shipped.
        assert_eq!((a.ranges[1].min, a.ranges[1].max), (1.25, 0.75));
        assert_eq!(a.ranges[1].high_is_good, Some(false));
        assert_eq!(t.get(MUT_B).unwrap().ranges[0].high_is_good, None);
        assert_eq!(t.result_for(MUT_A, SRC_2), Some(RES_X));
        assert_eq!(t.result_for(MUT_A, 1), None);
        assert_eq!(t.result_for(MUT_B, SRC_1), None);
        assert_eq!(t.mutators().count(), 2);
    }

    #[test]
    fn a_bad_row_is_refused_with_its_line() {
        let none: Vec<String> = Vec::new();
        for (row, why) in [
            ("1\t2\t0.5\t1.5", "want 5 columns"),
            ("x\t2\t0.5\t1.5\t1", "not an integer"),
            ("1\t2\tnan\t1.5\t1", "not a finite number"),
            ("1\t2\t0.5\tinf\t1", "not a finite number"),
            ("1\t2\t0.5\t1.5\tyes", "high_is_good"),
        ] {
            let e = Mutators::parse(&[row.to_owned()], &none).unwrap_err();
            assert!(e.contains(why) && e.contains("data line 1"), "{e}");
        }
        let twice = vec![ranges()[0].clone(), ranges()[0].clone()];
        assert!(
            Mutators::parse(&twice, &none)
                .unwrap_err()
                .contains("range twice")
        );
        for (row, why) in [
            (format!("{MUT_A}\t{RES_X}"), "want 3 columns"),
            (format!("{MUT_A}\t{RES_X}\tz"), "not an integer"),
        ] {
            let e = Mutators::parse(&ranges(), &[row]).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
        let twice = vec![mappings()[0].clone(), mappings()[0].clone()];
        assert!(
            Mutators::parse(&ranges(), &twice)
                .unwrap_err()
                .contains("source twice")
        );
    }

    #[test]
    fn lookups_and_dynamic_types() {
        let t = Mutators::parse(&ranges(), &mappings()).unwrap();
        assert!(t.is_mutator(MUT_A) && t.is_mutator(MUT_B));
        assert!(!t.is_mutator(SRC_1) && !t.is_mutator(RES_X));
        assert!(t.is_dynamic(RES_X));
        assert!(!t.is_dynamic(SRC_1) && !t.is_dynamic(MUT_A));
        assert_eq!(t.mutators_making(RES_X), &[MUT_A]);
        assert!(t.mutators_making(SRC_1).is_empty());
        assert_eq!(
            t.get(MUT_A).unwrap().mappings,
            vec![(SRC_1, RES_X), (SRC_2, RES_X)]
        );
        assert_eq!(t.get(MUT_A).unwrap().range(ATTR_Q).unwrap().max, 0.75);
        assert!(t.get(MUT_A).unwrap().range(1).is_none());
    }

    #[test]
    fn value_is_base_times_the_window() {
        let r = Range {
            attribute_id: ATTR_P,
            min: 0.5,
            max: 2.0,
            high_is_good: Some(true),
        };
        assert_eq!(value(10.0, 0.0, &r), 10.0 * 0.5);
        assert_eq!(value(10.0, 1.0, &r), 10.0 * 2.0);
        assert_eq!(value(10.0, 0.5, &r), 10.0 * (0.5 + 2.0) / 2.0);
        // A reversed window: m = 0 still gives base * min.
        let rev = Range {
            min: 1.25,
            max: 0.75,
            ..r
        };
        assert_eq!(value(10.0, 0.0, &rev), 10.0 * 1.25);
        assert_eq!(value(10.0, 1.0, &rev), 10.0 * 0.75);
    }

    #[test]
    fn goodness_follows_the_window_direction() {
        let mk = |min, max, h| Range {
            attribute_id: ATTR_P,
            min,
            max,
            high_is_good: h,
        };
        assert_eq!(modifier_is_good_up(&mk(0.5, 2.0, Some(true))), Some(true));
        assert_eq!(modifier_is_good_up(&mk(0.5, 2.0, Some(false))), Some(false));
        assert_eq!(modifier_is_good_up(&mk(2.0, 0.5, Some(true))), Some(false));
        assert_eq!(modifier_is_good_up(&mk(2.0, 0.5, Some(false))), Some(true));
        assert_eq!(modifier_is_good_up(&mk(0.5, 2.0, None)), None);
    }

    #[test]
    fn a_mapping_before_its_mutators_ranges_is_refused() {
        let none: Vec<String> = Vec::new();
        let e = Mutators::parse(&none, &mappings()).unwrap_err();
        assert!(
            e.contains("mapping before its mutator's ranges") && e.contains("data line 1"),
            "{e}"
        );
    }

    #[test]
    fn a_source_with_two_results_is_refused() {
        let two = vec![
            format!("{MUT_A}\t{RES_X}\t{SRC_1}"),
            format!("{MUT_A}\t{}\t{SRC_1}", RES_X + 1),
        ];
        let e = Mutators::parse(&ranges(), &two).unwrap_err();
        assert!(
            e.contains("source twice") && e.contains("data line 2"),
            "{e}"
        );
    }

    #[test]
    fn an_absent_file_is_an_empty_table() {
        // The fixture loads every table without a bundle, as an old bundle
        // without the files does.
        let c = Content::fixture();
        assert!(in_content(&c).is_empty());
        assert!(!is_dynamic_type(&c, RES_X));
    }

    #[test]
    fn the_header_lines_match_the_schema() {
        let schema = include_str!("../../../../docs/content/schema.md");
        assert!(schema.contains(RANGES.1));
        assert!(schema.contains(MAPPINGS.1));
    }
}
