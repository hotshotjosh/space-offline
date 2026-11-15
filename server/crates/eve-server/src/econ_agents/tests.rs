//! E1's tests: the driver and the careers over a registry. Every id and row
//! is invented or an `item::SPAWNABLE` fixture type; no `[A]` value is
//! written as a number: the tests read the named consts of `rules`.

use super::*;
use crate::asteroid::{Field, Law, Rock};
use crate::content::Content;
use crate::item::{Edit, Item, Slot};
use crate::model::{Character, FLAG_HANGAR, SandboxConfig};
use crate::service::{Request, ServiceError};
use eve_marshal::PyValue;

const BOT: i64 = 90_000_501;
const BUYER: i64 = 90_000_502;
const STATION: i64 = 60_003_760;
const SYSTEM: i64 = 30_990_142;
const REGION: i64 = 10_990_002;
const HULL_ITEM: i64 = 100_000_501;
/// Merlin, Miner I, 125mm, its round, Veldspar, Tritanium: `item::SPAWNABLE`.
const MERLIN: i64 = 603;
const MINER_I: i64 = 483;
const GUN: i64 = 484;
const ROUND: i64 = 184;
const VELDSPAR: i64 = 1230;
const TRITANIUM: i64 = 34;
const HI_SLOT_0: i64 = 27;
const ROCK0: i64 = crate::asteroid::MIN_ROCK_ID;
const LAW: Law = Law {
    growth: 0.5,
    unit_size: 90.0,
    max_radius: 16_255.0,
};
/// Skill ids [V] the tests grant by the GM command.
const SKILL_ACCOUNTING: i64 = 16_622;
const SKILL_INDUSTRY: i64 = 3_380;
/// Ten game hours, FILETIME.
const HOURS6: i64 = 6 * 3_600 * 10_000_000;

fn rock(i: i64, quantity: i64) -> Rock {
    Rock {
        id: ROCK0 + i,
        type_id: VELDSPAR,
        belt_id: 40_000_003,
        solar_system_id: SYSTEM,
        quantity,
        radius: LAW.radius(quantity),
        position: [80_000.0 + 3_000.0 * i as f64, 0.0, 0.0],
        law: LAW,
    }
}

/// Content with the loaded rows these tests need: Veldspar refines to
/// Tritanium (400 per 100 units), Tritanium and the fixture hulls have a
/// `P_ref`.
fn content() -> Content {
    content_with("")
}

/// [`content`] with `extra` typedata rows beside `typedata::FIXTURE`.
fn content_with(extra: &str) -> Content {
    let mut c = Content::fixture();
    if !extra.is_empty() {
        let text = format!("{}{extra}", crate::typedata::FIXTURE);
        let td = crate::typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", crate::typedata::Installed::build(td));
    }
    let files = {
        let f = crate::reprocess::Files::fixture();
        let td = |id: i64, cat: i64, attrs: &str| {
            format!(
                "type\t{id}\t99001\t{cat}\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t{attrs}\tInvented {id}"
            )
        };
        crate::reprocess::Files {
            materials: vec![format!("{VELDSPAR}\t{TRITANIUM}\t400")],
            random: vec![],
            compress: vec![],
            market: vec![
                format!("{TRITANIUM}\t18\t4\t99501\t2.5\t\t\t1\t\t\t\t\t1\t1"),
                format!("{VELDSPAR}\t462\t25\t99501\t10.0\t\t\t1\t\t\t\t\t100\t1"),
            ],
            prices: vec![format!("{TRITANIUM}\t3.0\tbase\t2.0\t4\t18\t\t1")],
            typedata: vec![
                td(VELDSPAR, 25, "790=990501.0"),
                td(990_501, 16, "379=2.0"),
                td(crate::reprocess::TYPE_REPROCESSING, 16, "379=4.0"),
                td(
                    crate::reprocess::TYPE_REPROCESSING_EFFICIENCY,
                    16,
                    "379=1.0",
                ),
                td(crate::reprocess::TYPE_SCRAPMETAL_PROCESSING, 16, "379=5.0"),
            ],
            ..f
        }
    };
    c.insert(
        "reprocess",
        crate::reprocess::Table::build(&files, true).expect("the rows parse"),
    );
    let prices = [
        format!("{TRITANIUM}\t3.0\tbase\t2.0\t4\t18\t\t1"),
        "35\t10.0\tbase\t1.0\t4\t18\t\t1".to_owned(),
        "21857\t900.0\tbase\t1.0\t7\t99\t\t1".to_owned(),
        format!("{MERLIN}\t300000.0\tbase\t1.0\t6\t99\t\t1"),
    ];
    c.insert(
        "econ_prices",
        crate::econ_prices::Table::parse(&prices).expect("prices parse"),
    );
    c
}

fn character(id: i64, name: &str, bot: bool, balance: f64) -> Character {
    Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: id + 100_000_000,
        station_id: Some(STATION),
        solar_system_id: Some(SYSTEM),
        region_id: REGION,
        balance,
        is_bot: bot,
        ..Character::default()
    }
}

/// A world: the bot and a human buyer docked in one station; `rocks` in the
/// field; the bot flying a Merlin with `fit` `(item, type, flag)` fitted.
fn world(rocks: Vec<Rock>, fit: &[(i64, i64, i64)]) -> Registry {
    world_in(content(), rocks, fit)
}

fn world_in(content: Content, rocks: Vec<Rock>, fit: &[(i64, i64, i64)]) -> Registry {
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            character(BOT, "Invented Agent", true, 1_000_000.0),
            character(BUYER, "Invented Buyer", false, 50_000_000.0),
        ],
        rocks: Field::from_rocks(rocks),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    let hull = *crate::item::find_in(reg.content(), MERLIN).expect("Merlin");
    let mut items = reg.store().items().to_vec();
    items.push(Item {
        item_id: HULL_ITEM,
        type_id: MERLIN,
        owner_id: BOT,
        location_id: STATION,
        flag_id: FLAG_HANGAR,
        quantity: 1,
        group_id: hull.group_id,
        category_id: hull.category_id,
        singleton: true,
    });
    for &(item_id, type_id, flag_id) in fit {
        let t = *crate::item::find_in(reg.content(), type_id).expect("fixture type");
        items.push(Item {
            item_id,
            type_id,
            owner_id: BOT,
            location_id: HULL_ITEM,
            flag_id,
            // A module is one object; a loaded charge is a stack.
            quantity: if t.category_id == crate::item::CATEGORY_CHARGE {
                500
            } else {
                1
            },
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: t.category_id != crate::item::CATEGORY_CHARGE,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: HULL_ITEM + 1_000,
        ..crate::store::Snapshot::default()
    });
    let c = reg.character_mut(BOT).expect("the bot");
    c.ship_id = HULL_ITEM;
    c.ship_type_id = MERLIN;
    reg
}

fn miner() -> Registry {
    world(
        vec![rock(0, 4_000), rock(1, 4_000)],
        &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)],
    )
}

/// Belts so deep that the share cap never binds.
fn deep_belt() -> Registry {
    world(
        vec![rock(0, 200_000), rock(1, 200_000)],
        &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)],
    )
}

fn count(reg: &Registry, who: i64, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.owner_id == who && i.type_id == type_id && !i.singleton)
        .map(|i| i.quantity)
        .sum()
}

fn rocks_left(reg: &Registry) -> i64 {
    reg.rocks().in_system(SYSTEM).map(|r| r.quantity).sum()
}

