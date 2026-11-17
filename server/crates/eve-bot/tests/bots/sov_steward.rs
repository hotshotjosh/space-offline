//! SOVX-STEWARD-1 over a real socket: a bot pilot of an alliance, through
//! [`eve_bot::sov_bot::turn`] alone (no slash line after the setup, no GM
//! tool), reads the planets' definitions and its corporation's skyhooks over
//! the gateway, carries the skyhook in its hangar into the hold, launches it
//! over a yield planet with the client's `ship.Drop`, and, once the skyhook
//! has built and made reagents, takes them with the skyhook window's own
//! component call and puts them into the hub's reagent bay.
//!
//! The server runs at `--time-scale 60`; the gateway is started here on a
//! loopback port of its own (not through `Config`, whose gateway field the
//! shared binary's source scan keeps out of every test) and the bot is told
//! where it is. **Invented furniture only**: the system is made null-sec, the
//! star, the planets' positions, the skyhook type and the reagent row are
//! invented; the client's group ids appear by name.

use super::common::{SYSTEM, config, gm, login, pilot};
use eve_bot::Conn;
use eve_bot::brain::{Memory, Policy};
use eve_bot::sov_bot::extras::gw::set_gateway_for;
use eve_bot::sov_bot::{self, Ctx, SovHall};
use eve_bot::travel::Map;
use eve_marshal::PyValue;
use eve_server::game_clock::GameClock;
use eve_server::skyhook::{self, GROUP_SKYHOOK};
use eve_server::sov_hub;
use eve_server::sov_resources::{
    FIXTURE_REAGENT_TYPE, GROUP_COLONY_REAGENTS, fixture_planets, fixture_stars,
};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_921;
const SKYHOOK_TYPE: i64 = 990_921_001;
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: it flies, and its hold takes the skyhook.
const HULL: i64 = 990_000_001;
/// The reagent planet has the lowest id, so the first launch lands on it.
const P_REAGENT: i64 = 40_921_001;
const P_POWER: i64 = 40_921_011;
const P_LAND: i64 = 40_921_012;
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

fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t1.0\t0.0\t1000.0\t{HUB_RADIUS}\t0\t\t\tInvented Hub Row\n\
         type\t{}\t{}\t4\t1\t40\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Fuel Block\n\
         type\t{SKYHOOK_TYPE}\t{GROUP_SKYHOOK}\t46\t1\t1\t20.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Skyhook\n\
         type\t{FIXTURE_REAGENT_TYPE}\t{GROUP_COLONY_REAGENTS}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        990_921_002,
        sov_hub::GROUP_FUEL_BLOCK,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    c.insert("sov planets", fixture_planets());
    c.insert("sov stars", fixture_stars());
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

/// A star placed so the hub lands at `(HUB_X, 0, 0)`, and three planets of
/// the fixture's invented types 9001 (power), 9002 (workforce), 9003
/// (reagent).
fn celestial_rows() -> String {
    let radius = 3.3e8;
    let at = sov_hub::hub_position([0.0; 3], radius);
    format!(
        "star\t40921000\t990921010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n\
         planet\t{P_REAGENT}\t9003\t6.0e6\t1.0e10\t2.0e8\t3.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_POWER}\t9001\t6.0e6\t-4.0e9\t1.0e8\t9.0e9\t{SYSTEM}\t0\t0\n\
         planet\t{P_LAND}\t9002\t6.0e6\t6.0e9\t-3.0e8\t-8.0e9\t{SYSTEM}\t0\t0\n",
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
/// pilot, a hub the alliance holds, a skyhook and fuel in her hangar, and
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
        vec![uni("Steward Invented"), uni("STWI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(&mut alice, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Steward Pact"), uni("STWP"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    // A ship, the hub and the skyhook in the hangar.
    let station = alice.session().station_id().expect("docked at home");
    gm(&mut alice, &format!("/createitem {HULL}"));
    let hull = hangar_item(&mut alice, HULL);
    alice.board(hull).expect("boards");
    gm(
        &mut alice,
        &format!("/createitem {}", eve_server::sov::TYPE_INFRASTRUCTURE_HUB),
    );
    gm(&mut alice, "/createitem 990921002 1000");
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
    // Back home, docked, with a skyhook in the hangar (the builders' kind
    // of item: bought, not made by a GM line during the bot's turns).
    let landed =
        super::common::warp_and_arrive(&mut alice, station, 20_000.0, Duration::from_secs(60));
    assert!(landed, "back at the station");
    alice.dock(station).expect("docks");
    gm(&mut alice, &format!("/createitem {SKYHOOK_TYPE}"));
    assert!(
        alice
            .hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.type_id == SKYHOOK_TYPE),
        "a skyhook in the hangar"
    );

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

    fn corp_skyhooks(&self) -> Vec<skyhook::Skyhook> {
        let w = self.world.read().expect("the world reads");
        skyhook::all(w.deltas())
            .into_iter()
            .filter(|s| s.corp == self.corp)
            .collect()
    }

    /// Turns until the corporation has a skyhook; the notes so far.
    fn until_launched(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        for _ in 0..8 {
            notes.push(self.turn());
            if !self.corp_skyhooks().is_empty() {
                break;
            }
        }
        notes
    }
}

#[test]
fn a_steward_launches_a_skyhook_over_the_yield_planet() {
    let mut s = setup();
    let notes = s.until_launched();
    let hooks = s.corp_skyhooks();
    assert_eq!(
        hooks.len(),
        1,
        "one skyhook launched for the corp\nnotes: {notes:#?}"
    );
    assert_eq!(
        hooks[0].planet, P_REAGENT,
        "over the reagent planet: {notes:#?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("sov steward:")),
        "the steward's lines: {notes:#?}"
    );
    // The bot sent no slash line of its own: the source scan of the extras
    // (`the_extras_send_no_slash_line`) holds the code to it; here the pilot's
    // own notes carry none either.
    assert!(
        notes.iter().all(|n| !n.to_lowercase().contains("slashcmd")),
        "{notes:#?}"
    );
    let _ = s.hub;
}

#[test]
fn the_reagents_reach_the_hub_bay() {
    let mut s = setup();
    let _ = s.until_launched();
    let hooks = s.corp_skyhooks();
    assert_eq!(hooks.len(), 1, "a skyhook was launched");
    let id = hooks[0].id;
    // The build finishes, and five cycles' worth of reagents have gathered.
    {
        let mut w = s.world.write().expect("the world writes");
        let now = skyhook::now();
        skyhook::finish_build(&mut w, id, now).expect("the skyhook builds");
        let mut sk = skyhook::by_id(w.deltas(), id).expect("the row");
        // Credit the cycles outright, as `/skyhook age` does: moving
        // `last_cycle` back is undone by the claim-time clamp.
        sk.unsecured = 35;
        skyhook::put(w.deltas_mut(), &sk);
    }
    let hub = s.hub;
    let mut notes = Vec::new();
    let mut got = false;
    for _ in 0..10 {
        notes.push(s.turn());
        let w = s.world.read().expect("the world reads");
        if eve_server::sov_hub_fuel::stock(w.deltas(), hub, FIXTURE_REAGENT_TYPE).amount > 0 {
            got = true;
            break;
        }
    }
    assert!(got, "the reagents reached the bay\nnotes: {notes:#?}");
    let w = s.world.read().expect("the world reads");
    let sk = skyhook::by_id(w.deltas(), id).expect("the row");
    assert!(
        sk.unsecured + sk.secured < 35,
        "the skyhook's stock fell: {}/{}",
        sk.secured,
        sk.unsecured
    );
}
