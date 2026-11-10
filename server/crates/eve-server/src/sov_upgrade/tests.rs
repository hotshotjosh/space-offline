//! SOV-UPG-1 tests. Invented ids throughout: upgrade types `9_201..=9_205`,
//! hub `8_900_200`, reagent `9_101`. The numbers of the rule's cases are
//! hand-worked from `hubUtil.UpdateEffectiveStatesForUpgrades`
//! (`sovereignty/client/sovHub/hubUtil.py:15-72`); no authored value is
//! asserted.

use super::*;
use crate::gateway::{Ground, field, get_uint};
use crate::sov_proto::read_id;
use std::cell::RefCell;
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// The pure rule
// ---------------------------------------------------------------------------

const A: i64 = 9_201;
const B: i64 = 9_202;
const R: i64 = FIXTURE_REAGENT;

fn plain(power: u64, workforce: u64, power_out: u64, workforce_out: u64) -> Def {
    Def {
        power,
        workforce,
        fuel: None,
        per_hour: 0,
        startup: 0,
        power_out,
        workforce_out,
    }
}

fn burner(per_hour: u32, startup: u32) -> Def {
    Def {
        fuel: Some(R),
        per_hour,
        startup,
        ..plain(0, 0, 0, 0)
    }
}

fn slot(type_id: i64, online: bool, prev: Option<Power>) -> Slot {
    Slot {
        type_id,
        online,
        prev,
        new: prev.is_none(),
    }
}

fn run(
    slots: &[Slot],
    defs: &[(i64, Def)],
    power: u64,
    workforce: u64,
    stock: u64,
) -> Vec<(Power, u64)> {
    simulate(
        slots,
        &|t| defs.iter().find(|(i, _)| *i == t).map(|(_, d)| *d),
        power,
        workforce,
        &|r| if r == R { stock } else { 0 },
    )
}

fn states(v: &[(Power, u64)]) -> Vec<Power> {
    v.iter().map(|(s, _)| *s).collect()
}

#[test]
fn order_is_priority_the_first_to_fit_takes_the_power() {
    // Budget 10, each wants 6: only the first fits.
    let defs = [(A, plain(6, 0, 0, 0)), (B, plain(6, 0, 0, 0))];
    let on = Some(Power::Online);
    let ab = run(&[slot(A, true, on), slot(B, true, on)], &defs, 10, 0, 0);
    assert_eq!(states(&ab), [Power::Online, Power::Low]);
    let ba = run(&[slot(B, true, on), slot(A, true, on)], &defs, 10, 0, 0);
    assert_eq!(states(&ba), [Power::Online, Power::Low]);
    // Which type runs is the one listed first.
    let by_type = |order: [i64; 2], got: &[(Power, u64)]| -> Vec<(i64, Power)> {
        order.iter().copied().zip(states(got)).collect()
    };
    assert_eq!(by_type([A, B], &ab), [(A, Power::Online), (B, Power::Low)]);
    assert_eq!(by_type([B, A], &ba), [(B, Power::Online), (A, Power::Low)]);
}

#[test]
fn produced_power_feeds_the_upgrades_after_it_only() {
    // A makes 5 and needs 0; B needs 12; the budget is 10.
    let defs = [(A, plain(0, 0, 5, 0)), (B, plain(12, 0, 0, 0))];
    let on = Some(Power::Online);
    let ab = run(&[slot(A, true, on), slot(B, true, on)], &defs, 10, 0, 0);
    assert_eq!(states(&ab), [Power::Online, Power::Online]);
    let ba = run(&[slot(B, true, on), slot(A, true, on)], &defs, 10, 0, 0);
    assert_eq!(states(&ba), [Power::Low, Power::Online]);
}

#[test]
fn produced_workforce_feeds_later_upgrades_and_a_lack_of_it_reads_low() {
    let defs = [(A, plain(0, 0, 0, 7)), (B, plain(0, 9, 0, 0))];
    let on = Some(Power::Online);
    let ab = run(&[slot(A, true, on), slot(B, true, on)], &defs, 0, 3, 0);
    assert_eq!(states(&ab), [Power::Online, Power::Online]);
    let ba = run(&[slot(B, true, on), slot(A, true, on)], &defs, 0, 3, 0);
    assert_eq!(states(&ba), [Power::Low, Power::Online]);
}

#[test]
fn a_startup_shortfall_reads_low_never_pending() {
    // The client's fuel test includes the startup, so a stock one short of
    // the startup is short of the fuel too, and Low wins (hubUtil.py:41, :60-65).
    let defs = [(A, burner(3, 5))];
    let r = run(&[slot(A, true, None)], &defs, 0, 0, 4);
    assert_eq!(states(&r), [Power::Low]);
    assert_eq!(
        r[0].1, 0,
        "nothing is paid for an upgrade that is not Online"
    );
    // No burn at all: still Low.
    let defs = [(A, burner(0, 5))];
    assert_eq!(
        states(&run(&[slot(A, true, None)], &defs, 0, 0, 4)),
        [Power::Low]
    );
}

#[test]
fn no_stock_and_no_input_ever_reads_pending() {
    // A grid over the rule's inputs: the Pending state is unreachable
    // (module header), so a port that "fixes" the rule to give it fails here.
    let prevs = [
        None,
        Some(Power::Offline),
        Some(Power::Online),
        Some(Power::Low),
        Some(Power::Pending),
    ];
    for prev in prevs {
        for per_hour in [0, 1, 3] {
            for startup in [0, 1, 5] {
                for stock in 0..9 {
                    let defs = [(A, burner(per_hour, startup))];
                    let r = run(&[slot(A, true, prev)], &defs, 0, 0, stock);
                    assert_ne!(
                        r[0].0,
                        Power::Pending,
                        "{prev:?} {per_hour} {startup} {stock}"
                    );
                }
            }
        }
    }
}

#[test]
fn one_unit_of_stock_keeps_a_burner_online_and_none_does_not() {
    // Startup 0, already Online, burns 3 an hour: the rule asks for ONE unit,
    // not an hour's burn (hubUtil.py:40).
    let defs = [(A, burner(3, 0))];
    let on = Some(Power::Online);
    assert_eq!(
        states(&run(&[slot(A, true, on)], &defs, 0, 0, 0)),
        [Power::Low]
    );
    assert_eq!(
        states(&run(&[slot(A, true, on)], &defs, 0, 0, 1)),
        [Power::Online]
    );
}

#[test]
fn a_low_upgrade_regains_online_without_paying_startup() {
    let defs = [(A, burner(3, 5))];
    let low = run(&[slot(A, true, Some(Power::Low))], &defs, 0, 0, 1);
    assert_eq!(low, [(Power::Online, 0)]);
    // An Offline one switched on owes the startup, so the same stock is short.
    let off = run(&[slot(A, true, Some(Power::Offline))], &defs, 0, 0, 1);
    assert_eq!(states(&off), [Power::Low]);
}

#[test]
fn startup_is_paid_for_a_new_or_a_re_enabled_upgrade_and_not_for_a_running_one() {
    let defs = [(A, burner(1, 4))];
    for (prev, paid) in [
        (None, 4),
        (Some(Power::Offline), 4),
        (Some(Power::Pending), 4),
        (Some(Power::Online), 0),
        (Some(Power::Low), 0),
    ] {
        let r = run(&[slot(A, true, prev)], &defs, 0, 0, 100);
        assert_eq!(r, [(Power::Online, paid)], "{prev:?}");
    }
}

#[test]
fn offline_is_offline_whatever_the_resources_and_it_uses_none() {
    let defs = [(A, plain(10, 0, 0, 0)), (B, plain(10, 0, 0, 0))];
    let on = Some(Power::Online);
    let r = run(&[slot(A, false, on), slot(B, true, on)], &defs, 10, 0, 0);
    // A takes nothing, so B gets the whole budget.
    assert_eq!(states(&r), [Power::Offline, Power::Online]);
    let burn = [(A, burner(2, 3))];
    let r = run(&[slot(A, false, None)], &burn, 0, 0, 1_000);
    assert_eq!(
        r,
        [(Power::Offline, 0)],
        "an offline new upgrade pays nothing"
    );
}

