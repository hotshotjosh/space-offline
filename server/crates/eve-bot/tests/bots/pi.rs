//! **The PI farmer** (SOC-B6) over a socket: the first visit sells the
//! hangar's commodities into a bid and reads the system's planets off the
//! overview; the second buys the planet's command centre from a seller's
//! ask, loads it, undocks and founds the templated colony; the third only
//! tends it. Every visit ends docked at home.
//!
//! **Synthetic furniture only** (the no-client-data rule): an invented
//! planet type, planet, pin types, schematic and commodities. The one
//! client id is the link type, [`pi::LINK_TYPE`] (2280 [V]): the server
//! builds every link as it, so the table must carry its row.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::pi;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

// Invented ids and numbers only.
const FARMER: i64 = 90_000_651;
const SELLER: i64 = 90_000_652;
const PT: i64 = 990_660_001;
const CC: i64 = 990_660_011;
const ECU: i64 = 990_660_012;
const FACTORY: i64 = 990_660_015;
const RES: i64 = 990_660_021;
const PROD: i64 = 990_660_022;
const SCHEMATIC: i64 = 99_066;
const PLANET: i64 = 40_966_001;
const PLANET_RADIUS: f64 = 3_000_000.0;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 10_000_000.0;
const CC_PRICE: f64 = 1_000.0;
const GOODS: i64 = 30;
const BID: f64 = 50.0;

fn home() -> Character {
    Character::default()
}

fn system() -> i64 {
    home()
        .solar_system_id
        .expect("the default pilot is in a system")
}

fn station() -> i64 {
    home().station_id.expect("the default pilot is docked")
}

fn catalogue() -> eve_server::celestial::Catalogue {
    let (a, st) = (system(), station());
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         station\t{st}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         planet\t{PLANET}\t{PT}\t{PLANET_RADIUS}\t50000000000.0\t0.0\t0.0\t{a}\t0\t0\n"
    ))
}

/// The PI table: a command centre the planet allows (10 m3, so a frigate
/// carries it), an extraction unit, a basic factory, the link, one P0 ->
/// P1 schematic, the planet type's one resource, and the item rows.
fn statics() -> eve_server::planet::Statics {
    let link = pi::LINK_TYPE;
    eve_server::planet::Statics::parse(&format!(
        "pintype\t{CC}\t1027\t500.0\t0.0\t0\t48=1675.0;11=6000.0;1632={PT}.0\n\
         pintype\t{ECU}\t1063\t0.0\t45000.0\t0\t49=400.0;15=2600.0;1690=110.0;1691=550.0;1689=0.3;1632={PT}.0\n\
         pintype\t{FACTORY}\t1028\t0.0\t75000.0\t0\t49=200.0;15=800.0;1632={PT}.0\n\
         pintype\t{link}\t1036\t0.0\t0.0\t0\t49=15.0;15=10.0;1631=1250.0;1633=0.15;1634=0.2;1635=1.4;1636=1.2\n\
         schematic\t{SCHEMATIC}\t1800\t{FACTORY}\t{RES}=3000\t{PROD}=20\n\
         resource\t{PT}\t{RES}=1.0\n\
         itemtype\t{CC}\t1027\t41\t10.0\t500.0\tFixture PI Centre\n\
         itemtype\t{RES}\t1032\t42\t0.005\t0.0\tFixture PI Resource\n\
         itemtype\t{PROD}\t1042\t43\t0.19\t0.0\tFixture PI Product\n"
    ))
    .expect("the invented rows parse")
}

fn content() -> Content {
    let mut c = Content::fixture();
    c.insert(
        eve_server::planet::STATICS.name,
        eve_server::planet::Installed::build(statics()),
    );
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: FARMER,
            name: "Farmer1".to_owned(),
            ship_id: 100_000_651,
            balance: WALLET,
            ..home()
        },
        Character {
            character_id: SELLER,
            name: "PiSeller".to_owned(),
            ship_id: 100_000_652,
            balance: WALLET,
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn kv<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    let PyValue::Dict(entries) = state.as_ref() else {
        return None;
    };
    entries
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()))
        .map(|(v, _)| v)
}

