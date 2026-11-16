//! Mercenary-den tests. Every id, type and number here is invented; the
//! client's constants appear only by name. The table is [`fixture_table`]'s
//! invented one (a level every 100 s, ten levels a stage, a tick every 60 s
//! of 1..3 infomorphs at stage 1 up to 5..9 at stage 5, 40 at most), the
//! reagent planets are the sov-resources fixture's. No authored `[A]` value
//! of the shipped table is pinned: the shipped file is only checked to be
//! well formed.

use super::*;
use crate::gateway::{field, get_bytes, get_uint};
use crate::sov_hub::fixture::{ALICE, BOB, CAROL, CORP, DAVE, LONE, RIVAL, SYSTEM, ship};
use crate::sov_resources::{fixture_planets, fixture_stars};
use crate::space_state::{Motion, Presence};
use std::sync::{Arc, RwLock};

/// An invented "now" in unix seconds.
const NOW: u64 = 2_000_000_000;

const STAR: i64 = 40_830_000;
/// Planet types 9_001 power, 9_002 workforce, 9_003 reagent (the fixture's).
const P_LAND: i64 = 40_830_012;
const P_LAND2: i64 = 40_830_013;
const P_REAGENT: i64 = 40_830_001;
const P_POWER: i64 = 40_830_011;
/// A workforce planet with no skyhook.
const P_NAKED: i64 = 40_830_014;
const DEN_TYPE: i64 = 990_830_010;
const HULL: i64 = 990_830_020;
/// A hull whose hold takes three units of anything of volume 1.
const TINY_HULL: i64 = 990_830_021;
const SK_LAND: i64 = 8_830_001;
const SK_LAND2: i64 = 8_830_002;
const SK_REAGENT: i64 = 8_830_003;
const SK_POWER: i64 = 8_830_004;
const ALICE_DEN: i64 = 100_830_101;
const ALICE_DEN2: i64 = 100_830_102;
const ALICE_DEN3: i64 = 100_830_103;
const BOB_DEN: i64 = 100_830_104;
const CAROL_DEN: i64 = 100_830_105;
const DAVE_DEN: i64 = 100_830_106;
const ALICE_HULL_ITEM: i64 = 100_830_107;
const HUB: i64 = 100_830_900;

fn catalogue() -> crate::celestial::Catalogue {
    crate::celestial::Catalogue::parse(&format!(
        "star\t{STAR}\t9000\t4.0e8\t0\t0\t0\t{SYSTEM}\t0\t0\n\
         planet\t{P_REAGENT}\t9003\t6.0e6\t1.0e10\t2.0e8\t3.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_POWER}\t9001\t6.0e6\t-4.0e9\t1.0e8\t9.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_LAND}\t9002\t6.0e6\t6.0e9\t-3.0e8\t-8.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_LAND2}\t9002\t6.0e6\t-7.0e9\t-3.0e8\t-2.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_NAKED}\t9002\t6.0e6\t-2.0e10\t-3.0e8\t4.0e9\t{SYSTEM}\t0\t0\n"
    ))
}

fn content() -> crate::content::Content {
    let mut c = crate::sov_hub::fixture::content();
    c.insert("sov planets", fixture_planets());
    c.insert("sov stars", fixture_stars());
    c.insert("sov mercenary dens", fixture_table());
    let text = format!(
        "{}type\t{DEN_TYPE}\t{GROUP_MERCENARY_DEN}\t22\t1\t1\t100.0\t0.0\t10.0\t2500.0\t0\t\t\tInvented Den\n\
         type\t{HULL}\t25\t6\t1\t1\t100.0\t5000.0\t1000.0\t20.0\t0\t\t\tInvented Roomy Hull\n\
         type\t{TINY_HULL}\t25\t6\t1\t1\t100.0\t3.0\t1000.0\t20.0\t0\t\t\tInvented Tiny Hull\n\
         type\t{FIXTURE_INFOMORPH}\t4824\t17\t1\t1\t1.0\t0.0\t0.1\t1.0\t0\t\t\tInvented Infomorph\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn item(c: &crate::content::Content, item_id: i64, owner: i64) -> crate::item::Item {
    let t = crate::item::find_in(c, DEN_TYPE).expect("a test type");
    crate::item::Item {
        item_id,
        type_id: DEN_TYPE,
        owner_id: owner,
        location_id: ship(owner),
        flag_id: crate::sov_hub::FLAG_CARGO,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    }
}

fn planet_at(reg: &Registry, planet: i64) -> [f64; 3] {
    skyhook::position_of(reg.celestials().get(planet).expect("a planet"))
}

fn off(at: [f64; 3], d: f64) -> [f64; 3] {
    [at[0], at[1], at[2] + d]
}

fn place_at(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        SYSTEM,
        Motion::arrived(at, [0.0; 3]),
    ));
}

fn at_spot(reg: &mut Registry, who: i64, planet: i64, d: f64) {
    let at = off(planet_at(reg, planet), d);
    place_at(reg, who, at);
}

fn skill(reg: &mut Registry, who: i64, level: i64) {
    let entries = vec![crate::skill::SkillEntry {
        type_id: SKILL_DEN_MANAGEMENT,
        level,
        points: 0,
        rank: 1,
    }];
    *reg.skills_mut(who).expect("a pilot has skills") = crate::skill::Skills::restore(entries);
}

/// A skyhook row, active and held (its corp is in the holder alliance).
fn stand(reg: &mut Registry, id: i64, planet: i64, corp: i64, active: bool) {
    skyhook::put(
        reg.deltas_mut(),
        &skyhook::Skyhook {
            id,
            planet,
            system: SYSTEM,
            corp,
            active,
            activated_at: NOW - 5_000,
            built_until: NOW - 5_000,
            secured: 0,
            unsecured: 0,
            last_cycle: NOW,
            vuln_start: 0,
            vuln_end: 0,
            state: skyhook::STATE_SHIELD_VULNERABLE,
        },
    );
}

/// The world: four pilots; alliance A (Alice's corp) holds the system with a
/// hub; Alice has den-management 2, Bob none, Carol 1 (no alliance, a tiny
/// hold), Dave 1. Skyhooks stand, switched on, over the two land planets, the
/// reagent planet and the power planet. Everybody is at the first land
/// planet's skyhook spot.
fn world() -> Registry {
    world_with(content())
}

fn world_with(content: crate::content::Content) -> Registry {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    };
    let content = Arc::new(content);
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
            pilot(DAVE, "Dave"),
        ],
        celestials: catalogue(),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for (who, ty) in [(ALICE, HULL), (BOB, HULL), (CAROL, TINY_HULL), (DAVE, HULL)] {
        let t = *crate::item::find_in(&content, ty).expect("a test hull");
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = ty;
            i.group_id = t.group_id;
            i.category_id = t.category_id;
        }
    }
    let mut hull = item(&content, ALICE_HULL_ITEM, ALICE);
    let t = *crate::item::find_in(&content, HULL).expect("a hull");
    (hull.type_id, hull.group_id, hull.category_id) = (HULL, t.group_id, t.category_id);
    items.extend([
        item(&content, ALICE_DEN, ALICE),
        item(&content, ALICE_DEN2, ALICE),
        item(&content, ALICE_DEN3, ALICE),
        item(&content, BOB_DEN, BOB),
        item(&content, CAROL_DEN, CAROL),
        item(&content, DAVE_DEN, DAVE),
        hull,
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_831_000,
        ..crate::store::Snapshot::default()
    });
    crate::service::sov_hub::test_corp(&mut reg, CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, LONE, CAROL, &[(CAROL, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, RIVAL, DAVE, &[(DAVE, 0)]);
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    let a = st
        .create(CORP, ALICE, "Invented Den Holders", "IDH", "", "", 1)
        .expect("an alliance");
    let d = st
        .create(RIVAL, DAVE, "Invented Den Rivals", "IDR", "", "", 1)
        .expect("a second alliance");
    st.save(reg.deltas_mut());
    for (who, corp, alliance) in [
        (ALICE, CORP, Some(a)),
        (BOB, CORP, Some(a)),
        (CAROL, LONE, None),
        (DAVE, RIVAL, Some(d)),
    ] {
        let c = reg.character_mut(who).expect("a pilot");
        c.corporation_id = corp;
        c.alliance_id = alliance;
    }
    crate::sov_hub::put(
        reg.deltas_mut(),
        &crate::sov_hub::Hub {
            system_id: SYSTEM,
            hub_id: HUB,
            position: [0.0; 3],
            launcher_id: None,
            launch_time: 0,
            offline_days: 0,
        },
    );
    crate::sov::set_claim(
        reg.deltas_mut(),
        SYSTEM,
        Some((a, CORP)),
        skyhook::filetime(1_000_000_000),
    );
    stand(&mut reg, SK_LAND, P_LAND, CORP, true);
    stand(&mut reg, SK_LAND2, P_LAND2, CORP, true);
    stand(&mut reg, SK_REAGENT, P_REAGENT, CORP, true);
    stand(&mut reg, SK_POWER, P_POWER, CORP, true);
    skill(&mut reg, ALICE, 2);
    skill(&mut reg, CAROL, 1);
    skill(&mut reg, DAVE, 1);
    let spot = off(planet_at(&reg, P_LAND), 5_000.0);
    for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
        place_at(&mut reg, who, spot);
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    let _ = reg.take_pending();
    let _ = crate::gateway_out::take(&mut reg);
    reg
}

/// Alice's first den, launched over the first land planet at [`NOW`].
fn launched(reg: &mut Registry) -> i64 {
    launch_one_at(reg, ALICE, ALICE_DEN, NOW).expect("Alice launches")
}

fn sent(reg: &mut Registry) -> Vec<crate::sov_hub::fixture::Sent> {
    crate::sov_hub::fixture::pending(reg)
}

fn notices(reg: &mut Registry) -> Vec<crate::gateway_out::Queued> {
    crate::gateway_out::take(reg)
}

fn ground(reg: Registry) -> Ground {
    Ground::Server(Arc::new(RwLock::new(reg)))
}

fn world_of(g: &Ground) -> std::sync::RwLockReadGuard<'_, Registry> {
    let Ground::Server(w) = g else {
        panic!("a server ground")
    };
    w.read().unwrap()
}