#[test]
fn upgrades_burning_one_reagent_share_its_stock() {
    // Each costs its startup 1 plus 1 for the burn = 2; stock 3 holds one.
    let defs = [(A, burner(1, 1)), (B, burner(1, 1))];
    let r = run(&[slot(A, true, None), slot(B, true, None)], &defs, 0, 0, 3);
    assert_eq!(states(&r), [Power::Online, Power::Low]);
    let r = run(&[slot(A, true, None), slot(B, true, None)], &defs, 0, 0, 4);
    assert_eq!(states(&r), [Power::Online, Power::Online]);
}

#[test]
fn a_type_with_no_definition_is_offline_and_uses_nothing() {
    let defs = [(B, plain(10, 0, 0, 0))];
    let r = run(&[slot(A, true, None), slot(B, true, None)], &defs, 10, 0, 0);
    assert_eq!(states(&r), [Power::Offline, Power::Online]);
}

// ---------------------------------------------------------------------------
// Rows and the table
// ---------------------------------------------------------------------------

#[test]
fn stored_upgrades_keep_their_order_and_an_empty_list_removes_the_row() {
    let mut d = Deltas::default();
    let list = [
        Installed {
            type_id: B,
            online: true,
            state: Power::Low,
        },
        Installed {
            type_id: A,
            online: false,
            state: Power::Offline,
        },
    ];
    put_installed(&mut d, 8_900_200, &list);
    assert_eq!(installed(&d, 8_900_200), list);
    assert!(installed(&d, 8_900_201).is_empty());
    put_installed(&mut d, 8_900_200, &[]);
    assert!(d.of(KIND).is_empty());
}

#[test]
fn a_malformed_stored_entry_is_skipped_not_fatal() {
    let mut d = Deltas::default();
    d.put(
        KIND,
        vec!["7".into()],
        vec![
            "9201:1:2".into(),
            "x:1:2".into(),
            "9202:7:2".into(),
            "9203:1:9".into(),
            "9204:0:1".into(),
        ],
    );
    let got: Vec<i64> = installed(&d, 7).iter().map(|i| i.type_id).collect();
    assert_eq!(got, [9_201, 9_204]);
}

fn parse_text(text: &str) -> (Vec<(i64, Row)>, Vec<String>) {
    let file = crate::authored::Dir::compiled_only()
        .load("x.tsv", text, UPGRADES_HEADER)
        .unwrap();
    let (rows, skipped) = file.parse_rows_logged(parse_row);
    (rows, skipped)
}

#[test]
fn the_loader_keeps_good_rows_and_drops_the_bad_ones() {
    let text = format!(
        "{UPGRADES_HEADER}\n\
         1\t5\t0\t-\t0\t0\tg\t0\t0\t-\tok no fuel\n\
         2\t5\t0\t{R}\t2\t3\tg2\t0\t0\t-\tok fuel\n\
         3\t5\t0\t-\t2\t0\tg3\t0\t0\t-\ta burn without a fuel type\n\
         4\t5\t0\t-\t0\t1\tg4\t0\t0\t-\ta startup without a fuel type\n\
         5\t5\t0\t-\t0\t0\t-\t0\t0\t-\tno group\n\
         6\t-1\t0\t-\t0\t0\tg6\t0\t0\t-\tnegative power\n\
         0\t5\t0\t-\t0\t0\tg7\t0\t0\t-\tno type\n\
         8\t5\t0\tice\t0\t0\tg8\t0\t0\t-\ta key, not an id\n"
    );
    let (rows, skipped) = parse_text(&text);
    let kept: Vec<i64> = rows.iter().map(|(t, _)| *t).collect();
    assert_eq!(kept, [1, 2]);
    assert!(skipped.len() >= 6, "{skipped:?}");
    assert_eq!(rows[1].1.def.fuel, Some(R));
    assert_eq!(rows[1].1.group, "g2");
}

#[test]
fn a_repeated_type_is_reported_and_the_last_row_wins() {
    let (rows, _) = parse_text(&format!(
        "{UPGRADES_HEADER}\n1\t5\t0\t-\t0\t0\tg\t0\t0\t-\tfirst\n1\t6\t0\t-\t0\t0\tg\t0\t0\t-\tsecond\n"
    ));
    let (t, dups) = UpgradeTable::new(rows);
    assert_eq!(dups.len(), 1);
    assert_eq!(t.def(1).map(|d| d.power), Some(6));
}

#[test]
fn the_shipped_table_is_well_formed() {
    // Well-formedness only, never a value (plan section 1.4).
    let file = crate::authored::Dir::compiled_only()
        .load(UPGRADES_FILE, UPGRADES_SHIPPED, UPGRADES_HEADER)
        .unwrap();
    let (rows, skipped) = file.parse_rows_logged(parse_row);
    assert!(skipped.is_empty(), "{skipped:?}");
    let (table, dups) = UpgradeTable::new(rows);
    assert!(dups.is_empty(), "{dups:?}");
    assert!(!table.is_empty());
}

// ---------------------------------------------------------------------------
// A world
// ---------------------------------------------------------------------------

const SYSTEM: i64 = 30_790_001;
const STATION: i64 = 60_790_001;
const STAR: i64 = 40_790_000;
const PLANET_POWER: i64 = 40_790_001;
const PLANET_LAND: i64 = 40_790_002;
const HUB: i64 = 8_900_200;
const ALICE: i64 = 90_790_001;
const BOB: i64 = 90_790_002;
const CAROL: i64 = 90_790_003;
const DAVE: i64 = 90_790_004;
const CORP: i64 = 98_790_001;
const OTHER_CORP: i64 = 98_790_002;
const ALLIANCE: i64 = 99_790_001;
const OTHER_ALLIANCE: i64 = 99_790_002;
const NOW: u64 = 1_800_000_000;
/// The star's invented power: the whole budget of the test world.
const STAR_POWER: u64 = 35;
/// The invented upgrade items, one per fixture type, in Alice's hold.
const ITEM_A: i64 = 100_790_101;
const ITEM_B: i64 = 100_790_102;
const ITEM_PRODUCER: i64 = 100_790_103;
const ITEM_BIG: i64 = 100_790_104;
const ITEM_BURNER: i64 = 100_790_105;
/// Not an upgrade group; an upgrade group but no table row; in a station;
/// in somebody else's hold.
const ITEM_PLAIN: i64 = 100_790_111;
const ITEM_NO_ROW: i64 = 100_790_112;
const ITEM_DOCKED: i64 = 100_790_113;
const ITEM_BOBS: i64 = 100_790_114;
const ITEM_DAVES: i64 = 100_790_115;
/// A second copy of the first type, in Alice's hold.
const ITEM_A_AGAIN: i64 = 100_790_116;
const TYPE_NO_ROW: i64 = 9_299;

fn ship(who: i64) -> i64 {
    100_790_000 + (who - ALICE) + 1
}

fn hub_at() -> [f64; 3] {
    [0.0; 3]
}

/// Next to the hub: half a kilometre off its surface.
fn near() -> [f64; 3] {
    [0.0, 0.0, sov_hub::HUB_RADIUS + 500.0]
}

fn far() -> [f64; 3] {
    [0.0, 0.0, sov_hub::HUB_RADIUS + 400_000.0]
}

fn place(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            who,
            ship(who),
            SYSTEM,
            crate::space_state::Motion::arrived(at, [0.0; 3]),
        ));
}

fn content_with(typedata_rows: &str) -> Content {
    let mut c = Content::fixture();
    c.insert("sov planets", crate::sov_resources::fixture_planets());
    // The budget is what is harvested (SKY-1): the star alone while no skyhook
    // is active. Invented star power, so the world has a budget of its own.
    c.insert(
        "sov stars",
        crate::sov_resources::StarTable::new(vec![(9_000, STAR_POWER)]),
    );
    c.insert("sov upgrades", fixture_upgrades());
    if !typedata_rows.is_empty() {
        let text = format!("{}{typedata_rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).unwrap();
        c.insert("typedata", crate::typedata::Installed::build(table));
    }
    c
}

fn item(item_id: i64, type_id: i64, group: i64, owner: i64, location: i64) -> crate::item::Item {
    crate::item::Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: location,
        flag_id: sov_hub::FLAG_CARGO,
        quantity: 1,
        group_id: group,
        category_id: 0,
        singleton: false,
    }
}

