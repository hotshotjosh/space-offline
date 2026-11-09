//! The weather of the abyssal rooms. **Every id and number here is invented**
//! (types 990,95x, collections 990,59x, attribute 990,60x, and a stage table
//! of the tests' own): the client's constants appear only by name, and no
//! `[A]` value of the shipped table is asserted.

use super::super::tests::{ALICE, BOB, HOME, NOW, ROOM_A, room, seat, ship, stand, table, world};
use super::*;
use crate::abyss::{Mode, begin, close_run, join, leave, run_of, runs, tick};
use crate::content::Content;
use crate::dbuff::{Boosts, held_by, holds, state_value_of};
use crate::service::{Request, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

const BEACON: i64 = 990_950;
const VISUALS: [i64; 3] = [990_951, 990_952, 990_953];
const PENALTY: i64 = 990_591;
const BONUS: i64 = 990_592;
const ATTR_A: i64 = 990_601;
const ATTR_B: i64 = 990_602;
const TIER: i64 = 3;
const HEAD: &str = "#tier\tstage_weights\n";

fn invented() -> Weather {
    Weather {
        beacon: BEACON,
        penalty: PENALTY,
        bonus: BONUS,
        visual: VISUALS,
    }
}

/// Every tier plays stage 2; stage 2 costs 40 and gives 60.
fn stages() -> Stages {
    parse(&format!(
        "{HEAD}0\t2:1\n1\t2:1\n2\t2:1\n3\t2:1\n4\t2:1\n5\t2:1\n6\t2:1\n\
         #stage\tpenalty_pct\tbonus_pct\n1\t10\t20\n2\t40\t60\n3\t70\t80\n"
    ))
    .expect("the test table parses")
}

/// A maximum collection (the penalty) and a minimum one (the bonus).
fn dbuffs() -> Dbuffs {
    let c = vec![
        format!("{PENALTY}\tPostPercent\tMaximum\tShowNormal\tX\tx"),
        format!("{BONUS}\tPostPercent\tMinimum\tShowNormal\tX\tx"),
    ];
    let m = vec![
        format!("{PENALTY}\titem\t{ATTR_A}\t\t"),
        format!("{BONUS}\titem\t{ATTR_B}\t\t"),
    ];
    Dbuffs::parse(&c, &m).expect("the test collections parse")
}

fn content() -> Arc<Content> {
    let mut c = Content::fixture();
    c.insert("universe", super::super::tests::universe());
    c.insert("dungeons", table(vec![room(ROOM_A)]));
    c.insert("typedata", super::super::tests::typedata());
    c.insert("dbuffs", dbuffs());
    Arc::new(c)
}

/// A world with the invented weather and stage table in force, Alice and Bob
/// in space at home, rooms with no waves.
fn weather_world() -> Registry {
    let mut reg = world().with_content(content());
    let s = reg.ext_mut::<AbyssState>();
    s.waves = Some(Vec::new());
    s.weathers = Some(vec![invented()]);
    s.stages = Some(stages());
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    reg
}

/// Alice alone in room 1 of a three-room run of the invented weather.
fn solo(weather: i64) -> (Registry, u64) {
    let mut reg = weather_world();
    let id = begin(&mut reg, ALICE, Mode::Solo, TIER, weather, 0, 3, NOW).expect("a run");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    (reg, id)
}

fn site_of_room(reg: &Registry, id: u64, room: usize) -> i64 {
    reg.ext::<AbyssState>().runs[&id].pockets[room - 1].site_id
}

fn held(reg: &Registry, who: i64, site: i64) -> Vec<(i64, f64)> {
    held_by(reg, ship(who), site)
}

/// What the invented weather holds in room 2 of the stage table: the penalty
/// (a maximum collection, so positive) and the bonus (a minimum, so negative).
fn both() -> Vec<(i64, f64)> {
    vec![(PENALTY, 40.0), (BONUS, -60.0)]
}

fn count_of(reg: &Registry, site: i64, type_id: i64) -> usize {
    reg.ext::<crate::site::Sites>().get(site).map_or(0, |i| {
        i.objects
            .iter()
            .filter(|o| o.type_id == type_id && crate::site::object(reg, o.item_id).is_some())
            .count()
    })
}

fn go_on(reg: &mut Registry, who: i64, gate: i64) {
    let args = [PyValue::Int(gate)];
    let r: Result<PyValue, ServiceError> = crate::abyss::dispatch(
        reg,
        &Request {
            service: "abyssalMgr",
            method: "AbyssalGateActivation",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        },
    );
    r.expect("the gate takes the pilot on");
}

#[test]
fn a_stage_table_parses_and_a_bad_weight_is_an_error_naming_its_line() {
    let (rows, strengths) = parse(&format!(
        "# a comment\n{HEAD}0\t1:50,2:50\n#stage\tpenalty_pct\tbonus_pct\n1\t30\t50\n2\t50\t50\n"
    ))
    .expect("parses");
    assert_eq!(
        rows,
        vec![StageRow {
            tier: 0,
            weights: vec![(1, 50), (2, 50)]
        }]
    );
    assert_eq!(strengths.get(&2), Some(&(50.0, 50.0)));
    let line = |text: &str| parse(text).expect_err("refused");
    let e = line(&format!(
        "{HEAD}0\t1:x\n#stage\tpenalty_pct\tbonus_pct\n1\t3\t4\n"
    ));
    assert!(e.contains("line 2"), "{e}");
    let e = line(&format!("{HEAD}0\t1:5\n\n0\t1:5\n#stage\ta\tb\n1\t3\t4\n"));
    assert!(e.contains("line 4") && e.contains("twice"), "{e}");
    let e = line(&format!("{HEAD}0\t1:0\n#stage\ta\tb\n1\t3\t4\n"));
    assert!(e.contains("line 2"), "{e}");
    let e = line(&format!("{HEAD}0\t1:5\n#stage\ta\tb\n2\t3\t4\n"));
    assert!(e.contains("line 2") && e.contains("stage 1"), "{e}");
    let e = line("0\t1:5\n");
    assert!(e.contains("line 1"), "{e}");
}

#[test]
fn the_stage_draw_is_stable_and_only_ever_a_listed_stage() {
    let rows = parse(&format!(
        "{HEAD}1\t1:3,3:1\n#stage\ta\tb\n1\t1\t1\n3\t1\t1\n"
    ))
    .expect("parses")
    .0;
    let mut seen = BTreeSet::new();
    for seed in 0..200u64 {
        let s = stage_for(&rows, 1, seed).expect("a stage");
        assert!(s == 1 || s == 3, "stage {s} is not listed");
        assert_eq!(stage_for(&rows, 1, seed), Some(s), "stable");
        seen.insert(s);
    }
    assert_eq!(seen.len(), 2, "both appear");
    // The weights are the shares: 3 parts of 4 are stage 1.
    assert_eq!(stage_for(&rows, 1, 0), Some(1));
    assert_eq!(stage_for(&rows, 1, 2), Some(1));
    assert_eq!(stage_for(&rows, 1, 3), Some(3));
    assert_eq!(stage_for(&rows, 2, 0), None, "a tier with no row");
}

#[test]
fn a_maximum_collection_gets_a_positive_value_and_a_minimum_one_a_negative() {
    assert_eq!(signed(30.0, Aggregate::Maximum), 30.0);
    assert_eq!(signed(30.0, Aggregate::Minimum), -30.0);
    let s = Strength {
        stage: 2,
        penalty_pct: 40.0,
        bonus_pct: 60.0,
    };
    assert_eq!(buffs(invented(), s, &dbuffs()), both());
    let none = Dbuffs::default();
    assert!(
        buffs(invented(), s, &none).is_empty(),
        "a bundle without them"
    );
    assert_eq!(of(&[invented()], BEACON), Some(invented()));
    assert_eq!(of(&[invented()], BEACON + 1), None);
}

#[test]
fn a_pilot_in_a_weathered_room_holds_both_collections_with_no_expiry() {
    let (reg, id) = solo(BEACON);
    let site = site_of_room(&reg, id, 1);
    assert_eq!(held(&reg, ALICE, site), both());
    let expected = PyValue::List(
        both()
            .into_iter()
            .map(|(c, v)| {
                PyValue::Tuple(vec![
                    PyValue::Int(c),
                    PyValue::Tuple(vec![PyValue::Real(v), PyValue::None]),
                ])
            })
            .collect(),
    );
    assert_eq!(
        state_value_of(&reg, ship(ALICE)),
        expected,
        "told, no timer"
    );
    assert_eq!(reg.ext::<AbyssState>().runs[&id].pockets[0].stage, 2);
}

#[test]
fn the_next_room_swaps_the_source_and_home_releases_it() {
    let (mut reg, id) = solo(BEACON);
    let one = reg.ext::<AbyssState>().runs[&id].pockets[0].clone();
    go_on(&mut reg, ALICE, one.gate);
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 2);
    let two = site_of_room(&reg, id, 2);
    assert_ne!(two, one.site_id);
    assert_eq!(held(&reg, ALICE, two), both(), "the new room's source");
    assert!(
        held(&reg, ALICE, one.site_id).is_empty(),
        "the old one let go"
    );
    // Out of the pocket: the ship is in an ordinary system.
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    tick(&mut reg, Instant::now(), NOW + 3);
    assert!(held(&reg, ALICE, two).is_empty(), "home holds nothing");
    assert!(state_value_of(&reg, ship(ALICE)) == PyValue::List(Vec::new()));
}