fn req(name: &str) -> String {
    format!("{API_PKG}{name}")
}

fn ask(g: &Ground, name: &str, body: &[u8], who: i64) -> Reply {
    gateway(&req(name), body, who, g).expect("the den arm owns it")
}

fn id_body(n: i64) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &id(n as u64));
    b
}

fn payload(r: &Reply) -> &[u8] {
    &r.payload.as_ref().expect("a payload").1
}

fn cargo_of(reg: &Registry, who: i64, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.location_id == ship(who) && i.type_id == type_id)
        .map(|i| i.quantity)
        .sum()
}

// ---------------------------------------------------------------------------
// The client's rule
// ---------------------------------------------------------------------------

#[test]
fn level_adds_whole_units_and_the_boundary_is_exact() {
    // 100 s a level: one second short of a unit adds nothing, a unit adds one.
    assert_eq!(level(7, 1_000, 100, false, 1_099), 7);
    assert_eq!(level(7, 1_000, 100, false, 1_100), 8);
    // Integer division: 2.99 units is 2.
    assert_eq!(level(7, 1_000, 100, false, 1_299), 9);
    assert_eq!(level(0, 1_000, 100, false, 1_300), 3);
}

#[test]
fn level_holds_when_paused_or_the_unit_is_zero_or_the_clock_is_behind() {
    assert_eq!(level(7, 1_000, 100, true, 9_000), 7, "paused");
    assert_eq!(level(7, 1_000, 0, false, 9_000), 7, "unit 0 never rises");
    assert_eq!(
        level(7, 1_000, 100, false, 900),
        7,
        "a clock behind adds none"
    );
}

#[test]
fn a_stage_is_the_highest_whose_lower_bound_is_reached() {
    let b = [(0, 9), (10, 19), (20, 29), (30, 39), (40, 99)];
    assert_eq!(stage_of(&b, 0), 1);
    assert_eq!(stage_of(&b, 9), 1);
    assert_eq!(stage_of(&b, 10), 2, "the lower bound belongs to the stage");
    assert_eq!(stage_of(&b, 39), 4);
    assert_eq!(stage_of(&b, 40), 5);
    assert_eq!(stage_of(&b, 5_000), 5, "above every band is the last stage");
    // A gap between bands stays in the stage below it.
    let gap = [(0, 4), (10, 19), (20, 29), (30, 39), (40, 99)];
    assert_eq!(stage_of(&gap, 7), 1);
    // A table whose first band starts above 0 still has a stage 1.
    let high = [(5, 9), (10, 19), (20, 29), (30, 39), (40, 99)];
    assert_eq!(stage_of(&high, 2), 1);
}

// ---------------------------------------------------------------------------
// Infomorph generation
// ---------------------------------------------------------------------------

fn den_at(infomorphs: u32, last_gen: u64, dev_base: u64) -> Den {
    Den {
        id: 8_831_001,
        owner: ALICE,
        skyhook: SK_LAND,
        planet: P_LAND,
        system: SYSTEM,
        type_id: DEN_TYPE,
        enabled: true,
        paused: false,
        dev_base,
        dev_at: last_gen,
        anarchy_base: 0,
        anarchy_at: last_gen,
        infomorphs,
        last_gen,
        seen: 11,
        held: false,
    }
}

#[test]
fn a_tick_yield_stays_in_its_stage_band_and_is_fixed_by_den_and_tick() {
    let def = fixture_table().get(0).unwrap().clone();
    for stage in 1..=5u32 {
        let (lo, hi) = def.gen_bands[stage as usize - 1];
        let mut seen = std::collections::BTreeSet::new();
        for n in 0..400u64 {
            let at = 1_000_000 + n * def.gen_tick_s;
            let y = tick_yield(8_831_001, &def, stage, at);
            assert!(
                (lo..=hi).contains(&y),
                "stage {stage}: {y} outside {lo}..={hi}"
            );
            assert_eq!(
                y,
                tick_yield(8_831_001, &def, stage, at),
                "fixed by (den, tick)"
            );
            seen.insert(y);
        }
        // The band is used, not just its floor.
        assert!(seen.len() > 1, "stage {stage} always made {seen:?}");
    }
}

#[test]
fn generation_never_passes_the_cap_and_stops_exactly_on_it() {
    let def = fixture_table().get(0).unwrap().clone();
    let den = den_at(def.cargo_cap - 1, NOW, 0);
    // Thousands of ticks later it is at the cap, not above.
    let (count, last) = generate(&den, &def, NOW + 5_000 * def.gen_tick_s);
    assert_eq!(count, def.cargo_cap);
    assert_eq!(
        last,
        NOW + 5_000 * def.gen_tick_s,
        "the clock still moves whole ticks"
    );
    // Already at the cap stays.
    let full = den_at(def.cargo_cap, NOW, 0);
    assert_eq!(
        generate(&full, &def, NOW + 10 * def.gen_tick_s).0,
        def.cargo_cap
    );
}

#[test]
fn generation_advances_by_whole_ticks_and_sums_the_ticks_it_passed() {
    let def = fixture_table().get(0).unwrap().clone();
    let den = den_at(0, NOW, 0);
    // Two and a half ticks: two ticks counted, the clock on the second.
    let (count, last) = generate(&den, &def, NOW + 2 * def.gen_tick_s + 30);
    assert_eq!(last, NOW + 2 * def.gen_tick_s);
    let want = tick_yield(den.id, &def, 1, NOW + def.gen_tick_s)
        + tick_yield(den.id, &def, 1, NOW + 2 * def.gen_tick_s);
    assert_eq!(count, want);
    // Under one tick: nothing, the clock stays.
    assert_eq!(generate(&den, &def, NOW + def.gen_tick_s - 1), (0, NOW));
}

#[test]
fn each_tick_uses_the_development_stage_at_that_tick() {
    let def = fixture_table().get(0).unwrap().clone();
    // Level 9 at the start (stage 1); a level every 100 s, so the first tick
    // (60 s) is level 9 and the second (120 s) is level 10, stage 2.
    let den = den_at(0, NOW, 9);
    let (count, _) = generate(&den, &def, NOW + 2 * def.gen_tick_s);
    let want = tick_yield(den.id, &def, 1, NOW + def.gen_tick_s)
        + tick_yield(den.id, &def, 2, NOW + 2 * def.gen_tick_s);
    assert_eq!(count, want);
    // A paused den's level does not rise, so every tick is stage 1.
    let mut paused = den;
    paused.paused = true;
    let (count, _) = generate(&paused, &def, NOW + 2 * def.gen_tick_s);
    let want = tick_yield(den.id, &def, 1, NOW + def.gen_tick_s)
        + tick_yield(den.id, &def, 1, NOW + 2 * def.gen_tick_s);
    assert_eq!(count, want);
}