/// Budget: power from the fixture planet and star, workforce from the fixture
/// land planet. Alice (CEO of the holding corp) is by the hub; Bob has no
/// role; Carol is in another alliance; Dave is a station manager.
fn world_with(content: Content) -> Registry {
    let pilot = |id: i64, alliance: i64| crate::model::Character {
        character_id: id,
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        alliance_id: Some(alliance),
        ..crate::model::Character::default()
    };
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, ALLIANCE),
            pilot(BOB, ALLIANCE),
            pilot(CAROL, OTHER_ALLIANCE),
            pilot(DAVE, ALLIANCE),
        ],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "star\t{STAR}\t9000\t1000\t0\t0\t0\t{SYSTEM}\t0\t0\n\
             planet\t{PLANET_POWER}\t9001\t100\t1\t0\t0\t{SYSTEM}\t0\t0\n\
             planet\t{PLANET_LAND}\t9002\t100\t2\t0\t0\t{SYSTEM}\t0\t0\n"
        )),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(content));
    let g = UPGRADE_GROUPS[0];
    let mut items = reg.store().items().to_vec();
    items.extend([
        item(ITEM_A, 9_201, g, ALICE, ship(ALICE)),
        item(ITEM_B, 9_202, g, ALICE, ship(ALICE)),
        item(ITEM_PRODUCER, 9_203, g, ALICE, ship(ALICE)),
        item(ITEM_BIG, 9_204, g, ALICE, ship(ALICE)),
        item(ITEM_BURNER, 9_205, g, ALICE, ship(ALICE)),
        item(ITEM_PLAIN, 9_206, 25, ALICE, ship(ALICE)),
        item(ITEM_NO_ROW, TYPE_NO_ROW, g, ALICE, ship(ALICE)),
        item(ITEM_DOCKED, 9_201, g, ALICE, STATION),
        item(ITEM_BOBS, 9_201, g, BOB, ship(BOB)),
        item(ITEM_DAVES, 9_204, g, DAVE, ship(DAVE)),
        item(ITEM_A_AGAIN, 9_201, g, ALICE, ship(ALICE)),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_791_000,
        ..crate::store::Snapshot::default()
    });
    crate::service::sov_hub::test_corp(
        &mut reg,
        CORP,
        ALICE,
        &[(ALICE, 0), (BOB, 0), (DAVE, 2_048)],
    );
    crate::service::sov_hub::test_corp(&mut reg, OTHER_CORP, CAROL, &[(CAROL, 0)]);
    for (who, corp, alliance) in [
        (ALICE, CORP, ALLIANCE),
        (BOB, CORP, ALLIANCE),
        (CAROL, OTHER_CORP, OTHER_ALLIANCE),
        (DAVE, CORP, ALLIANCE),
    ] {
        let c = reg.character_mut(who).expect("a pilot");
        c.corporation_id = corp;
        c.alliance_id = Some(alliance);
    }
    sov_hub::put(
        reg.deltas_mut(),
        &Hub {
            system_id: SYSTEM,
            hub_id: HUB,
            position: hub_at(),
            launcher_id: None,
            launch_time: 0,
            offline_days: 0,
        },
    );
    sov::set_claim(reg.deltas_mut(), SYSTEM, Some((ALLIANCE, CORP)), 1);
    for who in [ALICE, BOB, CAROL, DAVE] {
        place(&mut reg, who, near());
    }
    reg
}

fn world() -> Registry {
    world_with(content_with(""))
}

/// A bay of plain numbers that remembers what it was asked.
#[derive(Default)]
struct TestBay {
    stock: RefCell<BTreeMap<i64, u64>>,
    burn: RefCell<BTreeMap<i64, u32>>,
    taken: RefCell<Vec<(i64, u64)>>,
}

impl TestBay {
    fn with(reagent: i64, n: u64) -> TestBay {
        let b = TestBay::default();
        b.stock.borrow_mut().insert(reagent, n);
        b
    }
    fn set(&self, reagent: i64, n: u64) {
        self.stock.borrow_mut().insert(reagent, n);
    }
    fn burning(&self, reagent: i64) -> u32 {
        self.burn.borrow().get(&reagent).copied().unwrap_or(0)
    }
}

impl Bay for TestBay {
    fn amount(&self, _: &Deltas, _: i64, reagent: i64, _: u64) -> u64 {
        self.stock.borrow().get(&reagent).copied().unwrap_or(0)
    }
    fn take(&self, _: &mut Deltas, _: i64, reagent: i64, n: u64, _: u64) -> bool {
        let mut s = self.stock.borrow_mut();
        let have = s.entry(reagent).or_insert(0);
        if *have < n {
            return false;
        }
        *have -= n;
        self.taken.borrow_mut().push((reagent, n));
        true
    }
    fn set_burn(&self, _: &mut Deltas, _: i64, reagent: i64, per_hour: u32, _: u64) {
        self.burn.borrow_mut().insert(reagent, per_hour);
    }
}

fn install(
    reg: &mut Registry,
    bay: &dyn Bay,
    who: i64,
    items: &[i64],
    config: &[(i64, bool)],
) -> Result<Vec<Installed>, u32> {
    configure(reg, HUB, who, items, config, bay, NOW)
}

fn st(list: &[Installed]) -> Vec<(i64, Power)> {
    list.iter().map(|i| (i.type_id, i.state)).collect()
}

fn in_hold(reg: &Registry, item_id: i64) -> bool {
    reg.store().get(item_id).is_some()
}

// ---------------------------------------------------------------------------
// Install, online, offline, remove
// ---------------------------------------------------------------------------

#[test]
fn installing_consumes_the_item_stores_the_row_and_answers_the_state() {
    let mut reg = world();
    let bay = TestBay::default();
    let got = install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online)]);
    assert!(!in_hold(&reg, ITEM_A), "the item is the upgrade now");
    assert_eq!(installed(reg.deltas(), HUB), got);
    assert_eq!(online_types(reg.deltas(), SYSTEM), [9_201]);
}

#[test]
fn a_second_upgrade_that_does_not_fit_reads_low_and_the_first_stays_online() {
    let mut reg = world();
    let bay = TestBay::default();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    let got = install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_BIG],
        &[(9_201, true), (9_204, true)],
    )
    .unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online), (9_204, Power::Low)]);
    assert_eq!(online_types(reg.deltas(), SYSTEM), [9_201]);
}

#[test]
fn the_answer_and_the_stored_order_follow_the_request() {
    let mut reg = world();
    let bay = TestBay::default();
    install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_204, true), (9_201, true)],
    )
    .unwrap();
    let order = |l: &[Installed]| l.iter().map(|i| i.type_id).collect::<Vec<_>>();
    assert_eq!(order(&installed(reg.deltas(), HUB)), [9_204, 9_201]);
    // Reordering with nothing new runs the rule again from the top.
    let got = install(&mut reg, &bay, ALICE, &[], &[(9_201, true), (9_204, true)]).unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online), (9_204, Power::Low)]);
    assert_eq!(order(&installed(reg.deltas(), HUB)), [9_201, 9_204]);
}

#[test]
fn an_upgrade_saved_offline_is_offline_and_a_toggle_brings_it_back() {
    let mut reg = world();
    let bay = TestBay::default();
    let got = install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, false)]).unwrap();
    assert_eq!(st(&got), [(9_201, Power::Offline)]);
    assert!(online_types(reg.deltas(), SYSTEM).is_empty());
    let got = install(&mut reg, &bay, ALICE, &[], &[(9_201, true)]).unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online)]);
    let got = install(&mut reg, &bay, ALICE, &[], &[(9_201, false)]).unwrap();
    assert_eq!(st(&got), [(9_201, Power::Offline)]);
}

#[test]
fn a_group_clash_answers_409_and_consumes_nothing() {
    let mut reg = world();
    let bay = TestBay::default();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    let r = install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_B],
        &[(9_201, true), (9_202, true)],
    );
    assert_eq!(r, Err(409));
    assert!(in_hold(&reg, ITEM_B));
    // Two in one request clash with each other too.
    let mut reg = world();
    let r = install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_A, ITEM_B],
        &[(9_201, true), (9_202, true)],
    );
    assert_eq!(r, Err(409));
    assert!(in_hold(&reg, ITEM_A) && in_hold(&reg, ITEM_B));
}

#[test]
fn the_same_type_twice_answers_409() {
    let mut reg = world();
    let bay = TestBay::default();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    // A second copy of an installed type is "already installed" (hubController.py:195-199).
    let r = install(&mut reg, &bay, ALICE, &[ITEM_A_AGAIN], &[(9_201, true)]);
    assert_eq!(r, Err(409));
    assert!(in_hold(&reg, ITEM_A_AGAIN));
    // Two copies in one request are the same.
    let mut reg = world();
    let r = install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_A, ITEM_A_AGAIN],
        &[(9_201, true)],
    );
    assert_eq!(r, Err(409));
    assert!(in_hold(&reg, ITEM_A) && in_hold(&reg, ITEM_A_AGAIN));
}

