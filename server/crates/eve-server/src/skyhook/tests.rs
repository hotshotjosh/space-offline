//! Skyhook tests. Every id, type and number here is invented; the client's
//! constants appear only by name. The reagent row of the world is the
//! sov-resources fixture's, an invented one.

use super::*;
use crate::gateway::{field, get_bytes, get_uint};
use crate::sov_hub::fixture::{ALICE, BOB, CAROL, CORP, DAVE, LONE, RIVAL, SYSTEM, ship};
use crate::sov_resources::{FIXTURE_REAGENT_TYPE, fixture_planets, fixture_stars};
use crate::space_state::{Motion, Presence};
use std::sync::{Arc, RwLock};

/// An invented "now" in unix seconds.
const NOW: u64 = 2_000_000_000;

const STAR: i64 = 40_900_000;
/// Over this planet (type 9_001, power) the power skyhook stands.
const P_POWER: i64 = 40_900_011;
/// Workforce planet (type 9_002).
const P_LAND: i64 = 40_900_012;
/// Reagent planet (type 9_003).
const P_REAGENT: i64 = 40_900_001;
/// A planet whose type has no yield row.
const P_BARE: i64 = 40_900_014;
const SKYHOOK_TYPE: i64 = 990_900_001;
/// Invented hulls: a roomy hold (5000 m3) and a tiny one (120 m3).
const ROOMY_HULL: i64 = 990_900_010;
const TINY_HULL: i64 = 990_900_011;
/// The skyhooks and the cargo items.
const SK: i64 = 8_950_001;
const SK_POWER: i64 = 8_950_002;
const SK_LAND: i64 = 8_950_003;
const SK_RIVAL: i64 = 8_950_004;
const ALICE_ITEM: i64 = 100_900_101;
const BOB_ITEM: i64 = 100_900_102;
const CAROL_ITEM: i64 = 100_900_103;
const DAVE_ITEM: i64 = 100_900_104;
/// A second skyhook in Alice's cargo.
const ALICE_ITEM2: i64 = 100_900_105;
const HUB: i64 = 100_900_900;

fn catalogue() -> crate::celestial::Catalogue {
    crate::celestial::Catalogue::parse(&format!(
        "star\t{STAR}\t9000\t4.0e8\t0\t0\t0\t{SYSTEM}\t0\t0\n\
         planet\t{P_REAGENT}\t9003\t6.0e6\t1.0e10\t2.0e8\t3.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_POWER}\t9001\t6.0e6\t-4.0e9\t1.0e8\t9.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_LAND}\t9002\t6.0e6\t6.0e9\t-3.0e8\t-8.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_BARE}\t9999\t6.0e6\t-9.0e9\t0\t-2.0e9\t{SYSTEM}\t0\t0\n"
    ))
}

fn content() -> crate::content::Content {
    let mut c = crate::sov_hub::fixture::content();
    c.insert("sov planets", fixture_planets());
    c.insert("sov stars", fixture_stars());
    let text = format!(
        "{}type\t{SKYHOOK_TYPE}\t{GROUP_SKYHOOK}\t46\t1\t1\t100.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Skyhook\n\
         type\t{ROOMY_HULL}\t25\t6\t1\t1\t100.0\t5000.0\t1000.0\t20.0\t0\t\t\tInvented Roomy Hull\n\
         type\t{TINY_HULL}\t25\t6\t1\t1\t100.0\t120.0\t1000.0\t20.0\t0\t\t5646=30.0\tInvented Tiny Hull\n\
         type\t{FIXTURE_REAGENT_TYPE}\t{}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n",
        crate::typedata::FIXTURE,
        sov_resources::GROUP_COLONY_REAGENTS,
    );
    let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn item(c: &crate::content::Content, item_id: i64, owner: i64, flag_id: i64) -> crate::item::Item {
    let t = crate::item::find_in(c, SKYHOOK_TYPE).expect("a test type");
    crate::item::Item {
        item_id,
        type_id: SKYHOOK_TYPE,
        owner_id: owner,
        location_id: ship(owner),
        flag_id,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    }
}

fn in_space(c: &crate::content::Content, item_id: i64, corp: i64) -> crate::item::Item {
    let mut i = item(c, item_id, corp, FLAG_IN_SPACE);
    i.location_id = SYSTEM;
    i
}

fn planet_at(reg: &Registry, planet: i64) -> [f64; 3] {
    position_of(reg.celestials().get(planet).expect("a planet"))
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

/// The world: four pilots at the reagent planet's skyhook spot; alliance A
/// (Alice's corp) holds the system, with a hub; alliance D is the rival's.
fn world() -> (Registry, i64, i64) {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    };
    let content = Arc::new(content());
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
    // Alice flies a roomy hull, Bob a tiny one (extraction clips to the room).
    for (who, ty) in [(ALICE, ROOMY_HULL), (BOB, TINY_HULL)] {
        let t = *crate::item::find_in(&content, ty).expect("a test hull");
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = ty;
            i.group_id = t.group_id;
            i.category_id = t.category_id;
        }
    }
    items.extend([
        item(&content, ALICE_ITEM, ALICE, crate::sov_hub::FLAG_CARGO),
        item(&content, BOB_ITEM, BOB, crate::sov_hub::FLAG_CARGO),
        item(&content, CAROL_ITEM, CAROL, crate::sov_hub::FLAG_CARGO),
        item(&content, DAVE_ITEM, DAVE, crate::sov_hub::FLAG_CARGO),
        item(&content, ALICE_ITEM2, ALICE, crate::sov_hub::FLAG_CARGO),
        // The skyhooks already standing in space, for `stand` to give rows.
        in_space(&content, SK, CORP),
        in_space(&content, SK_POWER, CORP),
        in_space(&content, SK_LAND, CORP),
        in_space(&content, SK_RIVAL, RIVAL),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_901_000,
        ..crate::store::Snapshot::default()
    });
    crate::service::sov_hub::test_corp(&mut reg, CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, LONE, CAROL, &[(CAROL, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, RIVAL, DAVE, &[(DAVE, 0)]);
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    let a = st
        .create(CORP, ALICE, "Invented Sov Holders", "ISH", "", "", 1)
        .expect("an alliance");
    let d = st
        .create(RIVAL, DAVE, "Invented Rivals", "IRV", "", "", 1)
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
    // Alliance A holds the system, claimed long ago, and its
    // hub is a row (the budget announces to it).
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
        filetime(1_000_000_000),
    );
    let spot = off(planet_at(&reg, P_REAGENT), 10_000.0);
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
    (reg, a, d)
}

/// The row of a skyhook of `corp` over `planet` (its item is one of the
/// world's in-space ones). `active` skyhooks have been built and are
/// switched on.
fn stand(
    reg: &mut Registry,
    id: i64,
    planet: i64,
    corp: i64,
    active: bool,
    last_cycle: u64,
) -> Skyhook {
    let sk = Skyhook {
        id,
        planet,
        system: SYSTEM,
        corp,
        active,
        activated_at: if active { NOW - 5_000 } else { 0 },
        built_until: NOW - 5_000,
        secured: 0,
        unsecured: 0,
        last_cycle,
        vuln_start: 0,
        vuln_end: 0,
        state: STATE_SHIELD_VULNERABLE,
    };
    put(reg.deltas_mut(), &sk);
    sk
}

fn reagent() -> Reagent {
    sov_resources::planet_yield(&world().0, P_REAGENT)
        .expect("a yield")
        .reagent
        .expect("a reagent planet")
}

fn sent(reg: &mut Registry) -> Vec<crate::sov_hub::fixture::Sent> {
    crate::sov_hub::fixture::pending(reg)
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
    gateway(&req(name), body, who, g).expect("the skyhook arm owns it")
}

fn id_body(n: i64) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &id(n as u64));
    b
}

fn payload(r: &Reply) -> &[u8] {
    &r.payload.as_ref().expect("a payload").1
}

fn ident_of(b: &[u8]) -> Option<u64> {
    get_uint(b, 1)
}

fn wall_now() -> u64 {
    crate::sov_proto::unix_now()
}

// ---------------------------------------------------------------------------
// The harvest
// ---------------------------------------------------------------------------

/// A hand-made reagent: 10 per 100 s, caps as given.
fn rg(pct: u32, scap: u32, ucap: u32) -> Reagent {
    Reagent {
        type_id: 1,
        per_cycle: 10,
        cycle_s: 100,
        secured_pct: pct,
        secured_cap: scap,
        unsecured_cap: ucap,
    }
}

fn sim(s: u32, u: u32, last: u64) -> Sim {
    Sim {
        secured: s,
        unsecured: u,
        last_cycle: last,
    }
}

#[test]
fn harvest_zero_cycles_changes_nothing_and_keeps_the_phase() {
    let r = rg(30, 50, 60);
    // 99 s of a 100 s cycle: nothing, and last_cycle stays on the whole cycle.
    assert_eq!(harvest(&sim(4, 5, 1_000), &r, 1_099), sim(4, 5, 1_000));
    // A clock behind the record (a restored world) changes nothing either.
    assert_eq!(harvest(&sim(4, 5, 1_000), &r, 900), sim(4, 5, 1_000));
}

