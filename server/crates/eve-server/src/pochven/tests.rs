//! Invented map only: region 19_070_001, constellations 29_070_00x, systems
//! 39_070_0xx (and 30_999_0xx where a light-year distance needs a known-space
//! id), faction 599_026 standing in for the Triglavians. No client value, and
//! the `[A]` home names are never read: each test passes its own homes.

use super::*;

const FACTION: i64 = 599_026;
const OTHER: i64 = 599_027;
const KRAI_A: i64 = 29_070_001;
const KRAI_B: i64 = 29_070_002;

/// A faction row carries the region's wormhole class ([`CLASS`]); no faction, none.
fn row(id: i64, krai: i64, faction: i64, sec: &str, name: &str) -> String {
    let class = (faction != 0).then_some(CLASS);
    row_class(id, krai, faction, sec, name, class)
}

fn row_class(
    id: i64,
    krai: i64,
    faction: i64,
    sec: &str,
    name: &str,
    class: Option<i64>,
) -> String {
    let faction = if faction == 0 {
        String::new()
    } else {
        faction.to_string()
    };
    let class = class.map(|c| c.to_string()).unwrap_or_default();
    format!(
        "{id}\t19070001\t{krai}\t{faction}\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t{class}\t0\t\t\t{name}"
    )
}

fn universe_of(rows: &[String]) -> Universe {
    Universe::parse(rows, &[], &[]).expect("invented rows parse")
}

/// A and B hold two systems each; 39_070_005 is another faction's.
fn universe() -> Universe {
    universe_of(&[
        row(39_070_001, KRAI_A, FACTION, "-1.0", "Alpha"),
        row(39_070_002, KRAI_A, FACTION, "-1.0", "Beta"),
        row(39_070_003, KRAI_B, FACTION, "-1.0", "Gamma"),
        row(39_070_004, KRAI_B, FACTION, "-1.0", "Delta"),
        row(39_070_005, KRAI_B, OTHER, "-0.5", "Epsilon"),
    ])
}

/// Both directions of 1-2, 2-3 (the Krai crossing) and 3-4, and a gate from
/// 4 to the other faction's system 5.
fn gates() -> Vec<(i64, i64)> {
    let pairs = [
        (39_070_001, 39_070_002),
        (39_070_002, 39_070_003),
        (39_070_003, 39_070_004),
        (39_070_004, 39_070_005),
    ];
    pairs.iter().flat_map(|&(a, b)| [(a, b), (b, a)]).collect()
}

#[test]
fn a_system_of_the_faction_is_pochven_and_one_of_another_is_not() {
    let u = universe();
    assert!(is_pochven_by(&u, FACTION, 39_070_001));
    assert!(!is_pochven_by(&u, FACTION, 39_070_005));
    assert!(!is_pochven_by(&u, FACTION, 39_070_999), "unknown system");
    assert_eq!(
        systems_by(&u, FACTION),
        vec![39_070_001, 39_070_002, 39_070_003, 39_070_004]
    );
}

#[test]
fn the_real_faction_does_not_match_the_invented_one() {
    let u = universe();
    assert!(!is_pochven(&u, 39_070_001));
    assert!(systems(&u).is_empty());
}

#[test]
fn the_krai_is_the_constellation() {
    let u = universe();
    assert_eq!(krai_of_by(&u, FACTION, 39_070_002), Some(KRAI_A));
    assert_eq!(krai_of_by(&u, FACTION, 39_070_003), Some(KRAI_B));
    assert_eq!(krai_of_by(&u, FACTION, 39_070_005), None, "not Pochven");
}

#[test]
fn a_home_passed_in_is_home_a_gate_into_another_krai_is_border_the_rest_internal() {
    let u = universe();
    let homes = [39_070_001, 39_070_004];
    let class = |s| class_with(&u, FACTION, &gates(), &homes, s);
    assert_eq!(class(39_070_001), Some(SystemClass::Home));
    assert_eq!(class(39_070_004), Some(SystemClass::Home));
    assert_eq!(class(39_070_002), Some(SystemClass::Border));
    assert_eq!(class(39_070_003), Some(SystemClass::Border));
    // Same map, no homes: system 1 has only a gate inside its Krai.
    assert_eq!(
        class_with(&u, FACTION, &gates(), &[], 39_070_001),
        Some(SystemClass::Internal)
    );
    assert_eq!(
        systems_of_class_with(&u, FACTION, &gates(), &homes, SystemClass::Border),
        vec![39_070_002, 39_070_003]
    );
}

#[test]
fn a_gate_to_a_system_of_another_faction_is_not_a_border() {
    // 4's only far end outside its Krai is 5, which is not Pochven.
    let u = universe();
    let only: Vec<(i64, i64)> = vec![(39_070_004, 39_070_005)];
    assert_eq!(
        class_with(&u, FACTION, &only, &[], 39_070_004),
        Some(SystemClass::Internal)
    );
}

#[test]
fn no_homes_means_no_home_class() {
    let u = universe();
    assert!(systems_of_class_with(&u, FACTION, &gates(), &[], SystemClass::Home).is_empty());
}