#[test]
fn a_configuration_that_does_not_name_exactly_the_installed_and_new_types_answers_400() {
    let mut reg = world();
    let bay = TestBay::default();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    let before = installed(reg.deltas(), HUB);
    // A type that is neither installed nor new.
    let r = install(&mut reg, &bay, ALICE, &[], &[(9_201, true), (9_203, true)]);
    assert_eq!(r, Err(400));
    // An installed type left out.
    assert_eq!(install(&mut reg, &bay, ALICE, &[], &[]), Err(400));
    // A type named twice.
    let r = install(&mut reg, &bay, ALICE, &[], &[(9_201, true), (9_201, false)]);
    assert_eq!(r, Err(400));
    // A new item whose type the configuration does not carry.
    let r = install(&mut reg, &bay, ALICE, &[ITEM_PRODUCER], &[(9_201, true)]);
    assert_eq!(r, Err(400));
    assert!(in_hold(&reg, ITEM_PRODUCER), "nothing was consumed");
    assert_eq!(installed(reg.deltas(), HUB), before);
}

#[test]
fn an_item_that_cannot_be_dropped_is_refused_with_the_clients_reasons() {
    let mut reg = world();
    let bay = TestBay::default();
    let try_one = |reg: &mut Registry, item_id: i64, ty: i64| {
        install(reg, &bay, ALICE, &[item_id], &[(ty, true)])
    };
    // In a station: not in the pilot's ship (hubController.py:189).
    assert_eq!(try_one(&mut reg, ITEM_DOCKED, 9_201), Err(403));
    // In another pilot's hold: not the pilot's.
    assert_eq!(try_one(&mut reg, ITEM_BOBS, 9_201), Err(403));
    // Gone.
    assert_eq!(try_one(&mut reg, 100_790_999, 9_201), Err(404));
    // Not an upgrade group; an upgrade group with no row.
    assert_eq!(try_one(&mut reg, ITEM_PLAIN, 9_206), Err(400));
    assert_eq!(try_one(&mut reg, ITEM_NO_ROW, TYPE_NO_ROW), Err(400));
    assert!(installed(reg.deltas(), HUB).is_empty());
    assert!(in_hold(&reg, ITEM_DOCKED) && in_hold(&reg, ITEM_BOBS));
}

#[test]
fn the_drop_checks_run_in_the_clients_order_the_hold_check_last() {
    let mut reg = world();
    let bay = TestBay::default();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    // ITEM_DOCKED is the installed type 9_201 and in a station: "already
    // installed" comes before "not in the ship" (hubController.py:179, 189).
    let r = install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_DOCKED],
        &[(9_201, true), (9_201, true)],
    );
    assert_eq!(r, Err(409));
}

/// The consumed item's `OnItemChange` leaves on the clock's queue. `pending`
/// drains only after the client's next call, which left the pilot's hold
/// showing the item for about 20 s.
#[test]
fn an_install_pushes_the_consumed_item_on_the_clock_not_on_pending() {
    let mut reg = world();
    let bay = TestBay::default();
    let _ = reg.take_pending_raw();
    let queued = reg.scheduled_len();
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    let pending: Vec<&str> = reg
        .take_pending_raw()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    assert!(pending.is_empty(), "the push waits for a call: {pending:?}");
    assert!(reg.scheduled_len() > queued, "nothing was scheduled");
    let due = reg.take_due(std::time::Instant::now());
    let names: Vec<&str> = due.iter().map(|p| p.to_push().0).collect();
    assert!(names.contains(&"OnItemChange"), "{names:?}");
    assert!(
        due.iter().all(|p| p.audience() == Some(vec![ALICE])),
        "every push goes to the pilot alone"
    );
}

/// A refusal names its half: the holder alliance, then the role.
#[test]
fn a_manage_refusal_names_the_holder_or_the_role() {
    let reg = world();
    let hub = sov_hub::by_id(reg.deltas(), HUB).expect("the hub");
    assert_eq!(manage_refusal(&reg, &hub, ALICE), None);
    let why = manage_refusal(&reg, &hub, 90_999_999).expect("a stranger is refused");
    assert!(why.starts_with("holder"), "{why}");
    let why = manage_refusal(&reg, &hub, BOB).expect("no role is refused");
    assert!(why.starts_with("manage"), "{why}");
    assert_eq!(manage_refusal(&reg, &hub, DAVE), None, "a station manager");
}

#[test]
fn the_pilot_must_be_within_transfer_range_of_the_hub() {
    let mut reg = world();
    let bay = TestBay::default();
    place(&mut reg, ALICE, far());
    let r = install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]);
    assert_eq!(r, Err(403));
    assert!(in_hold(&reg, ITEM_A));
    place(&mut reg, ALICE, near());
    assert!(install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).is_ok());
    // A pilot who is docked is nowhere near it.
    let mut reg = world();
    let _ = reg.roster_mut().leave(ALICE);
    let r = install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]);
    assert_eq!(r, Err(403));
}

/// The type table with the sovereignty-days attribute on upgrade 9_201.
fn held_world(claim_age_days: i64) -> Registry {
    let rows = format!(
        "type\t9201\t{}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t{}=2.0\tInvented Upgrade\n",
        UPGRADE_GROUPS[0],
        crate::sov_index::ATTR_DEV_INDEX_SOVEREIGNTY
    );
    let mut reg = world_with(content_with(&rows));
    let now = crate::pyutil::filetime_now();
    sov::set_claim(reg.deltas_mut(), SYSTEM, None, now);
    sov::set_claim(
        reg.deltas_mut(),
        SYSTEM,
        Some((ALLIANCE, CORP)),
        now - claim_age_days * crate::seed::DAY,
    );
    reg
}

#[test]
fn a_type_that_needs_a_strategic_level_waits_for_the_days_the_clients_table_gives() {
    let bay = TestBay::default();
    // Level 2 needs 21 days in the client's table (devIndexUtil.timeIndexLevels).
    let mut young = held_world(20);
    let r = install(&mut young, &bay, ALICE, &[ITEM_A], &[(9_201, true)]);
    assert_eq!(r, Err(403));
    assert!(in_hold(&young, ITEM_A));
    let mut old = held_world(22);
    assert!(install(&mut old, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).is_ok());
    // A type without the attribute needs nothing, however young the claim.
    let mut young = held_world(0);
    assert!(install(&mut young, &bay, ALICE, &[ITEM_BIG], &[(9_204, true)]).is_ok());
}

#[test]
fn only_the_holders_station_managers_and_directors_may_manage() {
    let bay = TestBay::default();
    let mut reg = world();
    // Bob: in the holding alliance, no role.
    assert_eq!(
        install(&mut reg, &bay, BOB, &[ITEM_BOBS], &[(9_201, true)]),
        Err(403)
    );
    // Carol: another alliance.
    assert_eq!(install(&mut reg, &bay, CAROL, &[], &[]), Err(403));
    // Dave: a station manager of the holding alliance.
    assert!(install(&mut reg, &bay, DAVE, &[ITEM_DAVES], &[(9_204, true)]).is_ok());
    // Unknown hub.
    assert_eq!(
        configure(&mut reg, 8_900_999, ALICE, &[], &[], &bay, NOW),
        Err(404)
    );
}

#[test]
fn startup_is_paid_and_the_burn_set_when_a_burner_comes_online() {
    let mut reg = world();
    // Startup 2 plus one hour's unit is 3 in the fixture row.
    let bay = TestBay::with(R, 3);
    let got = install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, true)]).unwrap();
    assert_eq!(st(&got), [(9_205, Power::Online)]);
    assert_eq!(*bay.taken.borrow(), [(R, 2)]);
    assert_eq!(bay.burning(R), 3);
}

#[test]
fn a_new_burner_without_the_stock_is_saved_offline_pays_nothing_and_burns_nothing() {
    let mut reg = world();
    let bay = TestBay::with(R, 2);
    let got = install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, true)]).unwrap();
    // The rule alone says Low; saved Low it would later start free.
    assert_eq!(st(&got), [(9_205, Power::Offline)]);
    assert!(!got[0].online);
    assert!(!installed(reg.deltas(), HUB)[0].online);
    assert!(bay.taken.borrow().is_empty());
    assert_eq!(bay.burning(R), 0);
}

