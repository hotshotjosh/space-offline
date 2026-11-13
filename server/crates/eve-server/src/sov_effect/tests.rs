//! SOV-UPG-4 tests. Every id and number is invented (generator `9_701`,
//! collections `990_433` / `990_437`, attributes `990_601` / `990_602`); no
//! authored value of `upgrades.tsv` is asserted.

use super::*;
use crate::content::Content;
use crate::dbuff::held_by;
use crate::dbuff_data::Dbuffs;
use crate::sov;
use crate::sov_hub::{self, Hub};
use crate::sov_upgrade::{Installed, Power, Row, UpgradeTable, put_installed};
use std::sync::Arc;

const SYSTEM: i64 = 30_791_001;
const OTHER: i64 = 30_791_002;
const HUB: i64 = 8_900_701;
const ALICE: i64 = 90_791_001;
const BOB: i64 = 90_791_002;
const NPC: i64 = 90_791_099;
const ALLIANCE: i64 = 99_791_001;
const CORP: i64 = 98_791_001;
const GEN: i64 = 9_701;
const MAX_C: i64 = 990_433;
const MIN_C: i64 = 990_437;
const ATTR_A: i64 = 990_601;
const ATTR_B: i64 = 990_602;
const NOW: i64 = 1_800_000_000;

fn ship(who: i64) -> i64 {
    100_791_000 + (who - ALICE) + 1
}

fn dbuffs() -> Dbuffs {
    let c = vec![
        format!("{MAX_C}\tPostPercent\tMaximum\tShowNormal\tX\tx"),
        format!("{MIN_C}\tPostPercent\tMinimum\tShowNormal\tX\tx"),
    ];
    let m = vec![
        format!("{MAX_C}\titem\t{ATTR_A}\t\t"),
        format!("{MIN_C}\titem\t{ATTR_B}\t\t"),
    ];
    Dbuffs::parse(&c, &m).expect("the test collections parse")
}

fn row(effects: Vec<(i64, i64)>) -> Row {
    Row {
        def: crate::sov_upgrade::Def {
            power: 0,
            workforce: 0,
            fuel: None,
            per_hour: 0,
            startup: 0,
            power_out: 0,
            workforce_out: 0,
        },
        group: "g".to_owned(),
        services: std::collections::BTreeSet::new(),
        tier: None,
        effects,
    }
}

fn world_with(table: UpgradeTable) -> Registry {
    let pilot = |id: i64| crate::model::Character {
        character_id: id,
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    };
    let mut c = Content::fixture();
    c.insert("dbuffs", dbuffs());
    c.insert("sov upgrades", table);
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![pilot(ALICE), pilot(BOB)],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(c));
    sov_hub::put(
        reg.deltas_mut(),
        &Hub {
            system_id: SYSTEM,
            hub_id: HUB,
            position: [0.0; 3],
            launcher_id: None,
            launch_time: 0,
            offline_days: 0,
        },
    );
    sov::set_claim(reg.deltas_mut(), SYSTEM, Some((ALLIANCE, CORP)), 1);
    put_installed(
        reg.deltas_mut(),
        HUB,
        &[Installed {
            type_id: GEN,
            online: true,
            state: Power::Online,
        }],
    );
    reg
}

fn world() -> Registry {
    world_with(UpgradeTable::new(vec![(GEN, row(vec![(MAX_C, 7_000), (MIN_C, -4_000)]))]).0)
}

fn arrive(reg: &mut Registry, who: i64, ship_id: i64, system: i64) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            who,
            ship_id,
            system,
            crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
        ));
}

fn in_space(reg: &mut Registry) {
    arrive(reg, ALICE, ship(ALICE), SYSTEM);
    arrive(reg, BOB, ship(BOB), SYSTEM);
}

fn run(reg: &mut Registry) {
    tick(reg, Instant::now(), NOW);
}

fn both() -> Vec<(i64, f64)> {
    vec![(MAX_C, 7.0), (MIN_C, -4.0)]
}

fn held_of(reg: &Registry, who: i64) -> Vec<(i64, f64)> {
    held_by(reg, ship(who), HUB)
}

#[test]
fn an_online_generator_holds_its_collections_on_every_pilot_in_its_system() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    assert_eq!(held_of(&reg, ALICE), both());
    assert_eq!(held_of(&reg, BOB), both());
}

#[test]
fn a_pilot_of_another_alliance_is_held_too() {
    let mut reg = world();
    reg.character_mut(BOB).expect("a pilot").alliance_id = Some(ALLIANCE + 1);
    in_space(&mut reg);
    run(&mut reg);
    assert_eq!(held_of(&reg, BOB), both());
}