fn call(
    reg: &mut Registry,
    who: i64,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service: "marketProxy",
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

/// The buyer rests a bid for Tritanium: `qty` at `price`, a day.
fn bid(reg: &mut Registry, price: f64, qty: i64) {
    call(
        reg,
        BUYER,
        "PlaceBuyOrder",
        &[
            PyValue::Int(STATION),
            PyValue::Int(TRITANIUM),
            PyValue::Real(price),
            PyValue::Int(qty),
            PyValue::Int(-1),
            PyValue::Int(1),
            PyValue::Int(1),
            PyValue::Bool(false),
            PyValue::Real(0.0),
        ],
    )
    .expect("the bid rests");
}

/// Train `skill` to `level` outright (the GM grant needs a skill table the
/// fixture content does not carry for every id).
fn train(reg: &mut Registry, who: i64, skill: i64, level: i64) {
    let mut entries: Vec<crate::skill::SkillEntry> = reg
        .skills_for(Some(who))
        .map(|s| s.entries().cloned().collect())
        .unwrap_or_default();
    entries.retain(|e| e.type_id != skill);
    entries.push(crate::skill::SkillEntry {
        type_id: skill,
        level,
        points: 0,
        rank: 1,
    });
    reg.skills
        .insert(who, crate::skill::Skills::restore(entries));
}

/// Park the bot at now, as a bot's log-off would, then settle `hours` game
/// hours of it: `settle_span` with the span named, as `/settle` does.
fn park_and_settle(reg: &mut Registry, career: &str, hours: f64) -> Option<Report> {
    let now = crate::pyutil::filetime_now();
    park(reg, BOT, career, now).expect("parks");
    let a = agent(reg, BOT).expect("the row");
    settle_span(reg, BOT, a, hours * rules::HOUR_S, now)
}

fn bot_rows(reg: &Registry) -> Vec<crate::ledger::Entry> {
    reg.journal()
        .entries()
        .iter()
        .filter(|e| e.owner_id_1 == BOT || e.owner_id_2 == BOT)
        .cloned()
        .collect()
}

fn refs(reg: &Registry) -> Vec<i64> {
    bot_rows(reg).iter().map(|e| e.entry_type_id).collect()
}

/// The ledger stays total: every row the settlement wrote is a ref the
/// economy ledger classifies.
fn assert_classified(reg: &Registry) {
    let s = crate::econ_ledger::summarize(reg.journal().entries());
    assert_eq!(s.unclassified, 0, "an unclassified ref: {:?}", refs(reg));
}

/// Back-date the bot's settled-to mark by six game hours at scale 1.
fn away_six_hours(reg: &mut Registry) {
    let now = crate::pyutil::filetime_now();
    park(reg, BOT, "miner", now).expect("parks");
    let mut a = agent(reg, BOT).expect("row");
    a.settled_to = now - HOURS6;
    write(reg, BOT, a);
}

// ---- parking -------------------------------------------------------------------

#[test]
fn only_a_docked_bot_with_a_settled_career_parks() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    assert_eq!(park(&mut reg, BUYER, "miner", now), Err(Refusal::NotABot));
    assert!(matches!(
        park(&mut reg, BOT, "mission_runner", now),
        Err(Refusal::NoRule(why)) if !why.is_empty()
    ));
    assert!(matches!(
        park(&mut reg, BOT, "no_such_career", now),
        Err(Refusal::NoRule(_))
    ));
    assert_eq!(park(&mut reg, 1, "miner", now), Err(Refusal::NoCharacter));
    assert_eq!(park(&mut reg, BOT, "miner", now), Ok(Career::Miner));
    let a = agent(&reg, BOT).expect("a row");
    assert_eq!((a.career, a.station, a.runs), (Career::Miner, STATION, 0));
    // Parking again changes the career and keeps the mark.
    assert_eq!(park(&mut reg, BOT, "ratter", now + 5), Ok(Career::Ratter));
    let b = agent(&reg, BOT).expect("a row");
    assert_eq!(
        (b.career, b.settled_to, b.parked_at),
        (Career::Ratter, a.settled_to, a.parked_at)
    );
}

#[test]
fn every_settled_career_names_itself_and_parses_back() {
    for c in CAREERS {
        assert_eq!(Career::parse(c.name()), Some(*c));
    }
    let mut names: Vec<&str> = CAREERS.iter().map(|c| c.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CAREERS.len());
    for refused in ["mission_runner", "explorer", "pirate"] {
        assert!(Career::parse(refused).is_none());
        assert!(
            unsettled_reason(refused).is_some(),
            "{refused} has no reason"
        );
    }
}

#[test]
fn the_agent_row_round_trips_and_a_torn_row_is_no_agent() {
    let a = Agent {
        career: Career::Builder,
        station: 7,
        parked_at: 11,
        settled_to: 22,
        day: 3,
        used_h: 1.25,
        runs: 9,
    };
    assert_eq!(Agent::from_row(&a.to_row()), Some(a));
    assert_eq!(Agent::from_row(&["miner".to_owned()]), None);
    let mut torn = a.to_row();
    torn[0] = "nonsense".to_owned();
    assert_eq!(Agent::from_row(&torn), None);
    torn = a.to_row();
    torn[5] = "NaN".to_owned();
    assert_eq!(Agent::from_row(&torn), None);
}

#[test]
fn a_parked_agent_survives_the_world_file() {
    let mut reg = miner();
    park(&mut reg, BOT, "miner", crate::pyutil::filetime_now()).expect("parks");
    let before = agent(&reg, BOT).expect("a row");
    let snap = reg.to_snapshot();
    let mut again = miner();
    again.apply_snapshot(snap);
    assert_eq!(agent(&again, BOT), Some(before));
}

// ---- the miner ----------------------------------------------------------------

#[test]
fn a_miners_settlement_is_the_pure_rules_applied() {
    let mut reg = miner();
    let rig = gather::rig_of(&reg, BOT, Stuff::Ore).expect("a fitted miner");
    let hours = 6.0;
    let work_h = rules::active_hours(hours * rules::HOUR_S, 0.0);
    let m3 = rules::mined_m3(&rig, work_h);
    let unit = crate::item::find_in(reg.content(), VELDSPAR)
        .expect("ore")
        .volume;
    let views: Vec<rules::RockView> = reg
        .rocks()
        .in_system(SYSTEM)
        .map(|r| rules::RockView {
            id: r.id,
            quantity: r.quantity,
            unit_m3: unit,
        })
        .collect();
    let expected: i64 = rules::cut_plan(&views, m3).iter().map(|&(_, n)| n).sum();
    assert!(expected > 0, "the fixture must mine something");
    let before = rocks_left(&reg);

    let report = park_and_settle(&mut reg, "miner", hours).expect("a report");

    // The rocks gave up exactly the planned units, and nothing else mined.
    assert_eq!(before - rocks_left(&reg), expected);
    assert!((report.work_h - work_h).abs() < 1e-9);
    // No bid yet: the ore was refined into Tritanium (the real plan, with a
    // tax) and the minerals rest as an ask (soak A: they used to sit in the
    // hangar unlisted). Ore conservation: what left the rocks is what was
    // refined, a portion at a time.
    assert_eq!(count(&reg, BOT, VELDSPAR), expected % 100);
    let portions = expected / 100;
    assert!(portions > 0);
    assert!(count(&reg, BOT, TRITANIUM) <= portions * 400);
    assert!(
        reg.settle_has_ask(BOT, TRITANIUM, STATION),
        "the minerals rest as an ask"
    );
    // The refinery tax is a ref-127 row on the bot's wallet.
    assert!(refs(&reg).contains(&crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX));
    assert_classified(&reg);
    assert_eq!(agent(&reg, BOT).expect("still parked").runs, 1);
}

#[test]
fn the_minerals_are_sold_into_a_players_bid_with_the_real_fees() {
    let mut reg = miner();
    bid(&mut reg, 2.0, 1_000_000);
    let report = park_and_settle(&mut reg, "miner", 6.0).expect("a report");

    let sold_to_buyer = count(&reg, BUYER, TRITANIUM);
    assert!(sold_to_buyer > 0, "the bid filled: {}", report.line());
    // The minerals went to the buyer, not into thin air.
    assert_eq!(count(&reg, BOT, TRITANIUM), 0);
    // The seller got the bid price less the sales tax (ref 54) and the
    // refinery's tax (ref 127).
    let rows = refs(&reg);
    for r in [2, 54, crate::reprocess::ENTRY_TYPE_REPROCESSING_TAX] {
        assert!(rows.contains(&r), "ref {r} in {rows:?}");
    }
    assert!(report.isk != 0.0);
    assert_classified(&reg);
}

#[test]
fn a_miner_never_takes_more_than_its_share_of_a_belt() {
    // Two small rocks and a huge appetite: the share caps the cut.
    let mut reg = world(
        vec![rock(0, 60), rock(1, 60)],
        &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)],
    );
    park_and_settle(&mut reg, "miner", 24.0).expect("a report");
    let left = rocks_left(&reg);
    assert!(
        left as f64 >= 120.0 * (1.0 - rules::ROCK_SHARE) - 1.0,
        "{left} left of 120"
    );
}

// Invented ice, gas and a harvester, in the typedata's own format.
const ICE: i64 = 990_920_001;
const GAS: i64 = 990_920_002;
const HARVESTER: i64 = 990_920_003;

fn extra_types() -> String {
    format!(
        "type\t{ICE}\t465\t25\t1\t1\t100.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Ice\n\
         type\t{GAS}\t711\t2\t1\t1\t10.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Gas\n\
         effect\t2726\t2\t0\t73\t6\t54\t0\t0\tminingClouds\teffects.CloudMining\n\
         type\t{HARVESTER}\t737\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t2726\t12,16,2726\t\
         6=1.0;30=1.0;50=1.0;54=15000.0;73=60000.0;77=10.0\tFixture Harvester\n"
    )
}

fn field_of(type_id: i64, n: i64, quantity: i64) -> Vec<Rock> {
    (0..n)
        .map(|i| Rock {
            type_id,
            ..rock(i, quantity)
        })
        .collect()
}

#[test]
fn an_ice_miner_cuts_ice_and_an_ore_miner_leaves_it() {
    let fit = [(HULL_ITEM + 1, MINER_I, HI_SLOT_0)];
    let mut ice = world_in(content_with(&extra_types()), field_of(ICE, 2, 500), &fit);
    let r = park_and_settle(&mut ice, "ice_miner", 6.0).expect("a report");
    assert!(count(&ice, BOT, ICE) > 0, "{}", r.line());
    assert!(rocks_left(&ice) < 1_000);
    // The same belt, an ore miner: the laser cuts ice but the career does
    // not gather it.
    let mut ore = world_in(content_with(&extra_types()), field_of(ICE, 2, 500), &fit);
    let r = park_and_settle(&mut ore, "miner", 6.0).expect("a report");
    assert_eq!(rocks_left(&ore), 1_000, "{}", r.line());
    assert_eq!(count(&ore, BOT, ICE), 0);
}