#[test]
fn a_den_that_is_off_makes_nothing_and_its_clock_does_not_move() {
    let def = fixture_table().get(0).unwrap().clone();
    let mut den = den_at(3, NOW, 0);
    den.enabled = false;
    assert_eq!(generate(&den, &def, NOW + 100 * def.gen_tick_s), (3, NOW));
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

#[test]
fn the_shipped_file_is_well_formed_and_its_stages_ascend() {
    let dir = crate::authored::Dir::compiled_only();
    let f = dir.load(FILE, SHIPPED, HEADER).unwrap();
    let (rows, skipped) = f.parse_rows_logged(parse_def_row);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(!rows.is_empty());
    for (ty, def) in &rows {
        for bands in [&def.dev_bands, &def.anarchy_bands] {
            for w in bands.windows(2) {
                assert!(
                    w[0].0 < w[1].0,
                    "den type {ty}: stage lower bounds must ascend"
                );
            }
        }
        for w in def.gen_bands.windows(2) {
            assert!(
                w[0].0 <= w[1].0 && w[0].1 <= w[1].1,
                "den type {ty}: richer stages make no less"
            );
        }
    }
}

#[test]
fn the_loader_drops_a_row_that_cannot_work() {
    let good = "1\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t60\t1,2,2,3,3,4,4,5,5,6\t10\t7\tok\n";
    let text = format!(
        "{HEADER}\n{good}\
         2\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t0\t1,2,2,3,3,4,4,5,5,6\t10\t7\tzero tick\n\
         3\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t60\t1,2,2,3,3,4,4,5,5,6\t0\t7\tzero cap\n\
         4\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t60\t3,2,2,3,3,4,4,5,5,6\t10\t7\tlower above upper\n\
         5\t1\t100\t100\t0,9,10,19,20,29,30,39\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t60\t1,2,2,3,3,4,4,5,5,6\t10\t7\tfour stages\n\
         6\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3\t60\t1,2,2,3,3,4,4,5,5,6\t10\t7\tfour workforce\n\
         7\t1\t100\t100\t0,9,10,19,20,29,30,39,40,99\t0,9,10,19,20,29,30,39,40,99\t0,1,2,3,4\t60\t1,2,2,3,3,4,4,5,5,6\t10\t0\tno infomorph type\n"
    );
    let file = crate::authored::Dir::compiled_only()
        .load("x.tsv", &text, HEADER)
        .unwrap();
    let (rows, skipped) = file.parse_rows_logged(parse_def_row);
    let kept: Vec<i64> = rows.iter().map(|(t, _)| *t).collect();
    assert_eq!(kept, [1]);
    assert_eq!(skipped.len(), 6, "{skipped:?}");
}

#[test]
fn a_type_without_a_row_uses_the_default_row_and_none_without_one() {
    let t = fixture_table();
    assert!(t.get(DEN_TYPE).is_some(), "the 0 row is every other type");
    let empty = DenTable::default();
    assert!(empty.get(DEN_TYPE).is_none());
    assert!(empty.default_row().is_none());
}

// ---------------------------------------------------------------------------
// The row
// ---------------------------------------------------------------------------

#[test]
fn a_row_round_trips_and_an_older_thirteen_value_row_still_parses() {
    let mut reg = world();
    let den = den_at(5, NOW, 3);
    put(reg.deltas_mut(), &den);
    assert_eq!(by_id(reg.deltas(), den.id), Some(den));
    // The spec's thirteen values: `seen` is missing and reads as 0.
    let key = vec![den.id.to_string()];
    let mut vals = reg.deltas().of(KIND).get(&key).unwrap().clone();
    vals.truncate(13);
    reg.deltas_mut().put(KIND, key, vals);
    let old = by_id(reg.deltas(), den.id).unwrap();
    assert_eq!((old.seen, old.held), (0, false));
    // A fourteen-value row (before the GM-pause flag) reads as not held.
    assert_eq!(all(reg.deltas()).len(), 1);
}

// ---------------------------------------------------------------------------
// Deploy
// ---------------------------------------------------------------------------

#[test]
fn a_launch_makes_the_item_the_row_the_ball_and_tells_the_pilot() {
    let mut reg = world();
    let id = launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW).expect("launches");
    let den = by_id(reg.deltas(), id).expect("a row");
    assert_eq!(
        (den.owner, den.skyhook, den.planet, den.system),
        (ALICE, SK_LAND, P_LAND, SYSTEM)
    );
    assert!(den.enabled && !den.paused);
    assert_eq!((den.dev_base, den.dev_at, den.infomorphs), (0, NOW, 0));
    // The item is in space, owned by the pilot (not the corp).
    let i = reg.store().get(id).expect("the item");
    assert_eq!(
        (i.owner_id, i.location_id, i.flag_id),
        (ALICE, SYSTEM, FLAG_IN_SPACE)
    );
    // The ball and slim: the pilot is the slim's owner.
    let balls = balls_in(&reg, SYSTEM);
    assert_eq!(balls.len(), 1);
    assert_eq!(balls[0].0.ball_id, id);
    assert_eq!(balls[0].1.owner_id, ALICE);
    // The pilot is told, once, with the item id.
    let pushes = sent(&mut reg);
    let got: Vec<_> = pushes
        .iter()
        .filter(|(n, ..)| *n == "OnMercenaryDenRegistrationSuccess")
        .collect();
    assert_eq!(got.len(), 1, "{pushes:?}");
    assert_eq!(got[0].2, Some(vec![ALICE]));
    let PyValue::Dict(d) = &got[0].1[1] else {
        panic!("the args are a dict")
    };
    assert!(
        d.iter()
            .any(|(v, k)| *v == PyValue::Int(id) && *k == crate::pyutil::bstr("itemID")),
        "{d:?}"
    );
    // The text (728099) names the den by its type.
    assert_eq!(
        arg_of(&got[0].1[1], "typeID"),
        Some(PyValue::Int(DEN_TYPE)),
        "{d:?}"
    );
}

#[test]
fn any_owner_of_the_skyhook_will_do() {
    // Carol is in no alliance and not in the skyhook's corp.
    let mut reg = world();
    let id = launch_one_at(&mut reg, CAROL, CAROL_DEN, NOW).expect("launches");
    let den = by_id(reg.deltas(), id).unwrap();
    assert_eq!((den.owner, den.skyhook), (CAROL, SK_LAND));
    assert_eq!(skyhook::by_id(reg.deltas(), SK_LAND).unwrap().corp, CORP);
}

fn untouched(reg: &Registry, item: i64, who: i64) {
    let i = reg.store().get(item).expect("the item");
    assert_eq!(
        (i.owner_id, i.location_id, i.flag_id),
        (who, ship(who), crate::sov_hub::FLAG_CARGO),
        "a refused launch moves nothing"
    );
    assert!(all(reg.deltas()).is_empty(), "and writes no row");
}

#[test]
fn a_launch_is_refused_without_the_skill() {
    let mut reg = world();
    assert_eq!(
        launch_one_at(&mut reg, BOB, BOB_DEN, NOW),
        Err(Refused::NoSkill)
    );
    untouched(&reg, BOB_DEN, BOB);
    // A skill trained to 0 is none.
    skill(&mut reg, BOB, 0);
    assert_eq!(
        launch_one_at(&mut reg, BOB, BOB_DEN, NOW),
        Err(Refused::NoSkill)
    );
}

#[test]
fn a_launch_is_refused_past_the_skill_maximum() {
    let mut reg = world();
    // Two dens out, each by its own skyhook (one den to a skyhook).
    let mut dens = 0;
    for (item, planet, sk) in [
        (ALICE_DEN, P_LAND, SK_LAND),
        (ALICE_DEN2, P_LAND2, SK_LAND2),
    ] {
        at_spot(&mut reg, ALICE, planet, 5_000.0);
        launch_one_at(&mut reg, ALICE, item, NOW).expect("launches");
        assert_eq!(by_id(reg.deltas(), item).unwrap().skyhook, sk);
        dens += 1;
    }
    // Level 1 times the table's 2 per level is 2: the maximum is reached.
    skill(&mut reg, ALICE, 1);
    at_spot(&mut reg, ALICE, P_LAND, 5_000.0);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN3, NOW),
        Err(Refused::AtMaximum { max: 2, level: 1 })
    );
    assert_eq!(owned_by(reg.deltas(), ALICE).len(), dens);
    // Level 2 allows four: with the skill back, what stops a third at this
    // spot is that its skyhook has a den.
    skill(&mut reg, ALICE, 2);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN3, NOW),
        Err(Refused::Occupied)
    );
}

#[test]
fn a_launch_needs_a_skyhook_in_reach() {
    let mut reg = world();
    // 500 km off the nearest skyhook spot.
    let far = off(planet_at(&reg, P_LAND), 500_000.0);
    place_at(&mut reg, ALICE, far);
    let Err(Refused::TooFar { metres }) = launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW) else {
        panic!("too far")
    };
    assert!(metres as f64 > LAUNCH_RANGE);
    untouched(&reg, ALICE_DEN, ALICE);
    // No skyhook in the system at all.
    for sk in skyhook::all(reg.deltas()) {
        reg.deltas_mut().del(skyhook::KIND, &[sk.id.to_string()]);
    }
    at_spot(&mut reg, ALICE, P_LAND, 5_000.0);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW),
        Err(Refused::NoSkyhook)
    );
}

#[test]
fn a_launch_needs_the_skyhook_online_and_held() {
    let mut reg = world();
    stand(&mut reg, SK_LAND, P_LAND, CORP, false);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW),
        Err(Refused::SkyhookOffline)
    );
    // Switched on but its owner no longer holds the system.
    stand(&mut reg, SK_LAND, P_LAND, RIVAL, true);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW),
        Err(Refused::SkyhookOffline)
    );
    untouched(&reg, ALICE_DEN, ALICE);
}

#[test]
fn a_launch_is_refused_over_a_planet_that_gives_no_workforce() {
    let mut reg = world();
    for planet in [P_REAGENT, P_POWER] {
        at_spot(&mut reg, ALICE, planet, 5_000.0);
        assert_eq!(
            launch_one_at(&mut reg, ALICE, ALICE_DEN, NOW),
            Err(Refused::NotWorkforce),
            "planet {planet}"
        );
    }
    untouched(&reg, ALICE_DEN, ALICE);
}

#[test]
fn one_den_to_a_skyhook() {
    let mut reg = world();
    launched(&mut reg);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN2, NOW),
        Err(Refused::Occupied)
    );
    assert_eq!(all(reg.deltas()).len(), 1);
}

#[test]
fn the_plain_launch_problems_are_refused_before_any_den_rule() {
    let mut reg = world();
    // Not Alice's own item.
    assert_eq!(
        launch_one_at(&mut reg, ALICE, BOB_DEN, NOW),
        Err(Refused::NotInHold)
    );
    // Not a den.
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_HULL_ITEM, NOW),
        Err(Refused::NotADen)
    );
    // Not in space.
    let mut docked = world();
    let _ = docked.roster_mut().leave(ALICE);
    assert_eq!(
        launch_one_at(&mut docked, ALICE, ALICE_DEN, NOW),
        Err(Refused::NotInSpace)
    );
}

#[test]
fn a_den_rule_answers_under_the_clients_reason_key_the_rest_as_a_notify() {
    let rules = [
        (
            Refused::NoSkill,
            "MercenaryDen_DeploymentFailed_SkillLimits",
        ),
        (
            Refused::AtMaximum {
                max: 3,
                level: SKILL_TOP,
            },
            "MercenaryDen_DeploymentFailed_MaximumDensReached",
        ),
        (
            Refused::AtMaximum { max: 3, level: 1 },
            "MercenaryDen_DeploymentFailed_SkillLimits",
        ),
        (
            Refused::NoSkyhook,
            "MercenaryDen_DeploymentFailed_RequiresNearbySkyhook",
        ),
        (
            Refused::TooFar { metres: 9 },
            "MercenaryDen_DeploymentFailed_RequiresNearbySkyhook",
        ),
        (
            Refused::SkyhookOffline,
            "MercenaryDen_DeploymentFailed_RequiresOnlineSkyhook",
        ),
        (
            Refused::NotWorkforce,
            "MercenaryDen_DeploymentFailed_MustBeTemperatePlanet",
        ),
        (
            Refused::Occupied,
            "MercenaryDen_DeploymentFailed_SkyhookAlreadyOccupied",
        ),
    ];
    for (r, key) in rules {
        let PyValue::Tuple(t) = refusal(&r, DEN_TYPE, 0) else {
            panic!("a (label, args) tuple")
        };
        assert_eq!(t[0], crate::pyutil::bstr(key), "{r:?}");
    }
    for r in [Refused::NotInSpace, Refused::NotInHold, Refused::NotADen] {
        let PyValue::Tuple(t) = refusal(&r, DEN_TYPE, 0) else {
            panic!("a tuple")
        };
        assert_eq!(t[0], crate::pyutil::bstr("CustomNotify"), "{r:?}");
    }
}