#[test]
fn harvest_three_cycles_with_room_splits_by_the_percentage() {
    // 3 x 10 = 30 produced; 30 % of it is 9 secured, 21 unsecured.
    let got = harvest(&sim(0, 0, 1_000), &rg(30, 50, 60), 1_300);
    assert_eq!(got, sim(9, 21, 1_300));
    // On top of stock.
    let got = harvest(&sim(40, 0, 1_000), &rg(30, 50, 60), 1_100);
    assert_eq!(got, sim(43, 7, 1_100));
}

#[test]
fn harvest_secured_full_spills_into_the_unsecured_pool() {
    // Secured room 5: 9 -> 5, overflow 4 goes to unsecured (21 + 4).
    let got = harvest(&sim(0, 0, 1_000), &rg(30, 5, 60), 1_300);
    assert_eq!(got, sim(5, 25, 1_300));
    // Already full secured, 30 produced: 9 of it overflows into unsecured.
    let got = harvest(&sim(50, 0, 1_000), &rg(30, 50, 60), 1_300);
    assert_eq!(got, sim(50, 30, 1_300));
}

#[test]
fn harvest_both_full_loses_the_rest() {
    // 9 -> 5 (4 over), 21 -> 6 (15 over): 19 lost, both at the cap.
    let got = harvest(&sim(0, 0, 1_000), &rg(30, 5, 6), 1_300);
    assert_eq!(got, sim(5, 6, 1_300));
}

#[test]
fn harvest_unsecured_full_spills_into_the_secured_pool() {
    // The other direction: unsecured has room 2, secured room 50.
    // 21 -> 2 (19 over): secured 9 + 19 = 28.
    let got = harvest(&sim(0, 0, 1_000), &rg(30, 50, 2), 1_300);
    assert_eq!(got, sim(28, 2, 1_300));
}

#[test]
fn harvest_a_partial_cycle_leaves_last_cycle_on_a_whole_cycle() {
    // 250 s is two whole cycles and 50 s left over.
    let got = harvest(&sim(0, 0, 1_000), &rg(30, 50, 60), 1_250);
    assert_eq!(got.last_cycle, 1_200);
    assert_eq!(u64::from(got.secured + got.unsecured), 20);
}

#[test]
fn harvest_shares_with_the_clients_float_rule_not_integer_division() {
    // Python: int(float(29) / 100.0 * 100) is 28 (0.29 * 100 is
    // 28.999999999999996), where 29 * 100 / 100 would be 29. A harvest
    // done in integers would drift from what the client extrapolates.
    let r = Reagent {
        per_cycle: 100,
        ..rg(29, 500, 500)
    };
    let got = harvest(&sim(0, 0, 0), &r, 100);
    assert_eq!((got.secured, got.unsecured), (28, 72));
}

#[test]
fn harvest_with_a_zero_period_is_inert() {
    let r = Reagent {
        cycle_s: 0,
        ..rg(30, 50, 60)
    };
    assert_eq!(harvest(&sim(1, 2, 3), &r, 10_000), sim(1, 2, 3));
}

#[test]
fn an_inactive_skyhook_does_not_harvest() {
    let r = rg(30, 50, 60);
    let mut sk = Skyhook {
        id: SK,
        planet: P_REAGENT,
        system: SYSTEM,
        corp: CORP,
        active: false,
        activated_at: 1,
        built_until: 1,
        secured: 3,
        unsecured: 4,
        last_cycle: 1_000,
        vuln_start: 0,
        vuln_end: 0,
        state: STATE_SHIELD_VULNERABLE,
    };
    assert_eq!(current(&sk, Some(&r), 5_000), sim(3, 4, 1_000));
    sk.active = true;
    assert_ne!(current(&sk, Some(&r), 5_000), sim(3, 4, 1_000));
}

// ---------------------------------------------------------------------------
// The wire: the field-number trap, the staleness rule, the version
// ---------------------------------------------------------------------------

fn view_with_reagent(type_id: i64) -> View {
    View {
        id: SK,
        planet: P_REAGENT,
        active: true,
        reagent: Some((
            Reagent {
                type_id,
                ..rg(30, 50, 60)
            },
            sim(1, 2, 3_000),
        )),
        vulnerability: Some((false, NOW + SYNTH_AHEAD_S, NOW + SYNTH_AHEAD_S + 10)),
        workforce: Workforce::Absent,
        version: (1, 2, 3),
    }
}

/// The contract, from `gw-protos.txt` (the descriptors): the field number of
/// `reagent_type` in the simulation entry and in the definition entry of
/// each message. Written out here on purpose, not read from the encoder.
const TRAP: &[(Msg, u64, u64)] = &[
    (Msg::Get, 5, 5),
    (Msg::AllLocal, 5, 3),
    (Msg::ByCorp, 1, 1),
    (Msg::AllInSystem, 5, 6),
    (Msg::Simulations, 6, 0),
    (Msg::Definitions, 0, 3),
];

#[test]
fn the_reagent_type_sits_at_each_messages_own_field_and_at_no_other() {
    const T: i64 = 990_000_555;
    let v = view_with_reagent(T);
    for (msg, sim_field, def_field) in TRAP {
        let rec = record(*msg, &v);
        // The simulation entries: field 2 of a record, field 3 of the notice.
        let (sims, defs) = match msg {
            Msg::Simulations => (field(&rec, 3), Vec::new()),
            Msg::Definitions => (Vec::new(), field(&rec, 3)),
            _ => (field(&rec, 2), field(&rec, 4)),
        };
        if *sim_field != 0 {
            assert_eq!(sims.len(), 1, "{msg:?} has one simulation");
            for f in [1u64, 3, 5, 6] {
                let at = get_bytes(&sims[0], f).and_then(|b| ident_of(&b));
                if f == *sim_field {
                    assert_eq!(at, Some(T as u64), "{msg:?} sim reagent_type at {f}");
                } else {
                    assert_eq!(at, None, "{msg:?} sim has no reagent type at {f}");
                }
            }
            // The simulation message itself is field 4.
            assert!(get_bytes(&sims[0], 4).is_some(), "{msg:?}");
        }
        if *def_field != 0 {
            assert_eq!(defs.len(), 1, "{msg:?} has one definition");
            for f in [1u64, 3, 5, 6] {
                let at = get_bytes(&defs[0], f).and_then(|b| ident_of(&b));
                if f == *def_field {
                    assert_eq!(at, Some(T as u64), "{msg:?} def reagent_type at {f}");
                } else {
                    assert_eq!(at, None, "{msg:?} def has no reagent type at {f}");
                }
            }
            assert!(get_bytes(&defs[0], 2).is_some(), "{msg:?}");
        }
    }
    // The encoders differ: no two of these messages share one encoding.
    let mut seen = std::collections::BTreeSet::new();
    for (msg, ..) in TRAP {
        assert!(seen.insert(record(*msg, &v)), "{msg:?} shares an encoder");
    }
}

#[test]
fn the_workforce_and_the_version_ride_at_the_messages_own_fields() {
    let mut v = view_with_reagent(990_000_555);
    v.workforce = Workforce::Amount(77);
    for (msg, wf, ver) in [
        (Msg::Get, 7, 8),
        (Msg::AllLocal, 7, 8),
        (Msg::ByCorp, 7, 8),
        (Msg::AllInSystem, 9, 10),
    ] {
        let rec = record(msg, &v);
        let w = get_bytes(&rec, wf).unwrap_or_else(|| panic!("{msg:?} workforce at {wf}"));
        assert_eq!(get_uint(&w, 1), Some(77));
        let spec = get_bytes(&rec, ver).unwrap_or_else(|| panic!("{msg:?} version at {ver}"));
        assert_eq!(
            (get_uint(&spec, 1), get_uint(&spec, 2), get_uint(&spec, 3)),
            (Some(1), Some(2), Some(3))
        );
    }
    // A power planet's skyhook says `none`, a reagent planet's says nothing.
    v.workforce = Workforce::None;
    let w = get_bytes(&record(Msg::Get, &v), 7).expect("a none arm");
    assert_eq!(get_uint(&w, 2), Some(1));
    v.workforce = Workforce::Absent;
    assert!(get_bytes(&record(Msg::Get, &v), 7).is_none());
}

/// `SkyhookTheftVulnerabilityData.is_stale_at_time` **[V]**
/// (`sovereignty/skyhook/data_type.py:132-158`), 30 s grace.
fn client_says_stale(vulnerable: bool, start: u64, end: u64, now: u64) -> bool {
    let (s, e) = (start + 30, end + 30);
    if vulnerable {
        e < now || s > now
    } else {
        s < now && e > now
    }
}

fn vulnerability_of(rec: &[u8]) -> Option<(bool, u64, u64)> {
    let v = get_bytes(rec, 5)?;
    let t = |n| get_bytes(&v, n).and_then(|t| get_uint(&t, 1));
    Some((get_uint(&v, 1).unwrap_or(0) != 0, t(2)?, t(3)?))
}