/// **The lane's harness test** (module header).
#[test]
fn a_farmer_surveys_founds_a_templated_colony_then_tends_it() {
    let addr = start();

    // A human stand-in: one command centre listed, a bid for the P1.
    let mut s = login(addr, "PiSeller");
    gm(&mut s, &format!("/createitem {CC} 1"));
    let centre = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == CC)
        .expect("the seller's centre");
    s.sell(
        &[SellLine {
            station_id: station(),
            type_id: CC,
            item_id: centre.item_id,
            price: CC_PRICE,
            quantity: 1,
        }],
        pi_list_days(),
        0.0,
    )
    .expect("the seller lists the centre");
    s.buy(station(), PROD, BID, GOODS, pi_list_days(), 0.0)
        .expect("the seller bids for the P1");

    // The farmer's setup: a hull, and P1 left over from an earlier colony.
    let mut f = login(addr, "Farmer1");
    gm(&mut f, &format!("/createitem {FRIGATE}"));
    gm(&mut f, &format!("/createitem {PROD} {GOODS}"));
    let hull = f
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    f.board(hull.item_id).expect("boards the frigate");
    let content = content();

    // 1. Sell into the bid, and look at the planets.
    let first = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a first visit");
    assert_eq!(first.surveyed, Some(1), "{}", first.summary());
    assert!(
        matches!(first.sales.as_slice(), [pi::Sale::Bid(l)] if l.quantity == GOODS),
        "{}",
        first.summary()
    );
    assert!(first.after > first.before, "{}", first.summary());
    assert_eq!(f.session().station_id(), Some(station()), "docked at home");

    // 2. Buy the centre, undock, found the colony, dock.
    let second = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a second visit");
    let (planet, level, heads) = second
        .founded
        .unwrap_or_else(|| panic!("{}", second.summary()));
    assert_eq!((planet, level), (PLANET, 0), "{}", second.summary());
    assert!(heads >= 1, "{}", second.summary());
    assert_eq!(f.session().station_id(), Some(station()), "docked at home");
    assert!(
        f.cargo()
            .expect("the hold lists")
            .iter()
            .all(|r| r.type_id != CC),
        "the centre left the hold"
    );

    // The colony, as the planet window reads it.
    let info = f
        .moniker_call(
            "planetMgr",
            PyValue::Int(PLANET),
            "GetPlanetInfo",
            vec![],
            vec![],
        )
        .expect("the planet reads")
        .value
        .expect("an answer");
    let view = pi::read_colony(&info).expect("a colony");
    let types: Vec<i64> = view.pins.iter().map(|p| p.type_id).collect();
    assert_eq!(types.len(), 3, "{types:?}");
    for t in [CC, ECU, FACTORY] {
        assert!(types.contains(&t), "{types:?}");
    }
    let ecu = view.pins.iter().find(|p| p.type_id == ECU).unwrap();
    assert_eq!(ecu.program_type, Some(RES));
    assert_eq!(ecu.heads, heads);
    let factory = view.pins.iter().find(|p| p.type_id == FACTORY).unwrap();
    assert_eq!(factory.schematic_id, Some(SCHEMATIC));
    match kv(&info, "routes") {
        Some(PyValue::List(routes)) => assert_eq!(routes.len(), 2),
        other => panic!("routes: {other:?}"),
    }

    // 3. Tend only: no skill for a second planet, nothing expired yet.
    let third = pi::run(&mut f, Some(&content), "Farmer1", 0).expect("a third visit");
    assert_eq!(third.colonies, 1, "{}", third.summary());
    assert_eq!(third.restarted, 0, "{}", third.summary());
    assert!(third.founded.is_none() && third.surveyed.is_none());
    assert_eq!(f.session().station_id(), Some(station()));
}

fn pi_list_days() -> i64 {
    eve_bot::escort::LIST_DAYS
}

