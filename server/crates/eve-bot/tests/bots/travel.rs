//! **Gate travel** (the travel lane, R-SOCIETY SOC-A3) over a socket: a
//! builder whose station sells no blueprint finds one a jump away, flies
//! there, buys it, and brings it home in the hold.
//!
//! **Synthetic furniture only** (the no-client-data rule): two systems
//! with invented gates and an invented second station, one invented
//! blueprint type on the first rung of the builder's ladder
//! ([`builder::BLUEPRINT_LADDER`], named, never its value), and an invented
//! NPC corporation that owns the far station and lists that blueprint, so
//! under O5-a the exchange sells it there (E5) and nowhere else. The system
//! ids are the sandbox's home (`Character::default()`'s) and one other; a
//! third character, seeded at the far station, is what makes that station
//! one this world knows.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::{self, BLUEPRINT_LADDER};
use eve_bot::travel::{self, Map, SECURITY_FLOOR, Trips};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::industry;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

// Invented ids and numbers only.
const BUILDER: i64 = 90_000_301;
const CAROL: i64 = 90_000_302;
const CORP: i64 = 1_999_701;
const B: i64 = 30_990_003;
const STATION_B: i64 = 60_900_001;
const GATE_A: i64 = 50_900_001;
const GATE_B: i64 = 50_900_002;
const GATE_TYPE: i64 = 990_400_001;
const PRODUCT: i64 = 990_400_002;
const BLUEPRINT_GROUP: i64 = 99_105;
const PRODUCT_GROUP: i64 = 99_083;
const BLUEPRINT_REF: f64 = 100_000.0;
const PRODUCT_REF: f64 = 100.0;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 1_000_000.0;
/// Both systems: a security every floor passes.
const SECURE: f64 = 1.0;

fn blueprint() -> i64 {
    BLUEPRINT_LADDER[0]
}

fn home() -> Character {
    Character::default()
}

fn system_a() -> i64 {
    home()
        .solar_system_id
        .expect("the default pilot is in a system")
}

fn station_a() -> i64 {
    home().station_id.expect("the default pilot is docked")
}

/// Ten columns, as `tools/celestials.py` writes them: each system's
/// station at its origin, and one gate pair a thousand kilometres out,
/// beyond the client's warp floor.
fn catalogue() -> eve_server::celestial::Catalogue {
    let (a, home_station) = (system_a(), station_a());
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         #system\t{B}\t{constellation}\t{region}\n\
         station\t{home_station}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{B}\n\
         station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{B}\t{GATE_A}\t{a}\n"
    ))
}

/// `Content::fixture()` plus the blueprint, its product and recipe, and
/// the corporation that sells it at the far station only.
fn content() -> Content {
    let bp = blueprint();
    let types = format!(
        "{}\
         type\t{PRODUCT}\t{PRODUCT_GROUP}\t8\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Product\n\
         type\t{bp}\t{BLUEPRINT_GROUP}\t9\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Blueprint\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PRODUCT}\t{PRODUCT_REF}\tbase\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t\t1"),
        format!("{bp}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let recipes = industry::Table::parse(
        &[
            format!("{bp}\t1\tactivity\t0\t0\t\t1\t10"),
            format!("{bp}\t1\tproduct\t{PRODUCT}\t1\t\t1\t10"),
        ],
        &prices,
    )
    .expect("the recipe parses");
    let exchange = exchange::Table::new(
        vec![exchange::Station {
            station_id: STATION_B,
            system_id: B,
            region_id: home().region_id,
            owner_id: CORP,
        }],
        &[(CORP, bp, 1.0)],
        [(
            bp,
            exchange::TypeRow {
                group_id: BLUEPRINT_GROUP,
                category_id: exchange::CATEGORY_BLUEPRINT,
                base_price: BLUEPRINT_REF,
                meta_group_id: None,
                tech_level: None,
            },
        )]
        .into_iter()
        .collect(),
        &[],
    )
    .expect("the exchange builds");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("industry", recipes);
    c.insert("econ_exchange", exchange);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: BUILDER,
            name: "Builder1".to_owned(),
            ship_id: 100_000_301,
            balance: WALLET,
            ..home()
        },
        // Seeded at the far station, so this world knows it.
        Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_302,
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

/// The bot's map: the same catalogue, both systems secure.
fn map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(system_a(), SECURE)
        .with_security(B, SECURE)
}