#[test]
fn a_burner_that_could_not_start_cannot_start_free_when_the_stock_arrives() {
    let mut reg = world();
    let bay = TestBay::with(R, 2);
    install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, true)]).unwrap();
    // The reagent arrives: a re-run leaves it Offline, nothing is paid.
    bay.set(R, 50);
    assert!(!resimulate_with(&mut reg, HUB, &bay, NOW));
    assert_eq!(st(&installed(reg.deltas(), HUB)), [(9_205, Power::Offline)]);
    assert!(bay.taken.borrow().is_empty());
    // Only an explicit switch-on starts it, and pays the startup.
    let got = install(&mut reg, &bay, ALICE, &[], &[(9_205, true)]).unwrap();
    assert_eq!(st(&got), [(9_205, Power::Online)]);
    assert_eq!(*bay.taken.borrow(), [(R, 2)]);
}

#[test]
fn a_stored_offline_burner_marked_online_that_would_end_low_is_stored_offline() {
    let mut reg = world();
    let bay = TestBay::with(R, 1);
    // An Offline slot saved with the toggle on (not reachable through
    // `configure` any more) must not drift to Low and then start free.
    put_installed(
        reg.deltas_mut(),
        HUB,
        &[Installed {
            type_id: 9_205,
            online: true,
            state: Power::Offline,
        }],
    );
    assert!(resimulate_with(&mut reg, HUB, &bay, NOW));
    let got = installed(reg.deltas(), HUB);
    assert_eq!((got[0].state, got[0].online), (Power::Offline, false));
    assert!(bay.taken.borrow().is_empty());
}

#[test]
fn an_offline_burner_is_not_charged() {
    let mut reg = world();
    let bay = TestBay::with(R, 100);
    install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, false)]).unwrap();
    assert!(bay.taken.borrow().is_empty());
    assert_eq!(bay.burning(R), 0);
    // Switched on later it owes the startup then, not before.
    install(&mut reg, &bay, ALICE, &[], &[(9_205, true)]).unwrap();
    assert_eq!(*bay.taken.borrow(), [(R, 2)]);
    assert_eq!(bay.burning(R), 3);
    // Saving the same configuration again does not charge it twice.
    install(&mut reg, &bay, ALICE, &[], &[(9_205, true)]).unwrap();
    assert_eq!(bay.taken.borrow().len(), 1);
}

#[test]
fn uninstalling_removes_the_upgrade_and_lets_a_waiting_one_run() {
    let mut reg = world();
    let bay = TestBay::default();
    install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_201, true), (9_204, true)],
    )
    .unwrap();
    assert_eq!(online_types(reg.deltas(), SYSTEM), [9_201]);
    assert_eq!(uninstall(&mut reg, HUB, ALICE, 9_201, &bay, NOW), Ok(()));
    let left = installed(reg.deltas(), HUB);
    assert_eq!(
        st(&left),
        [(9_204, Power::Online)],
        "the freed power wakes it"
    );
    assert_eq!(uninstall(&mut reg, HUB, ALICE, 9_201, &bay, NOW), Err(404));
    assert_eq!(uninstall(&mut reg, HUB, BOB, 9_204, &bay, NOW), Err(403));
    assert_eq!(uninstall(&mut reg, HUB, ALICE, 9_204, &bay, NOW), Ok(()));
    assert!(
        reg.deltas().of(KIND).is_empty(),
        "the last one removes the row"
    );
}

#[test]
fn uninstalling_the_last_burner_stops_its_burn() {
    let mut reg = world();
    let bay = TestBay::with(R, 10);
    install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, true)]).unwrap();
    assert_eq!(bay.burning(R), 3);
    uninstall(&mut reg, HUB, ALICE, 9_205, &bay, NOW).unwrap();
    assert_eq!(bay.burning(R), 0);
}

#[test]
fn resimulating_follows_the_stock_without_charging_a_second_startup() {
    let mut reg = world();
    let bay = TestBay::with(R, 3);
    install(&mut reg, &bay, ALICE, &[ITEM_BURNER], &[(9_205, true)]).unwrap();
    let _ = crate::gateway_out::take(&mut reg);
    let rev = reg.deltas().kind_revision(KIND);
    // Nothing moved: nothing is written and nothing is announced.
    bay.set(R, 1);
    assert!(!resimulate_with(&mut reg, HUB, &bay, NOW));
    assert_eq!(reg.deltas().kind_revision(KIND), rev);
    assert!(crate::gateway_out::take(&mut reg).is_empty());
    // The reagent runs out: Online -> Low, the burn stops, the window is told.
    bay.set(R, 0);
    assert!(resimulate_with(&mut reg, HUB, &bay, NOW));
    assert_eq!(st(&installed(reg.deltas(), HUB)), [(9_205, Power::Low)]);
    assert_eq!(bay.burning(R), 0);
    let sent = crate::gateway_out::take(&mut reg);
    assert!(sent.iter().any(|(ty, _, _)| ty == NOTICE_SIMULATED));
    // A deposit brings it back with no new startup (it was Low, not Offline).
    bay.set(R, 1);
    assert!(resimulate_with(&mut reg, HUB, &bay, NOW));
    assert_eq!(st(&installed(reg.deltas(), HUB)), [(9_205, Power::Online)]);
    assert_eq!(*bay.taken.borrow(), [(R, 2)]);
    assert_eq!(bay.burning(R), 3);
}

#[test]
fn installing_tells_the_system_and_the_resources_window() {
    let mut reg = world();
    let bay = TestBay::default();
    let _ = crate::gateway_out::take(&mut reg);
    install(&mut reg, &bay, ALICE, &[ITEM_A], &[(9_201, true)]).unwrap();
    let sent = crate::gateway_out::take(&mut reg);
    let upgrades: Vec<_> = sent
        .iter()
        .filter(|(ty, _, _)| ty == NOTICE_SIMULATED)
        .collect();
    assert_eq!(upgrades.len(), 1);
    assert!(matches!(
        upgrades[0].2,
        Target::SolarSystem { id, .. } if id == SYSTEM
    ));
    let hub_upgrades = crate::gateway::get_bytes(&upgrades[0].1, 1).unwrap();
    assert_eq!(read_id(&hub_upgrades, 1), Some(HUB as u64));
    assert!(
        sent.iter()
            .any(|(ty, _, _)| ty == "eve_public.sovereignty.hub.api.ResourcesSimulatedNotice")
    );
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

fn ground(reg: Registry) -> Ground {
    Ground::Server(Arc::new(RwLock::new(reg)))
}

fn name(rest: &str) -> String {
    format!("{PKG}{rest}")
}

fn ident(v: i64) -> Vec<u8> {
    id(v as u64)
}

fn get_request(hub: i64) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &ident(hub));
    b
}

fn configure_request(hub: i64, items: &[i64], config: &[(i64, bool)]) -> Vec<u8> {
    let mut b = Vec::new();
    for i in items {
        put_bytes(&mut b, 1, &ident(*i));
    }
    for (t, online) in config {
        let mut c = Vec::new();
        put_bytes(&mut c, 1, &ident(*t));
        put_uint(&mut c, 2, u64::from(*online));
        put_bytes(&mut b, 2, &c);
    }
    put_bytes(&mut b, 3, &ident(hub));
    b
}

fn uninstall_request(hub: i64, upgrade_type: i64) -> Vec<u8> {
    let mut up = Vec::new();
    put_bytes(&mut up, 1, &ident(hub));
    put_bytes(&mut up, 3, &ident(upgrade_type));
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &up);
    b
}

fn payload(r: &Reply) -> &[u8] {
    &r.payload.as_ref().expect("a payload").1
}

/// `(installation type, upgrade_type, state)` of every upgrade of a
/// `HubUpgrades` message, in order.
fn decode(hub_upgrades: &[u8]) -> Vec<(u64, u64, u64)> {
    field(hub_upgrades, 2)
        .iter()
        .map(|up| {
            let ident = crate::gateway::get_bytes(up, 1).unwrap();
            let attrs = crate::gateway::get_bytes(up, 2).unwrap();
            let def = crate::gateway::get_bytes(&attrs, 1).unwrap();
            (
                read_id(&def, 1).unwrap(),
                read_id(&ident, 3).unwrap(),
                get_uint(&attrs, 2).unwrap(),
            )
        })
        .collect()
}

fn response_upgrades(r: &Reply) -> Vec<(u64, u64, u64)> {
    decode(&crate::gateway::get_bytes(payload(r), 1).expect("hub_upgrades"))
}

