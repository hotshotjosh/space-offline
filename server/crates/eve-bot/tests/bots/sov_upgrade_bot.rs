//! SOVX-STEWARD-2 over a real socket: a bot pilot of an alliance, through
//! [`eve_bot::sov_bot::turn`] alone (no slash line after the setup, no GM
//! tool), reads the hub's upgrade definitions, installed list, budget and
//! reagent bay over the gateway, and installs the upgrades the client's power
//! rule would bring online, the producer first, with the hub window's own
//! `ProcessConfigurationRequest`. An upgrade the budget cannot carry stays in
//! the hangar.
//!
//! The server runs at `--time-scale 60`; the gateway is started here on a
//! loopback port of its own and the bot is told where it is. **Invented
//! furniture only**: the system is made null-sec, the star, the upgrade table,
//! the upgrade and reagent types are invented; the client's group ids appear
//! by name.

use super::common::{SYSTEM, config, gm, login, pilot};
use eve_bot::Conn;
use eve_bot::brain::{Memory, Policy};
use eve_bot::sov_bot::extras::gw::set_gateway_for;
use eve_bot::sov_bot::{self, Ctx, SovHall};
use eve_bot::travel::Map;
use eve_marshal::PyValue;
use eve_server::game_clock::GameClock;
use eve_server::sov_hub;
use eve_server::sov_resources::{FIXTURE_REAGENT_TYPE, GROUP_COLONY_REAGENTS, fixture_stars};
use eve_server::sov_upgrade::{Def, Power, Row, UPGRADE_GROUPS, UpgradeTable};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_922;
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: it flies, and its hold takes an upgrade.
const HULL: i64 = 990_000_001;
const PRODUCER: i64 = 990_922_501;
const BIG: i64 = 990_922_502;
const BURNER: i64 = 990_922_503;
/// What the bay is seeded with: the steward's horizon of the burner's burn
/// (one unit an hour, named through the helper) and its startup to spare.
fn seeded() -> u64 {
    eve_bot::sov_bot::extras::steward::steward_days_units(1) + 10
}
const SCALE: u32 = 60;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn said(v: Option<PyValue>) -> String {
    match v {
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
        other => format!("{other:?}"),
    }
}

fn slash(c: &mut Conn, line: &str) -> String {
    let out = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    said(out.value())
}

/// Three invented upgrades: a producer that needs nothing, one the budget
/// cannot carry even with the producer, and one that burns the reagent.
fn upgrades() -> UpgradeTable {
    let row = |power, fuel, per_hour, startup, group: &str, power_out| Row {
        def: Def {
            power,
            workforce: 0,
            fuel,
            per_hour,
            startup,
            power_out,
            workforce_out: 0,
        },
        group: group.to_owned(),
        services: Default::default(),
        tier: None,
        effects: Vec::new(),
    };
    UpgradeTable::new(vec![
        (PRODUCER, row(0, None, 0, 0, "gp", 10)),
        (BIG, row(50, None, 0, 0, "gq", 0)),
        (BURNER, row(0, Some(FIXTURE_REAGENT_TYPE), 1, 2, "gr", 0)),
    ])
    .0
}

fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let g = UPGRADE_GROUPS[0];
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t1.0\t0.0\t1000.0\t{HUB_RADIUS}\t0\t\t\tInvented Hub Row\n\
         type\t{}\t{}\t4\t1\t40\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Fuel Block\n\
         type\t{PRODUCER}\t{g}\t46\t1\t1\t20.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Producer\n\
         type\t{BIG}\t{g}\t46\t1\t1\t20.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Big Upgrade\n\
         type\t{BURNER}\t{g}\t46\t1\t1\t20.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Burner\n\
         type\t{FIXTURE_REAGENT_TYPE}\t{GROUP_COLONY_REAGENTS}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        990_922_009,
        sov_hub::GROUP_FUEL_BLOCK,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    c.insert("sov stars", fixture_stars());
    c.insert("sov upgrades", upgrades());
    let u = eve_server::celestial::Universe::parse(
        &[format!(
            "{SYSTEM}\t10000001\t20000001\t\t\t-0.4\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented Null Home"
        )],
        &["10000001\t\t\t\t\tInvented Region".to_owned()],
        &["20000001\t10000001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the universe parses");
    c.insert("universe", u);
    c
}