#[test]
fn two_pilots_in_one_room_both_hold_it_and_one_leaving_leaves_the_other() {
    let mut reg = weather_world();
    let id = begin(&mut reg, ALICE, Mode::CoOp, TIER, BEACON, 0, 3, NOW).expect("a run");
    for who in [ALICE, BOB] {
        join(&mut reg, id, who, NOW).expect("in");
        seat(&mut reg, who);
    }
    tick(&mut reg, Instant::now(), NOW + 1);
    let site = site_of_room(&reg, id, 1);
    assert_eq!(held(&reg, ALICE, site), both());
    assert_eq!(held(&reg, BOB, site), both());
    assert!(leave(&mut reg, BOB));
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    tick(&mut reg, Instant::now(), NOW + 2);
    assert!(held(&reg, BOB, site).is_empty(), "the one who left");
    assert_eq!(held(&reg, ALICE, site), both(), "the one who stayed");
}

#[test]
fn a_run_with_no_weather_holds_nothing_and_places_no_beacon() {
    let (reg, id) = solo(0);
    let site = site_of_room(&reg, id, 1);
    assert!(held(&reg, ALICE, site).is_empty());
    assert_eq!(reg.ext::<AbyssState>().runs[&id].pockets[0].stage, 0);
    for t in std::iter::once(BEACON).chain(VISUALS) {
        assert_eq!(count_of(&reg, site, t), 0);
    }
    // A weather the table does not know is none either.
    let (reg, id) = solo(BEACON + 7);
    let site = site_of_room(&reg, id, 1);
    assert!(held(&reg, ALICE, site).is_empty());
    assert_eq!(count_of(&reg, site, BEACON), 0);
}

