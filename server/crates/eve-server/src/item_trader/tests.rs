//! Item-trader tests. Every id, type, row and number here is invented; the
//! client's constants appear only by name. The shipped numbers (the cap, the
//! access rule, the HQ places) are never pinned: the tests build their own
//! season and only check the shipped file to be well formed.

use super::*;
use crate::space_state::{Motion, Presence};
use std::sync::Arc;

const SYSTEM: i64 = 30_291_001;
const OTHER_SYSTEM: i64 = 30_291_002;
const STATION: i64 = 60_291_001;
const STATION_AT: [f64; 3] = [1.0e9, 0.0, 0.0];
const FAC: i64 = 990_291_500;
const FAC2: i64 = 990_291_501;
const TRADER: i64 = 990_291_001;
const OTHER_TRADER: i64 = 990_291_002;
const UNPLACED: i64 = 990_291_003;
const INTEL: i64 = 990_291_010;
const TOKEN: i64 = 990_291_011;
const ORE: i64 = 990_291_012;
const HULL: i64 = 990_291_020;
const TINY_HULL: i64 = 990_291_021;
const R_TURN: i64 = 990_291_101;
const R_PAID: i64 = 990_291_102;
const R_OTHER: i64 = 990_291_103;
const R_UNPLACED: i64 = 990_291_104;
const ALICE: i64 = 90_291_001;
const BOB: i64 = 90_291_002;
const STACK_CARGO: i64 = 100_291_101;
const STACK_HOLD: i64 = 100_291_102;
const STACK_ORE: i64 = 100_291_103;

fn ship(who: i64) -> i64 {
    100_291_000 + (who - ALICE) + 1
}

/// The invented trader files: `TRADER` (two dungeons, the lower one places),
/// `OTHER_TRADER`, and `UNPLACED` of a faction with no HQ.
fn files() -> Traders {
    let traders = [
        format!("{TRADER}\t{FAC}\t900\t1000\t50.0\t-20.0\t10.0"),
        format!("{TRADER}\t{FAC}\t800\t1000\t10.0\t0.0\t0.0"),
        format!("{OTHER_TRADER}\t{FAC}\t801\t800\t0.0\t0.0\t5000000.0"),
        format!("{UNPLACED}\t{FAC2}\t802\t1000\t0.0\t0.0\t0.0"),
    ];
    let recipes = [
        // R_TURN: 450 intel and 2 ore for 1 token.
        format!("{R_TURN}\t{TRADER}\tTURN_IN\tin\t{INTEL}\t450\t0"),
        format!("{R_TURN}\t{TRADER}\tTURN_IN\tin\t{ORE}\t2\t0"),
        format!("{R_TURN}\t{TRADER}\tTURN_IN\tout\t{TOKEN}\t1\t0"),
        // R_PAID: 1 token and 1000 ISK for 3 intel.
        format!("{R_PAID}\t{TRADER}\tSPEND\tin\t{TOKEN}\t1\t1000"),
        format!("{R_PAID}\t{TRADER}\tSPEND\tout\t{INTEL}\t3\t1000"),
        format!("{R_OTHER}\t{OTHER_TRADER}\tSPEND\tin\t{TOKEN}\t1\t0"),
        format!("{R_OTHER}\t{OTHER_TRADER}\tSPEND\tout\t{INTEL}\t1\t0"),
        format!("{R_UNPLACED}\t{UNPLACED}\tSPEND\tin\t{TOKEN}\t1\t0"),
        format!("{R_UNPLACED}\t{UNPLACED}\tSPEND\tout\t{INTEL}\t1\t0"),
    ];
    let mut t = Traders::parse(&traders, &recipes).expect("the invented rows parse");
    t.season = Season {
        access: Access::Anyone,
        cap: 10,
        hq: BTreeMap::from([(FAC, (SYSTEM, STATION))]),
    };
    t
}