#[test]
fn definitions_list_every_row_with_all_nine_fields() {
    let g = ground(world());
    let r = gateway(&name("GetDefinitionsRequest"), &[], ALICE, &g).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(
        r.payload.as_ref().unwrap().0,
        name("GetDefinitionsResponse")
    );
    let defs = field(payload(&r), 1);
    assert_eq!(defs.len(), fixture_upgrades().len());
    let burner = defs
        .iter()
        .find(|d| read_id(d, 1) == Some(9_205))
        .expect("the burner's definition");
    let want = fixture_upgrades();
    let row = want.get(9_205).unwrap();
    assert_eq!(read_id(burner, 4), Some(R as u64));
    assert_eq!(get_uint(burner, 5), Some(u64::from(row.def.per_hour)));
    assert_eq!(get_uint(burner, 6), Some(u64::from(row.def.startup)));
    assert_eq!(
        crate::gateway::get_bytes(burner, 7),
        Some(row.group.clone().into_bytes())
    );
    let producer = defs.iter().find(|d| read_id(d, 1) == Some(9_203)).unwrap();
    assert_eq!(
        get_uint(producer, 8),
        Some(want.get(9_203).unwrap().def.power_out)
    );
}

#[test]
fn an_install_over_the_wire_answers_the_state_in_request_order() {
    let g = ground(world());
    let body = configure_request(HUB, &[ITEM_BIG, ITEM_A], &[(9_204, true), (9_201, true)]);
    let r = gateway(&name("ProcessConfigurationRequest"), &body, ALICE, &g).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(
        r.payload.as_ref().unwrap().0,
        name("ProcessConfigurationResponse")
    );
    let got = response_upgrades(&r);
    // 9_204 needs 30 of 35, so 9_201 (20) does not fit after it.
    assert_eq!(
        got.iter().map(|(t, _, s)| (*t, *s)).collect::<Vec<_>>(),
        [(9_204, Power::Online as u64), (9_201, Power::Low as u64)]
    );
    let hub = crate::gateway::get_bytes(payload(&r), 1).unwrap();
    assert_eq!(read_id(&hub, 1), Some(HUB as u64));
    // Asking again agrees with what the save answered.
    let again = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    assert_eq!(again.status, 200);
    assert_eq!(response_upgrades(&again), got);
}

#[test]
fn the_wire_maps_refusals_to_the_status_the_client_branches_on() {
    let g = ground(world());
    let ask = |who: i64, hub: i64, items: &[i64], config: &[(i64, bool)]| {
        gateway(
            &name("ProcessConfigurationRequest"),
            &configure_request(hub, items, config),
            who,
            &g,
        )
        .unwrap()
    };
    let clash = ask(
        ALICE,
        HUB,
        &[ITEM_A, ITEM_B],
        &[(9_201, true), (9_202, true)],
    );
    assert_eq!((clash.status, clash.payload), (409, None));
    let docked = ask(ALICE, HUB, &[ITEM_DOCKED], &[(9_201, true)]);
    assert_eq!((docked.status, docked.payload), (403, None));
    let unnamed = ask(ALICE, HUB, &[ITEM_A], &[(9_201, true), (9_203, true)]);
    assert_eq!((unnamed.status, unnamed.payload), (400, None));
    let unknown = ask(ALICE, 8_900_999, &[], &[]);
    assert_eq!(unknown.status, 404);
    let outsider = ask(CAROL, HUB, &[], &[]);
    assert_eq!(outsider.status, 403);
    // Nothing above installed anything.
    let none = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    assert!(response_upgrades(&none).is_empty());
}

#[test]
fn a_read_that_finds_nothing_new_needs_only_the_read_lock() {
    let w = Arc::new(RwLock::new(world()));
    let g = Ground::Server(w.clone());
    gateway(
        &name("ProcessConfigurationRequest"),
        &configure_request(HUB, &[ITEM_A], &[(9_201, true)]),
        ALICE,
        &g,
    )
    .unwrap();
    let rev = w.read().unwrap().deltas().kind_revision(KIND);
    // Hold a read lock across the call: a call that wants the write lock
    // cannot finish. The answer must come back anyway.
    let held = w.read().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let gg = g.clone();
    std::thread::spawn(move || {
        let r = gateway(
            &name("GetHubUpgradesRequest"),
            &get_request(HUB),
            ALICE,
            &gg,
        );
        let _ = tx.send(r.map(|r| r.status));
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(10));
    drop(held);
    assert_eq!(got, Ok(Some(200)), "a no-change read took the write lock");
    assert_eq!(w.read().unwrap().deltas().kind_revision(KIND), rev);
}

#[test]
fn reading_a_hub_is_for_the_holders_only() {
    let g = ground(world());
    let ask = |who: i64, hub: i64| {
        gateway(&name("GetHubUpgradesRequest"), &get_request(hub), who, &g).unwrap()
    };
    assert_eq!(ask(ALICE, 8_900_999).status, 404);
    assert_eq!(ask(CAROL, HUB).status, 403);
    // Bob has no role but is in the alliance: he may look.
    assert_eq!(ask(BOB, HUB).status, 200);
}

#[test]
fn uninstall_over_the_wire_takes_the_type_the_server_gave() {
    let g = ground(world());
    let body = configure_request(HUB, &[ITEM_A, ITEM_BIG], &[(9_201, true), (9_204, true)]);
    gateway(&name("ProcessConfigurationRequest"), &body, ALICE, &g).unwrap();
    let list = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    // The client echoes the identifier's upgrade_type back (proto_parsers.py:21).
    let (_, upgrade_type, _) = response_upgrades(&list)[1];
    let r = gateway(
        &name("UninstallRequest"),
        &uninstall_request(HUB, upgrade_type as i64),
        ALICE,
        &g,
    )
    .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.payload.as_ref().unwrap().0, name("UninstallResponse"));
    let list = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    let left = response_upgrades(&list);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].0, 9_201);
    // Again: not installed.
    let r = gateway(
        &name("UninstallRequest"),
        &uninstall_request(HUB, upgrade_type as i64),
        ALICE,
        &g,
    )
    .unwrap();
    assert_eq!((r.status, r.payload), (404, None));
    let r = gateway(
        &name("UninstallRequest"),
        &uninstall_request(HUB, 9_201),
        BOB,
        &g,
    )
    .unwrap();
    assert_eq!(r.status, 403);
}

#[test]
fn the_real_chain_routes_the_two_writes_here_not_to_the_refusal_stub() {
    let g = ground(world());
    let body = configure_request(HUB, &[ITEM_A], &[(9_201, true)]);
    let r = crate::gateway::answer(&name("ProcessConfigurationRequest"), &body, ALICE, &g);
    assert_eq!(r.status, 200, "the stub's 403 would have answered");
    let r = crate::gateway::answer(
        &name("UninstallRequest"),
        &uninstall_request(HUB, 9_201),
        ALICE,
        &g,
    );
    assert_eq!(r.status, 200);
}

#[test]
fn without_a_world_every_answer_is_404_and_other_types_pass() {
    let g = Ground::local();
    for rest in [
        "GetDefinitionsRequest",
        "GetHubUpgradesRequest",
        "ProcessConfigurationRequest",
        "UninstallRequest",
    ] {
        let r = gateway(&name(rest), &[], ALICE, &g).unwrap();
        assert_eq!((r.status, r.payload), (404, None), "{rest}");
    }
    assert!(gateway(&name("GetRequest"), &[], ALICE, &g).is_none());
    assert!(
        gateway(
            "eve_public.sovereignty.hub.fuel.api.GetRequest",
            &[],
            ALICE,
            &g
        )
        .is_none()
    );
}

// ---------------------------------------------------------------------------
// The follow-up wiring (#79 with #218 and #81)
// ---------------------------------------------------------------------------

fn fuel_clock() -> u64 {
    unix_now()
}

#[test]
fn allocated_is_the_sum_of_the_online_slots_only() {
    let mut reg = world();
    let bay = TestBay::default();
    assert_eq!(crate::sov_resources::allocated(&reg, HUB), (0, 0));
    install(
        &mut reg,
        &bay,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_201, true), (9_204, true)],
    )
    .unwrap();
    // 9_201 is Online and 9_204 is Low (it does not fit beside it): only the
    // Online one counts.
    let a = table(reg.content()).def(9_201).unwrap();
    assert!(a.power > 0);
    assert_eq!(
        crate::sov_resources::allocated(&reg, HUB),
        (a.power, a.workforce)
    );
    let r = crate::sov_resources::hub_resources(&reg, HUB).unwrap();
    assert_eq!(r.power.allocated, a.power);
}