#[test]
fn a_room_holds_its_beacon_and_the_visual_of_its_stage() {
    let (reg, id) = solo(BEACON);
    let site = site_of_room(&reg, id, 1);
    assert_eq!(count_of(&reg, site, BEACON), 1);
    assert_eq!(count_of(&reg, site, VISUALS[1]), 1, "stage 2 is the second");
    assert_eq!(
        count_of(&reg, site, VISUALS[0]) + count_of(&reg, site, VISUALS[2]),
        0
    );
}

#[test]
fn a_room_made_again_does_not_place_a_second_beacon() {
    let (mut reg, id) = solo(BEACON);
    let site = site_of_room(&reg, id, 1);
    assert!(crate::site::vacate(&mut reg, site));
    tick(&mut reg, Instant::now(), NOW + 2);
    assert_eq!(count_of(&reg, site, BEACON), 1, "one in the new instance");
    tick(&mut reg, Instant::now(), NOW + 3);
    assert_eq!(count_of(&reg, site, BEACON), 1, "and the tick adds none");
    assert_eq!(count_of(&reg, site, VISUALS[1]), 1);
}

#[test]
fn a_closed_room_releases_a_pilot_still_held() {
    let (mut reg, id) = solo(BEACON);
    let site = site_of_room(&reg, id, 1);
    assert_eq!(held(&reg, ALICE, site), both());
    let run = run_of(&reg, ALICE).cloned().expect("a run");
    close_run(&mut reg, run);
    assert_eq!(runs(&reg).count(), 0);
    assert!(
        held(&reg, ALICE, site).is_empty(),
        "the hold went with the room"
    );
}