#[test]
fn a_gas_huffer_needs_a_harvester_and_clouds() {
    let mut reg = world_in(
        content_with(&extra_types()),
        field_of(GAS, 2, 500),
        &[(HULL_ITEM + 1, HARVESTER, HI_SLOT_0)],
    );
    let r = park_and_settle(&mut reg, "gas_huffer", 6.0).expect("a report");
    assert!(count(&reg, BOT, GAS) > 0, "{}", r.line());
    assert!(rocks_left(&reg) < 1_000);
    // A laser is no harvester: the same clouds, a Miner I, nothing cut.
    let mut laser = world_in(
        content_with(&extra_types()),
        field_of(GAS, 2, 500),
        &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)],
    );
    let r = park_and_settle(&mut laser, "gas_huffer", 6.0).expect("a report");
    assert_eq!(rocks_left(&laser), 1_000);
    assert!(
        r.notes.iter().any(|n| n.contains("gas harvester")),
        "{:?}",
        r.notes
    );
}

#[test]
fn splitting_a_span_does_not_change_the_work_or_the_ore() {
    let whole = {
        let mut reg = deep_belt();
        park_and_settle(&mut reg, "miner", 6.0).expect("a report");
        (rocks_left(&reg), agent(&reg, BOT).expect("row").used_h)
    };
    let split = {
        let mut reg = deep_belt();
        let now = crate::pyutil::filetime_now();
        park(&mut reg, BOT, "miner", now).expect("parks");
        for _ in 0..2 {
            let a = agent(&reg, BOT).expect("row");
            settle_span(&mut reg, BOT, a, 3.0 * rules::HOUR_S, now).expect("a report");
        }
        (rocks_left(&reg), agent(&reg, BOT).expect("row").used_h)
    };
    assert!((whole.1 - split.1).abs() < 1e-9, "the same hours of work");
    // Whole units per rock per settlement: at most a unit a rock apart.
    assert!(
        (whole.0 - split.0).abs() <= 2,
        "{} against {}",
        whole.0,
        split.0
    );
}

#[test]
fn a_settlement_only_makes_what_the_rules_name() {
    let mut reg = deep_belt();
    bid(&mut reg, 2.0, 1_000_000);
    let r = park_and_settle(&mut reg, "miner", 6.0).expect("a report");
    for t in r.goods.keys() {
        assert!(
            [VELDSPAR, TRITANIUM].contains(t),
            "type {t} is neither the ore nor its refine: {}",
            r.line()
        );
    }
}

#[test]
fn a_hull_without_lasers_mines_nothing_and_says_so() {
    let mut reg = world(vec![rock(0, 4_000)], &[]);
    let report = park_and_settle(&mut reg, "miner", 6.0).expect("a report");
    assert_eq!(rocks_left(&reg), 4_000);
    assert!(
        report.notes.iter().any(|n| n.contains("mining laser")),
        "{:?}",
        report.notes
    );
    assert!(report.goods.is_empty());
}

#[test]
fn an_empty_belt_gives_an_empty_hangar() {
    let mut reg = world(vec![], &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)]);
    let report = park_and_settle(&mut reg, "miner", 6.0).expect("a report");
    assert!(report.goods.is_empty());
    assert!(
        report.notes.iter().any(|n| n.contains("rocks")),
        "{:?}",
        report.notes
    );
}

#[test]
fn a_day_has_a_cap_of_work() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "miner", now).expect("parks");
    let mut worked = 0.0;
    for _ in 0..6 {
        let a = agent(&reg, BOT).expect("row");
        let r = settle_span(&mut reg, BOT, a, 24.0 * rules::HOUR_S, now).expect("a report");
        worked += r.work_h;
    }
    assert!(worked <= rules::DAY_HOURS_CAP + 1e-9, "{worked}");
    assert!(
        (worked - rules::DAY_HOURS_CAP).abs() < 1e-9,
        "the cap is reached: {worked}"
    );
    // The next game day starts fresh.
    reg.deltas_mut().world_mut().expect("world").day += 1;
    let a = agent(&reg, BOT).expect("row");
    let r = settle_span(&mut reg, BOT, a, 24.0 * rules::HOUR_S, now).expect("a report");
    assert!(r.work_h > 0.0);
}

#[test]
fn a_span_too_short_to_settle_is_left_for_later() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "miner", now).expect("parks");
    // Two minutes of wall time at scale 1.
    assert!(settle(&mut reg, BOT, now + 2 * 60 * 10_000_000).is_none());
    assert_eq!(agent(&reg, BOT).expect("row").runs, 0);
}

#[test]
fn the_game_clock_stretches_a_wall_span_into_more_work() {
    // Ten wall minutes at scale 60 are ten game hours.
    let mut reg = miner();
    crate::game_clock::set(
        &mut reg,
        crate::game_clock::GameClock::new(60).expect("scale"),
    );
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "miner", now).expect("parks");
    let r = settle(&mut reg, BOT, now + 10 * 60 * 10_000_000).expect("a report");
    assert!((r.span_h - 10.0).abs() < 1e-9, "{}", r.span_h);
    assert!((r.work_h - 10.0 * rules::DUTY).abs() < 1e-9, "{}", r.work_h);
}

#[test]
fn a_cold_systems_bite_is_the_delta_the_next_layout_reads() {
    let mut reg = miner();
    let r = rock(0, 500);
    let taken = crate::asteroid::settle_bite_cold(&mut reg, &r, 120);
    assert_eq!(taken, 120);
    let (k, v) = crate::asteroid::delta_row(&Rock {
        quantity: 380,
        radius: LAW.radius(380),
        ..r
    });
    assert_eq!(reg.deltas().of(crate::asteroid::KIND).get(&k), Some(&v));
    // A bite past what the rock holds takes what it holds, and leaves 0.
    assert_eq!(crate::asteroid::settle_bite_cold(&mut reg, &r, 9_999), 500);
    let (_, gone) = crate::asteroid::delta_row(&Rock { quantity: 0, ..r });
    assert_eq!(reg.deltas().of(crate::asteroid::KIND).get(&k), Some(&gone));
    assert_eq!(crate::asteroid::settle_bite_cold(&mut reg, &r, 0), 0);
}

// ---- the ratter ---------------------------------------------------------------

/// A Merlin with one 125mm and 500 rounds of `ROUND` loaded, and `spare`
/// more in the hangar.
fn ratter(spare: i64) -> Registry {
    let mut reg = world(
        vec![],
        &[
            (HULL_ITEM + 1, GUN, HI_SLOT_0),
            (HULL_ITEM + 2, ROUND, HI_SLOT_0),
        ],
    );
    if spare > 0 {
        reg.apply_items(&[Edit::Create {
            type_id: ROUND,
            quantity: spare,
            to: Slot::new(BOT, STATION, FLAG_HANGAR),
        }])
        .expect("spares");
    }
    reg
}

#[test]
fn a_ratters_guns_are_read_from_the_fit() {
    let reg = ratter(0);
    let g = combat::guns_of(&reg, BOT).expect("a loaded gun");
    assert_eq!(g.ammo, ROUND);
    assert!(g.battery.dps > 0.0 && g.battery.shots_per_s > 0.0);
    assert!(g.fitted.contains(&GUN));
    assert_eq!(g.loaded, vec![(HULL_ITEM + 2, 500)]);
    // An empty gun, or no gun, is no battery.
    let none = world(vec![], &[(HULL_ITEM + 1, GUN, HI_SLOT_0)]);
    assert!(combat::guns_of(&none, BOT).is_none());
    assert!(combat::guns_of(&miner(), BOT).is_none());
}

#[test]
fn a_ratter_is_paid_through_the_bounty_system_and_burns_rounds() {
    let mut reg = ratter(300);
    let rounds_before = count(&reg, BOT, ROUND) + 500;
    let report = park_and_settle(&mut reg, "ratter", 8.0).expect("a report");
    assert!(report.isk > 0.0, "{}", report.line());
    // The payout is the aggregated ref-85 row, and nothing unclassified.
    assert!(
        refs(&reg).contains(&crate::npc::bounty::REF_BOUNTY_PRIZES),
        "{:?}",
        refs(&reg)
    );
    assert_classified(&reg);
    // The DBS drained the system.
    assert!(
        crate::econ_dbs::multiplier(&reg, SYSTEM, crate::pyutil::filetime_now())
            <= crate::econ_dbs::DEFAULT
    );
    // The rule fired rounds, and the loot it drew is in the hangar (a
    // seeded draw may be rounds of the same type, so the stack count is
    // not the test of the fire: `rounds_come_off_the_spares_first` is).
    assert!(
        report.notes.iter().any(|n| n.contains("rounds")),
        "{:?}",
        report.notes
    );
    assert!(rounds_before > 0);
    // Nothing pending is left unpaid for this agent.
    assert!(crate::npc::bounty::pending(&reg, BOT, SYSTEM).is_none());
}