/// A star placed so the hub lands at `(HUB_X, 0, 0)`; no planets, so the
/// steward has nothing to want and the upgrades arm gets the turn.
fn celestial_rows() -> String {
    let radius = 3.3e8;
    let at = sov_hub::hub_position([0.0; 3], radius);
    format!(
        "star\t40922000\t990922010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
        HUB_X - at[0],
        -at[1],
        -at[2]
    )
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

fn hangar_item(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .unwrap_or_else(|| panic!("a {type_id} in the hangar"))
        .item_id
}

/// What a test holds: the bot pilot's connection and memory, the world and
/// the alliance's hub.
struct Setup {
    alice: Conn,
    mem: Memory,
    hall: Arc<SovHall>,
    map: Map,
    world: Arc<RwLock<eve_server::service::Registry>>,
    hub: i64,
    corp: i64,
}

/// A server with a gateway of its own, an alliance whose CEO is the bot
/// pilot, a hub the alliance holds, upgrades in her hangar, and
/// the pilot docked at home. All of it is a person's setup, done before the
/// bot's first turn.
fn setup() -> Setup {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_921)];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&celestial_rows());
    config.content = Some(Arc::new(content()));
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    let server = eve_server::server::Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let gateway = eve_server::gateway::spawn(
        "127.0.0.1:0".parse().expect("literal"),
        eve_server::gateway::Ground::Server(Arc::clone(&world)),
    )
    .expect("the gateway binds");
    set_gateway_for(addr, gateway);

    let mut alice = login(addr, "Alice");
    gm(&mut alice, "/giveisk 2000000000");
    let a = corp_call(
        &mut alice,
        "AddCorporation",
        vec![uni("Upgrade Invented"), uni("UPGI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(&mut alice, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Upgrade Pact"), uni("UPGP"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    // A ship, the hub and the upgrades below.
    let station = alice.session().station_id().expect("docked at home");
    gm(&mut alice, &format!("/createitem {HULL}"));
    let hull = hangar_item(&mut alice, HULL);
    alice.board(hull).expect("boards");
    gm(
        &mut alice,
        &format!("/createitem {}", eve_server::sov::TYPE_INFRASTRUCTURE_HUB),
    );
    gm(&mut alice, "/createitem 990922009 1000");
    let hub_stack = hangar_item(&mut alice, eve_server::sov::TYPE_INFRASTRUCTURE_HUB);
    alice.load_cargo(hub_stack, None).expect("the hub loads");
    alice.undock().expect("undocks");
    alice.enter_space(SYSTEM).expect("a state");
    let params = alice.session().location_moniker().expect("in a system");
    let answer = alice
        .moniker_call(
            "ship",
            params,
            "Drop",
            vec![
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(hub_stack),
                    PyValue::Int(1),
                ])]),
                PyValue::None,
                PyValue::Bool(false),
            ],
            vec![],
        )
        .expect("Drop is answered");
    let hub = match answer.value.as_ref() {
        Some(PyValue::Dict(pairs)) => match pairs.first() {
            Some((PyValue::List(ids), _)) => match ids.first() {
                Some(PyValue::Int(id)) => *id,
                other => panic!("refused: {other:?}"),
            },
            other => panic!("keyed by the source item: {other:?}"),
        },
        other => panic!("a dict: {other:?}"),
    };
    let out = slash(&mut alice, &format!("/sov capture here {alliance}"));
    assert!(out.contains("captured"), "{out}");
    // Back home, docked. The `/createitem` lines below are the person's
    // stand-in for upgrades a builder made and the pilot bought: setup, done
    // before the bot's first turn, never by the bot.
    let landed =
        super::common::warp_and_arrive(&mut alice, station, 20_000.0, Duration::from_secs(60));
    assert!(landed, "back at the station");
    alice.dock(station).expect("docks");
    for ty in [PRODUCER, BIG, BURNER] {
        gm(&mut alice, &format!("/createitem {ty}"));
        assert!(
            alice
                .hangar_items()
                .expect("lists")
                .iter()
                .any(|r| r.type_id == ty),
            "upgrade {ty} in the hangar"
        );
    }
    // The bay holds the reagent the burner starts up on: seeded by the test,
    // as a person's deposit would be, not by the bot.
    {
        let mut w = world.write().expect("the world writes");
        eve_server::sov_hub_fuel::add(
            w.deltas_mut(),
            hub,
            FIXTURE_REAGENT_TYPE,
            seeded(),
            eve_server::skyhook::now(),
        );
    }

    let hall = Arc::new(SovHall::new());
    let mut policy = Policy::human();
    policy.role = &sov_bot::ROLE;
    policy.sov = Some(Arc::clone(&hall));
    let mem = Memory::new("Alice", policy);
    let map = Map::new()
        .with_security(SYSTEM, -0.4)
        .with_station(station, SYSTEM);
    Setup {
        alice,
        mem,
        hall,
        map,
        world,
        hub,
        corp,
    }
}