fn content(t: Traders) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    // columns: volume, capacity, mass, radius. Goods of 0.1 m3, 5000 m3 of hold in HULL, 3 m3 in TINY_HULL;
    // the trader's radius is 20 m.
    let text = format!(
        "{}type\t{INTEL}\t4824\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Intel\n\
         type\t{TOKEN}\t4824\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Token\n\
         type\t{ORE}\t4824\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Ore\n\
         type\t{TRADER}\t5254\t2\t1\t1\t1.0\t0.0\t0.0\t20.0\t0\t\t\tInvented Trader\n\
         type\t{OTHER_TRADER}\t5254\t2\t1\t1\t1.0\t0.0\t0.0\t20.0\t0\t\t\tInvented Other Trader\n\
         type\t{HULL}\t25\t6\t1\t1\t100.0\t5000.0\t1000.0\t20.0\t0\t\t\tInvented Roomy Hull\n\
         type\t{TINY_HULL}\t25\t6\t1\t1\t100.0\t3.0\t1000.0\t20.0\t0\t\t\tInvented Tiny Hull\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c.insert("conquest item traders", t);
    c
}

fn item(
    c: &crate::content::Content,
    id: i64,
    ty: i64,
    who: i64,
    flag: i64,
    qty: i64,
) -> crate::item::Item {
    let t = crate::item::find_in(c, ty).expect("a test type");
    crate::item::Item {
        item_id: id,
        type_id: ty,
        owner_id: who,
        location_id: ship(who),
        flag_id: flag,
        quantity: qty,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: false,
    }
}

fn world_with(t: Traders) -> Registry {
    let c = Arc::new(content(t));
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        balance: 5000.0,
        ..crate::model::Character::default()
    };
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![pilot(ALICE, "Alice"), pilot(BOB, "Bob")],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{STATION}\t2502\t0.0\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
            STATION_AT[0], STATION_AT[1], STATION_AT[2]
        )),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(c.clone());
    let mut items = reg.store().items().to_vec();
    for who in [ALICE, BOB] {
        let t = *crate::item::find_in(&c, HULL).expect("a hull");
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            (i.type_id, i.group_id, i.category_id) = (HULL, t.group_id, t.category_id);
        }
    }
    items.extend([
        // Alice: 300 intel in the cargo, 200 in the mining hold, 5 ore.
        item(
            &c,
            STACK_CARGO,
            INTEL,
            ALICE,
            crate::sov_hub::FLAG_CARGO,
            300,
        ),
        item(
            &c,
            STACK_HOLD,
            INTEL,
            ALICE,
            crate::mining_hold::FLAG_GENERAL_MINING_HOLD,
            200,
        ),
        item(&c, STACK_ORE, ORE, ALICE, crate::sov_hub::FLAG_CARGO, 5),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_292_000,
        ..crate::store::Snapshot::default()
    });
    let at = [STATION_AT[0] + 510.0, STATION_AT[1], STATION_AT[2]];
    for who in [ALICE, BOB] {
        place_at(&mut reg, who, SYSTEM, at);
    }
    reg
}

fn world() -> Registry {
    world_with(files())
}

fn place_at(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// The placed `TRADER`.
fn trader(reg: &Registry) -> Placed {
    find(reg, trader_id(TRADER)).expect("the trader is placed")
}

/// `d` m from the trader's centre.
fn near(reg: &Registry, d: f64) -> [f64; 3] {
    let at = position(reg, &trader(reg));
    [at[0] + d, at[1], at[2]]
}

fn tid() -> i64 {
    trader_id(TRADER)
}

fn qty_of(reg: &Registry, who: i64, ty: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.owner_id == who && i.type_id == ty && i.location_id == ship(who))
        .map(|i| i.quantity)
        .sum()
}

/// `process_trade` as the router calls it: the arguments after the method
/// name, with the keyword pairs of the request.
fn process(
    reg: &mut Registry,
    who: i64,
    item: i64,
    kwargs: &[(PyValue, PyValue)],
) -> Result<PyValue, ServiceError> {
    let req = crate::service::Request {
        service: "beyonce",
        method: "CallComponentFromClient",
        args: &[],
        kwargs,
        bind_params: None,
        actor: Some(who),
    };
    let args = crate::service::args::Args::new(&req).rest(3);
    process_trade(
        reg,
        &CompCall {
            actor: who,
            system: SYSTEM,
            item_id: item,
            args,
            kwargs,
        },
    )
}

fn kw(recipe: i64, multiplier: i64) -> Vec<(PyValue, PyValue)> {
    vec![
        (PyValue::Int(recipe), PyValue::Bytes(b"recipeID".to_vec())),
        (
            PyValue::Int(multiplier),
            PyValue::Bytes(b"multiplier".to_vec()),
        ),
    ]
}