#[test]
fn rounds_come_off_the_spares_first_then_the_loaded_stack() {
    let mut reg = ratter(300);
    let spare: Vec<(i64, i64)> = combat::spares(&reg, BOT, STATION, ROUND);
    let loaded = vec![(HULL_ITEM + 2, 500)];
    assert_eq!(spare.iter().map(|s| s.1).sum::<i64>(), 300);
    let mut notes = Vec::new();
    combat::consume(&mut reg, &spare, &loaded, 350, &mut notes);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        count(&reg, BOT, ROUND),
        450,
        "300 spare + 500 loaded - 350 fired"
    );
    let still_loaded = reg.store().get(HULL_ITEM + 2).map(|i| i.quantity);
    assert_eq!(still_loaded, Some(450), "the spares went first");
    // More than there is: everything goes, and nothing underflows.
    combat::consume(&mut reg, &[], &[(HULL_ITEM + 2, 450)], 10_000, &mut notes);
    assert_eq!(count(&reg, BOT, ROUND), 0);
}

#[test]
fn a_ratter_without_rounds_fights_nothing() {
    let mut reg = world(vec![], &[(HULL_ITEM + 1, GUN, HI_SLOT_0)]);
    let report = park_and_settle(&mut reg, "ratter", 8.0).expect("a report");
    assert_eq!(report.isk, 0.0);
    assert!(
        report.notes.iter().any(|n| n.contains("no loaded guns")),
        "{:?}",
        report.notes
    );
}

#[test]
fn a_ratter_stops_when_the_rounds_do() {
    // One round: fewer than any one squad needs.
    let mut reg = world(vec![], &[(HULL_ITEM + 1, GUN, HI_SLOT_0)]);
    reg.apply_items(&[Edit::Create {
        type_id: ROUND,
        quantity: 1,
        to: Slot::new(BOT, HULL_ITEM, HI_SLOT_0),
    }])
    .expect("one round loaded");
    let report = park_and_settle(&mut reg, "ratter", 8.0).expect("a report");
    assert_eq!(report.isk, 0.0, "{}", report.line());
    assert!(
        report.notes.iter().any(|n| n.contains("Ammo")),
        "{:?}",
        report.notes
    );
}

#[test]
fn a_site_runner_draws_a_harder_pool_and_is_otherwise_a_ratter() {
    let mut reg = ratter(300);
    let report = park_and_settle(&mut reg, "site_runner", 8.0).expect("a report");
    assert!(report.isk > 0.0, "{}", report.line());
    assert_classified(&reg);
}

#[test]
fn a_settlement_draws_the_same_squads_again() {
    let draw = |reg: &Registry| {
        (0..4)
            .map(|k| crate::npc_gen::belt_squad_in(reg, SYSTEM, &[BOT as u64, 0, k]).members)
            .collect::<Vec<_>>()
    };
    assert_eq!(draw(&ratter(0)), draw(&ratter(0)));
}

// ---- the trader ---------------------------------------------------------------

#[test]
fn an_unskilled_trader_earns_nothing_and_a_skilled_one_does() {
    let mut reg = miner();
    reg.character_mut(BOT).expect("bot").balance = 50_000_000.0;
    let r = park_and_settle(&mut reg, "trader", 8.0).expect("a report");
    assert_eq!(
        r.isk,
        0.0,
        "the NPC loop loses at Accounting 0: {}",
        r.line()
    );
    assert!(r.notes.iter().any(|n| n.contains("nothing traded")));

    let mut reg = miner();
    reg.character_mut(BOT).expect("bot").balance = 50_000_000.0;
    train(&mut reg, BOT, SKILL_ACCOUNTING, 5);
    let rate = reg.settle_sales_tax_rate(BOT);
    assert!(
        rules::trade_edge(
            crate::econ_exchange::TRADE_GOOD_ASK,
            crate::econ_exchange::TIER_A_BID,
            rate
        ) > 0.0,
        "Accounting V must make the loop pay"
    );
    let r = park_and_settle(&mut reg, "trader", 8.0).expect("a report");
    assert!(r.isk > 0.0, "{}", r.line());
    // Both sides are NPC fills to the ledger: a sink, a faucet and a tax.
    let s = crate::econ_ledger::summarize(reg.journal().entries());
    assert!(s.faucet > 0.0 && s.sink > 0.0);
    assert_classified(&reg);
    // The margin is the edge on what was spent.
    let t = rules::trade_runs(
        50_000_000.0,
        r.work_h,
        rules::TRADER_TURNOVER_DAY * r.work_h / rules::DAY_HOURS_CAP,
        crate::econ_exchange::TRADE_GOOD_ASK,
        crate::econ_exchange::TIER_A_BID,
        rate,
    );
    assert!((r.isk - (t.revenue - t.tax - t.spent)).abs() < 1.0);
}

/// #141: the offline trader's bid obeys the same cap as the online book: a
/// raised NPC buy dial cannot lift it past `max(bid, retail ask)`, so the
/// settled trader mints no more than the 1.0 loop; a dial under 1 cuts it.
#[test]
fn the_npc_buy_dial_cannot_lift_the_trader_past_the_online_cap() {
    let earn = |npc_buy: f64| {
        let mut reg = miner();
        reg.character_mut(BOT).expect("bot").balance = 50_000_000.0;
        train(&mut reg, BOT, SKILL_ACCOUNTING, 5);
        let mut s = crate::econ_scale::EconScale::ONE;
        s.npc_buy = npc_buy;
        crate::econ_scale::set(&mut reg, s);
        park_and_settle(&mut reg, "trader", 8.0)
            .expect("a report")
            .isk
    };
    let base = earn(1.0);
    assert!(base > 0.0);
    assert_eq!(earn(2.5), base);
    assert_eq!(earn(1000.0), base);
    assert!(earn(0.9) < base);
}

// ---- the builder --------------------------------------------------------------

const BLUEPRINT: i64 = 21858;
const PRODUCT: i64 = 21857;

fn builder(trit: i64) -> Registry {
    let mut reg = miner();
    reg.character_mut(BOT).expect("bot").balance = 1_000_000_000.0;
    train(&mut reg, BOT, SKILL_INDUSTRY, 1);
    let mut edits = vec![Edit::Create {
        type_id: BLUEPRINT,
        quantity: 1,
        to: Slot::new(BOT, STATION, FLAG_HANGAR),
    }];
    if trit > 0 {
        edits.push(Edit::Create {
            type_id: TRITANIUM,
            quantity: trit,
            to: Slot::new(BOT, STATION, FLAG_HANGAR),
        });
    }
    reg.apply_items(&edits).expect("the blueprint and minerals");
    reg
}

fn jobs_of(reg: &Registry) -> Vec<crate::industry::Job> {
    reg.jobs()
        .for_owner(BOT, true)
        .into_iter()
        .copied()
        .collect()
}

#[test]
fn a_builder_installs_what_its_hangar_covers_and_a_later_settlement_delivers() {
    let per_run = 40;
    let mut reg = builder(per_run * 100);
    let r = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    let jobs = jobs_of(&reg);
    assert_eq!(jobs.len(), 1, "{}", r.line());
    assert!(jobs[0].runs >= 1 && jobs[0].runs <= rules::planned_runs(120));
    assert!(!jobs[0].completed());
    // The materials were consumed by the install, and the fee is a ref-56 row.
    assert!(count(&reg, BOT, TRITANIUM) < per_run * 100);
    assert!(refs(&reg).contains(&56), "{:?}", refs(&reg));
    assert_classified(&reg);

    // Nothing is delivered while the job runs.
    let again = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    assert_eq!(count(&reg, BOT, PRODUCT), 0, "{}", again.line());

    // Age the job past its end; the next settlement delivers it.
    let mut done = jobs_of(&reg)[0];
    done.end_date = crate::pyutil::filetime_now() - 10_000_000;
    reg.jobs_mut().commit(done);
    let r = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    assert!(
        jobs_of(&reg).iter().any(|j| j.completed()),
        "the job was delivered: {}",
        r.line()
    );
}

#[test]
fn a_builder_without_materials_or_a_margin_builds_nothing() {
    let mut reg = builder(0);
    // A hangar with no minerals and no asks to buy from.
    let r = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    assert!(jobs_of(&reg).is_empty(), "{}", r.line());
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("short") || n.contains("budget")),
        "{:?}",
        r.notes
    );
}