#[test]
fn the_production_bay_is_the_reagent_bay() {
    let mut reg = world();
    let now = fuel_clock();
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R, 3, now);
    assert_eq!(production_bay().amount(reg.deltas(), HUB, R, now), 3);
    // Startup 2 is taken from the stock and the burn is set on it.
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_BURNER],
        &[(9_205, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_205, Power::Online)]);
    let s = crate::sov_hub_fuel::stock(reg.deltas(), HUB, R);
    assert_eq!((s.amount, s.per_hour), (1, 3));
}

#[test]
fn a_reagent_that_runs_out_re_simulates_the_hub_through_the_fuel_hook() {
    let mut reg = world();
    let now = fuel_clock();
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R, 3, now);
    configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_BURNER],
        &[(9_205, true)],
        production_bay(),
        now,
    )
    .unwrap();
    // One unit is left at 3 an hour: an hour on, the bay is dry. The GM line
    // calls the fuel module's slot, which is the upgrades' re-simulation.
    let said = crate::sov_hub_fuel::slash(
        &mut reg,
        Some(ALICE),
        &["age", "fuel", &SYSTEM.to_string(), "1"],
    );
    assert!(said.contains("aged"), "{said}");
    assert_eq!(st(&installed(reg.deltas(), HUB)), [(9_205, Power::Low)]);
}

#[test]
fn the_hook_slot_is_filled_with_the_resimulation() {
    // The slot is process-wide: it is the first use here or an earlier one,
    // and either way the fn in it is the upgrades' own re-simulation.
    let _ = install_hook();
    assert!(!install_hook());
    assert!(std::ptr::fn_addr_eq(
        crate::sov_hub_fuel::resimulate_hook(),
        resimulate as fn(&mut Registry, i64)
    ));
}

fn sky_over(planet: i64, active: bool) -> crate::skyhook::Skyhook {
    crate::skyhook::Skyhook {
        id: 1,
        planet,
        system: SYSTEM,
        corp: CORP,
        active,
        activated_at: 1,
        built_until: 1,
        secured: 0,
        unsecured: 0,
        last_cycle: 0,
        vuln_start: 0,
        vuln_end: 0,
        state: 0,
    }
}

/// Make CORP a real member of a real alliance (the registry the skyhooks count
/// through), which holds the system; the pilots follow.
fn with_registry_alliance(reg: &mut Registry) {
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    let a = st
        .create(CORP, ALICE, "Invented Holders", "IHD", "", "", 1)
        .expect("an alliance");
    st.save(reg.deltas_mut());
    for who in [ALICE, BOB, DAVE] {
        reg.character_mut(who).expect("a pilot").alliance_id = Some(a);
    }
    sov::set_claim(reg.deltas_mut(), SYSTEM, Some((a, CORP)), 1);
}

/// The fixture table with some types redefined (invented numbers).
fn content_with_defs(rows: Vec<(i64, Row)>) -> Content {
    let mut c = content_with("");
    let mut all: Vec<(i64, Row)> = table(&c)
        .rows
        .iter()
        .map(|(t, r)| (*t, r.clone()))
        .collect();
    all.extend(rows);
    c.insert("sov upgrades", UpgradeTable::new(all).0);
    c
}

fn needs(power: u64, workforce: u64, fuel: Option<(u32, u32)>, group: &str) -> Row {
    Row {
        def: Def {
            power,
            workforce,
            fuel: fuel.map(|_| R),
            per_hour: fuel.map_or(0, |f| f.0),
            startup: fuel.map_or(0, |f| f.1),
            power_out: 0,
            workforce_out: 0,
        },
        group: group.to_owned(),
        services: BTreeSet::new(),
        tier: None,
    }
}

#[test]
fn a_skyhook_change_re_simulates_the_hub_budget() {
    let mut reg = world();
    with_registry_alliance(&mut reg);
    let now = fuel_clock();
    // An active power skyhook lifts the budget from the star's 35 to 65.
    let mut sk = sky_over(PLANET_POWER, true);
    crate::skyhook::put(reg.deltas_mut(), &sk);
    crate::skyhook::changed(&mut reg, &sk, now);
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_201, true), (9_204, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online), (9_204, Power::Online)]);
    // The skyhook goes off: the budget falls back to 35 and the last upgrade
    // no longer fits.
    sk.active = false;
    crate::skyhook::put(reg.deltas_mut(), &sk);
    crate::skyhook::changed(&mut reg, &sk, now);
    assert_eq!(
        st(&installed(reg.deltas(), HUB)),
        [(9_201, Power::Online), (9_204, Power::Low)]
    );
}

#[test]
fn removing_a_workforce_skyhook_re_simulates_the_upgrades_it_fed() {
    // 9_204 redefined: invented power 5, workforce 10, burns 3 an hour, startup 2.
    let mut reg = world_with(content_with_defs(vec![(
        9_204,
        needs(5, 10, Some((3, 2)), "gc"),
    )]));
    let now = fuel_clock();
    with_registry_alliance(&mut reg);
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R, 3, now);
    let sk = sky_over(PLANET_LAND, true);
    crate::skyhook::put(reg.deltas_mut(), &sk);
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_BIG],
        &[(9_204, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_204, Power::Online)]);
    // Gone: no workforce left.
    crate::skyhook::remove(&mut reg, &sk, now);
    assert_eq!(st(&installed(reg.deltas(), HUB)), [(9_204, Power::Low)]);
    // The startup was paid once, on the way in.
    assert_eq!(crate::sov_hub_fuel::stock(reg.deltas(), HUB, R).amount, 1);
}

#[test]
fn an_upgrade_that_needs_workforce_in_a_power_only_world_is_not_online() {
    // Power only (the star): 9_204 asks for workforce 10 and owes a startup,
    // 9_201 asks for workforce 10 and owes none.
    let mut reg = world_with(content_with_defs(vec![
        (9_204, needs(5, 10, Some((3, 2)), "gc")),
        (9_201, needs(5, 10, None, "ga")),
    ]));
    let now = fuel_clock();
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R, 3, now);
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_BIG],
        &[(9_204, true)],
        production_bay(),
        now,
    )
    .unwrap();
    // Would end Low and owes a startup: saved Offline, nothing paid.
    assert_eq!(st(&got), [(9_204, Power::Offline)]);
    assert_eq!(crate::sov_hub_fuel::stock(reg.deltas(), HUB, R).amount, 3);
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A],
        &[(9_204, false), (9_201, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_204, Power::Offline), (9_201, Power::Low)]);
}

#[test]
fn the_answer_is_stamped_with_the_bays_oldest_stamp() {
    let mut reg = world();
    // Two rows with different stamps (a deposit later than the other).
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R, 5, 2_000);
    crate::sov_hub_fuel::add(reg.deltas_mut(), HUB, R + 1, 5, 1_000);
    let g = ground(reg);
    let r = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    let inner = get_bytes(payload(&r), 1).unwrap();
    assert_eq!(get_bytes(&inner, 3).unwrap(), timestamp(1_000));
}

#[test]
fn the_answer_is_stamped_now_when_the_bay_is_empty() {
    let g = ground(world());
    let before = unix_now();
    let r = gateway(&name("GetHubUpgradesRequest"), &get_request(HUB), ALICE, &g).unwrap();
    let inner = get_bytes(payload(&r), 1).unwrap();
    let stamp = get_uint(&get_bytes(&inner, 3).unwrap(), 1).unwrap();
    assert!(stamp >= before);
}