fn run(reg: &mut Registry, who: i64, item: i64, recipe: i64, m: i64) -> PyValue {
    let k = kw(recipe, m);
    process(reg, who, item, &k).expect("answers, never raises")
}

fn is_refusal(v: &PyValue) -> bool {
    *v == refused()
}

// ---- the files and the placement -------------------------------------------

#[test]
fn the_files_parse_into_recipes_with_both_sides() {
    let t = files();
    let r = t.recipe(TRADER, R_TURN).expect("the recipe");
    assert_eq!(r.inputs, vec![(INTEL, 450), (ORE, 2)]);
    assert_eq!(r.outputs, vec![(TOKEN, 1)]);
    assert_eq!((r.isk, r.group.as_str()), (0, "TURN_IN"));
    assert_eq!(t.recipe_count(), 4);
    assert!(t.recipe(OTHER_TRADER, R_TURN).is_none());
}

#[test]
fn a_bad_row_names_its_file_and_line() {
    let e = Traders::parse(&["1\t2\t3"], &[] as &[&str]).unwrap_err();
    assert!(
        e.contains("conquest/traders.tsv") && e.contains("line 1"),
        "{e}"
    );
    let e = Traders::parse(&[] as &[&str], &["1\t2\tG\tsideways\t4\t5\t0"]).unwrap_err();
    assert!(
        e.contains("conquest/recipes.tsv") && e.contains("sideways"),
        "{e}"
    );
    let e = Traders::parse(&[] as &[&str], &["1\t2\tG\tin\t4\t0\t0"]).unwrap_err();
    assert!(e.contains("quantity"), "{e}");
}

#[test]
fn only_traders_of_a_faction_with_a_headquarters_are_placed() {
    let p = placed(&files());
    let types: Vec<i64> = p.iter().map(|p| p.type_id).collect();
    assert_eq!(types, vec![TRADER, OTHER_TRADER]);
    // The lowest dungeon of a type places it.
    assert_eq!(p[0].offset, [10.0, 0.0, 0.0]);
    assert_eq!((p[0].system, p[0].station), (SYSTEM, STATION));
}

#[test]
fn no_headquarters_places_nothing() {
    let mut t = files();
    t.season.hq.clear();
    assert!(placed(&t).is_empty());
    assert!(placed(&Traders::default()).is_empty());
}

#[test]
fn a_trader_stands_at_the_station_plus_its_offset() {
    let reg = world();
    let p = trader(&reg);
    assert_eq!(
        position(&reg, &p),
        [STATION_AT[0] + 10.0, STATION_AT[1], STATION_AT[2]]
    );
}

#[test]
fn the_ball_is_global_in_its_system_only() {
    let reg = world();
    let balls = balls_in(&reg, SYSTEM);
    assert_eq!(balls.len(), 2);
    let (ball, slim) = &balls[0];
    assert_eq!(ball.ball_id, tid());
    assert_eq!(slim.item_id, tid());
    assert_eq!(slim.type_id, TRADER);
    assert_eq!(ball.radius, 20.0);
    assert!(balls_in(&reg, OTHER_SYSTEM).is_empty());
    assert_eq!(locate(&reg, tid()), Some(SYSTEM));
    assert_eq!(locate(&reg, trader_id(UNPLACED)), None);
    assert_eq!(locate(&reg, 5), None);
    let t = target(&reg, tid()).expect("a target");
    assert_eq!((t.radius, t.warp_in), (20.0, None));
}

#[test]
fn the_default_table_has_no_traders() {
    let reg = Registry::new(crate::model::SandboxConfig::default());
    assert!(balls_in(&reg, SYSTEM).is_empty());
    assert_eq!(locate(&reg, tid()), None);
    assert!(target(&reg, tid()).is_none());
}

#[test]
fn the_id_band_is_clear_of_its_neighbours() {
    // The module's const asserts hold the band apart from the ESS and the probes.
    assert_eq!(type_of_id(trader_id(TRADER)), Some(TRADER));
    assert_eq!(type_of_id(TRADER_ID_BASE), None);
}

// ---- the season table ------------------------------------------------------