fn arg_of(args: &PyValue, name: &str) -> Option<PyValue> {
    let PyValue::Dict(d) = args else {
        panic!("the args are a dict")
    };
    d.iter()
        .find(|(_, k)| *k == crate::pyutil::bstr(name))
        .map(|(v, _)| v.clone())
}

#[test]
fn every_keyed_refusal_carries_the_den_type_the_texts_name() {
    // [V] loc.tsv 728096-728102 and 870990 all read {[item]typeID.name}.
    for r in [
        Refused::NoSkill,
        Refused::AtMaximum { max: 2, level: 1 },
        Refused::NoSkyhook,
        Refused::TooFar { metres: 9 },
        Refused::SkyhookOffline,
        Refused::NotWorkforce,
        Refused::Occupied,
        Refused::NoDefinition,
    ] {
        let PyValue::Tuple(t) = refusal(&r, 9_001, 0) else {
            panic!("a tuple")
        };
        assert_eq!(arg_of(&t[1], "typeID"), Some(PyValue::Int(9_001)), "{r:?}");
    }
    // Occupied reads nothing else.
    let PyValue::Tuple(t) = refusal(&Refused::Occupied, 9_001, 0) else {
        panic!("a tuple")
    };
    let PyValue::Dict(d) = &t[1] else { panic!() };
    assert_eq!(d.len(), 1);
}

#[test]
fn the_skill_and_distance_refusals_carry_their_extra_args() {
    let trained = 0;
    let PyValue::Tuple(t) = refusal(&Refused::NoSkill, 9_001, trained) else {
        panic!("a tuple")
    };
    assert_eq!(
        arg_of(&t[1], "skillTypeID"),
        Some(PyValue::Int(SKILL_DEN_MANAGEMENT))
    );
    assert_eq!(
        arg_of(&t[1], "level"),
        Some(PyValue::Int(i64::from(trained) + 1))
    );
    for r in [Refused::NoSkyhook, Refused::TooFar { metres: 9 }] {
        let PyValue::Tuple(t) = refusal(&r, 9_001, 0) else {
            panic!("a tuple")
        };
        assert_eq!(
            arg_of(&t[1], "minDistance"),
            Some(PyValue::Real(LAUNCH_RANGE)),
            "{r:?}"
        );
    }
}

#[test]
fn a_pilot_at_the_cap_is_told_the_next_level_until_the_top_level() {
    let below = Refused::AtMaximum { max: 4, level: 2 };
    let PyValue::Tuple(t) = refusal(&below, 9_001, 2) else {
        panic!("a tuple")
    };
    assert_eq!(
        t[0],
        crate::pyutil::bstr("MercenaryDen_DeploymentFailed_SkillLimits")
    );
    assert_eq!(
        arg_of(&t[1], "skillTypeID"),
        Some(PyValue::Int(SKILL_DEN_MANAGEMENT))
    );
    assert_eq!(arg_of(&t[1], "level"), Some(PyValue::Int(3)));
    let top = Refused::AtMaximum {
        max: 10,
        level: SKILL_TOP,
    };
    let PyValue::Tuple(t) = refusal(&top, 9_001, SKILL_TOP) else {
        panic!("a tuple")
    };
    assert_eq!(
        t[0],
        crate::pyutil::bstr("MercenaryDen_DeploymentFailed_MaximumDensReached")
    );
    assert_eq!(arg_of(&t[1], "level"), None);
    assert_eq!(arg_of(&t[1], "typeID"), Some(PyValue::Int(9_001)));
}

#[test]
fn the_refused_drop_answer_names_the_den_type() {
    let mut reg = world();
    let args = drop_args(BOB_DEN);
    let got = drop_call(&mut reg, &drop_request(&args, Some(BOB)))
        .unwrap()
        .unwrap();
    let PyValue::Tuple(t) = &entries_of(got)[0] else {
        panic!("a refusal")
    };
    assert_eq!(arg_of(&t[1], "typeID"), Some(PyValue::Int(DEN_TYPE)));
}

fn drop_request<'a>(args: &'a [PyValue], actor: Option<i64>) -> Request<'a> {
    Request {
        service: "ship",
        method: "Drop",
        args,
        kwargs: &[],
        bind_params: None,
        actor,
    }
}

fn drop_args(item: i64) -> [PyValue; 3] {
    [
        PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(item),
            PyValue::Int(1),
        ])]),
        PyValue::None,
        PyValue::Bool(false),
    ]
}

fn entries_of(got: PyValue) -> Vec<PyValue> {
    let PyValue::Dict(pairs) = got else {
        panic!("a dict")
    };
    assert_eq!(pairs.len(), 1);
    let PyValue::List(entries) = &pairs[0].0 else {
        panic!("a list of one entry")
    };
    entries.clone()
}

#[test]
fn the_drop_link_answers_only_for_a_den_and_in_the_launch_shape() {
    let mut reg = world();
    let args = drop_args(ALICE_DEN);
    let got = drop_call(&mut reg, &drop_request(&args, Some(ALICE)))
        .expect("a den is ours")
        .expect("no error");
    assert_eq!(
        entries_of(got),
        vec![PyValue::Int(ALICE_DEN)],
        "a whole-item move keeps its id"
    );
    assert!(by_id(reg.deltas(), ALICE_DEN).is_some());
    // A refusal is a (label, args) entry, not an id.
    let args = drop_args(BOB_DEN);
    let got = drop_call(&mut reg, &drop_request(&args, Some(BOB)))
        .unwrap()
        .unwrap();
    assert!(matches!(&entries_of(got)[0], PyValue::Tuple(t) if t.len() == 2));
    // No character: a refusal too.
    let args = drop_args(DAVE_DEN);
    let got = drop_call(&mut reg, &drop_request(&args, None))
        .unwrap()
        .unwrap();
    assert!(matches!(&entries_of(got)[0], PyValue::Tuple(_)));
    // Anything that is not a den is not ours.
    let other = drop_args(ALICE_HULL_ITEM);
    assert!(drop_call(&mut reg, &drop_request(&other, Some(ALICE))).is_none());
    let nothing = drop_args(999);
    assert!(drop_call(&mut reg, &drop_request(&nothing, Some(ALICE))).is_none());
}

// ---------------------------------------------------------------------------
// The gateway
// ---------------------------------------------------------------------------

#[test]
fn get_as_owner_gives_the_five_attributes_and_the_flags() {
    let mut reg = world();
    let id = launched(&mut reg);
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    assert_eq!(r.status, 200);
    assert_eq!(
        r.payload.as_ref().map(|p| p.0.as_str()),
        Some("eve_public.sovereignty.mercenaryden.api.GetAsOwnerResponse")
    );
    let b = payload(&r);
    let attrs = get_bytes(b, 1).expect("attributes");
    let seq = |n: u64| get_bytes(&attrs, n).and_then(|m| get_uint(&m, 1));
    assert_eq!(seq(1), Some(ALICE as u64), "owner");
    assert_eq!(seq(2), Some(SK_LAND as u64), "skyhook");
    assert_eq!(seq(3), Some(SYSTEM as u64), "solar system");
    assert_eq!(seq(4), Some(P_LAND as u64), "planet");
    assert_eq!(seq(5), Some(DEN_TYPE as u64), "type");
    assert_eq!(get_uint(b, 2), Some(1), "enabled");
    assert_eq!(get_uint(b, 5), Some(1), "cargo extraction enabled");
    // The skyhook's owner is a corporation identifier.
    let corp = get_bytes(b, 6).and_then(|m| get_uint(&m, 1));
    assert_eq!(corp, Some(CORP as u64));
}

#[test]
fn get_as_owner_carries_the_stored_base_and_the_time_it_was_taken() {
    let mut reg = world();
    let id = launched(&mut reg);
    // Development level 4 taken at NOW - 777; anarchy level 2 taken at NOW - 555.
    let mut den = by_id(reg.deltas(), id).unwrap();
    den.dev_base = 4;
    den.dev_at = NOW - 777;
    den.anarchy_base = 2;
    den.anarchy_at = NOW - 555;
    put(reg.deltas_mut(), &den);
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    let evo = get_bytes(payload(&r), 3).expect("evolution");
    let sim = get_bytes(&evo, 2).expect("simulation");
    assert_eq!(get_uint(&sim, 2), Some(1), "started, not paused");
    assert_eq!(get_uint(&sim, 1), None);
    let dev = get_bytes(&sim, 3).expect("development");
    assert_eq!(get_uint(&dev, 1), Some(4), "the base level");
    let at = get_bytes(&dev, 4).and_then(|t| get_uint(&t, 1));
    assert_eq!(at, Some(NOW - 777), "simulated_at");
    assert_eq!(get_bytes(&dev, 3), None, "not paused_at");
    let an = get_bytes(&sim, 4).expect("anarchy");
    assert_eq!(get_uint(&an, 1), Some(2));
    assert_eq!(
        get_bytes(&an, 4).and_then(|t| get_uint(&t, 1)),
        Some(NOW - 555)
    );
}