#[test]
fn a_reagent_skyhook_is_never_stale_by_the_clients_rule() {
    let (mut reg, ..) = world();
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    // Across a week of reads, in steps.
    for step in 0..400u64 {
        let now = NOW + step * 1_700;
        let rec = record(Msg::Get, &view(&reg, &sk, now));
        let (vulnerable, start, end) = vulnerability_of(&rec).expect("a reagent skyhook has one");
        assert!(!vulnerable);
        assert!(
            !client_says_stale(vulnerable, start, end, now),
            "stale at +{}",
            step * 1_700
        );
        assert!(
            !(start..=end).contains(&now),
            "the window must not hold now"
        );
    }
    // A window L9 stored round the clock is vulnerable and still not stale.
    let mut w = sk;
    w.vuln_start = NOW - 100;
    w.vuln_end = NOW + 500;
    let (v, s, e) = vulnerability(&w, NOW);
    assert!(v && !client_says_stale(v, s, e, NOW));
    let (v, s, e) = vulnerability(&w, NOW + 10_000);
    assert!(!v && !client_says_stale(v, s, e, NOW + 10_000));
    // The helper can fail: a flag that disagrees with its window is stale.
    assert!(client_says_stale(false, NOW - 100, NOW + 500, NOW));
}

#[test]
fn a_skyhook_without_a_reagent_says_unknown_vulnerability() {
    let (mut reg, ..) = world();
    let sk = stand(&mut reg, SK_POWER, P_POWER, CORP, true, NOW);
    let rec = record(Msg::Get, &view(&reg, &sk, NOW));
    assert!(get_bytes(&rec, 5).is_none());
    assert_eq!(get_uint(&rec, 6), Some(1));
    assert!(
        field(&rec, 2).is_empty(),
        "a power planet has no simulation"
    );
}

#[test]
fn the_version_is_exactly_the_definitions_version_never_ahead() {
    let (mut reg, ..) = world();
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    let ours = sov_resources::current_version(reg.content());
    let rec = record(Msg::Get, &view(&reg, &sk, NOW));
    let spec = get_bytes(&rec, 8).expect("a version");
    let got = |n| get_uint(&spec, n).unwrap_or(0);
    assert_eq!(
        (got(1), got(2), got(3)),
        (u64::from(ours.0), u64::from(ours.1), u64::from(ours.2))
    );
    // And it is the version the definitions gateway names.
    let g = ground(reg);
    let defs = crate::sov_resources::gateway(
        "eve_public.sovereignty.resource.planet.api.GetAllDefinitionsRequest",
        &[],
        ALICE,
        &g,
    )
    .expect("served");
    let named = get_bytes(payload(&defs), 2).expect("the definitions version");
    let n = |k| get_uint(&named, k).unwrap_or(0);
    assert_eq!((n(1), n(2), n(3)), (got(1), got(2), got(3)));
}

// ---------------------------------------------------------------------------
// Deploy
// ---------------------------------------------------------------------------

fn at_spot(reg: &mut Registry, who: i64, planet: i64, d: f64) {
    let at = off(planet_at(reg, planet), d);
    place_at(reg, who, at);
}

#[test]
fn a_launch_over_the_nearest_planet_makes_the_item_the_row_and_the_ball() {
    let (mut reg, a, _) = world();
    at_spot(&mut reg, ALICE, P_POWER, 5_000.0);
    let id = launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW).expect("launches");
    let sk = by_id(reg.deltas(), id).expect("a row");
    assert_eq!((sk.planet, sk.system, sk.corp), (P_POWER, SYSTEM, CORP));
    assert!(!sk.active && !sk.built());
    assert_eq!(sk.built_until, NOW + ACTIVATION_S);
    // The item is in space, owned by the corp.
    let i = reg.store().get(id).expect("the item");
    assert_eq!(
        (i.owner_id, i.location_id, i.flag_id),
        (CORP, SYSTEM, FLAG_IN_SPACE)
    );
    // A ball over the planet's own spot, with the corp's alliance.
    let balls = balls_in(&reg, SYSTEM);
    assert_eq!(balls.len(), 1);
    assert_eq!(balls[0].0.ball_id, id);
    assert_eq!(balls[0].0.position, planet_at(&reg, P_POWER));
    assert_eq!(balls[0].0.alliance_id, a as i32);
    // The slim names the planet and the build, not yet done.
    let extras = slim_extras(&reg, &[id]);
    let (_, rows) = &extras[0];
    let get = |k: &str| rows.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    assert_eq!(get("planetID"), Some(PyValue::Int(P_POWER)));
    assert_eq!(get("skyhook_state"), Some(PyValue::Int(0)));
    assert_eq!(get("allianceID"), Some(PyValue::Int(a)));
    assert_eq!(
        get("component_activate"),
        Some(PyValue::Tuple(vec![
            PyValue::Bool(false),
            PyValue::Int(filetime(NOW + ACTIVATION_S))
        ]))
    );
}

#[test]
fn a_launch_is_refused_where_the_rules_say_and_changes_nothing() {
    let (mut reg, ..) = world();
    let before = reg.store().items().to_vec();
    // No alliance (Carol), no role (Bob), another alliance's pilot where the
    // holder is Alice's (Dave): each refused for its own reason.
    assert_eq!(
        launch_one_at(&mut reg, CAROL, CAROL_ITEM, NOW),
        Err(Refused::NoAlliance)
    );
    assert_eq!(
        launch_one_at(&mut reg, BOB, BOB_ITEM, NOW),
        Err(Refused::NoRole)
    );
    assert_eq!(
        launch_one_at(&mut reg, DAVE, DAVE_ITEM, NOW),
        Err(Refused::NotHeld)
    );
    // An item that is not in the pilot's hold.
    assert_eq!(
        launch_one_at(&mut reg, ALICE, BOB_ITEM, NOW),
        Err(Refused::NotInHold)
    );
    // A system nobody holds.
    crate::sov::set_claim(reg.deltas_mut(), SYSTEM, None, filetime(NOW));
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW),
        Err(Refused::NotHeld)
    );
    assert_eq!(reg.store().items(), &before[..], "no item moved");
    assert!(all(reg.deltas()).is_empty(), "no row written");
}

#[test]
fn a_launch_too_far_from_every_planet_is_refused() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_REAGENT, 3.0 * LAUNCH_RANGE);
    match launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW) {
        Err(Refused::TooFar { metres }) => assert!(metres as f64 > LAUNCH_RANGE),
        other => panic!("expected TooFar, got {other:?}"),
    }
    // Just inside the range is fine.
    at_spot(&mut reg, ALICE, P_REAGENT, 0.9 * LAUNCH_RANGE);
    assert!(launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW).is_ok());
}

#[test]
fn one_skyhook_per_planet_and_a_planet_with_no_yield_is_refused() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_REAGENT, 1_000.0);
    launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW).expect("the first");
    // Alice has a second hook in her cargo.
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_ITEM2, NOW),
        Err(Refused::HasSkyhook)
    );
    // The planet with no yield row.
    at_spot(&mut reg, ALICE, P_BARE, 1_000.0);
    assert_eq!(
        launch_one_at(&mut reg, ALICE, ALICE_ITEM2, NOW),
        Err(Refused::NothingToHarvest)
    );
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

#[test]
fn the_drop_link_answers_only_for_a_skyhook_and_in_the_launch_shape() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_LAND, 2_000.0);
    let args = drop_args(ALICE_ITEM);
    let got = drop_call(&mut reg, &drop_request(&args, Some(ALICE)))
        .expect("a skyhook is ours")
        .expect("no error");
    let PyValue::Dict(pairs) = got else {
        panic!("a dict")
    };
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].1,
        PyValue::Int(ALICE_ITEM),
        "keyed by the source item"
    );
    let PyValue::List(entries) = &pairs[0].0 else {
        panic!("a list of one entry")
    };
    assert_eq!(
        entries,
        &vec![PyValue::Int(ALICE_ITEM)],
        "a whole-item move keeps its id"
    );
    assert!(by_id(reg.deltas(), ALICE_ITEM).is_some());
    // A refusal is a (label, args) entry, not an id.
    let args = drop_args(DAVE_ITEM);
    let PyValue::Dict(pairs) = drop_call(&mut reg, &drop_request(&args, Some(DAVE)))
        .unwrap()
        .unwrap()
    else {
        panic!("a dict")
    };
    let PyValue::List(entries) = &pairs[0].0 else {
        panic!("a list")
    };
    assert!(matches!(&entries[0], PyValue::Tuple(t) if t.len() == 2));
    // Anything that is not a skyhook is not ours.
    let other = drop_args(999);
    assert!(drop_call(&mut reg, &drop_request(&other, Some(ALICE))).is_none());
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