#[test]
fn a_builder_buys_what_it_lacks_from_a_players_ask() {
    let mut reg = builder(0);
    // The buyer lists minerals under the P_ref cap.
    reg.apply_items(&[Edit::Create {
        type_id: TRITANIUM,
        quantity: 10_000,
        to: Slot::new(BUYER, STATION, FLAG_HANGAR),
    }])
    .expect("stock");
    let item = reg
        .store()
        .items()
        .iter()
        .find(|i| i.owner_id == BUYER && i.type_id == TRITANIUM)
        .expect("the stack")
        .item_id;
    let line = crate::pyutil::keyval(vec![
        ("stationID", PyValue::Int(STATION)),
        ("typeID", PyValue::Int(TRITANIUM)),
        ("itemID", PyValue::Int(item)),
        ("price", PyValue::Real(2.0)),
        ("quantity", PyValue::Int(10_000)),
    ]);
    call(
        &mut reg,
        BUYER,
        "PlaceMultiSellOrder",
        &[
            PyValue::List(vec![line]),
            PyValue::Bool(false),
            PyValue::Int(3),
            PyValue::Real(0.0),
        ],
    )
    .expect("the ask rests");
    let r = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    assert!(
        !jobs_of(&reg).is_empty(),
        "it bought and installed: {}",
        r.line()
    );
    assert_classified(&reg);
}

// ---- the farmer ---------------------------------------------------------------

#[test]
fn a_farmers_colony_is_tended_and_its_goods_land_in_the_hangar() {
    let (mut reg, owner) = crate::service::planet::colony_world_for_agents(20);
    reg.character_mut(owner).expect("the owner").is_bot = true;
    let now = crate::pyutil::filetime_now();
    park(&mut reg, owner, "pi_farmer", now).expect("parks");
    let a = agent(&reg, owner).expect("row");
    let r = settle_span(&mut reg, owner, a, 20.0 * rules::HOUR_S, now).expect("a report");
    assert!(
        r.notes.iter().any(|n| n.contains("1 colonies")),
        "{:?}",
        r.notes
    );
    assert!(
        r.goods.values().any(|n| *n > 0),
        "the colony's product reached the hangar: {}",
        r.line()
    );
    // A farmer who owns nothing settles to a note and no goods.
    let mut empty = miner();
    let r = park_and_settle(&mut empty, "pi_farmer", 20.0).expect("a report");
    assert!(
        r.notes.iter().any(|n| n.contains("no colonies")),
        "{:?}",
        r.notes
    );
}

// ---- the three entry points -----------------------------------------------------

#[test]
fn login_settles_once_and_drops_the_row() {
    let mut reg = miner();
    away_six_hours(&mut reg);
    let before = rocks_left(&reg);
    on_login(&mut reg, BOT);
    assert!(rocks_left(&reg) < before, "the settlement ran");
    assert!(agent(&reg, BOT).is_none(), "the bot is live again");
    // A second login settles nothing.
    let rocks = rocks_left(&reg);
    on_login(&mut reg, BOT);
    assert_eq!(rocks_left(&reg), rocks);
    assert_eq!(reg.ext::<Book>().settled(), 1);
}

#[test]
fn the_sweep_settles_a_slice_of_the_agents_that_are_not_online() {
    let mut reg = miner();
    away_six_hours(&mut reg);
    let now = crate::pyutil::filetime_now();
    let before = rocks_left(&reg);
    assert_eq!(sweep(&mut reg, now, 8), 1);
    assert!(rocks_left(&reg) < before);
    // The row stays: the bot is still away.
    assert!(agent(&reg, BOT).is_some());
    // Settled up to now: nothing is due.
    assert_eq!(sweep(&mut reg, now, 8), 0);
}

#[test]
fn a_sweep_goes_on_tick_after_tick_until_every_due_agent_is_settled() {
    // Five bots with no ship: each settles to a note, which is enough to
    // count.
    let ids: Vec<i64> = (0..5).map(|i| BOT + 10 + i).collect();
    let mut reg = Registry::new(SandboxConfig {
        characters: ids
            .iter()
            .map(|&id| character(id, &format!("Invented Agent {id}"), true, 1_000.0))
            .collect(),
        ..SandboxConfig::default()
    });
    let now = crate::pyutil::filetime_now();
    for &id in &ids {
        park(&mut reg, id, "miner", now).expect("parks");
        let mut a = agent(&reg, id).expect("row");
        a.settled_to = now - HOURS6;
        write(&mut reg, id, a);
    }
    let period = (SWEEP_EVERY_H * 3_600.0 * 1.0e7) as i64;
    let mut ticks = 0;
    // The period passes; the sweep begins and runs on.
    while reg.ext::<Book>().settled() < ids.len() as u64 && ticks < 20 {
        tick(&mut reg, Instant::now(), now + period);
        ticks += 1;
    }
    assert_eq!(reg.ext::<Book>().settled(), ids.len() as u64);
    assert_eq!(ticks, ids.len().div_ceil(SWEEP_SLICE), "a slice a tick");
    // One more tick finds nothing due and ends the sweep; then it sleeps.
    tick(&mut reg, Instant::now(), now + period);
    tick(&mut reg, Instant::now(), now + period + 1);
    assert_eq!(reg.ext::<Book>().settled(), ids.len() as u64);
}

#[test]
fn a_row_for_a_character_who_is_gone_is_dropped_by_the_sweep() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    reg.deltas_mut().put(
        KIND,
        vec!["424242".to_owned()],
        Agent {
            career: Career::Miner,
            station: STATION,
            parked_at: now,
            settled_to: now - HOURS6,
            day: 0,
            used_h: 0.0,
            runs: 0,
        }
        .to_row(),
    );
    assert_eq!(sweep(&mut reg, now, 8), 0);
    assert!(agent(&reg, 424_242).is_none());
}

#[test]
fn the_tick_stage_waits_a_sweep_period() {
    let mut reg = miner();
    away_six_hours(&mut reg);
    let now = crate::pyutil::filetime_now();
    let period = (SWEEP_EVERY_H * 3_600.0 * 1.0e7) as i64;
    tick(&mut reg, Instant::now(), now + period);
    assert_eq!(reg.ext::<Book>().settled(), 1);
    // The same instant again, and half a period later: the stage sleeps.
    tick(&mut reg, Instant::now(), now + period);
    tick(&mut reg, Instant::now(), now + period + period / 2);
    assert_eq!(reg.ext::<Book>().settled(), 1);
}

#[test]
fn selecting_the_character_is_a_login_and_a_stale_row_goes_at_the_sweep() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "miner", now).expect("parks");
    reg.session_opened(crate::session_table::SessionEntry {
        client_id: 7,
        session_id: 1,
        user_id: 1,
        character_id: None,
    });
    reg.session_selected_character(7, BOT);
    // Selecting the character is a login: the hook ran and dropped the row.
    assert!(agent(&reg, BOT).is_none());
    // A row written while online (a stale park) goes at the next sweep.
    park(&mut reg, BOT, "miner", now).expect("parks");
    assert_eq!(sweep(&mut reg, now, 8), 0);
    assert!(agent(&reg, BOT).is_none());
}

#[test]
fn the_rollover_slice_settles_a_parked_miner_before_the_reset() {
    let mut reg = miner();
    away_six_hours(&mut reg);
    let now = crate::pyutil::filetime_now();
    let day = crate::rollover::Day {
        day: 1,
        days: 1,
        now,
    };
    assert_eq!(on_rollover(&mut reg, &day), 1);
    assert!(rocks_left(&reg) < 8_000);
}

/// `n` parked agents with no ship, each six hours behind `now`.
fn parked_crowd(n: usize, now: BlueTime) -> (Registry, Vec<i64>) {
    let ids: Vec<i64> = (0..n as i64).map(|i| BOT + 10 + i).collect();
    let mut reg = Registry::new(SandboxConfig {
        characters: ids
            .iter()
            .map(|&id| character(id, &format!("Invented Agent {id}"), true, 1_000.0))
            .collect(),
        ..SandboxConfig::default()
    });
    for &id in &ids {
        park(&mut reg, id, "miner", now).expect("parks");
        let mut a = agent(&reg, id).expect("row");
        a.settled_to = now - HOURS6;
        write(&mut reg, id, a);
    }
    (reg, ids)
}

fn rollover_day(now: BlueTime) -> crate::rollover::Day {
    crate::rollover::Day {
        day: 1,
        days: 1,
        now,
    }
}

#[test]
fn rollover_settles_at_most_the_slice() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, _) = parked_crowd(ROLLOVER_SLICE + 3, now);
    assert_eq!(on_rollover(&mut reg, &rollover_day(now)), ROLLOVER_SLICE);
    assert_eq!(reg.ext::<Book>().settled(), ROLLOVER_SLICE as u64);
}