impl Setup {
    /// One sov turn (the extras pass is not held back by its own pause).
    fn turn(&mut self) -> String {
        self.mem.sov.extras.next = None;
        let content = content();
        let ctx = Ctx {
            map: Some(&self.map),
            content: Some(&content),
            hall: &self.hall,
            relation: None,
            society: None,
        };
        sov_bot::turn(&mut self.alice, &mut self.mem, &ctx).expect("a sov turn")
    }

    /// The hub's installed upgrades, in order, with their states.
    fn installed(&self) -> Vec<(i64, Power)> {
        let w = self.world.read().expect("the world reads");
        eve_server::sov_upgrade::installed(w.deltas(), self.hub)
            .into_iter()
            .map(|i| (i.type_id, i.state))
            .collect()
    }
}

#[test]
fn a_steward_installs_the_producer_then_the_burner_and_leaves_the_one_it_cannot_carry() {
    let mut s = setup();
    let mut notes = Vec::new();
    for _ in 0..12 {
        notes.push(s.turn());
        if s.installed().len() >= 2 {
            break;
        }
    }
    // One more pass: the budget cannot carry the big one, nothing else to do.
    notes.push(s.turn());
    let got = s.installed();
    assert_eq!(
        got,
        vec![(PRODUCER, Power::Online), (BURNER, Power::Online)],
        "the producer first, then the burner, both Online\nnotes: {notes:#?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains(&format!("sov steward: upgrade {PRODUCER} online"))),
        "the soak line: {notes:#?}"
    );
    // The installed items are gone from the hangar and the hold (consumed by
    // the save); the big upgrade is still in the hangar, not installed.
    let hangar = s.alice.hangar_items().expect("lists");
    let hold = s.alice.cargo().expect("lists");
    for ty in [PRODUCER, BURNER] {
        assert!(
            !hangar.iter().chain(&hold).any(|r| r.type_id == ty),
            "upgrade {ty} was consumed"
        );
    }
    assert!(
        s.alice
            .hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.type_id == BIG),
        "the big upgrade stays in the hangar"
    );
    assert!(
        notes.iter().all(|n| !n.to_lowercase().contains("slashcmd")),
        "{notes:#?}"
    );
    // The burner's startup came out of the bay.
    let w = s.world.read().expect("the world reads");
    let left = eve_server::sov_hub_fuel::stock(w.deltas(), s.hub, FIXTURE_REAGENT_TYPE).amount;
    assert!(left < seeded(), "the startup was paid from the bay: {left}");
    let _ = s.corp;
}