#[test]
fn a_paused_den_sends_paused_and_paused_at_not_started() {
    let mut reg = world();
    let id = launched(&mut reg);
    set_paused(&mut reg, id, true, NOW + 500).expect("paused");
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    let sim = get_bytes(&get_bytes(payload(&r), 3).unwrap(), 2).unwrap();
    assert_eq!(get_uint(&sim, 1), Some(1), "paused");
    assert_eq!(get_uint(&sim, 2), None, "not started");
    let dev = get_bytes(&sim, 3).unwrap();
    assert_eq!(
        get_bytes(&dev, 3).and_then(|t| get_uint(&t, 1)),
        Some(NOW + 500)
    );
    assert_eq!(get_bytes(&dev, 4), None, "no simulated_at while paused");
}

#[test]
fn the_definitions_name_five_stages_and_the_infomorph_bands() {
    let mut reg = world();
    let id = launched(&mut reg);
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    let b = payload(&r);
    let def = fixture_table().get(0).unwrap().clone();
    // Evolution definition.
    let evo = get_bytes(b, 3).unwrap();
    let ed = get_bytes(&evo, 1).unwrap();
    let dev = get_bytes(&ed, 1).unwrap();
    assert_eq!(get_uint(&dev, 1), Some(def.dev_unit_s));
    let stages = field(&dev, 2);
    assert_eq!(stages.len(), 5);
    for (i, s) in stages.iter().enumerate() {
        assert_eq!(get_uint(s, 1), Some(i as u64 + 1), "stage enum");
        assert_eq!(get_uint(s, 2), Some(u64::from(def.dev_bands[i].0)));
        assert_eq!(get_uint(s, 3), Some(u64::from(def.dev_bands[i].1)));
        assert_eq!(get_uint(s, 4), None, "development has no workforce");
    }
    let an = get_bytes(&ed, 2).unwrap();
    assert_eq!(get_uint(&an, 1), Some(def.anarchy_unit_s));
    for (i, s) in field(&an, 2).iter().enumerate() {
        assert_eq!(get_uint(s, 4), Some(def.anarchy_workforce[i]));
    }
    // Infomorphs.
    let inf = get_bytes(b, 4).unwrap();
    let idf = get_bytes(&inf, 1).unwrap();
    assert_eq!(
        get_bytes(&idf, 1).and_then(|m| get_uint(&m, 1)),
        Some(FIXTURE_INFOMORPH as u64)
    );
    assert_eq!(get_uint(&idf, 2), Some(def.gen_tick_s));
    let bands = field(&idf, 3);
    assert_eq!(bands.len(), 5);
    for (i, s) in bands.iter().enumerate() {
        assert_eq!(get_uint(s, 1), Some(i as u64 + 1));
        assert_eq!(get_uint(s, 2), Some(u64::from(def.gen_bands[i].0)));
        assert_eq!(get_uint(s, 3), Some(u64::from(def.gen_bands[i].1)));
    }
    assert_eq!(get_uint(&idf, 4), Some(u64::from(def.cargo_cap)));
    let contents = get_bytes(&inf, 2).unwrap();
    assert_eq!(get_uint(&contents, 1), Some(0));
    assert!(get_bytes(&contents, 2).is_some(), "last_generation_tick");
}

#[test]
fn get_as_owner_shows_what_has_been_generated_since_without_writing_it() {
    let mut reg = world();
    let id = launched(&mut reg);
    // The row was written long ago on the wall clock, so by now the den is full.
    let long_ago = 1_000_000_000;
    let mut den = by_id(reg.deltas(), id).unwrap();
    den.dev_at = long_ago;
    den.last_gen = long_ago;
    put(reg.deltas_mut(), &den);
    let def = def_for(&reg, &den).unwrap();
    assert!(skyhook::now() > long_ago + 1_000 * def.gen_tick_s);
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    let contents = get_bytes(&get_bytes(payload(&r), 4).unwrap(), 2).unwrap();
    assert_eq!(get_uint(&contents, 1), Some(u64::from(def.cargo_cap)));
    // A read does not write.
    let stored = by_id(world_of(&g).deltas(), id).unwrap();
    assert_eq!((stored.infomorphs, stored.last_gen), (0, long_ago));
}

#[test]
fn another_pilots_or_an_unknown_den_is_404() {
    let mut reg = world();
    let id = launched(&mut reg);
    let g = ground(reg);
    assert_eq!(ask(&g, "GetAsOwnerRequest", &id_body(id), BOB).status, 404);
    assert_eq!(ask(&g, "GetAsOwnerRequest", &id_body(id), DAVE).status, 404);
    assert_eq!(
        ask(&g, "GetAsOwnerRequest", &id_body(77), ALICE).status,
        404
    );
    // A request with no id at all.
    assert_eq!(ask(&g, "GetAsOwnerRequest", &[], ALICE).status, 404);
    // Alice's own is there.
    assert_eq!(
        ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE).status,
        200
    );
}

#[test]
fn get_all_owned_lists_the_pilots_own_dens_only() {
    let mut reg = world();
    let a = launched(&mut reg);
    at_spot(&mut reg, ALICE, P_LAND2, 5_000.0);
    let a2 = launch_one_at(&mut reg, ALICE, ALICE_DEN2, NOW).unwrap();
    // Carol's den is by a skyhook of its own.
    stand(&mut reg, 8_830_005, P_NAKED, CORP, true);
    at_spot(&mut reg, CAROL, P_NAKED, 5_000.0);
    let c = launch_one_at(&mut reg, CAROL, CAROL_DEN, NOW).unwrap();
    let g = ground(reg);
    let ids = |who: i64| -> Vec<u64> {
        let r = ask(&g, "GetAllOwnedRequest", &[], who);
        assert_eq!(r.status, 200);
        field(payload(&r), 1)
            .iter()
            .filter_map(|m| get_uint(m, 1))
            .collect()
    };
    assert_eq!(ids(ALICE), [a as u64, a2 as u64]);
    assert_eq!(ids(CAROL), [c as u64]);
    assert!(ids(BOB).is_empty(), "an empty list is a 200");
}

#[test]
fn the_maximum_follows_the_skill_level() {
    let g = ground(world());
    let per = u64::from(fixture_table().get(0).unwrap().dens_per_level);
    // Alice trained level 2.
    let r = ask(&g, "GetMaximumForCharacterRequest", &[], ALICE);
    assert_eq!(r.status, 200);
    assert_eq!(get_uint(payload(&r), 1), Some(2 * per));
    assert_eq!(get_uint(payload(&r), 2), Some(u64::from(SKILL_TOP) * per));
    // Carol level 1.
    let r = ask(&g, "GetMaximumForCharacterRequest", &[], CAROL);
    assert_eq!(get_uint(payload(&r), 1), Some(per));
    // No skill: none now, the top is still the top.
    let r = ask(&g, "GetMaximumForCharacterRequest", &[], BOB);
    assert_eq!(r.status, 200);
    assert_eq!(get_uint(payload(&r), 1).unwrap_or(0), 0);
    assert_eq!(get_uint(payload(&r), 2), Some(u64::from(SKILL_TOP) * per));
}

#[test]
fn the_arm_owns_three_requests_and_leaves_the_rest() {
    let g = Ground::local();
    for name in ["GetAsOwnerRequest", "GetAllOwnedRequest"] {
        assert!(gateway(&req(name), &[], ALICE, &g).is_some(), "{name}");
    }
    // The maximum needs the world: with none it falls through to the stub.
    assert!(gateway(&req("GetMaximumForCharacterRequest"), &[], ALICE, &g).is_none());
    assert_eq!(
        gateway(&req("GetAsOwnerRequest"), &id_body(1), ALICE, &g).map(|r| r.status),
        Some(404),
        "no world, no den"
    );
    // The activity messages are `activity::gateway`'s (its own tests);
    // a skyhook request is the skyhook's.
    for ty in [
        "eve_public.sovereignty.skyhook.api.GetRequest",
        "eve_public.sovereignty.mercenaryden.api.admin.ModifyAnarchyLevelRequest",
    ] {
        assert!(gateway(ty, &[], ALICE, &g).is_none(), "{ty}");
    }
}

#[test]
fn the_real_chain_serves_the_den_requests_before_the_stub() {
    let mut reg = world();
    let id = launched(&mut reg);
    let g = ground(reg);
    let ty = req("GetAsOwnerRequest");
    let r = crate::gateway_ext::handle(&ty, &id_body(id), ALICE, &g).expect("answered");
    assert_eq!(r.status, 200, "the stub would answer 404");
}

// ---------------------------------------------------------------------------
// Evolution: pause, resume, the clock stage
// ---------------------------------------------------------------------------

#[test]
fn a_pause_freezes_the_levels_where_they_are_and_a_resume_counts_from_then() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    let paused_at = NOW + 5 * def.dev_unit_s + 3;
    let den = set_paused(&mut reg, id, true, paused_at).unwrap();
    assert!(den.paused);
    assert_eq!(den.dev_base, 5, "five whole units had passed");
    assert_eq!(den.dev_at, paused_at, "paused_at");
    // Frozen however long it stays so.
    assert_eq!(den.dev_level(&def, paused_at + 100 * def.dev_unit_s), 5);
    // Pausing again changes nothing.
    assert_eq!(set_paused(&mut reg, id, true, paused_at + 9_999), Some(den));
    // Resume 10 units later: the count starts from the resume.
    let resume = paused_at + 10 * def.dev_unit_s;
    let den = set_paused(&mut reg, id, false, resume).unwrap();
    assert!(!den.paused);
    assert_eq!(den.dev_level(&def, resume), 5);
    assert_eq!(den.dev_level(&def, resume + 2 * def.dev_unit_s), 7);
}