#[test]
fn the_sweep_finishes_what_the_rollover_left() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, ids) = parked_crowd(ROLLOVER_SLICE + 3, now);
    on_rollover(&mut reg, &rollover_day(now));
    // The tick is well past the rollover; the left-over agents still settle
    // up to the rollover's time, the day they belong to.
    let late = now + 5 * 3_600 * 10_000_000;
    let mut ticks = 0;
    while reg.ext::<Book>().settled() < ids.len() as u64 && ticks < 20 {
        tick(&mut reg, Instant::now(), late);
        ticks += 1;
    }
    assert_eq!(reg.ext::<Book>().settled(), ids.len() as u64);
    assert_eq!(ticks, 3usize.div_ceil(SWEEP_SLICE), "a slice a tick");
    for &id in &ids {
        assert_eq!(agent(&reg, id).expect("row").settled_to, now);
    }
    // One more tick finds nothing due and ends the sweep.
    tick(&mut reg, Instant::now(), late);
    assert!(!reg.ext::<Book>().sweeping);
    // The next timed sweep settles to the tick's own time again.
    let period = (SWEEP_EVERY_H * 3_600.0 * 1.0e7) as i64;
    let timed = late + 2 * period;
    let mut ticks = 0;
    loop {
        tick(&mut reg, Instant::now(), timed);
        ticks += 1;
        if !reg.ext::<Book>().sweeping || ticks >= 40 {
            break;
        }
    }
    assert!(!reg.ext::<Book>().sweeping);
    for &id in &ids {
        assert_eq!(agent(&reg, id).expect("row").settled_to, timed);
    }
}

#[test]
fn the_sweep_counts_its_late_agents_for_the_closed_day() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, _) = parked_crowd(ROLLOVER_SLICE + 3, now);
    on_rollover(&mut reg, &rollover_day(now));
    assert!(reg.ext::<Book>().last_late().is_none());
    let mut ticks = 0;
    while reg.ext::<Book>().sweeping && ticks < 20 {
        assert!(
            reg.ext::<Book>().last_late().is_none(),
            "not before the end"
        );
        tick(&mut reg, Instant::now(), now + 1);
        ticks += 1;
    }
    let late = reg.ext::<Book>().last_late().expect("kept at the end");
    assert_eq!(late.agents, 3);
    assert_eq!(late.day, 1);
}

#[test]
fn the_rollover_runs_one_slice_even_when_two_steps_call_it() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, _) = parked_crowd(ROLLOVER_SLICE + 3, now);
    let day = rollover_day(now);
    assert_eq!(on_rollover(&mut reg, &day), ROLLOVER_SLICE);
    assert_eq!(on_rollover(&mut reg, &day), 0);
    assert_eq!(reg.ext::<Book>().settled(), ROLLOVER_SLICE as u64);
}

#[test]
fn the_rollover_slice_takes_gatherers_first() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, ids) = parked_crowd(ROLLOVER_SLICE + 1, now);
    // The ratter is the longest unsettled, yet it waits for the sweep.
    let ratter = ids[0];
    let mut a = agent(&reg, ratter).expect("row");
    a.career = Career::Ratter;
    a.settled_to = now - 2 * HOURS6;
    write(&mut reg, ratter, a);
    assert_eq!(on_rollover(&mut reg, &rollover_day(now)), ROLLOVER_SLICE);
    assert_eq!(
        agent(&reg, ratter).expect("row").settled_to,
        now - 2 * HOURS6
    );
    for &id in &ids[1..] {
        assert_eq!(agent(&reg, id).expect("row").settled_to, now);
    }
}

#[test]
fn a_second_rollover_mid_sweep_skips_and_repeats_nothing() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, ids) = parked_crowd(ROLLOVER_SLICE + SWEEP_SLICE + 1, now);
    reg.deltas_mut().world_mut().expect("world").day = 1;
    on_rollover(&mut reg, &rollover_day(now));
    tick(&mut reg, Instant::now(), now + 1);
    let mid = reg.ext::<Book>().settled();
    assert_eq!(mid, (ROLLOVER_SLICE + SWEEP_SLICE) as u64);
    let behind = ids
        .iter()
        .filter(|&&id| agent(&reg, id).expect("row").settled_to == now - HOURS6)
        .count();
    assert_eq!(behind, 1, "one agent is still unreached");
    // The next day's rollover arrives with one agent still unsettled.
    let next = now + 24 * 3_600 * 10_000_000;
    reg.deltas_mut().world_mut().expect("world").day = 2;
    let n = on_rollover(
        &mut reg,
        &crate::rollover::Day {
            day: 2,
            days: 1,
            now: next,
        },
    );
    // Every agent has a span to the new time, so the slice settles again.
    assert_eq!(n, ROLLOVER_SLICE);
    let mut ticks = 0;
    while ticks < 20 {
        tick(&mut reg, Instant::now(), next + 1);
        ticks += 1;
        if !reg.ext::<Book>().sweeping {
            break;
        }
    }
    for &id in &ids {
        assert_eq!(
            agent(&reg, id).expect("row").settled_to,
            next,
            "every agent is settled to the second rollover"
        );
    }
    // The agent the first sweep never reached settles each day's span apart,
    // each on its own day's cap; nobody settles a span twice.
    assert_eq!(reg.ext::<Book>().settled(), mid + ids.len() as u64 + 1);
}

#[test]
fn the_closed_days_hours_do_not_eat_the_new_days_cap() {
    let mut reg = miner();
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "miner", now).expect("parks");
    let mut a = agent(&reg, BOT).expect("row");
    a.settled_to = now - 4 * HOURS6;
    write(&mut reg, BOT, a);
    let d = rollover_day(now);
    reg.deltas_mut().world_mut().expect("world").day = d.day;
    assert_eq!(on_rollover(&mut reg, &d), 1);
    assert_eq!(
        agent(&reg, BOT).expect("row").day,
        0,
        "the hours count against the day the rollover closed"
    );
    let r = settle(&mut reg, BOT, now + HOURS6).expect("a report");
    let want = rules::active_hours(6.0 * rules::HOUR_S, 0.0);
    assert!((r.work_h - want).abs() < 1e-9, "{} != {want}", r.work_h);
}

#[test]
fn two_rollovers_settle_an_unreached_agent_one_span_each() {
    let now = crate::pyutil::filetime_now();
    let (mut reg, ids) = parked_crowd(ROLLOVER_SLICE + 1, now);
    reg.deltas_mut().world_mut().expect("world").day = 1;
    on_rollover(&mut reg, &rollover_day(now));
    let behind: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|&id| agent(&reg, id).expect("row").settled_to == now - HOURS6)
        .collect();
    assert_eq!(behind.len(), 1, "one agent is unreached");
    let next = now + 24 * 3_600 * 10_000_000;
    reg.deltas_mut().world_mut().expect("world").day = 2;
    on_rollover(
        &mut reg,
        &crate::rollover::Day {
            day: 2,
            days: 1,
            now: next,
        },
    );
    // The unreached agent settles twice in the second slice: once per day.
    assert_eq!(reg.ext::<Book>().settled(), (2 * ROLLOVER_SLICE + 1) as u64);
    let a = agent(&reg, behind[0]).expect("row");
    assert_eq!(a.settled_to, next);
    assert_eq!(a.day, 1, "the second span counts against the day it closed");
}

/// `day`'s `econ` totals, read back.
fn day_totals(reg: &Registry, day: u32) -> crate::econ_ledger::Totals {
    *crate::econ_ledger::history(reg)
        .iter()
        .find(|t| t.day == day)
        .expect("the day is folded")
}

/// Tick the sweep at `at` until it ends, at most 20 ticks.
fn sweep_out(reg: &mut Registry, at: BlueTime) {
    let mut ticks = 0;
    while reg.ext::<Book>().sweeping && ticks < 20 {
        tick(reg, Instant::now(), at);
        ticks += 1;
    }
    assert!(!reg.ext::<Book>().sweeping, "the sweep ended");
}

#[test]
fn a_late_ratters_bounty_is_booked_to_the_closed_day() {
    let mut reg = ratter(300);
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "ratter", now).expect("parks");
    let mut a = agent(&reg, BOT).expect("row");
    a.settled_to = now - HOURS6;
    write(&mut reg, BOT, a);
    let d = rollover_day(now);
    reg.deltas_mut().world_mut().expect("world").day = d.day;
    // The slice is empty: the ratter is left to the sweep, after the fold.
    assert_eq!(start_rollover(&mut reg, &d, 0), 0);
    crate::econ_ledger::step(&mut reg, &d, false);
    let f0 = day_totals(&reg, 1).faucet;
    let tx = crate::econ_ledger::cursor(&reg);
    sweep_out(&mut reg, now + 1);
    let bounty: f64 = reg
        .journal()
        .entries()
        .iter()
        .filter(|e| {
            e.entry_type_id == crate::npc::bounty::REF_BOUNTY_PRIZES && e.transaction_id > tx
        })
        .map(|e| e.amount)
        .sum();
    assert!(bounty > 0.0, "the ratter was paid");
    let f1 = day_totals(&reg, 1).faucet;
    assert!(
        (f1 - f0 - bounty).abs() < 1e-6,
        "day 1 holds the bounty: {f0} -> {f1}, bounty {bounty}"
    );
    let late = reg.ext::<Book>().last_late().expect("a late count").clone();
    assert!(late.faucet > 0.0, "{late:?}");
    assert_eq!(late.unbooked, 0);
    // The next fold does not count the bounty again.
    let next = crate::rollover::Day {
        day: 2,
        days: 1,
        now: now + 24 * 3_600 * 10_000_000,
    };
    reg.deltas_mut().world_mut().expect("world").day = next.day;
    crate::econ_ledger::step(&mut reg, &next, false);
    let f2 = day_totals(&reg, 2).faucet;
    assert!(f2.abs() < 1e-6, "day 2 counts it again: {f2}");
}

