//! SOVX-DEN over a real socket: a hauler pilot of an alliance with the den
//! skill, through [`eve_bot::sov_bot::turn`] alone (no slash line after the
//! setup, no GM tool), reads her dens, skyhooks and activities over the
//! gateway, carries the den in her hangar into the hold, launches it beside an
//! own workforce skyhook with the client's `ship.Drop`, empties it with the
//! den window's own component call and lists the infomorphs, and (with a gun
//! fitted) starts an activity the den offers. The site run itself is left to
//! the soak.
//!
//! The server runs at `--time-scale 60`; the gateway is started here on a
//! loopback port of its own (not through `Config`, whose gateway field the
//! shared binary's source scan keeps out of every test) and the bot is told
//! where it is. A second pilot id holds the hub's steward claim, so the
//! steward arm (which runs first) leaves the pilot to the den arm. **Invented
//! furniture only**: the system, the planets' positions, the den, the
//! infomorph and the dungeons are invented.

use super::common::{SYSTEM, config, gm, login, pilot};
use eve_bot::Conn;
use eve_bot::brain::{Memory, Policy};
use eve_bot::sov_bot::extras::gw::set_gateway_for;
use eve_bot::sov_bot::extras::steward;
use eve_bot::sov_bot::{self, Ctx, SovHall};
use eve_bot::travel::Map;
use eve_marshal::PyValue;
use eve_server::game_clock::GameClock;
use eve_server::merc_den::{self, FIXTURE_INFOMORPH, GROUP_MERCENARY_DEN, SKILL_DEN_MANAGEMENT};
use eve_server::skyhook::{self, GROUP_SKYHOOK};
use eve_server::sov_hub;
use eve_server::sov_resources::{
    FIXTURE_REAGENT_TYPE, GROUP_COLONY_REAGENTS, fixture_planets, fixture_stars,
};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_922;
/// The pilot that holds the steward claim.
const STEWARD: i64 = 90_000_923;
const SKYHOOK_TYPE: i64 = 990_922_001;
const DEN_TYPE: i64 = 990_922_003;
const GUN: i64 = 990_922_004;
const FACTION: i64 = 599_041;
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: it flies, and its hold takes the den.
const HULL: i64 = 990_000_001;
const P_REAGENT: i64 = 40_922_001;
const P_POWER: i64 = 40_922_011;
const P_LAND: i64 = 40_922_012;
const SCALE: u32 = 60;
/// An invented skill group.
const SKILL_GROUP: i64 = 990_922_900;

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