// ---- #165: no command centre at home, one for sale a jump away ----------------
//
// Each test has its own world and home system (the planets a bot has seen are
// remembered per system, per process). Home sells no centre; a station one
// gate away sells one as an NPC ask (the exchange table, the command-centre category).

mod away {
    use super::*;
    use eve_bot::travel::{self, Map, SECURITY_FLOOR, Trips};
    use eve_server::econ_exchange as exchange;

    const CORP: i64 = 1_999_711;
    const CAROL: i64 = 90_000_653;
    const GATE_TYPE: i64 = 990_400_001;
    const CC_GROUP: i64 = eve_server::planet::GROUP_COMMAND_PINS;
    const CC_REF: f64 = 5_000.0;
    const MARKET: i64 = 31_700_100;
    const MARKET_STATION: i64 = 61_700_100;

    struct World {
        i: i64,
        /// The market station sells a centre as an NPC ask.
        asks: bool,
    }

    impl World {
        fn system(&self) -> i64 {
            31_700_000 + self.i
        }
        fn station(&self) -> i64 {
            61_700_000 + self.i
        }
        fn planet(&self) -> i64 {
            41_700_000 + self.i
        }
        fn catalogue(&self) -> eve_server::celestial::Catalogue {
            let (h, sh, pl) = (self.system(), self.station(), self.planet());
            let (gh, gm) = (51_700_000 + self.i, 51_700_100 + self.i);
            let (constellation, region) = (home().constellation_id, home().region_id);
            let station_type = home().station_type_id;
            eve_server::celestial::Catalogue::parse(&format!(
                "#system\t{h}\t{constellation}\t{region}\n\
                 #system\t{MARKET}\t{constellation}\t{region}\n\
                 station\t{sh}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{h}\t0\t0\n\
                 planet\t{pl}\t{PT}\t{PLANET_RADIUS}\t50000000000.0\t0.0\t0.0\t{h}\t0\t0\n\
                 gate\t{gh}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{h}\t{gm}\t{MARKET}\n\
                 station\t{MARKET_STATION}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{MARKET}\t0\t0\n\
                 gate\t{gm}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{MARKET}\t{gh}\t{h}\n"
            ))
        }
        fn content(&self) -> Content {
            let prices = eve_server::econ_prices::Table::parse(&[format!(
                "{CC}\t{CC_REF}\tbase\t{CC_REF}\t{}\t{CC_GROUP}\t\t1",
                exchange::CATEGORY_COMMAND_CENTER
            )])
            .expect("the prices parse");
            let table = exchange::Table::new(
                vec![exchange::Station {
                    station_id: MARKET_STATION,
                    system_id: MARKET,
                    region_id: home().region_id,
                    owner_id: CORP,
                }],
                if self.asks { &[(CORP, CC, 1.0)] } else { &[] },
                [(
                    CC,
                    exchange::TypeRow {
                        group_id: CC_GROUP,
                        category_id: exchange::CATEGORY_COMMAND_CENTER,
                        base_price: CC_REF,
                        meta_group_id: None,
                        tech_level: None,
                    },
                )]
                .into_iter()
                .collect(),
                &[],
            )
            .expect("the exchange builds");
            let mut c = super::content();
            c.insert("econ_prices", prices);
            c.insert("econ_exchange", table);
            c
        }
        fn start(&self, farmer: i64) -> SocketAddr {
            let mut c = config();
            c.sandbox.characters = vec![
                Character {
                    character_id: farmer,
                    name: format!("AwayFarmer{}", self.i),
                    ship_id: 100_000_700 + self.i,
                    balance: WALLET,
                    station_id: Some(self.station()),
                    solar_system_id: Some(self.system()),
                    ..home()
                },
                // Seeded at the market station, so this world knows it.
                Character {
                    character_id: CAROL + self.i,
                    name: format!("Carol{}", self.i),
                    ship_id: 100_000_710 + self.i,
                    station_id: Some(MARKET_STATION),
                    solar_system_id: Some(MARKET),
                    ..home()
                },
            ];
            c.sandbox.celestials = self.catalogue();
            c.content = Some(Arc::new(self.content()));
            serve(c)
        }
        fn map(&self) -> Map {
            Map::from_catalogue(&self.catalogue(), None)
                .with_security(self.system(), 1.0)
                .with_security(MARKET, 1.0)
        }
    }