#[test]
fn the_stage_finishes_a_build_only_when_its_time_is_up() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_POWER, 1_000.0);
    let id = launch_one_at(&mut reg, ALICE, ALICE_ITEM, NOW).unwrap();
    let _ = sent(&mut reg);
    tick_at(&mut reg, NOW + ACTIVATION_S - 1);
    assert!(!by_id(reg.deltas(), id).unwrap().built(), "too early");
    tick_at(&mut reg, NOW + ACTIVATION_S);
    let sk = by_id(reg.deltas(), id).unwrap();
    assert!(sk.built() && sk.active);
    assert_eq!(
        sk.last_cycle,
        NOW + ACTIVATION_S,
        "harvest starts at activation"
    );
    // The corp's online members are told, nobody else.
    let pushes = sent(&mut reg);
    let first: Vec<_> = pushes
        .iter()
        .filter(|(n, ..)| *n == "OnSkyhookFirstActivation")
        .collect();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].1, vec![PyValue::Int(id), PyValue::Int(P_POWER)]);
    let to = first[0].2.clone().expect("an audience");
    assert!(to.contains(&ALICE) && to.contains(&BOB));
    assert!(!to.contains(&DAVE) && !to.contains(&CAROL));
    // And the slim now says the build is done.
    let (_, rows) = &slim_extras(&reg, &[id])[0];
    let done = rows
        .iter()
        .find(|(n, _)| *n == "component_activate")
        .unwrap();
    assert!(matches!(&done.1, PyValue::Tuple(t) if t[0] == PyValue::Bool(true)));
    // A second run does nothing more.
    let _ = sent(&mut reg);
    tick_at(&mut reg, NOW + 10 * ACTIVATION_S);
    assert!(
        !sent(&mut reg)
            .iter()
            .any(|(n, ..)| *n == "OnSkyhookFirstActivation"),
        "told once"
    );
}

// ---------------------------------------------------------------------------
// What counts toward the hub
// ---------------------------------------------------------------------------

#[test]
fn only_active_skyhooks_of_the_holder_alliance_feed_the_hub() {
    let (mut reg, ..) = world();
    let star = fixture_stars().power(9_000);
    let power = fixture_planets().get(9_001).unwrap().power;
    let land = fixture_planets().get(9_002).unwrap().workforce;
    assert!(power > 0 && land > 0);
    // No skyhook: the star alone.
    assert_eq!(sov_resources::harvested(&reg, SYSTEM), (star, 0));
    let sk_power = stand(&mut reg, SK_POWER, P_POWER, CORP, false, NOW);
    stand(&mut reg, SK_LAND, P_LAND, CORP, true, NOW);
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    // The power one is inactive, the workforce one active, the reagent one
    // gives neither.
    assert_eq!(sov_resources::harvested(&reg, SYSTEM), (star, land));
    let mut on = sk_power;
    on.active = true;
    put(reg.deltas_mut(), &on);
    assert_eq!(sov_resources::harvested(&reg, SYSTEM), (star + power, land));
    // The budget the hub reads follows.
    let r = sov_resources::hub_resources(&reg, HUB).expect("a hub");
    assert_eq!(r.power.available, star + power);
    assert_eq!(r.workforce.available, land);
    // The same skyhook in a rival corp's hands does not count.
    let mut rival = on;
    rival.corp = RIVAL;
    put(reg.deltas_mut(), &rival);
    assert_eq!(sov_resources::harvested(&reg, SYSTEM), (star, land));
}

// ---------------------------------------------------------------------------
// The gateway
// ---------------------------------------------------------------------------

#[test]
fn get_answers_the_owner_and_refuses_a_stranger_and_an_unknown_id() {
    let (mut reg, ..) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, true, wall_now());
    let g = ground(reg);
    let r = ask(&g, "GetRequest", &id_body(SK), ALICE);
    assert_eq!(r.status, 200);
    let rec = payload(&r);
    assert_eq!(
        get_bytes(rec, 1).and_then(|b| ident_of(&b)),
        Some(SK as u64)
    );
    assert_eq!(get_uint(rec, 3), Some(1), "active");
    let sim = &field(rec, 2)[0];
    assert_eq!(
        get_bytes(sim, 5).and_then(|b| ident_of(&b)),
        Some(FIXTURE_REAGENT_TYPE as u64),
        "the reagent type survives the Get trap"
    );
    // Bob is in the corp; Dave is in neither the corp nor the holder alliance.
    assert_eq!(ask(&g, "GetRequest", &id_body(SK), BOB).status, 200);
    assert_eq!(ask(&g, "GetRequest", &id_body(SK), DAVE).status, 403);
    assert_eq!(ask(&g, "GetRequest", &id_body(SK + 77), ALICE).status, 403);
    assert_eq!(ask(&g, "GetRequest", &[], ALICE).status, 403);
}

#[test]
fn get_all_local_names_the_session_system_and_lists_its_skyhooks() {
    let (mut reg, ..) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, true, wall_now());
    stand(&mut reg, SK_POWER, P_POWER, CORP, true, wall_now());
    let g = ground(reg);
    let r = ask(&g, "GetAllLocalRequest", &[], ALICE);
    assert_eq!(r.status, 200);
    let body = payload(&r);
    assert_eq!(
        get_bytes(body, 1).and_then(|b| ident_of(&b)),
        Some(SYSTEM as u64),
        "the cache primes only when field 1 is the session system"
    );
    let list = field(body, 2);
    assert_eq!(list.len(), 2);
    // The reagent one carries its type at the AllLocal definition field 3.
    let with_reagent: Vec<_> = list.iter().filter(|s| !field(s, 4).is_empty()).collect();
    assert_eq!(with_reagent.len(), 1);
    assert_eq!(
        get_bytes(&field(with_reagent[0], 4)[0], 3).and_then(|b| ident_of(&b)),
        Some(FIXTURE_REAGENT_TYPE as u64)
    );
}

#[test]
fn get_all_by_corporation_answers_your_own_and_refuses_another() {
    let (mut reg, ..) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, true, wall_now());
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, wall_now());
    let g = ground(reg);
    let r = ask(&g, "GetAllByCorporationRequest", &id_body(CORP), ALICE);
    assert_eq!(r.status, 200);
    let list = field(payload(&r), 1);
    assert_eq!(list.len(), 1);
    assert_eq!(
        get_bytes(&field(&list[0], 2)[0], 1).and_then(|b| ident_of(&b)),
        Some(FIXTURE_REAGENT_TYPE as u64),
        "ByCorp has its own field for the type"
    );
    assert_eq!(
        ask(&g, "GetAllByCorporationRequest", &id_body(RIVAL), ALICE).status,
        403
    );
}

#[test]
fn activate_and_deactivate_need_the_owner_corps_manager_and_a_finished_build() {
    let (mut reg, ..) = world();
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, false, NOW);
    let unbuilt = Skyhook {
        id: SK_POWER,
        activated_at: 0,
        planet: P_POWER,
        ..sk
    };
    put(reg.deltas_mut(), &unbuilt);
    // The first one was built but is switched off.
    let mut built = sk;
    built.activated_at = NOW - 10;
    put(reg.deltas_mut(), &built);
    let g = ground(reg);
    // Bob has no role, Dave is another corp, the id may be unknown.
    assert_eq!(ask(&g, "ActivateRequest", &id_body(SK), BOB).status, 403);
    assert_eq!(ask(&g, "ActivateRequest", &id_body(SK), DAVE).status, 403);
    assert_eq!(
        ask(&g, "ActivateRequest", &id_body(SK + 9), ALICE).status,
        404
    );
    // Still building.
    assert_eq!(
        ask(&g, "ActivateRequest", &id_body(SK_POWER), ALICE).status,
        403
    );
    // Alice may.
    let r = ask(&g, "ActivateRequest", &id_body(SK), ALICE);
    assert_eq!(r.status, 200);
    assert!(r.payload.is_some(), "a typed, empty ActivateResponse");
    assert!(by_id(world_of(&g).deltas(), SK).unwrap().active);
    assert_eq!(
        ask(&g, "DeactivateRequest", &id_body(SK), ALICE).status,
        200
    );
    assert!(!by_id(world_of(&g).deltas(), SK).unwrap().active);
}

#[test]
fn deactivate_settles_the_harvest_then_freezes_and_activate_restarts_the_cycle() {
    let (mut reg, ..) = world();
    let r = reagent();
    let start = wall_now() - 5 * r.cycle_s - 1;
    stand(&mut reg, SK, P_REAGENT, CORP, true, start);
    let g = ground(reg);
    assert_eq!(
        ask(&g, "DeactivateRequest", &id_body(SK), ALICE).status,
        200
    );
    let frozen = by_id(world_of(&g).deltas(), SK).unwrap();
    // Five whole cycles were settled in, split by the rule.
    let want = harvest(&sim(0, 0, start), &r, start + 5 * r.cycle_s);
    assert_eq!(frozen.sim(), want);
    assert!(u64::from(frozen.secured + frozen.unsecured) > 0);
    // Reading later does not grow it.
    let rec = ask(&g, "GetRequest", &id_body(SK), ALICE);
    let s = &field(payload(&rec), 2)[0];
    let simulation = get_bytes(s, 4).unwrap();
    assert_eq!(get_uint(&simulation, 1), Some(u64::from(want.secured)));
    assert_eq!(get_uint(&simulation, 2), Some(u64::from(want.unsecured)));
    // Switching on again starts a new cycle now, keeping the stock.
    assert_eq!(ask(&g, "ActivateRequest", &id_body(SK), ALICE).status, 200);
    let again = by_id(world_of(&g).deltas(), SK).unwrap();
    assert!(again.active);
    assert!(again.last_cycle + 5 >= wall_now(), "moved to now");
    assert_eq!(
        (again.secured, again.unsecured),
        (want.secured, want.unsecured)
    );
}