#[test]
fn a_non_pochven_system_has_no_class() {
    let u = universe();
    assert_eq!(
        class_with(&u, FACTION, &gates(), &[39_070_005], 39_070_005),
        None
    );
    assert_eq!(class_with(&u, FACTION, &gates(), &[], 39_070_999), None);
}

#[test]
fn homes_are_the_candidates_that_are_region_systems() {
    let u = universe();
    assert_eq!(
        homes_among(
            &u,
            FACTION,
            &[39_070_003, 39_070_001, 39_070_005, 39_070_999]
        ),
        vec![39_070_001, 39_070_003]
    );
    assert!(homes_among(&u, FACTION, &[]).is_empty());
}

#[test]
fn one_home_per_krai_is_checked() {
    let u = universe();
    assert!(one_home_per_krai(&u, FACTION, &[39_070_001, 39_070_004]));
    assert!(
        !one_home_per_krai(&u, FACTION, &[39_070_001]),
        "Krai B has none"
    );
    assert!(
        !one_home_per_krai(&u, FACTION, &[39_070_001, 39_070_002, 39_070_003]),
        "Krai A has two"
    );
}

#[test]
fn a_faction_system_of_another_wormhole_class_is_pochven_by_the_clients_test_but_in_no_krai() {
    // 31_xxx-style id, the faction, a class other than CLASS.
    let mut rows = vec![row(39_070_001, KRAI_A, FACTION, "-1.0", "Alpha")];
    rows.push(row_class(
        31_070_001,
        KRAI_A,
        FACTION,
        "-1.0",
        "Odd",
        Some(CLASS + 1),
    ));
    let u = universe_of(&rows);
    assert_eq!(
        u.system(31_070_001).unwrap().wormhole_class_id,
        Some(CLASS + 1)
    );
    assert!(is_pochven_by(&u, FACTION, 31_070_001));
    assert_eq!(krai_of_by(&u, FACTION, 31_070_001), None);
    assert_eq!(
        class_with(&u, FACTION, &[], &[31_070_001], 31_070_001),
        None
    );
    assert_eq!(
        systems_by(&u, FACTION),
        vec![39_070_001],
        "the outlier is not listed"
    );
}

#[test]
fn gates_come_from_the_catalogue_as_system_and_far_system() {
    let cat = Catalogue::parse(
        "#system\t39070001\t29070001\t19070001\n\
         #system\t39070002\t29070001\t19070001\n\
         gate\t990070001\t16\t100.0\t0.0\t0.0\t0.0\t39070001\t990070002\t39070002\n\
         gate\t990070002\t16\t100.0\t0.0\t0.0\t0.0\t39070002\t990070001\t39070001\n\
         planet\t990070003\t11\t100.0\t0.0\t0.0\t0.0\t39070001\t0\t0\n",
    );
    let mut g = gates_of(&cat);
    g.sort_unstable();
    assert_eq!(g, vec![(39_070_001, 39_070_002), (39_070_002, 39_070_001)]);
}

fn centers() -> Centers {
    let ly = 9.46e15;
    let at = |id: i64, ly_x: f64| format!("s\t{id}\t{}\t0.0\t0.0", ly_x * ly);
    Centers::parse(&[
        at(30_999_070, 0.0),  // the origin, a faction system
        at(30_999_071, 0.1),  // faction system next door: left out
        at(30_999_073, 1.0),  // k-space, nearest
        at(30_999_072, 3.0),  // k-space, lower id than the nearest
        at(30_999_074, 10.0), // k-space, out of a small reach
        at(30_999_075, 2.0),  // k-space, between the two
    ])
    .expect("invented centres")
}

fn kspace_universe() -> Universe {
    universe_of(&[
        row(30_999_070, KRAI_A, FACTION, "-1.0", "O"),
        row(30_999_071, KRAI_A, FACTION, "-1.0", "N"),
        row(30_999_073, 0, 0, "0.9", "H"),
        row(30_999_072, 0, 0, "-0.5", "Z"),
        row(30_999_074, 0, 0, "0.2", "L"),
        row(30_999_075, 0, 0, "0.5", "M"),
    ])
}

#[test]
fn kspace_within_keeps_the_reach_and_falls_back_to_the_nearest() {
    let (u, c) = (kspace_universe(), centers());
    // Reach 5 ly: 073 (1) and 072 (3) and 075 (2); ascending by id.
    assert_eq!(
        kspace_within_by(&u, FACTION, &c, 30_999_070, 5.0),
        vec![30_999_072, 30_999_073, 30_999_075]
    );
    // Reach 0.5 ly holds only the faction neighbour, which is left out: the
    // single nearest k-space system stands in.
    assert_eq!(
        kspace_within_by(&u, FACTION, &c, 30_999_070, 0.5),
        vec![30_999_073]
    );
}

#[test]
fn kspace_within_with_an_unknown_origin_is_empty() {
    let (u, c) = (kspace_universe(), centers());
    assert!(kspace_within_by(&u, FACTION, &c, 30_999_999, 5.0).is_empty());
}