/// The bounty paid in journal rows after transaction `tx`.
fn bounty_after(reg: &Registry, tx: i64) -> f64 {
    reg.journal()
        .entries()
        .iter()
        .filter(|e| {
            e.entry_type_id == crate::npc::bounty::REF_BOUNTY_PRIZES && e.transaction_id > tx
        })
        .map(|e| e.amount)
        .sum()
}

/// The `econ` rows the next fold still has to subtract.
fn late_rows(reg: &Registry) -> usize {
    reg.deltas()
        .of(crate::econ_ledger::KIND)
        .keys()
        .filter(|k| k.first().is_some_and(|h| h == crate::econ_ledger::LATE))
        .count()
}

/// The parked ratter six hours behind, day 1's rollover with an empty slice
/// and its fold: the ratter is owed day 1 late. Answers the time and the
/// cursor after the fold.
fn ratter_owed_day_one() -> (Registry, BlueTime, i64) {
    let mut reg = ratter(300);
    let now = crate::pyutil::filetime_now();
    park(&mut reg, BOT, "ratter", now).expect("parks");
    let mut a = agent(&reg, BOT).expect("row");
    a.settled_to = now - HOURS6;
    write(&mut reg, BOT, a);
    let d1 = rollover_day(now);
    reg.deltas_mut().world_mut().expect("world").day = d1.day;
    assert_eq!(start_rollover(&mut reg, &d1, 0), 0);
    crate::econ_ledger::step(&mut reg, &d1, false);
    let tx = crate::econ_ledger::cursor(&reg);
    (reg, now, tx)
}

#[test]
fn a_second_rollovers_slice_books_the_older_days_piece_before_its_fold() {
    let (mut reg, now, tx1) = ratter_owed_day_one();
    let f1 = day_totals(&reg, 1).faucet;
    let d2 = crate::rollover::Day {
        day: 2,
        days: 1,
        now: now + 24 * 3_600 * 10_000_000,
    };
    reg.deltas_mut().world_mut().expect("world").day = d2.day;
    // The slice reaches the ratter: one piece per rollover.
    assert_eq!(start_rollover(&mut reg, &d2, 1), 1);
    let t = &reg.ext::<Book>().targets;
    assert_eq!((t[0].late.agents, t[0].late.unbooked), (1, 0), "{t:?}");
    assert_eq!(t[1].late.agents, 0, "the newest piece is before its fold");
    crate::econ_ledger::step(&mut reg, &d2, false);
    let early = day_totals(&reg, 1).faucet - f1;
    let today = day_totals(&reg, 2).faucet;
    assert!(early > 0.0, "day 1 holds its piece: {early}");
    assert!(today > 0.0, "day 2 holds its own: {today}");
    let bounty = bounty_after(&reg, tx1);
    assert!(
        (early + today - bounty).abs() < 1e-6,
        "{early} + {today} != {bounty}"
    );
    assert_eq!(late_rows(&reg), 0, "the fold cleared the late rows");
}

#[test]
fn a_bot_logging_in_mid_sweep_books_its_owed_day_late() {
    let (mut reg, _, tx) = ratter_owed_day_one();
    let f1 = day_totals(&reg, 1).faucet;
    assert!(reg.ext::<Book>().sweeping);
    on_login(&mut reg, BOT);
    let bounty = bounty_after(&reg, tx);
    assert!(bounty > 0.0, "the ratter was paid");
    let rose = day_totals(&reg, 1).faucet - f1;
    assert!((rose - bounty).abs() < 1e-6, "{rose} != {bounty}");
    let w = crate::econ_ledger::window(&reg).faucet;
    assert!(w.abs() < 1e-6, "the next fold would count it again: {w}");
    assert!(agent(&reg, BOT).is_none(), "the bot is live again");
}

#[test]
fn a_late_miners_ore_is_booked_to_the_closed_days_goods() {
    let mut reg = miner();
    away_six_hours(&mut reg);
    let now = crate::pyutil::filetime_now();
    let d = rollover_day(now);
    reg.deltas_mut().world_mut().expect("world").day = d.day;
    assert_eq!(start_rollover(&mut reg, &d, 0), 0);
    crate::econ_ledger::step(&mut reg, &d, false);
    let ore = |g: &crate::econ_ledger::Goods| g.0.get("ore").map_or(0, |p| p.0);
    let g0 = crate::econ_ledger::goods_of_day(&reg, 1);
    sweep_out(&mut reg, now + 1);
    let g1 = crate::econ_ledger::goods_of_day(&reg, 1);
    assert!(ore(&g1) > ore(&g0), "{g0:?} -> {g1:?}");
    let today = crate::econ_ledger::goods_today(&reg);
    assert_eq!(ore(&today), 0, "{today:?}");
}

// ---- the wire -----------------------------------------------------------------

fn econ_agent(reg: &mut Registry, who: Option<i64>, method: &str, args: &[PyValue]) -> PyValue {
    reg.dispatch(&Request {
        service: "econAgent",
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: who,
    })
    .expect("econAgent answers")
}

fn field(v: &PyValue, key: &str) -> PyValue {
    crate::pyutil::dict_get(v, key.as_bytes())
        .cloned()
        .unwrap_or(PyValue::None)
}

#[test]
fn the_park_call_parks_a_bot_and_refuses_with_a_reason() {
    let mut reg = miner();
    let career = |s: &str| [PyValue::Bytes(s.as_bytes().to_vec())];
    let ok = econ_agent(&mut reg, Some(BOT), "Park", &career("miner"));
    assert_eq!(field(&ok, "parked"), PyValue::Bool(true));
    assert_eq!(field(&ok, "career"), crate::pyutil::bstr("miner"));
    let status = econ_agent(&mut reg, Some(BOT), "Status", &[]);
    assert_eq!(field(&status, "parked"), PyValue::Bool(true));
    assert_eq!(field(&status, "career"), crate::pyutil::bstr("miner"));

    for (who, name) in [
        (Some(BUYER), "miner"),
        (Some(BOT), "explorer"),
        (None, "miner"),
    ] {
        let no = econ_agent(&mut reg, who, "Park", &career(name));
        assert_eq!(field(&no, "parked"), PyValue::Bool(false), "{name}");
        assert!(matches!(field(&no, "reason"), PyValue::Bytes(b) if !b.is_empty()));
    }
    let bad = econ_agent(&mut reg, Some(BOT), "Park", &[PyValue::Int(3)]);
    assert_eq!(field(&bad, "parked"), PyValue::Bool(false));
    assert!(matches!(
        reg.dispatch(&Request {
            service: "econAgent",
            method: "Nope",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOT),
        }),
        Err(ServiceError::NoSuchMethod { .. })
    ));
}

#[test]
fn the_gm_settle_parks_settles_and_reports() {
    let mut reg = miner();
    let text = |r: Result<PyValue, ServiceError>| match r.expect("answers") {
        PyValue::Bytes(b) => String::from_utf8(b).expect("utf-8"),
        other => panic!("not text: {other:?}"),
    };
    assert!(text(reg.slash(Some(BOT), "/settle")).contains("parked 0"));
    assert!(text(reg.slash(Some(BOT), "/settle 3")).contains("not parked"));
    assert!(text(reg.slash(Some(BOT), "/settle park miner")).contains("parked as a miner"));
    assert!(text(reg.slash(Some(BOT), "/settle park mission_runner")).contains("not parked"));
    let before = rocks_left(&reg);
    let line = text(reg.slash(Some(BOT), "/settle 5"));
    assert!(line.starts_with("[settle] Invented Agent"), "{line}");
    assert!(rocks_left(&reg) < before);
    assert!(text(reg.slash(Some(BOT), "/settle 0")).contains("positive"));
    assert!(text(reg.slash(Some(BOT), "/settle x")).contains("not a number"));
    assert!(text(reg.slash(Some(BOT), "/settle 5 Nobody At All")).contains("no such character"));
    let overview = text(reg.slash(Some(BOT), "/settle"));
    assert!(
        overview.contains("parked 1") && overview.contains("miner=1"),
        "{overview}"
    );
    assert!(text(reg.slash(Some(BOT), "/settle sweep")).contains("swept"));
}

#[test]
fn the_report_line_names_the_flows() {
    let r = Report {
        character: 5,
        name: "Invented".to_owned(),
        career: Career::Ratter,
        station: 9,
        span_h: 6.0,
        work_h: 3.0,
        isk: 1234.5,
        goods: [(34, 100), (35, -2), (36, 0)].into_iter().collect(),
        notes: vec!["a".to_owned(), "b".to_owned()],
        belt: None,
    };
    let line = r.line();
    assert!(line.starts_with("[settle] Invented (5) ratter at 9"));
    assert!(line.contains("isk=+1234.50") && line.contains("34:+100") && line.contains("35:-2"));
    assert!(!line.contains("36:"));
    assert!(line.ends_with("notes: a; b"));
}