/// **The lane's harness test**: no ask at home, so the builder's climb
/// finds the blueprint one jump away, flies there in its frigate (undock,
/// warp to the gate, jump, warp, dock), buys it, and flies home with it in
/// the hold: docked at home, the blueprint in the home hangar, two jumps.
#[test]
fn a_builder_fetches_a_blueprint_one_jump_away_and_comes_home() {
    let addr = start();
    let mut b = login(addr, "Builder1");
    // Fixture: a hull with a hold, as a player would have.
    gm(&mut b, &format!("/createitem {FRIGATE}"));
    let hull = b
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    b.board(hull.item_id).expect("boards the frigate");

    let book = b.market_orders(blueprint()).expect("the book reads");
    assert!(
        book.asks.iter().all(|a| a.station_id != station_a()),
        "no blueprint ask at home: {book:?}"
    );
    assert!(
        book.asks.iter().any(|a| a.station_id == STATION_B),
        "{book:?}"
    );

    let map = map();
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };
    let content = content();
    let cycle = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");

    assert_eq!(cycle.blueprint, Some(blueprint()), "{cycle:?}");
    assert!(
        cycle
            .notes
            .iter()
            .any(|n| n.starts_with(&format!("trip to station {STATION_B} (2 jumps)"))),
        "one jump out, one back: {cycle:?}"
    );
    assert_eq!(
        b.session().station_id(),
        Some(station_a()),
        "docked at home"
    );
    let held = b.blueprints().expect("the blueprints list");
    assert!(
        held.iter()
            .any(|r| r.type_id == blueprint() && r.location_id == station_a()),
        "the blueprint is in the home hangar: {held:?}"
    );
    assert!(
        b.cargo().expect("the hold lists").is_empty(),
        "the hold is unloaded"
    );
}

/// A route the floor forbids is no trip: with the far system under the
/// floor, the seller is out of reach and the builder stays docked at home
/// with nothing bought.
#[test]
fn no_trip_through_a_system_under_the_floor() {
    let addr = start();
    let mut b = login(addr, "Builder1");
    let map = Map::from_catalogue(&catalogue(), None).with_security(system_a(), SECURE);
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };
    let found = travel::find(&mut b, trips, blueprint(), 1, None).expect("the book reads");
    assert_eq!(found, None, "B's security is unknown, so B is out of reach");
    let content = content();
    let cycle = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");
    assert_eq!(cycle.blueprint, None, "{cycle:?}");
    assert_eq!(b.session().station_id(), Some(station_a()));
}

// ---- BOT-LAW: a low-status hunter routes around the police net ------------------
//
// Five invented systems. Home `A` and the far station's system `D` are
// low-sec; `P` is high-sec, where faction police attack a pilot of the
// status below; `Q` and `R` are low-sec. The short way home-to-far is
// `A-P-D` (two jumps); the way round the police is `A-Q-R-D` (three).

const P: i64 = 30_990_004;
const Q: i64 = 30_990_005;
const R: i64 = 30_990_006;
const D: i64 = 30_990_007;
const STATION_D: i64 = 60_900_002;
const HUNTER: i64 = 90_000_303;
/// A status the police attack from 0.8 up, checked against the band table
/// in the test (the table is the server's, not a number written here).
const OUTLAW: f64 = -3.0;
/// High-sec inside that net; and low-sec outside it.
const HIGH: f64 = 0.9;
const LOW: f64 = 0.3;

/// Each gate pair is `(gate here, system here, gate there, system there)`.
/// Gate ids are `50_910_000 + n`.
fn gate_pairs() -> Vec<(i64, i64, i64, i64)> {
    let a = system_a();
    let g = |n: i64| 50_910_000 + n;
    vec![
        (g(1), a, g(2), P),
        (g(3), P, g(4), D),
        (g(5), a, g(6), Q),
        (g(7), Q, g(8), R),
        (g(9), R, g(10), D),
    ]
}

/// The gate ids that lead into `P`.
fn gates_into_p() -> Vec<i64> {
    let mut into = Vec::new();
    for (g1, s1, g2, s2) in gate_pairs() {
        if s2 == P {
            into.push(g1);
        }
        if s1 == P {
            into.push(g2);
        }
    }
    into
}