#[test]
fn the_shipped_stage_table_parses_and_every_tier_has_a_row() {
    let (rows, strengths) = parse(SHIPPED).expect("the shipped table parses");
    for tier in 0..=6 {
        assert!(
            rows.iter().any(|r| r.tier == tier),
            "tier {tier} has no row"
        );
        assert!(stage_for(&rows, tier, 1).is_some());
    }
    assert!(!strengths.is_empty());
    assert_eq!(WEATHERS.len(), 5);
}

#[test]
fn holds_tells_a_held_source_without_listing_it() {
    let (mut reg, id) = solo(BEACON);
    let site = site_of_room(&reg, id, 1);
    assert!(holds(&reg, ship(ALICE), site));
    assert!(!holds(&reg, ship(ALICE), site + 1), "another source");
    assert!(!holds(&reg, ship(BOB), site), "another ship");
    crate::dbuff::release(&mut reg, ship(ALICE), site, NOW);
    assert!(!holds(&reg, ship(ALICE), site));
}

#[test]
fn a_run_with_no_weather_leaves_the_boosts_as_they_were() {
    let mut reg = weather_world();
    // Something else holds a buff on Alice's ship before the run starts.
    const OTHER: i64 = 990_701;
    crate::dbuff::hold(&mut reg, ship(ALICE), OTHER, &[(PENALTY, 5.0)], NOW);
    let (before, told) = (
        reg.ext::<Boosts>().boosted(),
        state_value_of(&reg, ship(ALICE)),
    );
    let id = begin(&mut reg, ALICE, Mode::Solo, TIER, 0, 0, 3, NOW).expect("a run");
    seat(&mut reg, ALICE);
    for n in 1..4 {
        tick(&mut reg, Instant::now(), NOW + n);
    }
    let site = site_of_room(&reg, id, 1);
    assert!(!holds(&reg, ship(ALICE), site));
    assert_eq!(reg.ext::<Boosts>().boosted(), before);
    assert_eq!(held_by(&reg, ship(ALICE), OTHER), vec![(PENALTY, 5.0)]);
    assert_eq!(state_value_of(&reg, ship(ALICE)), told);
}

// ---------------------------------------------------------------------------
// The room's clouds (AD-4). Cloud types 990,81x, reach and buffs invented; the
// attribute ids of the pairs and of the reach are the client's own constants.
// ---------------------------------------------------------------------------

const CLOUD_BUFFING: i64 = 990_811;
const CLOUD_BARE: i64 = 990_812;
const CLOUD_BIG: i64 = 990_813;
const REACH_M: f64 = 500.0;
const CLOUD_AT: [f64; 3] = [1_000.0, 0.0, 0.0];

fn cloud_row(id: i64, name: &str, attrs: &str) -> String {
    format!("type\t{id}\t{GROUP_CLOUD}\t2\t0\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t{attrs}\t{name}\n")
}