#[test]
fn the_settle_line_names_the_belt_a_gatherer_started_at() {
    let field = || {
        vec![
            rock_in(0, 40_000_001, 200_000),
            rock_in(1, 40_000_002, 200_000),
        ]
    };
    // Two characters of a two-belt system: belt_first answers each one's
    // start belt, and the line prints it; some pair of characters differs.
    let starts: Vec<Option<i64>> = (1..=40)
        .map(|c| gather::belt_first(&mut field(), c))
        .collect();
    let a = starts[0].expect("a belt");
    let b = starts
        .iter()
        .flatten()
        .copied()
        .find(|&x| x != a)
        .expect("a second belt among 40 characters");
    let line = |belt: i64| {
        Report {
            character: 5,
            name: "Invented".to_owned(),
            career: Career::Miner,
            station: 9,
            span_h: 1.0,
            work_h: 1.0,
            isk: 0.0,
            goods: BTreeMap::new(),
            notes: vec!["n".to_owned()],
            belt: Some(belt),
        }
        .line()
    };
    assert!(line(a).contains(&format!(" belt={a} ")), "{}", line(a));
    assert!(line(b).contains(&format!(" belt={b} ")));
    assert_ne!(line(a), line(b));
    assert_eq!(gather::belt_first(&mut [], 1), None);
}

#[test]
fn the_book_keeps_the_last_reports_only() {
    let mut book = Book::default();
    let r = |i: i64| Report {
        character: i,
        name: String::new(),
        career: Career::Miner,
        station: 0,
        span_h: 0.0,
        work_h: 0.0,
        isk: 1.0,
        goods: BTreeMap::new(),
        notes: vec![],
        belt: None,
    };
    for i in 0..(KEEP_REPORTS as i64 + 5) {
        book.keep(&r(i));
    }
    assert_eq!(book.recent().len(), KEEP_REPORTS);
    assert_eq!(book.recent()[0].character, 5);
    assert_eq!(book.settled(), KEEP_REPORTS as u64 + 5);
    assert!((book.isk() - (KEEP_REPORTS + 5) as f64).abs() < 1e-9);
}

// ---- belts and the rollover (settle-spread) -------------------------------------

fn rock_in(i: i64, belt: i64, quantity: i64) -> Rock {
    Rock {
        belt_id: belt,
        ..rock(i, quantity)
    }
}

#[test]
fn two_miners_settle_into_different_belts() {
    let field = |_: ()| {
        vec![
            rock_in(0, 40_000_001, 200_000),
            rock_in(1, 40_000_002, 200_000),
            rock_in(2, 40_000_003, 200_000),
        ]
    };
    // Which belt's rock the first rock of the cut order belongs to, per miner.
    let first: Vec<i64> = (1..=40)
        .map(|c| {
            let mut v = field(());
            gather::belt_first(&mut v, c);
            v[0].belt_id
        })
        .collect();
    assert!(
        first.contains(&40_000_001) && first.contains(&40_000_002) && first.contains(&40_000_003),
        "{first:?}"
    );
    // And a settlement bites its own belt first, not always belt 0.
    let mut reg = world(field(()), &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)]);
    let mut want = field(());
    gather::belt_first(&mut want, BOT);
    away_six_hours(&mut reg);
    on_login(&mut reg, BOT);
    let hit = reg
        .rocks()
        .in_system(SYSTEM)
        .find(|r| r.quantity < 200_000)
        .expect("a rock was bitten");
    assert_eq!(hit.belt_id, want[0].belt_id);
}

#[test]
fn the_rollover_settles_before_the_belts_reset() {
    let pos = |n: &str| {
        crate::rollover_steps::STEPS
            .iter()
            .position(|s| s.name == n)
            .unwrap_or_else(|| panic!("no step {n}"))
    };
    assert!(pos("econ_agents_close") < pos("asteroid"));
    // The step settles a parked agent.
    let mut reg = miner();
    away_six_hours(&mut reg);
    let before = rocks_left(&reg);
    let step = &crate::rollover_steps::STEPS[pos("econ_agents_close")];
    let d = crate::rollover::Day {
        day: 1,
        days: 1,
        now: crate::pyutil::filetime_now(),
    };
    let crate::rollover::Run::Once(f) = step.run else {
        panic!("a once step")
    };
    assert_eq!(f(&mut reg, &d), 1);
    assert!(rocks_left(&reg) < before);
}

// ---- soak A fixes (2026-11-05) -----------------------------------------------

/// A civilian railgun (an invented fixture row of `modules`): its damage is
/// its own and it takes no round.
const CIVILIAN_GUN: i64 = 3638;

#[test]
fn a_civilian_gun_with_no_charge_is_a_battery_that_never_runs_dry() {
    let reg = world(vec![], &[(HULL_ITEM + 1, CIVILIAN_GUN, HI_SLOT_0)]);
    let g = combat::guns_of(&reg, BOT).expect("a civilian gun is a battery");
    assert!(g.chargeless);
    assert!(g.battery.dps > 0.0 && g.battery.shots_per_s > 0.0);
    assert!(g.loaded.is_empty());
    assert!(g.fitted.contains(&CIVILIAN_GUN));
}

#[test]
fn a_ratter_with_a_civilian_gun_earns_and_spends_no_rounds() {
    let mut reg = world(vec![], &[(HULL_ITEM + 1, CIVILIAN_GUN, HI_SLOT_0)]);
    let report = park_and_settle(&mut reg, "ratter", 8.0).expect("a report");
    assert!(report.isk > 0.0, "{}", report.line());
    assert!(
        !report.notes.iter().any(|n| n.contains("no loaded guns")),
        "{:?}",
        report.notes
    );
    assert_classified(&reg);
}

#[test]
fn a_settled_miner_rests_an_ask_for_minerals_no_bid_took() {
    let mut reg = miner();
    reg.apply_items(&[Edit::Create {
        type_id: TRITANIUM,
        quantity: 5_000,
        to: Slot::new(BOT, STATION, FLAG_HANGAR),
    }])
    .expect("minerals");
    let asks = |reg: &Registry| reg.settle_has_ask(BOT, TRITANIUM, STATION);
    assert!(!asks(&reg));
    let r = park_and_settle(&mut reg, "miner", 1.0).expect("a report");
    assert!(
        r.notes.iter().any(|n| n.contains("listed")),
        "{:?}",
        r.notes
    );
    assert!(asks(&reg), "an ask rests for the mineral");
    // A second settlement lists nothing again.
    let again = park_and_settle(&mut reg, "miner", 1.0).expect("a report");
    assert!(
        !again.notes.iter().any(|n| n.contains("listed")),
        "{:?}",
        again.notes
    );
}

#[test]
fn a_builder_holding_every_material_installs_without_a_margin() {
    // The product is priced at exactly what its materials cost: no margin.
    let mut c = content();
    c.insert(
        "econ_prices",
        crate::econ_prices::Table::parse(&[
            format!("{TRITANIUM}\t3.0\tbase\t2.0\t4\t18\t\t1"),
            "35\t10.0\tbase\t1.0\t4\t18\t\t1".to_owned(),
            format!("{PRODUCT}\t120.0\tbase\t1.0\t7\t99\t\t1"),
            format!("{MERLIN}\t300000.0\tbase\t1.0\t6\t99\t\t1"),
        ])
        .expect("prices parse"),
    );
    let mut reg = world_in(
        c,
        vec![rock(0, 4_000), rock(1, 4_000)],
        &[(HULL_ITEM + 1, MINER_I, HI_SLOT_0)],
    );
    reg.character_mut(BOT).expect("bot").balance = 1_000_000_000.0;
    train(&mut reg, BOT, SKILL_INDUSTRY, 1);
    reg.apply_items(&[
        Edit::Create {
            type_id: BLUEPRINT,
            quantity: 1,
            to: Slot::new(BOT, STATION, FLAG_HANGAR),
        },
        Edit::Create {
            type_id: TRITANIUM,
            quantity: 40 * 100,
            to: Slot::new(BOT, STATION, FLAG_HANGAR),
        },
    ])
    .expect("blueprint and minerals");
    assert!(
        !rules::worth_building(120.0, 120.0),
        "the case has no margin"
    );
    let r = park_and_settle(&mut reg, "builder", 1.0).expect("a report");
    assert_eq!(jobs_of(&reg).len(), 1, "{}", r.line());
}

#[test]
fn settling_a_parked_miner_leaves_no_unaddressed_wallet_push() {
    let mut reg = miner();
    let _ = reg.take_pending();
    let r = park_and_settle(&mut reg, "miner", 6.0).expect("a report");
    let wallet: Vec<_> = reg
        .take_pending()
        .into_iter()
        .filter(|(_, p)| p.to_push().0 == "OnAccountChange")
        .collect();
    assert!(!wallet.is_empty(), "the settlement moved ISK: {}", r.line());
    for (_, p) in wallet {
        assert!(p.audience().is_some(), "an unaddressed wallet push");
    }
}