#[test]
fn the_shipped_season_file_is_well_formed_and_gives_four_headquarters() {
    let (rows, skipped) = crate::authored::parse(SEASON_SHIPPED, SEASON_HEADER).expect("parses");
    assert!(skipped.is_empty(), "{skipped:?}");
    let file = crate::authored::Dir::compiled_only()
        .load(SEASON_FILE, SEASON_SHIPPED, SEASON_HEADER)
        .expect("loads");
    assert_eq!(file.rows.len(), rows.len());
    let (parsed, bad) = file.parse_rows_logged(parse_season_row);
    assert!(bad.is_empty(), "{bad:?}");
    let s = Season::from_rows(parsed);
    assert_eq!(s.hq.len(), 4);
    assert!(s.cap >= 1);
}

#[test]
fn season_rows_set_access_cap_and_headquarters() {
    let s = Season::from_rows(vec![
        SeasonRow::Access(Access::Anyone),
        SeasonRow::Cap(7),
        SeasonRow::HqSystem(1, 2),
        SeasonRow::HqStation(1, 3),
        // A system without a station is not a headquarters.
        SeasonRow::HqSystem(4, 5),
        SeasonRow::Other,
    ]);
    assert_eq!((s.access, s.cap), (Access::Anyone, 7));
    assert_eq!(s.hq, BTreeMap::from([(1, (2, 3))]));
    assert_eq!(Season::from_rows(vec![]), Season::default());
}

// ---- the trade ---------------------------------------------------------------

#[test]
fn inputs_are_taken_across_the_cargo_and_the_mining_hold() {
    let mut reg = world();
    let v = run(&mut reg, ALICE, tid(), R_TURN, 1);
    assert_eq!(
        v,
        PyValue::Tuple(vec![
            PyValue::Bool(true),
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(TOKEN),
                PyValue::Int(1)
            ])]),
        ])
    );
    // 450 of 500 gone: the cargo stack (lower id) whole, 150 of the hold's.
    assert_eq!(qty_of(&reg, ALICE, INTEL), 50);
    assert!(reg.store().get(STACK_CARGO).is_none());
    assert_eq!(reg.store().get(STACK_HOLD).map(|i| i.quantity), Some(50));
    assert_eq!(qty_of(&reg, ALICE, ORE), 3);
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 1);
}

#[test]
fn too_few_inputs_changes_nothing_and_answers_false() {
    let mut reg = world();
    // 500 intel is not 1000, and the first input alone would be enough.
    let v = run(&mut reg, ALICE, tid(), R_TURN, 2);
    assert!(is_refusal(&v));
    assert_eq!(qty_of(&reg, ALICE, INTEL), 500);
    assert_eq!(qty_of(&reg, ALICE, ORE), 5);
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 0);
    // Bob has nothing at all.
    assert!(is_refusal(&run(&mut reg, BOB, tid(), R_TURN, 1)));
}

#[test]
fn a_second_input_short_leaves_the_first_alone() {
    let mut reg = world();
    reg.apply_items(&[Edit::Consume {
        item_id: STACK_ORE,
        quantity: 4,
    }])
    .expect("consumes");
    assert!(is_refusal(&run(&mut reg, ALICE, tid(), R_TURN, 1)));
    assert_eq!(qty_of(&reg, ALICE, INTEL), 500);
}

#[test]
fn out_of_range_answers_false() {
    let mut reg = world();
    // The trader's range is 1000 m from its surface (20 m radius).
    let far = near(&reg, 1000.0 + 20.0 + 5.0);
    place_at(&mut reg, ALICE, SYSTEM, far);
    assert!(is_refusal(&run(&mut reg, ALICE, tid(), R_TURN, 1)));
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 0);
    let edge = near(&reg, 1000.0 + 20.0 - 5.0);
    place_at(&mut reg, ALICE, SYSTEM, edge);
    assert!(!is_refusal(&run(&mut reg, ALICE, tid(), R_TURN, 1)));
}

#[test]
fn another_traders_recipe_answers_false() {
    let mut reg = world();
    let before = qty_of(&reg, ALICE, INTEL);
    // R_OTHER belongs to OTHER_TRADER, which is also placed in this system.
    assert!(is_refusal(&run(&mut reg, ALICE, tid(), R_OTHER, 1)));
    assert!(is_refusal(&run(&mut reg, ALICE, tid(), 5, 1)));
    assert_eq!(qty_of(&reg, ALICE, INTEL), before);
}