/// A buffing small caustic cloud (two pairs: the penalty collection at 7, the
/// bonus collection at -9; reach 500 m), a small one with no pairs, and a
/// large filament one with a pair but no reach.
fn cloud_typedata() -> crate::typedata::Installed {
    let rows = format!(
        "{}{}{}{}",
        crate::typedata::FIXTURE,
        cloud_row(
            CLOUD_BUFFING,
            "Small Caustic Cloud",
            &format!("{ATTR_REACH}={REACH_M};2468={PENALTY}.0;2469=7.0;2470={BONUS}.0;2471=-9.0")
        ),
        cloud_row(
            CLOUD_BARE,
            "Small Bioluminescence Cloud",
            &format!("{ATTR_REACH}={REACH_M}")
        ),
        cloud_row(
            CLOUD_BIG,
            "Large Filament Cloud",
            &format!("2468={PENALTY}.0;2469=3.0")
        ),
    );
    crate::typedata::Installed::build(
        crate::typedata::Table::parse(&rows).expect("the cloud rows parse"),
    )
}

fn cloud_world() -> Registry {
    let mut c = Content::fixture();
    c.insert("universe", super::super::tests::universe());
    c.insert("dungeons", table(vec![room(ROOM_A)]));
    c.insert("typedata", cloud_typedata());
    c.insert("dbuffs", dbuffs());
    let mut reg = world().with_content(Arc::new(c));
    reg.ext_mut::<AbyssState>().waves = Some(Vec::new());
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    reg
}

/// Alice alone in room 1 of a run with no weather, in a world of clouds.
fn cloud_solo() -> (Registry, u64) {
    let mut reg = cloud_world();
    let id = begin(&mut reg, ALICE, Mode::Solo, TIER, 0, 0, 3, NOW).expect("a run");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    // The shipped chances may have drawn a cloud at the locator: start clear.
    gm_clear(&mut reg, ALICE);
    (reg, id)
}

fn pocket_of_run(reg: &Registry, id: u64) -> Pocket {
    reg.ext::<AbyssState>().runs[&id].pockets[0].clone()
}

fn put_cloud(reg: &mut Registry, id: u64, ty: i64, at: [f64; 3]) -> i64 {
    let site = site_of_room(reg, id, 1);
    crate::site::add_object(reg, site, ty, at)
        .expect("the cloud is placed")
        .item_id
}

fn cloud_buffs_held() -> Vec<(i64, f64)> {
    vec![(PENALTY, 7.0), (BONUS, -9.0)]
}

#[test]
fn a_cloud_buffs_a_ship_inside_and_releases_it_outside() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    let ball = put_cloud(&mut reg, id, CLOUD_BUFFING, CLOUD_AT);
    // Outside the reach: nothing.
    stand(
        &mut reg,
        ALICE,
        p.system,
        [CLOUD_AT[0] + REACH_M * 4.0, 0.0, 0.0],
    );
    tick(&mut reg, Instant::now(), NOW + 2);
    assert!(held(&reg, ALICE, ball).is_empty(), "outside holds nothing");
    // Inside: both buffs, under the cloud's own source, told with no timer.
    stand(
        &mut reg,
        ALICE,
        p.system,
        [CLOUD_AT[0] + REACH_M * 0.5, 0.0, 0.0],
    );
    tick(&mut reg, Instant::now(), NOW + 3);
    assert_eq!(held(&reg, ALICE, ball), cloud_buffs_held());
    assert!(
        !holds(&reg, ship(ALICE), p.site_id),
        "not the room's source"
    );
    let expected = PyValue::List(
        cloud_buffs_held()
            .into_iter()
            .map(|(c, v)| {
                PyValue::Tuple(vec![
                    PyValue::Int(c),
                    PyValue::Tuple(vec![PyValue::Real(v), PyValue::None]),
                ])
            })
            .collect(),
    );
    assert_eq!(state_value_of(&reg, ship(ALICE)), expected);
    // Out again: released.
    stand(
        &mut reg,
        ALICE,
        p.system,
        [CLOUD_AT[0] + REACH_M * 4.0, 0.0, 0.0],
    );
    tick(&mut reg, Instant::now(), NOW + 4);
    assert!(held(&reg, ALICE, ball).is_empty(), "leaving released it");
    assert!(state_value_of(&reg, ship(ALICE)) == PyValue::List(Vec::new()));
}