#[test]
fn the_theft_lists_are_empty_ok_and_the_admin_calls_fall_through_to_the_stub() {
    let g = ground(world().0);
    for name in [
        "GetSolarSystemsWithTheftVulnerableSkyhooksRequest",
        "GetTheftVulnerableSkyhooksInSolarSystemRequest",
    ] {
        let r = ask(&g, name, &id_body(SYSTEM), ALICE);
        assert_eq!(r.status, 200, "{name}");
        assert!(payload(&r).is_empty());
    }
    assert!(gateway(&req("admin.ModifyReagentsRequest"), &[], ALICE, &g).is_none());
    // The chain reaches this arm before the stub for a read, and the stub's
    // 403 for the admin namespace still holds.
    assert_eq!(
        crate::gateway::answer(&req("admin.ModifyReagentsRequest"), &[], ALICE, &g).status,
        403
    );
}

#[test]
fn on_a_local_ground_there_is_no_world() {
    let local = Ground::local();
    assert!(gateway(&req("GetAllLocalRequest"), &[], ALICE, &local).is_none());
    assert_eq!(
        gateway(&req("GetRequest"), &id_body(SK), ALICE, &local)
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        gateway(&req("ActivateRequest"), &id_body(SK), ALICE, &local)
            .unwrap()
            .status,
        404
    );
    assert!(
        gateway(
            "eve_public.sovereignty.hub.api.GetRequest",
            &[],
            ALICE,
            &local
        )
        .is_none()
    );
}

// ---------------------------------------------------------------------------
// Component calls
// ---------------------------------------------------------------------------

fn call(actor: i64, item: i64, args: &[PyValue]) -> CompCall<'_> {
    CompCall {
        actor,
        system: SYSTEM,
        item_id: item,
        args,
        kwargs: &[],
    }
}

/// A skyhook with known stocks over the reagent planet, the pilots beside it.
fn stocked(reg: &mut Registry, secured: u32, unsecured: u32) {
    let mut sk = stand(reg, SK, P_REAGENT, CORP, true, NOW);
    sk.secured = secured;
    sk.unsecured = unsecured;
    put(reg.deltas_mut(), &sk);
    // A pilot within reach of the ball's surface.
    let r = radius(reg.content(), SKYHOOK_TYPE);
    let at = planet_at(reg, P_REAGENT);
    place_at(reg, ALICE, [at[0] + r + 500.0, at[1], at[2]]);
    place_at(reg, DAVE, [at[0] + r + 500.0, at[1], at[2]]);
}

fn cargo_of(reg: &Registry, who: i64, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.location_id == ship(who) && i.type_id == type_id)
        .map(|i| i.quantity)
        .sum()
}

#[test]
fn authorisation_is_the_owner_corp() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 1, 1);
    let yes = is_character_authorised(&mut reg, &call(BOB, SK, &[])).unwrap();
    let no = is_character_authorised(&mut reg, &call(DAVE, SK, &[])).unwrap();
    assert_eq!((yes, no), (PyValue::Bool(true), PyValue::Bool(false)));
}

#[test]
fn extraction_takes_the_unsecured_stock_first_and_makes_that_many_items() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 10, 6);
    let args = [PyValue::Int(5), PyValue::Int(4)];
    let moved = extract_at(&mut reg, &call(ALICE, SK, &args), NOW).unwrap();
    assert_eq!(moved, PyValue::Int(4));
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (10, 2), "unsecured went first");
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_REAGENT_TYPE), 4);
    // Asking for more than is there gives what is there: the 2 unsecured and
    // then the 10 secured ones.
    let args = [PyValue::Int(5), PyValue::Int(1_000)];
    let moved = extract_at(&mut reg, &call(ALICE, SK, &args), NOW).unwrap();
    assert_eq!(moved, PyValue::Int(12));
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (0, 0));
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_REAGENT_TYPE), 16);
    // Nothing left: a refusal, and no item.
    assert!(extract_at(&mut reg, &call(ALICE, SK, &args), NOW).is_err());
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_REAGENT_TYPE), 16);
}

#[test]
fn extraction_settles_the_harvest_first() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 0, 0);
    let r = reagent();
    // Three whole cycles have run since the record.
    let mut sk = by_id(reg.deltas(), SK).unwrap();
    sk.last_cycle = NOW - 3 * r.cycle_s;
    put(reg.deltas_mut(), &sk);
    let args = [PyValue::Int(5), PyValue::Int(1_000)];
    let moved = extract_at(&mut reg, &call(ALICE, SK, &args), NOW).unwrap();
    let want = harvest(&sim(0, 0, NOW - 3 * r.cycle_s), &r, NOW);
    let total = i64::from(want.secured + want.unsecured);
    assert!(total > 0, "the fixture row must yield in three cycles");
    assert_eq!(moved, PyValue::Int(total));
}

#[test]
fn extraction_is_refused_for_a_non_member_a_bad_hold_and_a_long_way_off() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 5, 5);
    let args = [PyValue::Int(5), PyValue::Int(3)];
    assert!(extract_at(&mut reg, &call(DAVE, SK, &args), NOW).is_err());
    // Not a hold a reagent may go to.
    let bad = [PyValue::Int(27), PyValue::Int(3)];
    assert!(extract_at(&mut reg, &call(ALICE, SK, &bad), NOW).is_err());
    let none = [PyValue::Int(5), PyValue::Int(0)];
    assert!(extract_at(&mut reg, &call(ALICE, SK, &none), NOW).is_err());
    // A long way off.
    let at = planet_at(&reg, P_REAGENT);
    place_at(
        &mut reg,
        ALICE,
        [at[0] + 5.0 * TRANSFER_RANGE + 10_000.0, at[1], at[2]],
    );
    assert!(extract_at(&mut reg, &call(ALICE, SK, &args), NOW).is_err());
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (5, 5), "nothing taken");
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_REAGENT_TYPE), 0);
    assert_eq!(cargo_of(&reg, DAVE, FIXTURE_REAGENT_TYPE), 0);
}

#[test]
fn the_component_row_locates_a_live_skyhook_only() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 1, 1);
    assert_eq!(locate(&reg, SK), Some(SYSTEM));
    assert_eq!(locate(&reg, SK + 99), None);
    // The shipped table routes both methods.
    let names: Vec<_> = crate::space_component::CALLS
        .iter()
        .filter(|(c, ..)| *c == ORBITAL_SKYHOOK)
        .flat_map(|(_, _, m)| m.iter().map(|(n, _)| *n))
        .collect();
    assert_eq!(
        names,
        vec!["IsCharacterAuthorised", "ExtractReagentsToShip"]
    );
}

// ---------------------------------------------------------------------------
// The cleanup after a claim
// ---------------------------------------------------------------------------

fn credit(reg: &mut Registry, corp: i64, isk: f64) {
    crate::corp_wallet::adjust(
        reg.deltas_mut(),
        corp,
        crate::corp_wallet::MASTER_KEY,
        crate::corp_wallet::to_cents(isk),
    )
    .expect("a credit");
}

fn balance(reg: &Registry, corp: i64) -> i64 {
    crate::corp_wallet::balance(reg.deltas(), corp, crate::corp_wallet::MASTER_KEY)
}

/// The claim was made `secs` before `now` (the holder stays alliance `a`).
fn claimed_ago(reg: &mut Registry, a: i64, secs: u64, now: u64) {
    let mut c = crate::sov::claim(reg.deltas(), SYSTEM).expect("a claim");
    assert_eq!(c.alliance_id, Some(a));
    c.claim_time = Some(filetime(now - secs));
    crate::sov::put(reg.deltas_mut(), &c);
}

#[test]
fn acquire_inside_the_window_charges_the_fee_and_moves_the_corp() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    credit(&mut reg, CORP, 3.0 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, CLAIM_WINDOW_S - 60, NOW);
    let before = balance(&reg, CORP);
    let got = acquire(&mut reg, ALICE, &[SK_RIVAL], NOW).expect("no error");
    assert_eq!(got, vec![SK_RIVAL]);
    assert_eq!(
        before - balance(&reg, CORP),
        crate::corp_wallet::to_cents(SKYHOOK_CLAIM_FEE)
    );
    let sk = by_id(reg.deltas(), SK_RIVAL).unwrap();
    assert_eq!(sk.corp, CORP);
    assert_eq!(reg.store().get(SK_RIVAL).unwrap().owner_id, CORP);
    // The fee is journalled under the ledger's label (ref 184).
    let rows = crate::corp_wallet::all_rows(reg.deltas(), CORP);
    assert!(
        rows.iter()
            .any(|r| r.ref_type == REF_SKYHOOK_CLAIM_FEE && r.reference == SK_RIVAL)
    );
    // It counts toward the hub now that its owner is in the holder alliance.
    let power = fixture_planets().get(9_001).unwrap().power;
    assert_eq!(
        sov_resources::harvested(&reg, SYSTEM).0,
        fixture_stars().power(9_000) + power
    );
}