#[test]
fn the_multiplier_multiplies_and_is_bounded() {
    let mut reg = world();
    reg.apply_items(&[Edit::Create {
        type_id: INTEL,
        quantity: 1000,
        to: Slot::new(ALICE, ship(ALICE), crate::sov_hub::FLAG_CARGO),
    }])
    .expect("creates");
    reg.apply_items(&[Edit::Create {
        type_id: ORE,
        quantity: 20,
        to: Slot::new(ALICE, ship(ALICE), crate::sov_hub::FLAG_CARGO),
    }])
    .expect("creates");
    // 1500 intel, 25 ore: three trades take 1350 and 6.
    let v = run(&mut reg, ALICE, tid(), R_TURN, 3);
    assert_ne!(v, refused());
    assert_eq!(qty_of(&reg, ALICE, INTEL), 150);
    assert_eq!(qty_of(&reg, ALICE, ORE), 19);
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 3);
    for bad in [0, -1, 11] {
        assert!(
            is_refusal(&run(&mut reg, ALICE, tid(), R_TURN, bad)),
            "{bad}"
        );
    }
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 3);
}

#[test]
fn the_isk_is_debited_with_a_journal_row() {
    let mut reg = world();
    reg.apply_items(&[Edit::Create {
        type_id: TOKEN,
        quantity: 2,
        to: Slot::new(ALICE, ship(ALICE), crate::sov_hub::FLAG_CARGO),
    }])
    .expect("creates");
    let rows = reg.journal().entries().len();
    let v = run(&mut reg, ALICE, tid(), R_PAID, 2);
    assert_ne!(v, refused());
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 0);
    assert_eq!(qty_of(&reg, ALICE, INTEL), 506);
    assert_eq!(reg.character(ALICE).unwrap().balance, 3000.0);
    let new = &reg.journal().entries()[rows..];
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].entry_type_id, REF_ITEM_TRADER_PAYMENT);
    assert_eq!(new[0].amount, -2000.0);
    assert_eq!(new[0].reference_id, R_PAID);
}

#[test]
fn too_little_isk_changes_nothing() {
    let mut reg = world();
    reg.character_mut(ALICE).unwrap().balance = 1999.0;
    reg.apply_items(&[Edit::Create {
        type_id: TOKEN,
        quantity: 2,
        to: Slot::new(ALICE, ship(ALICE), crate::sov_hub::FLAG_CARGO),
    }])
    .expect("creates");
    let rows = reg.journal().entries().len();
    assert!(is_refusal(&run(&mut reg, ALICE, tid(), R_PAID, 2)));
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 2);
    assert_eq!(reg.character(ALICE).unwrap().balance, 1999.0);
    assert_eq!(reg.journal().entries().len(), rows);
}

/// Bob's hull is the tiny one (3 m3), holding `ore` units of 0.1 m3 and a token.
fn tiny_bob(reg: &mut Registry, ore: i64) {
    let tiny = *crate::item::find_in(reg.content(), TINY_HULL).unwrap();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(BOB)) {
        (i.type_id, i.group_id, i.category_id) = (TINY_HULL, tiny.group_id, tiny.category_id);
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_293_000,
        ..crate::store::Snapshot::default()
    });
    let cargo = Slot::new(BOB, ship(BOB), crate::sov_hub::FLAG_CARGO);
    reg.apply_items(&[
        Edit::Create {
            type_id: ORE,
            quantity: ore,
            to: cargo,
        },
        Edit::Create {
            type_id: TOKEN,
            quantity: 1,
            to: cargo,
        },
    ])
    .expect("creates");
}

#[test]
fn a_hold_without_room_for_the_net_gain_refuses_and_nothing_moves() {
    let mut reg = world();
    // 28 ore and a token: 2.9 of 3 m3. R_PAID frees 0.1 (the token) and adds
    // 0.3 (three intel): the net 0.2 does not fit in 0.1.
    tiny_bob(&mut reg, 28);
    assert_eq!(trade(&mut reg, BOB, tid(), R_PAID, 1), Err(No::NoRoom));
    assert_eq!(qty_of(&reg, BOB, TOKEN), 1);
    assert_eq!(qty_of(&reg, BOB, INTEL), 0);
    assert_eq!(reg.character(BOB).unwrap().balance, 5000.0);
}