fn net_catalogue() -> eve_server::celestial::Catalogue {
    let (a, home_station) = (system_a(), station_a());
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    let mut text = String::new();
    for s in [a, P, Q, R, D] {
        text += &format!("#system\t{s}\t{constellation}\t{region}\n");
    }
    text += &format!("station\t{home_station}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n");
    text += &format!("station\t{STATION_D}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{D}\t0\t0\n");
    // Every gate sits a distinct million kilometres out on the x axis, so
    // two gates of one system are never close.
    for (k, (g1, s1, g2, s2)) in gate_pairs().into_iter().enumerate() {
        let k = i64::try_from(k).unwrap_or(0) + 1;
        let x1 = k * 1_000_000;
        let x2 = -k * 1_000_000;
        text += &format!("gate\t{g1}\t{GATE_TYPE}\t2473.0\t{x1}.0\t0.0\t0.0\t{s1}\t{g2}\t{s2}\n");
        text += &format!("gate\t{g2}\t{GATE_TYPE}\t2473.0\t{x2}.0\t0.0\t0.0\t{s2}\t{g1}\t{s1}\n");
    }
    eve_server::celestial::Catalogue::parse(&text)
}

fn net_map() -> Map {
    Map::from_catalogue(&net_catalogue(), None)
        .with_security(system_a(), LOW)
        .with_security(P, HIGH)
        .with_security(Q, LOW)
        .with_security(R, LOW)
        .with_security(D, LOW)
}

fn start_net() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: HUNTER,
            name: "Hunter1".to_owned(),
            ship_id: 100_000_303,
            balance: WALLET,
            ..home()
        },
        // Seeded at the far station, so this world knows it.
        Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_302,
            station_id: Some(STATION_D),
            solar_system_id: Some(D),
            ..home()
        },
    ];
    c.sandbox.celestials = net_catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn jumped_gates(trip: &travel::Trip) -> Vec<i64> {
    trip.moves
        .iter()
        .filter_map(|m| match m {
            travel::Move::Jump { gate, .. } => Some(*gate),
            _ => None,
        })
        .collect()
}

fn boarded_hunter(addr: SocketAddr) -> eve_bot::Conn {
    let mut b = login(addr, "Hunter1");
    gm(&mut b, &format!("/createitem {FRIGATE}"));
    let hull = b
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    b.board(hull.item_id).expect("boards the frigate");
    b
}

/// **BOT-LAW over a socket**: a hunter whose status the police attack at
/// high-sec flies from home to the far station the long way round, three
/// jumps through low-sec, and does not enter the high-sec system on the
/// way there or on the way back. The same trip at no status takes the
/// short way through it, so the detour is the status's doing.
#[test]
fn a_low_status_hunter_flies_round_the_police_net() {
    assert!(
        eve_server::sec_status::band_attacked_in(OUTLAW).is_some_and(|min| min <= HIGH),
        "the test status is policed in P"
    );
    let map = net_map();
    assert!(map.in_police_net(P, OUTLAW));
    assert!(!map.in_police_net(Q, OUTLAW) && !map.in_police_net(D, OUTLAW));
    assert_eq!(
        map.route(system_a(), D, travel::ANY_SECURITY),
        Some(vec![system_a(), P, D]),
        "without the net, P is on the short way"
    );
    let trips = Trips {
        map: &map,
        floor: travel::ANY_SECURITY,
    };
    let into_p = gates_into_p();
    assert_eq!(into_p.len(), 2);

    // Control: a pilot nobody polices takes the short way through P.
    let mut clean = boarded_hunter(start_net());
    let short = travel::go(&mut clean, trips, STATION_D).expect("the clean trip flies");
    assert_eq!(short.jumps, 2, "{short:?}");
    assert!(
        jumped_gates(&short).iter().any(|g| into_p.contains(g)),
        "the control enters P: {short:?}"
    );

    // The hunter: the status is set on the server (GM command) and the bot
    // knows nothing of it; the trip reads it itself. Three jumps, none into
    // P, there and back.
    let mut b = boarded_hunter(start_net());
    gm(&mut b, &format!("/secstatus {OUTLAW}"));
    assert_eq!(b.police_status(), None, "the bot has not been told");
    let out = travel::go(&mut b, trips, STATION_D).expect("the hunter's trip flies");
    assert_eq!(out.jumps, 3, "round the net: {out:?}");
    assert!(
        jumped_gates(&out).iter().all(|g| !into_p.contains(g)),
        "no gate into P on the way out: {out:?}"
    );
    assert_eq!(b.session().station_id(), Some(STATION_D), "docked at D");
    let back = travel::go(&mut b, trips, station_a()).expect("the hunter flies home");
    assert_eq!(back.jumps, 3, "round the net, back: {back:?}");
    assert!(
        jumped_gates(&back).iter().all(|g| !into_p.contains(g)),
        "no gate into P on the way back: {back:?}"
    );
    assert_eq!(b.session().station_id(), Some(station_a()), "docked home");
}