#[test]
fn acquire_after_the_window_or_by_a_non_manager_or_of_a_friend_does_nothing() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    credit(&mut reg, CORP, 3.0 * SKYHOOK_CLAIM_FEE);
    let before = balance(&reg, CORP);
    // Outside the window.
    claimed_ago(&mut reg, a, CLAIM_WINDOW_S + 60, NOW);
    assert_eq!(
        acquire(&mut reg, ALICE, &[SK_RIVAL], NOW).unwrap(),
        Vec::<i64>::new()
    );
    // Inside, but Bob has no role.
    claimed_ago(&mut reg, a, 100, NOW);
    assert_eq!(
        acquire(&mut reg, BOB, &[SK_RIVAL], NOW).unwrap(),
        Vec::<i64>::new()
    );
    // Inside, but the holder's own skyhook is not for acquiring.
    assert_eq!(
        acquire(&mut reg, ALICE, &[SK], NOW).unwrap(),
        Vec::<i64>::new()
    );
    assert_eq!(balance(&reg, CORP), before, "no fee");
    assert_eq!(by_id(reg.deltas(), SK_RIVAL).unwrap().corp, RIVAL);
}

#[test]
fn acquire_with_a_short_wallet_refuses_and_moves_nothing() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    credit(&mut reg, CORP, 0.5 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, 100, NOW);
    let before = balance(&reg, CORP);
    assert!(acquire(&mut reg, ALICE, &[SK_RIVAL], NOW).is_err());
    assert_eq!(balance(&reg, CORP), before);
    assert_eq!(by_id(reg.deltas(), SK_RIVAL).unwrap().corp, RIVAL);
    assert_eq!(reg.store().get(SK_RIVAL).unwrap().owner_id, RIVAL);
}

#[test]
fn destroy_inside_the_window_removes_item_row_and_ball_and_outside_does_nothing() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    claimed_ago(&mut reg, a, CLAIM_WINDOW_S + 5, NOW);
    assert!(destroy(&mut reg, ALICE, &[SK_RIVAL], NOW).is_empty());
    assert!(by_id(reg.deltas(), SK_RIVAL).is_some());
    claimed_ago(&mut reg, a, 100, NOW);
    let _ = sent(&mut reg);
    assert_eq!(destroy(&mut reg, ALICE, &[SK_RIVAL], NOW), vec![SK_RIVAL]);
    assert!(by_id(reg.deltas(), SK_RIVAL).is_none());
    assert!(reg.store().get(SK_RIVAL).is_none());
    assert!(balls_in(&reg, SYSTEM).is_empty());
    let q = crate::gateway_out::take(&mut reg);
    assert!(
        q.iter()
            .any(|(ty, ..)| ty.ends_with("AllInSolarSystemNotice")),
        "the system's cache is re-primed"
    );
}

fn macho_call(
    reg: &mut Registry,
    svc: &'static str,
    method: &'static str,
    args: Vec<PyValue>,
) -> Result<PyValue, ServiceError> {
    let r = Request {
        service: svc,
        method,
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: Some(ALICE),
    };
    macho(reg, &r)
}

#[test]
fn the_macho_arms_list_and_clean_up() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    claimed_ago(&mut reg, a, 100, NOW);
    credit(&mut reg, CORP, 2.0 * SKYHOOK_CLAIM_FEE);
    let PyValue::List(rows) = macho_call(
        &mut reg,
        "colonyResourcesMgr",
        "GetMyCorporationSkyhooks",
        vec![],
    )
    .unwrap() else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1, "only the corp's own");
    // A KeyVal: the instance's state maps each attribute name to its value.
    let PyValue::Instance { state, .. } = &rows[0] else {
        panic!("an instance, got {:?}", rows[0])
    };
    let PyValue::Dict(attrs) = &**state else {
        panic!("a dict state")
    };
    let get = |name: &str| {
        attrs
            .iter()
            .find(|(_, k)| *k == PyValue::Bytes(name.as_bytes().to_vec()))
            .map(|(v, _)| v.clone())
    };
    assert_eq!(get("skyhookID"), Some(PyValue::Int(SK)));
    assert_eq!(get("planetID"), Some(PyValue::Int(P_REAGENT)));
    assert_eq!(get("solarSystemID"), Some(PyValue::Int(SYSTEM)));
    assert_eq!(get("typeID"), Some(PyValue::Int(SKYHOOK_TYPE)));
    assert!(get("state").is_some());
    assert!(macho_call(&mut reg, "sovMgr", "NoSuchThing", vec![]).is_err());
    assert!(macho_call(&mut reg, "sovMgr", "AcquireSkyhooks", vec![]).is_err());
}

#[test]
fn the_service_router_serves_the_skyhook_calls() {
    let (mut reg, ..) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    for (svc, method, args) in [
        (
            "sovMgr",
            "AcquireSkyhooks",
            vec![PyValue::List(vec![PyValue::Int(SK)]), PyValue::None],
        ),
        (
            "sovMgr",
            "DestroySkyhooks",
            vec![PyValue::List(vec![PyValue::Int(SK)])],
        ),
        ("colonyResourcesMgr", "GetMyCorporationSkyhooks", vec![]),
    ] {
        let r = Request {
            service: svc,
            method,
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert!(reg.dispatch(&r).is_ok(), "{svc}.{method} is served");
    }
}

// ---------------------------------------------------------------------------
// The row, the position, the GM line
// ---------------------------------------------------------------------------

#[test]
fn a_row_round_trips_and_an_old_twelve_value_row_defaults_the_state() {
    let mut d = Deltas::default();
    let sk = Skyhook {
        id: SK,
        planet: P_POWER,
        system: SYSTEM,
        corp: CORP,
        active: true,
        activated_at: 11,
        built_until: 10,
        secured: 7,
        unsecured: 8,
        last_cycle: 9,
        vuln_start: 1,
        vuln_end: 2,
        state: 212,
    };
    put(&mut d, &sk);
    assert_eq!(by_id(&d, SK), Some(sk));
    assert_eq!(by_id(&d, SK + 1), None);
    // A row written before the state column existed.
    let key = vec![(SK + 5).to_string()];
    let eleven: Vec<String> = ["1", "2", "3", "1", "5", "4", "0", "0", "6", "0", "0"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    d.put(KIND, key, eleven);
    assert_eq!(by_id(&d, SK + 5).unwrap().state, STATE_SHIELD_VULNERABLE);
    // A row that is garbage is left out and does not stop the others.
    d.put(KIND, vec!["77".to_owned()], vec!["x".to_owned()]);
    assert_eq!(all(&d).len(), 2);
}

#[test]
fn the_skyhook_stands_one_standoff_inside_the_warp_in_point() {
    let (reg, ..) = world();
    let planet = reg.celestials().get(P_REAGENT).unwrap();
    let w = crate::warp_in::warp_in_point(planet).expect("a warp-in point");
    let at = position_of(planet);
    let gap = crate::space_state::distance(at, w);
    assert!((gap - crate::planet::CUSTOMS_OFFICE_STANDOFF).abs() < 1.0);
    // The customs office is that far beyond the point on the same line, so
    // the skyhook is on the planet's side of it, never at the same spot.
    let from_planet = |p: [f64; 3]| crate::space_state::distance(p, planet.position);
    assert!(from_planet(at) < from_planet(w));
}

#[test]
fn gm_spawn_activate_and_age_make_the_harvest_visible() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_REAGENT, 1_000.0);
    let said = slash_cmd::run(&mut reg, Some(ALICE), &["spawn", "nearest"], NOW);
    assert!(said.contains("over planet"), "{said}");
    let sk = all(reg.deltas())[0];
    assert_eq!((sk.planet, sk.corp), (P_REAGENT, CORP));
    assert!(!sk.built());
    let said = slash_cmd::run(
        &mut reg,
        Some(ALICE),
        &["activate", &sk.id.to_string()],
        NOW,
    );
    assert!(said.contains("active"), "{said}");
    let r = reagent();
    let said = slash_cmd::run(
        &mut reg,
        Some(ALICE),
        &["age", &sk.id.to_string(), "5"],
        NOW,
    );
    assert!(said.contains("5 cycles"), "{said}");
    let aged = by_id(reg.deltas(), sk.id).unwrap();
    // The cycles are credited and the phase sits at now.
    assert_eq!(aged.last_cycle, NOW);
    let s = aged.sim();
    assert_eq!(
        u64::from(s.secured + s.unsecured),
        (5 * u64::from(r.per_cycle)).min(u64::from(r.secured_cap + r.unsecured_cap))
    );
    // Stocks queued as a notice so the client's cache refreshes.
    let q = crate::gateway_out::take(&mut reg);
    assert!(
        q.iter()
            .any(|(ty, ..)| ty.ends_with("ReagentSimulationsNotice"))
    );
    // Bad words get the usage line; a state outside the table is refused.
    assert!(slash_cmd::run(&mut reg, Some(ALICE), &["nonsense"], NOW).contains("usage"));
    let said = slash_cmd::run(
        &mut reg,
        Some(ALICE),
        &["state", &sk.id.to_string(), "999"],
        NOW,
    );
    assert!(said.contains("not a skyhook state"));
    let said = slash_cmd::run(
        &mut reg,
        Some(ALICE),
        &["state", &sk.id.to_string(), "212"],
        NOW,
    );
    assert!(said.contains("212"));
    assert_eq!(by_id(reg.deltas(), sk.id).unwrap().state, 212);
}