    fn market_volume(c: &mut eve_bot::Conn) -> i64 {
        c.market_orders(CC)
            .expect("the book reads")
            .asks
            .iter()
            .filter(|a| a.station_id == MARKET_STATION)
            .map(|a| a.vol_remaining)
            .sum()
    }

    fn give_frigate(f: &mut eve_bot::Conn, board: bool) {
        gm(f, &format!("/createitem {FRIGATE}"));
        if board {
            let hull = f
                .hangar_items()
                .expect("the hangar lists")
                .into_iter()
                .find(|r| r.type_id == FRIGATE)
                .expect("the frigate");
            f.board(hull.item_id).expect("boards the frigate");
        }
    }

    /// A stale `looked_day` and every planet type in `no_centre`: the farmer
    /// has failed before. A new game day must forget that **before** a
    /// planet is picked (item 2), or nothing below is reached.
    fn failed_before(name: &str) {
        pi::set_note(
            name,
            pi::Note {
                no_centre: [PT].into_iter().collect(),
                looked_day: Some(0),
                ..pi::Note::default()
            },
        );
    }

    /// **End to end** (soak C's path): home has no ask, a capsule pilot with
    /// a frigate in the hangar boards it before the trip, flies a jump, buys
    /// the NPC ask, flies home, loads the centre and founds the colony.
    #[test]
    fn a_farmer_without_a_home_centre_fetches_one_a_jump_away_and_founds() {
        let w = World { i: 1, asks: true };
        let addr = w.start(FARMER + 100);
        let name = "AwayFarmer1";
        let mut f = login(addr, name);
        give_frigate(&mut f, false);
        failed_before(name);
        let map = w.map();
        let trips = Some(Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        });
        let content = w.content();
        let book = f.market_orders(CC).expect("the book reads");
        assert!(
            book.asks.iter().all(|a| a.station_id != w.station()),
            "no ask at home: {book:?}"
        );
        let before = market_volume(&mut f);
        assert!(before > 0, "the NPC asks at the market: {book:?}");