#[test]
fn a_pause_and_a_resume_are_told_to_the_owner_with_the_new_state() {
    let mut reg = world();
    let id = launched(&mut reg);
    let _ = notices(&mut reg);
    set_paused(&mut reg, id, true, NOW + 10).unwrap();
    let got = notices(&mut reg);
    assert_eq!(got.len(), 1);
    let (ty, body, target) = &got[0];
    assert_eq!(
        ty,
        "eve_public.sovereignty.mercenaryden.api.EvolutionStateChangedNotice"
    );
    assert!(matches!(target, Target::Character(c) if *c == ALICE));
    assert_eq!(
        get_bytes(body, 1).and_then(|m| get_uint(&m, 1)),
        Some(id as u64)
    );
    let sim = get_bytes(body, 2).expect("state");
    assert_eq!(get_uint(&sim, 1), Some(1), "paused");
    set_paused(&mut reg, id, false, NOW + 20).unwrap();
    let got = notices(&mut reg);
    let sim = get_bytes(&got[0].1, 2).unwrap();
    assert_eq!(get_uint(&sim, 2), Some(1), "started");
    // No change, no notice.
    set_paused(&mut reg, id, false, NOW + 30).unwrap();
    assert!(notices(&mut reg).is_empty());
}

#[test]
fn the_stage_stage_tells_a_change_of_stage_once() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    let _ = notices(&mut reg);
    // Nine levels in: still stage 1, nothing to say.
    tick_at(&mut reg, NOW + 9 * def.dev_unit_s);
    assert!(notices(&mut reg).is_empty());
    // The tenth level is stage 2: told once.
    tick_at(&mut reg, NOW + 10 * def.dev_unit_s);
    let got = notices(&mut reg);
    assert_eq!(got.len(), 1, "one notice");
    let sim = get_bytes(&got[0].1, 2).unwrap();
    let dev = get_bytes(&sim, 3).unwrap();
    assert_eq!(get_uint(&dev, 2), Some(2), "the new stage");
    tick_at(&mut reg, NOW + 10 * def.dev_unit_s + 1);
    assert!(notices(&mut reg).is_empty(), "and not again");
    assert_eq!(by_id(reg.deltas(), id).unwrap().seen, 21);
}

#[test]
fn the_stage_settles_the_infomorphs_into_the_row() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    tick_at(&mut reg, NOW + 3 * def.gen_tick_s);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(den.infomorphs >= 3, "at least one a tick at stage 1");
    assert_eq!(den.last_gen, NOW + 3 * def.gen_tick_s);
    // No den rows: no work, no panic.
    let mut empty = world();
    tick_at(&mut empty, NOW);
}

#[test]
fn a_den_follows_its_skyhook_off_and_on() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    // Three levels and some ticks in, the skyhook goes off.
    let off_at = NOW + 3 * def.dev_unit_s;
    stand(&mut reg, SK_LAND, P_LAND, CORP, false);
    tick_at(&mut reg, off_at);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(!den.enabled && den.paused);
    assert_eq!(den.dev_base, 3, "frozen at the level it had");
    let held = den.infomorphs;
    assert!(held > 0, "what it made before it stopped is kept");
    // A long time off: nothing more, no level.
    tick_at(&mut reg, off_at + 500 * def.dev_unit_s);
    let den = by_id(reg.deltas(), id).unwrap();
    assert_eq!((den.infomorphs, den.dev_base), (held, 3));
    // Back on: it resumes from now, with no burst for the time it was off.
    let on_at = off_at + 500 * def.dev_unit_s + 7;
    stand(&mut reg, SK_LAND, P_LAND, CORP, true);
    tick_at(&mut reg, on_at);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(den.enabled && !den.paused);
    assert_eq!(den.infomorphs, held, "no burst");
    assert_eq!(den.last_gen, on_at);
    assert_eq!(den.dev_level(&def, on_at), 3);
    assert_eq!(den.dev_level(&def, on_at + def.dev_unit_s), 4);
}

#[test]
fn a_den_whose_skyhook_is_destroyed_is_disabled() {
    let mut reg = world();
    let id = launched(&mut reg);
    reg.deltas_mut().del(skyhook::KIND, &[SK_LAND.to_string()]);
    tick_at(&mut reg, NOW + 10);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(!den.enabled && den.paused);
    // The window shows it as off, with extraction off.
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    assert_eq!(get_uint(payload(&r), 2), Some(0), "enabled");
    assert_eq!(
        get_uint(payload(&r), 5),
        Some(0),
        "cargo extraction enabled"
    );
    assert_eq!(
        get_bytes(payload(&r), 6).and_then(|m| get_uint(&m, 1)),
        Some(0),
        "no skyhook owner left"
    );
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

/// The clock stage's pushes leave on the clock, as structure_space's clock pushes do (#44): `pending` drains
/// only after a client's call.
#[test]
fn a_den_removed_on_the_clock_is_scheduled_not_pushed() {
    let mut reg = world();
    let id = launched(&mut reg);
    let _ = reg.take_pending_raw();
    reg.apply_items_quiet(&[Edit::Destroy { item_id: id }])
        .expect("destroyed");
    tick(&mut reg, std::time::Instant::now(), 0);
    assert!(by_id(reg.deltas(), id).is_none(), "the row is gone");
    let removes =
        |p: &dyn Push| matches!(p.grid(), crate::grid::GridRole::Removes(ids) if ids.contains(&id));
    let pending = reg.take_pending_raw();
    assert!(
        pending
            .iter()
            .all(|(_, p)| p.to_push().0 != "OnMyMercenaryDenRemoved" && !removes(p.as_ref())),
        "nothing waits for somebody's call"
    );
    let due = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60));
    let told = due
        .iter()
        .find(|p| p.to_push().0 == "OnMyMercenaryDenRemoved")
        .expect("the clock sends it");
    assert!(told.audience().is_some_and(|t| t.contains(&ALICE)));
    let gone = due
        .iter()
        .find(|p| removes(p.as_ref()))
        .expect("the ball goes too");
    assert!(gone.audience().is_some_and(|t| t.contains(&ALICE)));
}

#[test]
fn a_den_whose_item_is_gone_is_removed_and_the_owner_told() {
    let mut reg = world();
    let id = launched(&mut reg);
    let _ = sent(&mut reg);
    reg.apply_items_quiet(&[Edit::Destroy { item_id: id }])
        .expect("destroyed");
    tick_at(&mut reg, NOW + 10);
    assert!(by_id(reg.deltas(), id).is_none(), "the row is gone");
    assert!(balls_in(&reg, SYSTEM).is_empty(), "and the ball");
    let pushes = sent(&mut reg);
    let got: Vec<_> = pushes
        .iter()
        .filter(|(n, ..)| *n == "OnMyMercenaryDenRemoved")
        .collect();
    assert_eq!(got.len(), 1, "{pushes:?}");
    assert_eq!(got[0].2, Some(vec![ALICE]));
    let PyValue::Dict(d) = &got[0].1[1] else {
        panic!("a dict")
    };
    assert!(
        d.iter()
            .any(|(v, k)| *v == PyValue::Int(id) && *k == crate::pyutil::bstr("itemID"))
    );
    // Once.
    tick_at(&mut reg, NOW + 20);
    assert!(
        sent(&mut reg)
            .iter()
            .all(|(n, ..)| *n != "OnMyMercenaryDenRemoved")
    );
    // The skyhook can have another den now.
    assert!(launch_one_at(&mut reg, ALICE, ALICE_DEN2, NOW + 30).is_ok());
}

#[test]
fn a_den_scooped_out_of_space_is_removed_too() {
    let mut reg = world();
    let id = launched(&mut reg);
    // Back into the pilot's hold: still an item, no longer in space.
    reg.apply_items_quiet(&[Edit::Move {
        item_id: id,
        to: Slot::new(ALICE, ship(ALICE), crate::sov_hub::FLAG_CARGO),
    }])
    .expect("moved");
    tick_at(&mut reg, NOW + 10);
    assert!(by_id(reg.deltas(), id).is_none());
    // The item is the pilot's again and is not destroyed.
    assert!(reg.store().get(id).is_some(), "a scooped den is kept");
}

// ---------------------------------------------------------------------------
// The ball and the slim
// ---------------------------------------------------------------------------

#[test]
fn the_den_stands_by_its_skyhook_and_carries_component_activate() {
    let mut reg = world();
    let id = launched(&mut reg);
    let den = by_id(reg.deltas(), id).unwrap();
    let at = position(&reg, &den).expect("a place");
    let spot = planet_at(&reg, P_LAND);
    assert!(
        crate::space_state::distance(at, spot) > 0.0
            && crate::space_state::distance(at, spot) < LAUNCH_RANGE,
        "near its skyhook's spot, not on it"
    );
    let extras = slim_extras(&reg, &[id, 12_345]);
    assert_eq!(extras.len(), 1, "only the den's own id");
    assert_eq!(extras[0].0, id);
    let (key, value) = &extras[0].1[0];
    assert_eq!(*key, "component_activate");
    assert!(matches!(value, PyValue::Tuple(t) if t[0] == PyValue::Bool(true)));
    // The ball target for approach and warp.
    let t = target(&reg, id).expect("a target");
    assert_eq!(t.position, at);
    assert!(target(&reg, 12_345).is_none());
    // Another system has none.
    assert!(balls_in(&reg, SYSTEM + 1).is_empty());
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn call(actor: i64, item: i64, args: &[PyValue]) -> CompCall<'_> {
    CompCall {
        actor,
        system: SYSTEM,
        item_id: item,
        args: crate::service::args::Args::of(args),
        kwargs: &[],
    }
}