#[test]
fn age_after_activate_leaves_a_snapshot_with_the_credited_stock_last() {
    let (mut reg, ..) = world();
    at_spot(&mut reg, ALICE, P_REAGENT, 1_000.0);
    slash_cmd::run(&mut reg, Some(ALICE), &["spawn", "nearest"], NOW);
    let id = all(reg.deltas())[0].id;
    slash_cmd::run(&mut reg, Some(ALICE), &["activate", &id.to_string()], NOW);
    slash_cmd::run(&mut reg, Some(ALICE), &["age", &id.to_string(), "5"], NOW);
    let aged = by_id(reg.deltas(), id).unwrap();
    assert!(aged.sim().unsecured > 0);
    let want = record(Msg::AllInSystem, &view(&reg, &aged, NOW));
    let q = crate::gateway_out::take(&mut reg);
    let last = q
        .iter()
        .rev()
        .find(|(ty, ..)| ty.ends_with("AllInSolarSystemNotice"))
        .expect("a whole-system snapshot");
    // The snapshot holds the aged skyhook's record, byte for byte.
    assert!(
        last.1.windows(want.len()).any(|w| w == &want[..]),
        "the last snapshot carries the credited stock"
    );
    // Nothing whole-system comes after the stocks notice that would undo it.
    let pos = |name: &str| q.iter().rposition(|(ty, ..)| ty.ends_with(name)).unwrap();
    assert!(pos("AllInSolarSystemNotice") < pos("ReagentSimulationsNotice"));
}

// ---------------------------------------------------------------------------
// Round 2: a dispossessed owner, the frozen stock on the wire, the room in
// the hold, and the cleanup's money
// ---------------------------------------------------------------------------

/// Alliance D takes the system (hub owner: the rival corp) at `NOW`.
fn rival_takes_the_system(reg: &mut Registry, d: i64) {
    crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((d, RIVAL)), filetime(NOW));
}

fn last_cycle_of(rec: &[u8]) -> u64 {
    let sim = &field(rec, 2)[0];
    let simulation = get_bytes(sim, 4).expect("a simulation");
    let ts = get_bytes(&simulation, 3).expect("a last cycle");
    get_uint(&ts, 1).expect("seconds")
}

#[test]
fn a_dispossessed_owners_skyhook_does_not_harvest() {
    let (mut reg, _, d) = world();
    let r = reagent();
    let start = NOW - 20 * r.cycle_s;
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, true, start);
    assert!(running(reg.deltas(), &sk));
    rival_takes_the_system(&mut reg, d);
    assert!(!held(reg.deltas(), &sk));
    assert!(!running(reg.deltas(), &sk));
    // Neither settling nor reading adds a cycle.
    let s = settle(&mut reg, SK, NOW).expect("a row");
    assert_eq!(s.sim(), sim(0, 0, start));
    let v = view(&reg, &sk, NOW);
    assert!(!v.active);
    let (_, shown) = v.reagent.expect("a reagent");
    assert_eq!((shown.secured, shown.unsecured), (0, 0));
}

#[test]
fn slash_age_credits_cycles_that_the_claim_and_join_clamps_leave_alone() {
    let (mut reg, a, _) = world();
    let r = reagent();
    // The claim is as young as the skyhook, as in a scenario.
    crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((a, CORP)), filetime(NOW));
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    let id = SK.to_string();
    let said = slash_cmd::run(&mut reg, Some(ALICE), &["age", &id, "5"], NOW);
    assert!(said.contains("credited 5 cycles"), "{said}");
    let want = harvest(&sim(0, 0, NOW - 5 * r.cycle_s), &r, NOW);
    assert!(
        want.secured + want.unsecured > 0,
        "five cycles produce something"
    );
    assert_eq!(want.last_cycle, NOW);
    let s = settle(&mut reg, SK, NOW).expect("a row");
    assert_eq!(s.sim(), want);
    // Reading at the same moment shows the stock too.
    let sk = by_id(reg.deltas(), SK).expect("a row");
    assert_eq!(view(&reg, &sk, NOW).reagent.expect("a reagent").1, want);
}

#[test]
fn slash_age_keeps_the_phase_of_the_partial_cycle() {
    let (mut reg, a, _) = world();
    let r = reagent();
    crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((a, CORP)), filetime(NOW));
    stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    // Half a cycle in: nothing is whole yet, and the phase stays at NOW.
    let later = NOW + r.cycle_s / 2;
    let id = SK.to_string();
    slash_cmd::run(&mut reg, Some(ALICE), &["age", &id, "5"], later);
    let want = harvest(&sim(0, 0, NOW - 5 * r.cycle_s), &r, later);
    assert_eq!(want.last_cycle, NOW);
    assert_eq!(by_id(reg.deltas(), SK).expect("a row").sim(), want);
}

#[test]
fn slash_age_on_a_skyhook_that_is_not_running_credits_nothing() {
    let (mut reg, ..) = world();
    stand(&mut reg, SK, P_REAGENT, CORP, false, NOW);
    let id = SK.to_string();
    let said = slash_cmd::run(&mut reg, Some(ALICE), &["age", &id, "5"], NOW);
    assert!(said.contains("not running"), "{said}");
    let s = by_id(reg.deltas(), SK).expect("a row");
    assert_eq!((s.secured, s.unsecured), (0, 0));
}

#[test]
fn the_slim_extras_carry_a_dun_rotation_of_three_reals() {
    let (mut reg, ..) = world();
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, true, NOW);
    let extras = extras_of(&reg, &sk);
    let (_, v) = extras
        .iter()
        .find(|(k, _)| *k == "dunRotation")
        .expect("a dunRotation key");
    let PyValue::Tuple(t) = v else {
        panic!("a tuple, got {v:?}");
    };
    assert_eq!(t.len(), 3);
    assert!(t.iter().all(|x| matches!(x, PyValue::Real(_))));
}

#[test]
fn slash_info_prints_the_rows_last_cycle_and_whether_it_runs() {
    let (mut reg, _, d) = world();
    let r = reagent();
    let start = NOW - 7 * r.cycle_s;
    stand(&mut reg, SK, P_REAGENT, CORP, true, start);
    let id = SK.to_string();
    let said = slash_cmd::run(&mut reg, Some(ALICE), &["info", &id], NOW);
    assert!(said.contains(&format!("last_cycle {start}")), "{said}");
    assert!(said.contains("active running"), "{said}");
    rival_takes_the_system(&mut reg, d);
    let said = slash_cmd::run(&mut reg, Some(ALICE), &["info", &id], NOW);
    assert!(said.contains(&format!("last_cycle {start}")), "{said}");
    assert!(said.contains("not-running"), "{said}");
}

#[test]
fn a_system_lost_and_regained_does_not_pay_for_the_gap() {
    let (mut reg, a, d) = world();
    let r = reagent();
    let start = NOW - 20 * r.cycle_s;
    let sk = stand(&mut reg, SK, P_REAGENT, CORP, true, start);
    // Lost ten cycles ago, regained now: the holder changes twice.
    crate::sov::set_claim(
        reg.deltas_mut(),
        SYSTEM,
        Some((d, RIVAL)),
        filetime(NOW - 10 * r.cycle_s),
    );
    crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((a, CORP)), filetime(NOW));
    assert!(running(reg.deltas(), &sk));
    let later = NOW + 2 * r.cycle_s;
    let want = harvest(&sim(0, 0, NOW), &r, later);
    assert_ne!(want, sim(0, 0, NOW), "two cycles produce something");
    // Reading shows two cycles, not twenty-two ...
    let v = view(&reg, &sk, later);
    assert_eq!(v.reagent.expect("a reagent").1, want);
    // ... and settling stores them.
    let s = settle(&mut reg, SK, later).expect("a row");
    assert_eq!(s.sim(), want);
}

#[test]
fn an_owner_that_joins_the_holder_late_does_not_pay_for_the_gap() {
    let (mut reg, a, _) = world();
    let r = reagent();
    let start = NOW - 20 * r.cycle_s;
    // Carol's corp stands outside the alliance with a switched-on skyhook.
    let sk = stand(&mut reg, SK, P_REAGENT, LONE, true, start);
    assert!(!running(reg.deltas(), &sk), "outside the holder alliance");
    // It joins two cycles ago; the claim is far older than that.
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    st.apply(LONE, a, "", filetime(start)).expect("applies");
    st.answer(
        a,
        LONE,
        crate::alliance::APP_ACCEPTED,
        None,
        filetime(NOW - 2 * r.cycle_s),
    )
    .expect("accepted");
    st.save(reg.deltas_mut());
    assert!(running(reg.deltas(), &sk));
    let want = harvest(&sim(0, 0, NOW - 2 * r.cycle_s), &r, NOW);
    assert_ne!(want, sim(0, 0, NOW), "two cycles produce something");
    // Two cycles since the join, not twenty since the last cycle.
    assert_eq!(view(&reg, &sk, NOW).reagent.expect("a reagent").1, want);
    assert_eq!(settle(&mut reg, SK, NOW).expect("a row").sim(), want);
}