fn universe() -> eve_server::celestial::Universe {
    eve_server::celestial::Universe::parse(
        &[format!(
            "{SYSTEM}\t10000001\t20000001\t\t\t-0.4\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented Null Home"
        )],
        &["10000001\t\t\t\t\tInvented Region".to_owned()],
        &["20000001\t10000001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the universe parses")
}

fn dungeons() -> eve_server::dungeon_data::Table {
    use eve_server::dungeon_data::{Dungeon, Room, Table};
    let d = |id: i64| Dungeon {
        id,
        archetype: eve_server::combat_site::ESCALATION_ARCHETYPE,
        faction: Some(FACTION),
        difficulty: Some(eve_server::combat_site::ESCALATION_MIN_DIFFICULTY),
        name_id: Some(id + 1),
        rooms: vec![Room {
            id: 7,
            ..Room::default()
        }],
        ..Dungeon::default()
    };
    Table {
        dungeons: [
            eve_server::merc_den::activity::FIXTURE_DUNGEON,
            eve_server::merc_den::activity::FIXTURE_DUNGEON2,
        ]
        .into_iter()
        .map(|id| (id, d(id)))
        .collect(),
        ..Table::default()
    }
}

fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t1.0\t0.0\t1000.0\t{HUB_RADIUS}\t0\t\t\tInvented Hub Row\n\
         type\t{}\t{}\t4\t1\t40\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Fuel Block\n\
         type\t{SKYHOOK_TYPE}\t{GROUP_SKYHOOK}\t46\t1\t1\t20.0\t0.0\t1000.0\t3000.0\t0\t\t\tInvented Skyhook\n\
         type\t{FIXTURE_REAGENT_TYPE}\t{GROUP_COLONY_REAGENTS}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n\
         type\t{DEN_TYPE}\t{GROUP_MERCENARY_DEN}\t22\t1\t1\t100.0\t0.0\t10.0\t2500.0\t0\t\t\tInvented Den\n\
         type\t{FIXTURE_INFOMORPH}\t4824\t17\t1\t1\t1.0\t0.0\t0.1\t1.0\t0\t\t\tInvented Infomorph\n\
         type\t{GUN}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
         6=0.0;30=2.0;50=4.0;51=4000.0;54=100000.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=83.0\tInvented Gun\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        990_922_002,
        sov_hub::GROUP_FUEL_BLOCK,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    c.insert("sov planets", fixture_planets());
    c.insert("sov stars", fixture_stars());
    c.insert("sov mercenary dens", merc_den::fixture_table());
    c.insert(
        "sov mercenary den activities",
        merc_den::activity::fixture_table(),
    );
    c.insert("dungeons", dungeons());
    // The den skill as an invented row under its id.
    let skills = eve_server::skill::Table::parse(&format!(
        "{}skill\t{SKILL_DEN_MANAGEMENT}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tInvented Den Skill\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skills parse");
    c.insert(eve_server::skill::TABLE.name, skills);
    c.insert("universe", universe());
    c
}

/// A star placed so the hub lands at `(HUB_X, 0, 0)`, and three planets of
/// the fixture's invented types 9001 (power), 9002 (workforce), 9003
/// (reagent).
fn celestial_rows() -> String {
    let radius = 3.3e8;
    let at = sov_hub::hub_position([0.0; 3], radius);
    format!(
        "star\t40922000\t990922010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n\
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

/// What a test holds.
struct Setup {
    alice: Conn,
    mem: Memory,
    hall: Arc<SovHall>,
    map: Map,
    world: Arc<RwLock<eve_server::service::Registry>>,
    skyhook: i64,
}

/// A server with a gateway of its own, an alliance whose CEO is the bot
/// pilot (den skill 3, a den in her hangar, a gun fitted when `armed`), a hub
/// the alliance holds and an active skyhook of her corporation over the
/// workforce planet. All of it is a person's setup, done before the bot's
/// first turn.
fn setup(armed: bool) -> Setup {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_922)];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&celestial_rows());
    config.content = Some(Arc::new(content()));
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    let server = eve_server::server::Server::bind(config).expect("bind");
    let addr: SocketAddr = server.local_addr().expect("local addr");
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
    {
        // The activities' sites need the population's slots.
        let mut w = world.write().expect("the world writes");
        let prep = eve_server::population::Prepared::new(
            &eve_server::population::Table::fixture(),
            &universe(),
            &dungeons(),
            &|_| Some(FACTION),
        );
        *w.ext_mut::<eve_server::population::Population>() =
            eve_server::population::Population::new(prep);
    }

    let mut alice = login(addr, "Alice");
    gm(&mut alice, "/giveisk 2000000000");
    let a = corp_call(
        &mut alice,
        "AddCorporation",
        vec![uni("Den Invented"), uni("DENI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(&mut alice, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Den Pact"), uni("DENP"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    let station = alice.session().station_id().expect("docked at home");
    gm(&mut alice, &format!("/createitem {HULL}"));
    let hull = hangar_item(&mut alice, HULL);
    alice.board(hull).expect("boards");
    if armed {
        gm(&mut alice, &format!("/createitem {GUN}"));
        let gun = hangar_item(&mut alice, GUN);
        let fitted = alice.fit_auto(&[gun]).expect("fits");
        assert!(fitted.iter().any(|r| r.type_id == GUN), "the gun is fitted");
    }
    gm(
        &mut alice,
        &format!("/createitem {}", eve_server::sov::TYPE_INFRASTRUCTURE_HUB),
    );
    gm(&mut alice, "/createitem 990922002 1000");
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
    // An own skyhook over the workforce planet, built and switched on.
    let out = slash(&mut alice, &format!("/skyhook spawn {P_LAND}"));
    assert!(out.contains("skyhook"), "{out}");
    let sk = {
        let mut w = world.write().expect("the world writes");
        let id = skyhook::all(w.deltas())
            .into_iter()
            .find(|s| s.corp == corp)
            .expect("the skyhook row")
            .id;
        skyhook::finish_build(&mut w, id, skyhook::now()).expect("the skyhook builds");
        id
    };
    let landed =
        super::common::warp_and_arrive(&mut alice, station, 20_000.0, Duration::from_secs(60));
    assert!(landed, "back at the station");
    alice.dock(station).expect("docks");
    // The den, bought from a builder in the real game: in the hangar here.
    gm(&mut alice, &format!("/createitem {DEN_TYPE}"));
    let said = slash(&mut alice, &format!("/giveskill {SKILL_DEN_MANAGEMENT} 3"));
    assert!(said.contains("level 3"), "{said}");

    let hall = Arc::new(SovHall::new());
    let mut policy = Policy::human();
    policy.role = &sov_bot::ROLE;
    policy.sov = Some(Arc::clone(&hall));
    let mem = Memory::new("Alice", policy);
    let map = Map::new()
        .with_security(SYSTEM, -0.4)
        .with_station(station, SYSTEM);
    // Someone else is the hub's steward.
    assert!(steward::claim(
        Some(gateway),
        hub,
        STEWARD,
        eve_bot::clock::instant()
    ));
    Setup {
        alice,
        mem,
        hall,
        map,
        world,
        skyhook: sk,
    }
}

impl Setup {
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

    fn dens(&self) -> Vec<merc_den::Den> {
        let w = self.world.read().expect("the world reads");
        merc_den::owned_by(w.deltas(), ALICE)
    }

    /// Turns until `done`; the notes so far.
    fn until(&mut self, turns: usize, done: impl Fn(&Setup) -> bool) -> Vec<String> {
        let mut notes = Vec::new();
        for _ in 0..turns {
            notes.push(self.turn());
            if done(self) {
                break;
            }
        }
        notes
    }
}

#[test]
fn a_den_runner_launches_a_den_beside_the_workforce_skyhook_and_lists_the_infomorphs() {
    let mut s = setup(false);
    let notes = s.until(8, |s| !s.dens().is_empty());
    let dens = s.dens();
    assert_eq!(dens.len(), 1, "one den launched\nnotes: {notes:#?}");
    assert_eq!(dens[0].skyhook, s.skyhook, "beside the skyhook: {notes:#?}");
    assert!(
        notes.iter().any(|n| n.contains("sov den:")),
        "the den runner's lines: {notes:#?}"
    );
    assert!(
        notes.iter().all(|n| !n.to_lowercase().contains("slashcmd")),
        "{notes:#?}"
    );

    // The den fills; the pilot empties it and lists what it took.
    let id = dens[0].id;
    let out = slash(&mut s.alice, &format!("/mercden fill {id}"));
    assert!(out.contains("infomorphs"), "{out}");
    let notes = s.until(10, |s| {
        let w = s.world.read().expect("the world reads");
        merc_den::by_id(w.deltas(), id).is_some_and(|d| d.infomorphs == 0)
    });
    assert!(
        notes.iter().any(|n| n.contains("took up to")),
        "the extraction: {notes:#?}"
    );
    let w = s.world.read().expect("the world reads");
    assert!(
        w.store()
            .items()
            .iter()
            .any(|i| i.type_id == FIXTURE_INFOMORPH && i.owner_id == ALICE),
        "the pilot holds (or has listed) the infomorphs: {notes:#?}"
    );
}

#[test]
fn an_armed_den_runner_starts_an_activity_its_den_offers() {
    let mut s = setup(true);
    let notes = s.until(8, |s| !s.dens().is_empty());
    let dens = s.dens();
    assert_eq!(dens.len(), 1, "one den launched\nnotes: {notes:#?}");
    let id = dens[0].id;
    let out = slash(&mut s.alice, &format!("/mercden generate {id}"));
    assert!(out.contains("activity"), "{out}");
    let started = |s: &Setup| {
        let w = s.world.read().expect("the world reads");
        merc_den::activity::of_den(w.deltas(), id)
            .iter()
            .any(|a| a.started)
    };
    let notes = s.until(6, started);
    assert!(started(&s), "the activity was started\nnotes: {notes:#?}");
}