/// A den holding `n` infomorphs, Alice at its surface.
fn stocked(reg: &mut Registry, n: u32) -> i64 {
    let id = launched(reg);
    let mut den = by_id(reg.deltas(), id).unwrap();
    den.infomorphs = n;
    den.last_gen = NOW;
    put(reg.deltas_mut(), &den);
    let at = position(reg, &den).unwrap();
    let r = radius(reg, id);
    for who in [ALICE, BOB, DAVE] {
        place_at(reg, who, [at[0] + r + 500.0, at[1], at[2]]);
    }
    id
}

fn args(flag: i64, qty: i64) -> [PyValue; 2] {
    [PyValue::Int(flag), PyValue::Int(qty)]
}

fn raised(r: Result<PyValue, ServiceError>) -> bool {
    matches!(r, Err(ServiceError::Raise(_)))
}

#[test]
fn extraction_moves_the_count_into_the_hold_and_out_of_the_den() {
    let mut reg = world();
    let id = stocked(&mut reg, 9);
    let a = args(crate::sov_hub::FLAG_CARGO, 4);
    let got = extract_at(&mut reg, &call(ALICE, id, &a), NOW).expect("takes");
    assert_eq!(got, PyValue::Int(4));
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_INFOMORPH), 4);
    assert_eq!(by_id(reg.deltas(), id).unwrap().infomorphs, 5);
    // The rest, and then nothing.
    let a = args(crate::sov_hub::FLAG_CARGO, 5);
    assert_eq!(
        extract_at(&mut reg, &call(ALICE, id, &a), NOW).unwrap(),
        PyValue::Int(5)
    );
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_INFOMORPH), 9);
    let a = args(crate::sov_hub::FLAG_CARGO, 1);
    assert!(raised(extract_at(&mut reg, &call(ALICE, id, &a), NOW)));
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_INFOMORPH), 9, "nothing more");
}

#[test]
fn extraction_is_clipped_to_the_count_held() {
    let mut reg = world();
    let id = stocked(&mut reg, 7);
    let a = args(crate::sov_hub::FLAG_CARGO, 100);
    assert_eq!(
        extract_at(&mut reg, &call(ALICE, id, &a), NOW).unwrap(),
        PyValue::Int(7)
    );
    assert_eq!(by_id(reg.deltas(), id).unwrap().infomorphs, 0);
}

#[test]
fn extraction_settles_what_was_made_since_the_row_was_written() {
    let mut reg = world();
    let id = stocked(&mut reg, 0);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    let want = projected(
        &by_id(reg.deltas(), id).unwrap(),
        &def,
        NOW + 4 * def.gen_tick_s,
    );
    assert!(want >= 4);
    let a = args(crate::sov_hub::FLAG_CARGO, 1_000);
    let got = extract_at(&mut reg, &call(ALICE, id, &a), NOW + 4 * def.gen_tick_s).unwrap();
    assert_eq!(got, PyValue::Int(i64::from(want)));
}

#[test]
fn extraction_is_clipped_to_the_room_in_the_hold() {
    let mut reg = world();
    let id = stocked(&mut reg, 30);
    // Her own den would fill the hold; it is not what this test is about.
    reg.apply_items_quiet(&[Edit::Destroy { item_id: CAROL_DEN }])
        .expect("destroyed");
    // Carol's hold takes three units of volume 1; she must own the den.
    let mut den = by_id(reg.deltas(), id).unwrap();
    den.owner = CAROL;
    put(reg.deltas_mut(), &den);
    let at = position(&reg, &den).unwrap();
    let r = radius(&reg, id);
    place_at(&mut reg, CAROL, [at[0] + r + 500.0, at[1], at[2]]);
    let a = args(crate::sov_hub::FLAG_CARGO, 30);
    assert_eq!(
        extract_at(&mut reg, &call(CAROL, id, &a), NOW).unwrap(),
        PyValue::Int(3)
    );
    assert_eq!(by_id(reg.deltas(), id).unwrap().infomorphs, 27);
    // The hold is full: refused, not a silent zero.
    assert!(raised(extract_at(&mut reg, &call(CAROL, id, &a), NOW)));
}

#[test]
fn extraction_is_refused_to_a_stranger_far_off_a_disabled_den_and_a_bad_flag() {
    let mut reg = world();
    let id = stocked(&mut reg, 9);
    let a = args(crate::sov_hub::FLAG_CARGO, 2);
    // Not the owner: Bob is beside it, in Alice's corp.
    assert!(raised(extract_at(&mut reg, &call(BOB, id, &a), NOW)));
    // Too far: 50 km.
    let den = by_id(reg.deltas(), id).unwrap();
    let at = position(&reg, &den).unwrap();
    place_at(&mut reg, ALICE, [at[0] + 50_000.0, at[1], at[2]]);
    assert!(raised(extract_at(&mut reg, &call(ALICE, id, &a), NOW)));
    let r = radius(&reg, id);
    place_at(&mut reg, ALICE, [at[0] + r + 500.0, at[1], at[2]]);
    // A flag that is not the cargo or the infrastructure hangar, and qty 0.
    let bad = args(11, 2);
    assert!(raised(extract_at(&mut reg, &call(ALICE, id, &bad), NOW)));
    let zero = args(crate::sov_hub::FLAG_CARGO, 0);
    assert!(raised(extract_at(&mut reg, &call(ALICE, id, &zero), NOW)));
    // A den that is off.
    let mut off = by_id(reg.deltas(), id).unwrap();
    off.enabled = false;
    put(reg.deltas_mut(), &off);
    assert!(raised(extract_at(&mut reg, &call(ALICE, id, &a), NOW)));
    // An unknown den.
    assert!(raised(extract_at(&mut reg, &call(ALICE, 4_242, &a), NOW)));
    assert_eq!(
        cargo_of(&reg, ALICE, FIXTURE_INFOMORPH),
        0,
        "nothing was made"
    );
    assert_eq!(by_id(reg.deltas(), id).unwrap().infomorphs, 9, "nor taken");
}

#[test]
fn the_component_call_is_routed_through_the_real_table() {
    let mut reg = world();
    let id = stocked(&mut reg, 6);
    let args = [
        PyValue::Int(id),
        crate::pyutil::bstr(MERCENARY_DEN),
        crate::pyutil::bstr("ExtractInfomorphsToShip"),
        PyValue::Int(crate::sov_hub::FLAG_CARGO),
        PyValue::Int(2),
    ];
    let r = Request {
        service: "beyonce",
        method: "CallComponentFromClient",
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: Some(ALICE),
    };
    let got = crate::space_component::call(&mut reg, &r).expect("routed");
    assert_eq!(got, PyValue::Int(2));
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_INFOMORPH), 2);
    // A den in another system is not in the caller's.
    assert!(locate(&reg, id).is_some_and(|s| s == SYSTEM));
    assert!(locate(&reg, 4_242).is_none());
}

// ---------------------------------------------------------------------------
// The GM line
// ---------------------------------------------------------------------------

fn gm(reg: &mut Registry, line: &str, now: u64) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    slash_cmd::run(reg, Some(ALICE), &words, now)
}

#[test]
fn age_moves_both_simulated_at_back_so_the_levels_rise() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    let before = by_id(reg.deltas(), id).unwrap();
    let text = gm(&mut reg, &format!("age {id} 10"), NOW);
    assert!(text.contains("moved back 10 h"), "{text}");
    let after = by_id(reg.deltas(), id).unwrap();
    assert_eq!(after.dev_at, before.dev_at - 10 * 3_600);
    assert_eq!(after.anarchy_at, before.anarchy_at - 10 * 3_600);
    assert_eq!(
        after.dev_level(&def, NOW),
        before.dev_level(&def, NOW) + 10 * 3_600 / def.dev_unit_s
    );
    // The wire carries the new time, so the client's own extrapolation rises.
    let g = ground(reg);
    let r = ask(&g, "GetAsOwnerRequest", &id_body(id), ALICE);
    let dev = get_bytes(
        &get_bytes(&get_bytes(payload(&r), 3).unwrap(), 2).unwrap(),
        3,
    )
    .unwrap();
    assert_eq!(
        get_bytes(&dev, 4).and_then(|t| get_uint(&t, 1)),
        Some(after.dev_at)
    );
}

#[test]
fn fill_sets_the_cap_and_info_reports_it() {
    let mut reg = world();
    let id = launched(&mut reg);
    let cap = def_for(&reg, &by_id(reg.deltas(), id).unwrap())
        .unwrap()
        .cargo_cap;
    let text = gm(&mut reg, &format!("fill {id}"), NOW);
    assert!(text.contains(&format!("{cap} infomorphs")), "{text}");
    assert_eq!(by_id(reg.deltas(), id).unwrap().infomorphs, cap);
    let info = gm(&mut reg, &format!("info {id}"), NOW);
    assert!(info.contains(&format!("infomorphs {cap}/{cap}")), "{info}");
    assert!(info.contains(&format!("owner {ALICE}")), "{info}");
}

#[test]
fn pause_resume_and_remove_work_and_unknown_ids_are_said() {
    let mut reg = world();
    let id = launched(&mut reg);
    assert!(gm(&mut reg, &format!("pause {id}"), NOW + 5).contains("paused"));
    assert!(by_id(reg.deltas(), id).unwrap().paused);
    assert!(gm(&mut reg, &format!("resume {id}"), NOW + 6).contains("resumed"));
    assert!(!by_id(reg.deltas(), id).unwrap().paused);
    assert!(gm(&mut reg, "pause 4242", NOW).contains("no mercenary den"));
    assert!(gm(&mut reg, &format!("remove {id}"), NOW + 7).contains("destroyed"));
    assert!(by_id(reg.deltas(), id).is_none());
    assert!(reg.store().get(id).is_none());
}