#[test]
fn a_cloud_survives_a_room_change_of_the_room_weather_source() {
    // The cloud's source is its ball, not the room's site id: the room's own
    // release (the weather's source) leaves a held cloud alone.
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    let ball = put_cloud(&mut reg, id, CLOUD_BUFFING, CLOUD_AT);
    stand(&mut reg, ALICE, p.system, CLOUD_AT);
    tick(&mut reg, Instant::now(), NOW + 2);
    assert_eq!(held(&reg, ALICE, ball), cloud_buffs_held());
    crate::dbuff::release(&mut reg, ship(ALICE), p.site_id, NOW + 2);
    assert_eq!(held(&reg, ALICE, ball), cloud_buffs_held());
}

#[test]
fn a_cloud_type_without_dbuff_attributes_is_placed_and_buffs_nothing() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    let before = reg.ext::<Boosts>().boosted();
    let bare = put_cloud(&mut reg, id, CLOUD_BARE, CLOUD_AT);
    let noreach = put_cloud(&mut reg, id, CLOUD_BIG, CLOUD_AT);
    assert_eq!(count_of(&reg, p.site_id, CLOUD_BARE), 1, "placed");
    assert!(!has_cloud(&reg, p.site_id), "but not a cloud that buffs");
    stand(&mut reg, ALICE, p.system, CLOUD_AT);
    tick(&mut reg, Instant::now(), NOW + 2);
    assert!(held(&reg, ALICE, bare).is_empty(), "no pairs, no buffs");
    assert!(held(&reg, ALICE, noreach).is_empty(), "no reach, no buffs");
    assert_eq!(reg.ext::<Boosts>().boosted(), before);
}

#[test]
fn a_closed_room_releases_its_clouds() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    let ball = put_cloud(&mut reg, id, CLOUD_BUFFING, CLOUD_AT);
    stand(&mut reg, ALICE, p.system, CLOUD_AT);
    tick(&mut reg, Instant::now(), NOW + 2);
    assert_eq!(held(&reg, ALICE, ball), cloud_buffs_held());
    let run = run_of(&reg, ALICE).cloned().expect("a run");
    close_run(&mut reg, run);
    assert!(held(&reg, ALICE, ball).is_empty());
}

#[test]
fn the_size_follows_the_locators_name() {
    assert_eq!(size_of_name("AOE_10k_locator"), Some(Size::Small));
    assert_eq!(size_of_name("AOE_30k_locator"), Some(Size::Medium));
    assert_eq!(size_of_name("aoe_80K"), Some(Size::Large));
    for bad in [
        "AOE_20k_locator",
        "AOE_structure_1",
        "10k",
        "AOE_10",
        "",
        "AOE_k",
    ] {
        assert_eq!(size_of_name(bad), None, "{bad:?}");
    }
    let sizes: Vec<_> = LOCATOR_NAMES
        .iter()
        .map(|(id, _)| locator_size(*id))
        .collect();
    assert_eq!(
        sizes,
        vec![Some(Size::Small), Some(Size::Medium), Some(Size::Large)]
    );
    assert_eq!(locator_size(1), None);
    assert!(
        LOCATOR_NAMES
            .iter()
            .all(|(id, _)| super::super::AOE_LOCATORS.contains(id))
    );
}