#[test]
fn a_dispossessed_owners_skyhook_cannot_be_switched_on_or_emptied() {
    let (mut reg, _, d) = world();
    stocked(&mut reg, 5, 5);
    let mut off = by_id(reg.deltas(), SK).unwrap();
    off.active = false;
    put(reg.deltas_mut(), &off);
    rival_takes_the_system(&mut reg, d);
    // The owner corp's manager is refused (403) ...
    assert_eq!(
        set_active(&mut reg, ALICE, SK, true, NOW),
        Err(Switch::Forbidden)
    );
    // ... a skyhook already marked active is refused too ...
    let mut on = off;
    on.active = true;
    put(reg.deltas_mut(), &on);
    assert_eq!(
        set_active(&mut reg, ALICE, SK, true, NOW),
        Err(Switch::Forbidden)
    );
    // ... but may still be switched off.
    assert_eq!(set_active(&mut reg, ALICE, SK, false, NOW), Ok(()));
    // Extraction is refused, and nothing moves.
    let args = [PyValue::Int(5), PyValue::Int(3)];
    assert!(extract_at(&mut reg, &call(ALICE, SK, &args), NOW).is_err());
    assert_eq!(cargo_of(&reg, ALICE, FIXTURE_REAGENT_TYPE), 0);
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (5, 5));
    // The client's own button is hidden.
    assert_eq!(
        is_character_authorised(&mut reg, &call(ALICE, SK, &[])).unwrap(),
        PyValue::Bool(false)
    );
}

#[test]
fn an_inactive_skyhook_reports_a_last_cycle_the_client_counts_no_cycle_from() {
    let (mut reg, ..) = world();
    let r = reagent();
    let start = NOW - 10 * r.cycle_s;
    let off = stand(&mut reg, SK, P_REAGENT, CORP, false, start);
    let rec = record(Msg::Get, &view(&reg, &off, NOW));
    let last = last_cycle_of(&rec);
    // The client's rule: `max(0, int((now - last) / period))` ticks.
    let ticks = (NOW as i64 - last as i64)
        .div_euclid(r.cycle_s as i64)
        .max(0);
    assert_eq!(ticks, 0, "the client extrapolates nothing");
    assert_eq!(get_uint(&rec, 3), Some(0), "and says inactive");
    // The stored row keeps its own last cycle.
    assert_eq!(by_id(reg.deltas(), SK).unwrap().last_cycle, start);
    // A running one reports its true last cycle.
    let on = stand(&mut reg, SK_LAND, P_REAGENT, CORP, true, start);
    let rec = record(Msg::Get, &view(&reg, &on, NOW));
    assert_eq!(last_cycle_of(&rec), start + 10 * r.cycle_s);
}

#[test]
fn extraction_is_clipped_to_the_room_in_the_hold() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 100, 100);
    // Bob's hull holds 120 m3 and the skyhook item in his cargo takes 100,
    // so 20 reagents fit (1 m3 each).
    let at = planet_at(&reg, P_REAGENT);
    let r = radius(reg.content(), SKYHOOK_TYPE);
    place_at(&mut reg, BOB, [at[0] + r + 500.0, at[1], at[2]]);
    let args = [PyValue::Int(5), PyValue::Int(150)];
    let moved = extract_at(&mut reg, &call(BOB, SK, &args), NOW).unwrap();
    assert_eq!(moved, PyValue::Int(20));
    assert_eq!(cargo_of(&reg, BOB, FIXTURE_REAGENT_TYPE), 20);
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!(
        (sk.secured, sk.unsecured),
        (100, 80),
        "only 20 left the stock"
    );
    // The hold is full: refused, nothing taken.
    assert!(extract_at(&mut reg, &call(BOB, SK, &args), NOW).is_err());
    assert_eq!(cargo_of(&reg, BOB, FIXTURE_REAGENT_TYPE), 20);
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (100, 80));
}

#[test]
fn extraction_to_the_colony_hold_is_clipped_to_that_hold() {
    let (mut reg, ..) = world();
    stocked(&mut reg, 100, 100);
    // Bob's colony resources hold (flag 185) takes 30 m3 while his cargo
    // takes 120 minus the skyhook item: the clip must use the hold asked for.
    let at = planet_at(&reg, P_REAGENT);
    let r = radius(reg.content(), SKYHOOK_TYPE);
    place_at(&mut reg, BOB, [at[0] + r + 500.0, at[1], at[2]]);
    let flag = crate::sov_hub::FLAG_INFRASTRUCTURE_HANGAR;
    let args = [PyValue::Int(flag), PyValue::Int(150)];
    let moved = extract_at(&mut reg, &call(BOB, SK, &args), NOW).unwrap();
    assert_eq!(moved, PyValue::Int(30));
    let sk = by_id(reg.deltas(), SK).unwrap();
    assert_eq!((sk.secured, sk.unsecured), (100, 70));
    // Full: refused.
    assert!(extract_at(&mut reg, &call(BOB, SK, &args), NOW).is_err());
}

#[test]
fn a_repeated_id_is_acquired_and_charged_once() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    credit(&mut reg, CORP, 5.0 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, 100, NOW);
    let before = balance(&reg, CORP);
    let got = acquire(&mut reg, ALICE, &[SK_RIVAL, SK_RIVAL, SK_RIVAL], NOW).unwrap();
    assert_eq!(got, vec![SK_RIVAL]);
    assert_eq!(
        before - balance(&reg, CORP),
        crate::corp_wallet::to_cents(SKYHOOK_CLAIM_FEE),
        "one fee"
    );
}

#[test]
fn an_acquire_that_cannot_move_the_item_costs_nothing() {
    let (mut reg, a, _) = world();
    // A row whose item is gone from the store.
    let ghost = SK_RIVAL + 90;
    assert!(reg.store().get(ghost).is_none());
    stand(&mut reg, ghost, P_POWER, RIVAL, true, NOW);
    credit(&mut reg, CORP, 3.0 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, 100, NOW);
    let before = balance(&reg, CORP);
    let got = acquire(&mut reg, ALICE, &[ghost], NOW).unwrap();
    assert!(got.is_empty());
    assert_eq!(balance(&reg, CORP), before, "no fee");
    assert_eq!(by_id(reg.deltas(), ghost).unwrap().corp, RIVAL);
}

#[test]
fn only_the_hub_owner_corp_may_clean_up() {
    let (mut reg, a, _) = world();
    stand(&mut reg, SK_RIVAL, P_POWER, RIVAL, true, NOW);
    credit(&mut reg, CORP, 3.0 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, 100, NOW);
    // The hub belongs to another corp now (the client offers the cleanup to
    // `hubInfo.ownerID == session.corpid` only, `sovSvc.py:390-401`).
    let mut c = crate::sov::claim(reg.deltas(), SYSTEM).expect("a claim");
    c.corporation_id = LONE;
    crate::sov::put(reg.deltas_mut(), &c);
    let before = balance(&reg, CORP);
    assert!(
        acquire(&mut reg, ALICE, &[SK_RIVAL], NOW)
            .unwrap()
            .is_empty()
    );
    assert!(destroy(&mut reg, ALICE, &[SK_RIVAL], NOW).is_empty());
    assert_eq!(balance(&reg, CORP), before);
    assert_eq!(by_id(reg.deltas(), SK_RIVAL).unwrap().corp, RIVAL);
}

#[test]
fn an_acquired_skyhook_starts_its_cycle_at_the_handover() {
    let (mut reg, a, _) = world();
    let r = reagent();
    let idle = NOW - 50 * r.cycle_s;
    stand(&mut reg, SK_RIVAL, P_REAGENT, RIVAL, true, idle);
    credit(&mut reg, CORP, 3.0 * SKYHOOK_CLAIM_FEE);
    claimed_ago(&mut reg, a, 100, NOW);
    assert_eq!(
        acquire(&mut reg, ALICE, &[SK_RIVAL], NOW).unwrap(),
        vec![SK_RIVAL]
    );
    let sk = by_id(reg.deltas(), SK_RIVAL).unwrap();
    assert_eq!(sk.last_cycle, NOW);
    let (_, shown) = view(&reg, &sk, NOW).reagent.expect("a reagent");
    assert_eq!(
        (shown.secured, shown.unsecured),
        (0, 0),
        "the idle time is not harvested"
    );
}

#[test]
fn a_launch_of_something_else_says_it_is_not_a_skyhook() {
    assert_eq!(Refused::NotASkyhook.text(), "That is not a skyhook.");
}