#[test]
fn go_puts_the_gm_beside_the_den_and_a_bad_line_gives_the_usage() {
    let mut reg = world();
    let id = launched(&mut reg);
    let far = off(planet_at(&reg, P_LAND), 80_000.0);
    place_at(&mut reg, ALICE, far);
    let text = gm(&mut reg, &format!("go {id}"), NOW);
    assert!(text.contains("moved you"), "{text}");
    let den = by_id(reg.deltas(), id).unwrap();
    let at = position(&reg, &den).unwrap();
    let here = reg.roster().get(ALICE).unwrap().motion.position;
    assert!(
        crate::space_state::distance(here, at) < radius(&reg, id) + 1_000.0,
        "beside the den"
    );
    for bad in ["", "age", "age x 3", "age 1 0", "nonsense 1"] {
        assert!(gm(&mut reg, bad, NOW).starts_with("usage:"), "{bad:?}");
    }
}

// ---------------------------------------------------------------------------
// Round 2 (review findings)
// ---------------------------------------------------------------------------

#[test]
fn a_skyhook_rebuilt_over_the_planet_takes_the_den_over() {
    let mut reg = world();
    let id = launched(&mut reg);
    reg.deltas_mut().del(skyhook::KIND, &[SK_LAND.to_string()]);
    tick_at(&mut reg, NOW + 10);
    assert!(!by_id(reg.deltas(), id).unwrap().enabled, "off without it");
    // A new skyhook (a new id) stands over the same planet.
    let new_sk = SK_LAND + 90;
    stand(&mut reg, new_sk, P_LAND, CORP, true);
    tick_at(&mut reg, NOW + 20);
    let den = by_id(reg.deltas(), id).unwrap();
    assert_eq!(den.skyhook, new_sk, "re-attached");
    assert!(den.enabled && !den.paused, "and working again");
    // Still one den to the planet.
    at_spot(&mut reg, ALICE, P_LAND, 5_000.0);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN2, NOW + 30),
        Err(Refused::Occupied)
    );
}

#[test]
fn the_window_gets_the_projected_last_tick_with_the_projected_count() {
    let mut reg = world();
    let id = launched(&mut reg);
    let den = by_id(reg.deltas(), id).unwrap();
    let def = def_for(&reg, &den).unwrap();
    let now = NOW + 3 * def.gen_tick_s + 7;
    let r = response(&reg, &den, &def, now);
    let contents = get_bytes(&get_bytes(&r, 4).unwrap(), 2).unwrap();
    assert_eq!(
        get_uint(&contents, 1),
        Some(u64::from(projected(&den, &def, now)))
    );
    assert_eq!(
        get_bytes(&contents, 2),
        Some(timestamp(NOW + 3 * def.gen_tick_s)),
        "the tick the count includes, not the stored one"
    );
    assert_ne!(den.last_gen, NOW + 3 * def.gen_tick_s);
}

#[test]
fn the_launch_check_and_the_window_use_the_same_dens_per_level() {
    let mut c = content();
    let mut own = fixture_table().get(0).unwrap().clone();
    own.dens_per_level = 9;
    let mut def0 = own.clone();
    def0.dens_per_level = 2;
    c.insert(
        "sov mercenary dens",
        DenTable::new(vec![(0, def0), (DEN_TYPE, own)]),
    );
    let mut reg = world_with(c);
    for (item, planet) in [(ALICE_DEN, P_LAND), (ALICE_DEN2, P_LAND2)] {
        at_spot(&mut reg, ALICE, planet, 5_000.0);
        launch_one_at(&mut reg, ALICE, item, NOW).expect("launches");
    }
    skill(&mut reg, ALICE, 1);
    assert_eq!(maximum_for(&reg, ALICE).0, 2);
    at_spot(&mut reg, ALICE, P_LAND, 5_000.0);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_DEN3, NOW),
        Err(Refused::AtMaximum { max: 2, level: 1 }),
        "the type's own row (9 a level) does not raise the maximum"
    );
}

#[test]
fn a_gm_pause_survives_the_skyhook_going_off_and_on() {
    let mut reg = world();
    let id = launched(&mut reg);
    let def = def_for(&reg, &by_id(reg.deltas(), id).unwrap()).unwrap();
    set_paused(&mut reg, id, true, NOW + 10).unwrap();
    stand(&mut reg, SK_LAND, P_LAND, CORP, false);
    tick_at(&mut reg, NOW + 20);
    stand(&mut reg, SK_LAND, P_LAND, CORP, true);
    tick_at(&mut reg, NOW + 30);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(den.enabled && den.held && den.paused, "still GM-paused");
    assert_eq!(den.dev_level(&def, NOW + 50 * def.dev_unit_s), 0);
    // Resume while the skyhook is off keeps it frozen; once on, it runs.
    stand(&mut reg, SK_LAND, P_LAND, CORP, false);
    tick_at(&mut reg, NOW + 40);
    set_paused(&mut reg, id, false, NOW + 45).unwrap();
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(!den.held && den.paused, "off skyhook: frozen");
    stand(&mut reg, SK_LAND, P_LAND, CORP, true);
    tick_at(&mut reg, NOW + 50);
    let den = by_id(reg.deltas(), id).unwrap();
    assert!(!den.held && !den.paused && den.enabled);
}

// ---------------------------------------------------------------------------
// The hub's workforce (SOV-TIDY item 1)
// ---------------------------------------------------------------------------

#[test]
fn a_dens_workforce_use_is_taken_off_its_hubs_workforce() {
    let mut reg = world();
    let before = crate::sov_resources::allocated(&reg, HUB);
    let id = launched(&mut reg);
    let mut den = by_id(reg.deltas(), id).expect("the den");
    assert!(den.enabled, "the skyhook is on, so is the den");
    // Level 25 is stage 3; a clock behind `at` adds nothing, so the stage
    // holds whatever the wall clock reads.
    den.anarchy_base = 25;
    den.anarchy_at = u64::MAX;
    put(reg.deltas_mut(), &den);
    let def = fixture_table().get(0).unwrap().clone();
    let draw = def.anarchy_workforce[2];
    assert!(draw > 0, "the fixture stage draws something");
    let (power, workforce) = crate::sov_resources::allocated(&reg, HUB);
    assert_eq!(workforce, before.1 + draw, "the den's draw is allocated");
    assert_eq!(power, before.0, "power is not touched");
    let res = crate::sov_resources::hub_resources(&reg, HUB).expect("the hub");
    assert_eq!(res.workforce.allocated, workforce, "the window reads it");
    // A den whose skyhook is off draws nothing.
    den.enabled = false;
    put(reg.deltas_mut(), &den);
    assert_eq!(crate::sov_resources::allocated(&reg, HUB).1, before.1);
}

/// Queued `ResourcesSimulatedNotice`s for the hub since the last take.
fn simulated_for_hub(reg: &mut Registry) -> usize {
    notices(reg)
        .iter()
        .filter(|(t, _, _)| t.ends_with("ResourcesSimulatedNotice"))
        .count()
}

#[test]
fn a_launch_a_stage_change_and_a_removal_tell_the_hubs_window() {
    let mut reg = world();
    let _ = notices(&mut reg);
    let id = launched(&mut reg);
    assert!(simulated_for_hub(&mut reg) > 0, "the launch is announced");
    // The first pass reads the stage the launch stored: nothing moved.
    tick_at(&mut reg, NOW);
    assert_eq!(simulated_for_hub(&mut reg), 0, "no stage change, no notice");
    let mut den = by_id(reg.deltas(), id).expect("the den");
    den.anarchy_base = 25;
    den.anarchy_at = u64::MAX;
    put(reg.deltas_mut(), &den);
    tick_at(&mut reg, NOW);
    assert!(
        simulated_for_hub(&mut reg) > 0,
        "a new anarchy stage is told"
    );
    tick_at(&mut reg, NOW);
    assert_eq!(simulated_for_hub(&mut reg), 0, "once");
    let den = by_id(reg.deltas(), id).expect("the den");
    remove(&mut reg, &den);
    assert!(simulated_for_hub(&mut reg) > 0, "the removal is told");
}

#[test]
fn an_upgrade_goes_offline_when_a_dens_draw_uses_up_the_budget() {
    use crate::sov_upgrade::{Def, Installed, Power, Row, UpgradeTable};
    const UPGRADE: i64 = 9_950;
    let budget = crate::sov_resources::harvested(&world(), SYSTEM).1;
    assert!(budget > 0, "the world harvests some workforce");
    let row = Row {
        def: Def {
            power: 0,
            workforce: budget,
            fuel: None,
            per_hour: 0,
            startup: 0,
            power_out: 0,
            workforce_out: 0,
        },
        group: "invented".to_owned(),
        services: std::collections::BTreeSet::new(),
        tier: None,
        effects: Vec::new(),
    };
    let mut c = content();
    c.insert("sov upgrades", UpgradeTable::new(vec![(UPGRADE, row)]).0);
    let mut reg = world_with(c);
    let up = |state| {
        [Installed {
            type_id: UPGRADE,
            online: true,
            state,
        }]
    };
    crate::sov_upgrade::put_installed(reg.deltas_mut(), HUB, &up(Power::Online));
    crate::sov_upgrade::resimulate(&mut reg, HUB);
    let state = |reg: &Registry| crate::sov_upgrade::installed(reg.deltas(), HUB)[0].state;
    assert_eq!(state(&reg), Power::Online, "it fits the whole budget");
    // A den at stage 3 draws from the same budget (dens go first **[A]**).
    let id = launched(&mut reg);
    let mut den = by_id(reg.deltas(), id).expect("the den");
    den.anarchy_base = 25;
    den.anarchy_at = u64::MAX;
    put(reg.deltas_mut(), &den);
    crate::sov_upgrade::resimulate(&mut reg, HUB);
    assert_ne!(state(&reg), Power::Online, "the den's draw leaves no room");
}