#[test]
fn the_cloud_draw_is_stable_and_places_the_type_of_the_size_and_family() {
    let rows = vec![(Family::Caustic, 30), (Family::Filament, 30)];
    for seed in 0..300u64 {
        assert_eq!(draw_family(&rows, seed), draw_family(&rows, seed + 100));
    }
    assert_eq!(draw_family(&rows, 0), Some(Family::Caustic));
    assert_eq!(draw_family(&rows, 29), Some(Family::Caustic));
    assert_eq!(draw_family(&rows, 30), Some(Family::Filament));
    assert_eq!(draw_family(&rows, 60), None, "the rest is no cloud");
    assert_eq!(draw_family(&[], 5), None);
    // The type is the group-1971 one named for both words.
    let (mut reg, id) = cloud_solo();
    assert_eq!(
        cloud_type(&reg, Size::Small, Family::Caustic),
        Some(CLOUD_BUFFING)
    );
    assert_eq!(
        cloud_type(&reg, Size::Large, Family::Filament),
        Some(CLOUD_BIG)
    );
    assert_eq!(cloud_type(&reg, Size::Medium, Family::Caustic), None);
    // The test room's one locator is the small one: a sure caustic draw puts
    // the small caustic cloud there, once, and the same again in a room made again.
    let (run, p) = (
        reg.ext::<AbyssState>().runs[&id].clone(),
        pocket_of_run(&reg, id),
    );
    assert_eq!(count_of(&reg, p.site_id, CLOUD_BUFFING), 0);
    place_clouds(&mut reg, &run, &p, &[(Family::Caustic, 100)]);
    assert_eq!(count_of(&reg, p.site_id, CLOUD_BUFFING), 1);
    place_clouds(&mut reg, &run, &p, &[(Family::Filament, 100)]);
    assert_eq!(
        count_of(&reg, p.site_id, CLOUD_BIG),
        0,
        "no large locator here"
    );
    place_clouds(&mut reg, &run, &p, &[]);
    assert_eq!(
        count_of(&reg, p.site_id, CLOUD_BUFFING),
        1,
        "no rows, no cloud"
    );
}

#[test]
fn the_cloud_rows_parse_and_a_bad_one_names_its_line() {
    let rows = parse_clouds("#cloud\tfamily\tchance_pct\ncaustic\t30\nnone\t70\n").expect("parses");
    assert_eq!(rows, vec![(Family::Caustic, 30)]);
    let e = |t: &str| parse_clouds(t).expect_err("refused");
    assert!(e("#cloud\tf\tc\nmoss\t5\n").contains("line 2"));
    assert!(e("#cloud\tf\tc\ncaustic\t101\n").contains("line 2"));
    assert!(e("#cloud\tf\tc\ncaustic\t5\ncaustic\t6\n").contains("twice"));
    assert!(e("#cloud\tf\tc\ncaustic\t60\nfilament\t60\n").contains("over 100"));
    // The shipped table: the stage parser skips the cloud section and the
    // cloud parser reads it.
    assert!(parse(SHIPPED).is_ok());
    let shipped = parse_clouds(SHIPPED).expect("the shipped clouds parse");
    assert!(
        !shipped.is_empty(),
        "comment lines after the header end nothing"
    );
    let c = parse_clouds("#cloud\tf\tc\n# a comment\ncaustic\t5\n").expect("parses");
    assert_eq!(c, vec![(Family::Caustic, 5)]);
    assert!(shipped.iter().map(|(_, p)| *p).sum::<u32>() <= 100);
}

#[test]
fn gm_cloud_puts_one_at_the_pilot_and_answers_its_buffs() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    stand(&mut reg, ALICE, p.system, [123.0, 0.0, 0.0]);
    let said = gm_cloud(&mut reg, ALICE);
    assert!(said.contains("buffs: 2"), "{said}");
    assert!(said.contains(&format!("type {CLOUD_BUFFING}")), "{said}");
    assert_eq!(count_of(&reg, p.site_id, CLOUD_BUFFING), 1);
    tick(&mut reg, Instant::now(), NOW + 2);
    let ball = cloud_balls(&reg, p.site_id)[0].0;
    assert_eq!(
        held(&reg, ALICE, ball),
        cloud_buffs_held(),
        "it buffs at once"
    );
    assert_eq!(gm_cloud(&mut reg, BOB), "you are not in a pocket");
}