#[test]
fn the_gm_line_adds_lists_and_clears() {
    let rows: String = [9201, 9205]
        .iter()
        .map(|t| {
            format!(
                "type\t{t}\t{}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Upgrade\n",
                UPGRADE_GROUPS[0]
            )
        })
        .collect();
    let mut reg = world_with(content_with(&rows));
    let sys = SYSTEM.to_string();
    let words = |line: &str| {
        line.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let say = |reg: &mut Registry, line: &str| {
        let w = words(line);
        let a: Vec<&str> = w.iter().map(String::as_str).collect();
        slash(reg, Some(ALICE), &a)
    };
    let out = say(&mut reg, &format!("upgrade {sys} add 9201"));
    assert!(out.contains("9201 online"), "{out}");
    let out = say(&mut reg, &format!("upgrade {sys} add 9205 offline"));
    assert!(out.contains("9205 offline"), "{out}");
    assert_eq!(
        installed(reg.deltas(), HUB)
            .iter()
            .map(|i| (i.type_id, i.online))
            .collect::<Vec<_>>(),
        [(9_201, true), (9_205, false)]
    );
    let out = say(&mut reg, &format!("upgrade {sys} list"));
    assert!(
        out.contains("9201 online") && out.contains("9205 offline"),
        "{out}"
    );
    // The same group twice is the install path's 409, not a second row.
    let held = |reg: &Registry| {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ALICE && i.type_id == 9_201)
            .count()
    };
    let before = held(&reg);
    let out = say(&mut reg, &format!("upgrade {sys} add 9201"));
    assert!(out.contains("409"), "{out}");
    // A refused install leaves no stray item behind.
    assert_eq!(held(&reg), before);
    let out = say(&mut reg, &format!("upgrade {sys} add 424242"));
    assert!(out.contains("not a hub upgrade"), "{out}");
    let out = say(&mut reg, &format!("upgrade {sys} clear"));
    assert!(out.contains("2 upgrade(s) removed"), "{out}");
    assert!(installed(reg.deltas(), HUB).is_empty());
}

// ---------------------------------------------------------------------------
// SOV-UPG-2 (#80): what an Online upgrade does
// ---------------------------------------------------------------------------

#[test]
fn the_enables_column_parses_services_and_a_site_tier() {
    let (s, t) = parse_enables("cyno_jammer, jump_bridge").unwrap();
    assert_eq!(
        s,
        BTreeSet::from(["cyno_jammer".to_owned(), "jump_bridge".to_owned()])
    );
    assert_eq!(t, None);
    assert_eq!(
        parse_enables("sites:4").unwrap(),
        (BTreeSet::new(), Some(4))
    );
    assert_eq!(parse_enables("-").unwrap(), (BTreeSet::new(), None));
    for bad in ["sites:0", "sites:x", "sites:1,sites:2", "teleport", "sites"] {
        assert!(parse_enables(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_row_with_a_bad_enables_is_not_loaded() {
    let text = format!("{UPGRADES_HEADER}\n9301\t1\t1\t-\t0\t0\tg\t0\t0\tteleport\tx\n");
    let (raw, _) = crate::authored::parse(&text, UPGRADES_HEADER).unwrap();
    assert!(parse_row(&raw[0]).unwrap_err().contains("teleport"));
}

/// The shipped table parses whole and every shipped `sovupg:` population row
/// has an upgrade that carries its group and tier (and no other row claims a
/// tier twice within one group).
#[test]
fn every_shipped_sovupg_band_has_an_upgrade_that_opens_it() {
    let (raw, _) = crate::authored::parse(UPGRADES_SHIPPED, UPGRADES_HEADER).unwrap();
    let mut rows = Vec::new();
    for r in &raw {
        rows.push(parse_row(r).unwrap_or_else(|e| panic!("line {}: {e}", r.line)));
    }
    let (table, dups) = UpgradeTable::new(rows);
    assert!(dups.is_empty(), "{dups:?}");
    let mut tiers = BTreeSet::new();
    for (_, r) in table.rows() {
        if let Some(t) = r.tier {
            assert!(tiers.insert((r.group.clone(), t)), "{} {t} twice", r.group);
        }
    }
    let (pop, _) =
        crate::authored::parse(crate::population::SHIPPED, crate::population::HEADER).unwrap();
    let mut bands = 0;
    for r in &pop {
        if let Some(b) = crate::population::upgrade_band(r.field(1)) {
            bands += 1;
            assert!(
                tiers.contains(&b),
                "{} needs an upgrade of {b:?}",
                r.field(0)
            );
        }
    }
    assert!(bands > 0, "the shipped population has no sovupg row");
}

#[test]
fn upgrade_online_answers_for_type_service_and_site_tier() {
    let mut reg = world();
    let now = fuel_clock();
    let ask =
        |reg: &Registry, w: Want| upgrade_online(reg.deltas(), table(reg.content()), SYSTEM, w);
    assert!(!ask(&reg, Want::Type(9_201)), "nothing installed yet");
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A, ITEM_PRODUCER],
        &[(9_201, true), (9_203, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online), (9_203, Power::Online)]);
    assert!(ask(&reg, Want::Type(9_201)));
    assert!(!ask(&reg, Want::Type(9_202)));
    assert!(ask(&reg, Want::Service("cyno_jammer")));
    assert!(!ask(&reg, Want::Service("jump_bridge")));
    assert!(ask(&reg, Want::Site("ga", 1)));
    assert!(!ask(&reg, Want::Site("ga", 2)), "another tier");
    assert!(!ask(&reg, Want::Site("gb", 1)), "another group");
    // Offline, it does nothing.
    configure(
        &mut reg,
        HUB,
        ALICE,
        &[],
        &[(9_201, false), (9_203, false)],
        production_bay(),
        now,
    )
    .unwrap();
    assert!(!ask(&reg, Want::Service("cyno_jammer")));
    assert!(!ask(&reg, Want::Site("ga", 1)));
    // Online again, then the hub goes neutral: its upgrades do nothing.
    configure(
        &mut reg,
        HUB,
        ALICE,
        &[],
        &[(9_201, true), (9_203, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert!(ask(&reg, Want::Service("cyno_jammer")));
    sov::set_claim(reg.deltas_mut(), SYSTEM, None, 2);
    assert!(!ask(&reg, Want::Service("cyno_jammer")));
    assert!(!ask(&reg, Want::Type(9_201)));
}

#[test]
fn the_hub_lapsing_clears_its_upgrades() {
    let mut reg = world();
    let now = fuel_clock();
    configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A],
        &[(9_201, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(installed(reg.deltas(), HUB).len(), 1);
    let h = sov_hub::hub(reg.deltas(), SYSTEM).unwrap();
    sov_hub::lapse(&mut reg, &h, 5);
    assert!(installed(reg.deltas(), HUB).is_empty());
    assert!(reg.deltas().of(KIND).is_empty(), "the row itself is gone");
}

/// Two upgrades that fit only while the skyhook's power counts; the hub then
/// passes to an alliance the skyhook's corporation is not in.
#[test]
fn a_capture_re_simulates_the_hub_for_the_new_holder() {
    let mut reg = world();
    with_registry_alliance(&mut reg);
    let now = fuel_clock();
    let sk = sky_over(PLANET_POWER, true);
    crate::skyhook::put(reg.deltas_mut(), &sk);
    crate::skyhook::changed(&mut reg, &sk, now);
    let got = configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_201, true), (9_204, true)],
        production_bay(),
        now,
    )
    .unwrap();
    assert_eq!(st(&got), [(9_201, Power::Online), (9_204, Power::Online)]);
    sov::set_claim(
        reg.deltas_mut(),
        SYSTEM,
        Some((OTHER_ALLIANCE, OTHER_CORP)),
        9,
    );
    sov_hub::claim_changed(&mut reg, SYSTEM);
    assert_eq!(
        st(&installed(reg.deltas(), HUB)),
        [(9_201, Power::Online), (9_204, Power::Low)],
        "the skyhook no longer counts for the new holder"
    );
}

#[test]
fn a_corporation_leaving_the_holder_alliance_re_simulates_when_told() {
    let mut reg = world();
    with_registry_alliance(&mut reg);
    let now = fuel_clock();
    let sk = sky_over(PLANET_POWER, true);
    crate::skyhook::put(reg.deltas_mut(), &sk);
    crate::skyhook::changed(&mut reg, &sk, now);
    configure(
        &mut reg,
        HUB,
        ALICE,
        &[ITEM_A, ITEM_BIG],
        &[(9_201, true), (9_204, true)],
        production_bay(),
        now,
    )
    .unwrap();
    reg.deltas_mut()
        .del(crate::alliance::KIND_MEMBER, &[CORP.to_string()]);
    // Nothing told yet: the stored states are as they were.
    assert_eq!(
        st(&installed(reg.deltas(), HUB)),
        [(9_201, Power::Online), (9_204, Power::Online)]
    );
    corp_membership_changed(&mut reg, CORP);
    assert_eq!(
        st(&installed(reg.deltas(), HUB)),
        [(9_201, Power::Online), (9_204, Power::Low)]
    );
    // A corporation with no skyhook changes nothing.
    corp_membership_changed(&mut reg, OTHER_CORP);
}