#[test]
fn a_low_or_offline_generator_holds_nothing() {
    for state in [Power::Low, Power::Offline] {
        let mut reg = world();
        put_installed(
            reg.deltas_mut(),
            HUB,
            &[Installed {
                type_id: GEN,
                online: state != Power::Offline,
                state,
            }],
        );
        in_space(&mut reg);
        run(&mut reg);
        assert!(held_of(&reg, ALICE).is_empty(), "{state:?}");
    }
}

#[test]
fn a_neutral_hub_holds_nothing() {
    let mut reg = world();
    sov::set_claim(reg.deltas_mut(), SYSTEM, None, 2);
    in_space(&mut reg);
    run(&mut reg);
    assert!(held_of(&reg, ALICE).is_empty());
    assert!(reg.ext::<Held>().systems.is_empty());
}

#[test]
fn clearing_the_upgrades_releases_every_pilot() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    put_installed(reg.deltas_mut(), HUB, &[]);
    run(&mut reg);
    assert!(held_of(&reg, ALICE).is_empty());
    assert!(held_of(&reg, BOB).is_empty());
    assert!(reg.ext::<Held>().by_character.is_empty());
}

#[test]
fn a_lapsed_claim_releases() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    sov::set_claim(reg.deltas_mut(), SYSTEM, None, 2);
    run(&mut reg);
    assert!(held_of(&reg, ALICE).is_empty());
    assert!(reg.ext::<Held>().systems.is_empty());
}

#[test]
fn leaving_the_system_releases_and_coming_back_holds_again() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    arrive(&mut reg, ALICE, ship(ALICE), OTHER);
    run(&mut reg);
    assert!(held_of(&reg, ALICE).is_empty());
    assert_eq!(held_of(&reg, BOB), both());
    arrive(&mut reg, ALICE, ship(ALICE), SYSTEM);
    run(&mut reg);
    assert_eq!(held_of(&reg, ALICE), both());
}

#[test]
fn docking_releases() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    let _ = reg.roster_mut().leave(ALICE);
    run(&mut reg);
    assert!(!reg.ext::<Held>().by_character.contains_key(&ALICE));
    // The dbuff stage forgets a ship that left the roster.
    dbuff::tick(&mut reg, Instant::now(), NOW);
    assert!(held_of(&reg, ALICE).is_empty());
    assert!(!reg.ext::<Held>().by_character.contains_key(&ALICE));
}

#[test]
fn a_new_ship_is_held_and_the_old_one_released() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    let new_ship = ship(ALICE) + 500;
    arrive(&mut reg, ALICE, new_ship, SYSTEM);
    run(&mut reg);
    // Before the dbuff stage forgets the old ship, only this stage can have moved the record.
    assert_eq!(reg.ext::<Held>().by_character[&ALICE], (new_ship, HUB));
    dbuff::tick(&mut reg, Instant::now(), NOW);
    assert!(held_of(&reg, ALICE).is_empty());
    assert_eq!(held_by(&reg, new_ship, HUB), both());
}

#[test]
fn an_npc_in_the_system_is_not_held() {
    let mut reg = world();
    in_space(&mut reg);
    arrive(&mut reg, NPC, ship(NPC), SYSTEM);
    run(&mut reg);
    assert!(held_by(&reg, ship(NPC), HUB).is_empty());
    assert!(!reg.ext::<Held>().by_character.contains_key(&NPC));
}

#[test]
fn a_lost_hold_is_restored() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    dbuff::release(&mut reg, ship(ALICE), HUB, NOW);
    assert!(held_of(&reg, ALICE).is_empty());
    run(&mut reg);
    assert_eq!(held_of(&reg, ALICE), both());
}

#[test]
fn no_effect_rows_and_nobody_held_is_a_no_op() {
    let mut reg = world_with(UpgradeTable::new(vec![(GEN, row(Vec::new()))]).0);
    in_space(&mut reg);
    run(&mut reg);
    assert!(reg.ext::<Held>().by_character.is_empty());
    assert!(held_of(&reg, ALICE).is_empty());
    assert!(reg.ext::<Held>().systems.is_empty());
}

#[test]
fn the_sov_rows_are_read_again_only_when_they_change() {
    let mut reg = world();
    in_space(&mut reg);
    run(&mut reg);
    assert_eq!(reg.ext::<Held>().systems.len(), 1);
    // An unchanged world keeps its cache (here: emptied by hand); a changed row rebuilds it.
    reg.ext_mut::<Held>().systems.clear();
    run(&mut reg);
    assert!(reg.ext::<Held>().systems.is_empty());
    put_installed(reg.deltas_mut(), HUB, &[]);
    put_installed(
        reg.deltas_mut(),
        HUB,
        &[Installed {
            type_id: GEN,
            online: true,
            state: Power::Online,
        }],
    );
    run(&mut reg);
    assert_eq!(reg.ext::<Held>().systems.len(), 1);
}