const SCENERY: i64 = 990_814;

#[test]
fn scenery_of_group_1971_without_buffs_is_not_a_cloud() {
    let mut c = Content::fixture();
    c.insert("universe", super::super::tests::universe());
    c.insert("dungeons", table(vec![room(ROOM_A)]));
    let rows = format!(
        "{}{}{}{}",
        crate::typedata::FIXTURE,
        cloud_row(
            CLOUD_BUFFING,
            "Small Caustic Cloud",
            &format!("{ATTR_REACH}={REACH_M};2468={PENALTY}.0;2469=7.0")
        ),
        cloud_row(SCENERY, "Invented Scenery", ""),
        cloud_row(SCENERY + 1, "Invented Cloud Without Buffs", "")
    );
    c.insert(
        "typedata",
        crate::typedata::Installed::build(crate::typedata::Table::parse(&rows).expect("rows")),
    );
    c.insert("dbuffs", dbuffs());
    let mut reg = world().with_content(Arc::new(c));
    reg.ext_mut::<AbyssState>().waves = Some(Vec::new());
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    let id = begin(&mut reg, ALICE, Mode::Solo, TIER, 0, 0, 3, NOW).expect("a run");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    gm_clear(&mut reg, ALICE);
    let p = pocket_of_run(&reg, id);
    // The room already holds scenery, placed first.
    put_cloud(&mut reg, id, SCENERY, CLOUD_AT);
    put_cloud(&mut reg, id, SCENERY + 1, CLOUD_AT);
    assert!(!has_cloud(&reg, p.site_id));
    assert!(cloud_balls(&reg, p.site_id).is_empty());
    assert_eq!(
        gm_cloud_type(&reg, p.site_id),
        Some(CLOUD_BUFFING),
        "never the scenery"
    );
    stand(&mut reg, ALICE, p.system, CLOUD_AT);
    let said = gm_cloud(&mut reg, ALICE);
    assert!(
        said.contains("buffs: 1") && said.contains(&format!("type {CLOUD_BUFFING}")),
        "{said}"
    );
}

#[test]
fn gm_clear_removes_the_clouds_and_lets_go_and_the_cap_holds() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    stand(&mut reg, ALICE, p.system, CLOUD_AT);
    let ball = put_cloud(&mut reg, id, CLOUD_BUFFING, CLOUD_AT);
    tick(&mut reg, Instant::now(), NOW + 2);
    assert_eq!(held(&reg, ALICE, ball), cloud_buffs_held());
    assert_eq!(gm_clear(&mut reg, ALICE), "1 cloud(s) removed");
    assert!(held(&reg, ALICE, ball).is_empty());
    assert!(crate::site::object(&reg, ball).is_none());
    tick(&mut reg, Instant::now(), NOW + 3);
    assert!(held(&reg, ALICE, ball).is_empty(), "and not held again");
    assert_eq!(gm_clear(&mut reg, ALICE), "0 cloud(s) removed");
    for _ in 0..GM_CLOUD_CAP {
        assert!(gm_cloud(&mut reg, ALICE).contains("buffs: 2"));
    }
    assert!(gm_cloud(&mut reg, ALICE).contains("already"), "the cap");
    assert_eq!(cloud_balls(&reg, p.site_id).len(), GM_CLOUD_CAP);
    assert_eq!(gm_clear(&mut reg, BOB), "you are not in a pocket");
}

#[test]
fn a_clouds_source_is_never_a_run_rooms_site_id() {
    let (mut reg, id) = cloud_solo();
    let p = pocket_of_run(&reg, id);
    let ball = put_cloud(&mut reg, id, CLOUD_BUFFING, CLOUD_AT);
    assert!(crate::site::is_instance_id(ball));
    assert!(!crate::site::is_instance_id(p.site_id));
}