#[test]
fn what_the_trade_frees_counts_toward_the_room() {
    let mut reg = world();
    // 27 ore and a token: 2.8 of 3 m3, so 0.2 free plus the token's 0.1.
    tiny_bob(&mut reg, 27);
    assert_eq!(trade(&mut reg, BOB, tid(), R_PAID, 1), Ok(vec![(INTEL, 3)]));
    assert_eq!(qty_of(&reg, BOB, INTEL), 3);
}

#[test]
fn a_militia_only_trader_serves_its_own_militia() {
    let mut t = files();
    t.season.access = Access::Militia;
    let mut reg = world_with(t);
    // Not enlisted: refused.
    assert_eq!(
        trade(&mut reg, ALICE, tid(), R_TURN, 1),
        Err(No::NotYourMilitia)
    );
    // Enlisted in another militia: refused.
    reg.deltas_mut().put(
        crate::service::facwar_mgr::KIND,
        vec![ALICE.to_string()],
        vec![FAC2.to_string(), "1".to_owned(), "0".to_owned()],
    );
    assert_eq!(
        trade(&mut reg, ALICE, tid(), R_TURN, 1),
        Err(No::NotYourMilitia)
    );
    // Enlisted in the trader's own: served.
    reg.deltas_mut().put(
        crate::service::facwar_mgr::KIND,
        vec![ALICE.to_string()],
        vec![FAC.to_string(), "1".to_owned(), "0".to_owned()],
    );
    assert_eq!(
        trade(&mut reg, ALICE, tid(), R_TURN, 1),
        Ok(vec![(TOKEN, 1)])
    );
}

#[test]
fn a_pilot_in_another_system_is_not_served() {
    let mut reg = world();
    let at = near(&reg, 500.0);
    place_at(&mut reg, ALICE, OTHER_SYSTEM, at);
    assert_eq!(
        trade(&mut reg, ALICE, tid(), R_TURN, 1),
        Err(No::NotInSpace)
    );
}

#[test]
fn missing_keywords_answer_false_and_never_raise() {
    let mut reg = world();
    let none: Vec<(PyValue, PyValue)> = Vec::new();
    let v = process(&mut reg, ALICE, tid(), &none).expect("answers");
    assert!(is_refusal(&v));
    let half = vec![(PyValue::Int(R_TURN), PyValue::Bytes(b"recipeID".to_vec()))];
    assert!(is_refusal(
        &process(&mut reg, ALICE, tid(), &half).expect("answers")
    ));
}

#[test]
fn the_router_reaches_the_handler_with_keyword_arguments() {
    let mut reg = world();
    let table = &[(
        ITEM_TRADER,
        locate as crate::space_component::Locate,
        &[(
            "ProcessTrade",
            process_trade as crate::space_component::CompFn,
        )][..],
    )];
    let args = vec![
        PyValue::Int(tid()),
        PyValue::Bytes(ITEM_TRADER.as_bytes().to_vec()),
        PyValue::Unicode("ProcessTrade".to_owned()),
    ];
    let k = kw(R_TURN, 1);
    let req = crate::service::Request {
        service: "beyonce",
        method: "CallComponentFromClient",
        args: &args,
        kwargs: &k,
        bind_params: None,
        actor: Some(ALICE),
    };
    let got = crate::space_component::call_in(table, &mut reg, &req).expect("routed");
    assert_ne!(got, refused());
    assert_eq!(qty_of(&reg, ALICE, TOKEN), 1);
    // The real table has the row.
    assert!(
        crate::space_component::CALLS
            .iter()
            .any(|(c, _, m)| *c == ITEM_TRADER && m.iter().any(|(n, _)| *n == "ProcessTrade"))
    );
}

#[test]
fn a_station_that_is_not_in_the_hq_system_places_nothing() {
    let mut t = files();
    // A station the catalogue lacks, and one that is in another system.
    t.season.hq = BTreeMap::from([(FAC, (SYSTEM, 60_291_999))]);
    let reg = world_with(t);
    assert!(balls_in(&reg, SYSTEM).is_empty());
    assert_eq!(locate(&reg, tid()), None);
    assert!(target(&reg, tid()).is_none());
    let mut t = files();
    t.season.hq = BTreeMap::from([(FAC, (OTHER_SYSTEM, STATION))]);
    let reg = world_with(t);
    assert!(balls_in(&reg, OTHER_SYSTEM).is_empty());
}