        let t0 = eve_server::pyutil::filetime_now();
        let day = eve_bot::driver::DAY_SECS;
        let first = pi::run_with(&mut f, Some(&content), name, 0, trips, day).expect("a visit");
        assert_eq!(first.surveyed, Some(1), "{}", first.summary());
        let second = pi::run_with(&mut f, Some(&content), name, 0, trips, day).expect("a visit");
        let (planet, level, heads) = second
            .founded
            .unwrap_or_else(|| panic!("{}", second.summary()));
        assert_eq!((planet, level), (w.planet(), 0), "{}", second.summary());
        assert!(heads >= 1);
        assert_eq!(f.session().station_id(), Some(w.station()), "home");
        let after = market_volume(&mut f);
        let t1 = eve_server::pyutil::filetime_now();
        // The NPC ask refills on wall-clock-aligned TRICKLE_PERIOD boundaries
        // (econ_exchange::periods); this world runs at scale 1. A boundary
        // crossed between the take and this read refills the one unit.
        let period = eve_server::econ_exchange::TRICKLE_PERIOD;
        let crossed = t1.div_euclid(period) != t0.div_euclid(period);
        assert!(
            after == before - 1 || (crossed && after == before),
            "one centre bought: {before} -> {after} (a refill boundary crossed: {crossed})"
        );
        assert!(
            f.cargo()
                .expect("the hold lists")
                .iter()
                .all(|r| r.type_id != CC)
        );
        let info = f
            .moniker_call(
                "planetMgr",
                PyValue::Int(w.planet()),
                "GetPlanetInfo",
                vec![],
                vec![],
            )
            .expect("the planet reads")
            .value
            .expect("an answer");
        assert_eq!(pi::read_colony(&info).expect("a colony").pins.len(), 3);
    }

    /// A hold too small is found out **before** the trip: a capsule pilot
    /// with no hull buys nothing, stays docked at home, and asks for a hull.
    #[test]
    fn a_hold_too_small_buys_nothing_and_asks_for_a_hull() {
        let w = World { i: 2, asks: true };
        let addr = w.start(FARMER + 200);
        let name = "AwayFarmer2";
        let mut f = login(addr, name);
        failed_before(name);
        let map = w.map();
        let trips = Some(Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        });
        let content = w.content();
        let before = market_volume(&mut f);
        let day = eve_bot::driver::DAY_SECS;
        pi::run_with(&mut f, Some(&content), name, 0, trips, day).expect("a visit");
        let second = pi::run_with(&mut f, Some(&content), name, 0, trips, day).expect("a visit");
        assert!(second.founded.is_none(), "{}", second.summary());
        assert!(
            second
                .refusals
                .iter()
                .any(|r| r.contains("no command centre")),
            "{}",
            second.summary()
        );
        assert!(pi::note_of(name).lacks_hull, "a hull is asked for");
        assert_eq!(market_volume(&mut f), before, "nothing was bought");
        assert_eq!(f.session().station_id(), Some(w.station()), "home");
    }

    /// A centre of ours already in the seller's hangar (an earlier trip
    /// could not carry it) is collected, not bought again.
    #[test]
    fn a_centre_already_in_the_sellers_hangar_is_collected_not_bought_again() {
        let w = World { i: 3, asks: true };
        let addr = w.start(FARMER + 300);
        let name = "AwayFarmer3";
        let mut f = login(addr, name);
        give_frigate(&mut f, true);
        let map = w.map();
        let trips = Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        };
        travel::go(&mut f, trips, MARKET_STATION).expect("flies to the market");
        gm(&mut f, &format!("/createitem {CC} 1"));
        travel::go(&mut f, trips, w.station()).expect("flies home");
        failed_before(name);
        let content = w.content();
        let before = market_volume(&mut f);
        assert!(before > 0);
        let day = eve_bot::driver::DAY_SECS;
        pi::run_with(&mut f, Some(&content), name, 0, Some(trips), day).expect("a visit");
        let second =
            pi::run_with(&mut f, Some(&content), name, 0, Some(trips), day).expect("a visit");
        assert!(second.founded.is_some(), "{}", second.summary());
        assert_eq!(market_volume(&mut f), before, "no second centre bought");
    }

    /// **#187**: a centre of ours at another station with no ask anywhere is
    /// fetched home all the same, and nothing is bought.
    #[test]
    fn an_owned_centre_at_another_station_is_fetched_with_no_ask_anywhere() {
        let w = World { i: 4, asks: false };
        let addr = w.start(FARMER + 400);
        let name = "AwayFarmer4";
        let mut f = login(addr, name);
        give_frigate(&mut f, true);
        let map = w.map();
        let trips = Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        };
        travel::go(&mut f, trips, MARKET_STATION).expect("flies to the market");
        gm(&mut f, &format!("/createitem {CC} 1"));
        travel::go(&mut f, trips, w.station()).expect("flies home");
        failed_before(name);
        let content = w.content();
        let book = f.market_orders(CC).expect("the book reads");
        assert!(book.asks.is_empty(), "no ask anywhere: {book:?}");
        let day = eve_bot::driver::DAY_SECS;
        pi::run_with(&mut f, Some(&content), name, 0, Some(trips), day).expect("a visit");
        let second =
            pi::run_with(&mut f, Some(&content), name, 0, Some(trips), day).expect("a visit");
        assert!(second.founded.is_some(), "{}", second.summary());
        assert!(
            f.market_orders(CC).expect("the book reads").asks.is_empty(),
            "nothing listed or bought"
        );
        assert_eq!(f.session().station_id(), Some(w.station()), "home");
    }
}
