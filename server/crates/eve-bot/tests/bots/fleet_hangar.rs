//! **FLEET-HANGAR over a real socket**: a miner in a fleet compresses at the
//! boss's compressor and moves its ore into the boss's fleet hangar
//! ([`eve_bot::fleet_hangar::drop_into`]); with the boss's switch off it is
//! refused, gives the drop up and goes home.
//!
//! Alice, the fleet boss, flies an invented command hull (`ORCA`'s id, the
//! one the bot's tables name [V-data]) with a fleet hangar and a compressor
//! fitted and online; Miner1 flies an invented hull with raw ore in the
//! hold. They form a fleet the player way and sit a kilometre apart. The
//! switch is `ship.ConfigureShip({key: bool})` (`shipConfigSvc.py`) [V].
//!
//! **Invented rows only**: every hull, volume, range and quantity is this
//! file's own.

use super::common::{SYSTEM, config, gm, login, pilot, until};
use eve_bot::brain::{Homing, Memory, Policy};
use eve_bot::fleet_hangar::{self, FLAG_FLEET_HANGAR};
use eve_bot::mine_ladder::ORCA;
use eve_bot::{Conn, FleetInvite, ItemRow, foreman};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::reprocess::fixture::{ORE_MIXED, ORE_MIXED_COMPRESSED};
use eve_server::ship_config::KEY_FLEET_HANGAR_FLEET;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_760_001;
const MINER: i64 = 90_760_002;
/// The miner's invented hull.
const SMALL: i64 = 990_760_001;
const ORE_GROUP: i64 = 99_761;
const LIST: i64 = 990_760_010;
const COMPRESSOR: i64 = foreman::COMPRESSOR_LARGE;
/// The hangar's size, m3, and the compressor's reach, m: invented.
const HANGAR_M3: f64 = 500.0;
const COMPRESSOR_RANGE: f64 = 83_000.0;
const ORE_UNITS: i64 = 400;

fn content() -> Arc<Content> {
    let hull = |id: i64, group: i64, cap: f64, extra: &str| {
        format!(
            "type\t{id}\t{group}\t6\t1\t1\t20000.0\t{cap:?}\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
             76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;479=600000.0;\
             482=260.0;552=40.0;564=640.0;600=5.0;1137=3.0{extra}\tInvented Hull {id}\n"
        )
    };
    let rows = format!(
        "{fixture}\
         effect\t{ce}\t1\t0\t73\t6\t54\t0\t0\tindustrialItemCompression\teffects.IndustrialItemCompression\n\
         {h1}{h2}\
         type\t{COMPRESSOR}\t4174\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{ce}\t12,16,{ce}\t6=0.0;30=1.0;50=1.0;54={COMPRESSOR_RANGE:?};73=60000.0;3255={LIST}.0\tInvented Compressor\n\
         type\t{ORE_MIXED}\t{ORE_GROUP}\t25\t1\t100\t0.35\t0.0\t0.0\t1.0\t0\t\t\tInvented Ore\n\
         type\t{ORE_MIXED_COMPRESSED}\t{ORE_GROUP}\t25\t1\t100\t0.0035\t0.0\t0.0\t1.0\t0\t\t\tInvented Compressed Ore\n",
        fixture = eve_server::typedata::FIXTURE,
        ce = eve_server::compression::EFFECT_COMPRESSION,
        h1 = hull(ORCA, 941, 130.0, &format!(";912={HANGAR_M3}")),
        h2 = hull(SMALL, 25, 500.0, ";1556=500.0"),
    );
    let td = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    let mut dungeons = eve_server::dungeon_data::Table::fixture();
    dungeons.typelists.insert(
        LIST,
        eve_server::dungeon_data::TypeList {
            inc_groups: vec![ORE_GROUP],
            ..Default::default()
        },
    );
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("dungeons", dungeons);
    Arc::new(c)
}

fn text(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn hangar_of(c: &mut Conn, type_id: i64) -> Vec<ItemRow> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .collect()
}

/// Units of `type_id` in `ship` at `flag` (any flag when `None`).
fn held(c: &mut Conn, ship: i64, flag: Option<i64>, type_id: i64) -> i64 {
    c.ship_items(ship, flag)
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

struct Scene {
    world: std::sync::Arc<std::sync::RwLock<eve_server::service::Registry>>,
    alice: Conn,
    miner: Conn,
    orca: i64,
    small: i64,
}

/// Alice in a fitted command hull, Miner1 in a small hull with raw ore, one
/// fleet, a kilometre apart at rest. The boss's switch is left off.
fn scene(hold: i64) -> Scene {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_760_001),
        pilot(MINER, "Miner1", 100_760_002),
    ];
    cfg.content = Some(content());
    let server = eve_server::server::Server::bind(cfg).expect("bind");
    let addr: SocketAddr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    // The boss: a non-bot's setup (GM items, boards, fits, onlines).
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {ORCA}"));
    gm(&mut alice, &format!("/createitem {COMPRESSOR}"));
    let hull = hangar_of(&mut alice, ORCA).last().expect("an Orca").item_id;
    alice.board(hull).expect("boards");
    let spare: Vec<i64> = hangar_of(&mut alice, COMPRESSOR)
        .iter()
        .map(|r| r.item_id)
        .collect();
    let comp = alice
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == COMPRESSOR)
        .expect("the compressor is fitted")
        .item_id;
    if !alice.is_online(comp).expect("asks") {
        alice.set_online(comp).expect("onlines");
    }

    // The miner: raw ore in the hold.
    let mut miner = login(addr, "Miner1");
    miner.keep_pushes();
    gm(&mut miner, &format!("/createitem {SMALL}"));
    let small = hangar_of(&mut miner, SMALL).last().expect("a hull").item_id;
    miner.board(small).expect("boards");
    gm(&mut miner, &format!("/createitem {ORE_MIXED} {ORE_UNITS}"));
    let stack = hangar_of(&mut miner, ORE_MIXED)
        .last()
        .expect("the ore")
        .item_id;
    let station = miner.session().station_id().expect("docked");
    let ship = miner.session().ship_id().expect("a ship");
    let inventory = miner.open_ship(ship).expect("binds");
    let put = miner
        .call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(stack), PyValue::Int(station)],
            vec![
                (PyValue::Int(ORE_UNITS), text("qty")),
                (PyValue::Int(hold), text("flag")),
            ],
        )
        .expect("answered");
    assert_eq!(put.refusal(), None, "the ore goes into hold {hold}");

    alice.undock().expect("undocks");
    alice.enter_space(SYSTEM).expect("the park");
    miner.undock().expect("undocks");
    miner.enter_space(SYSTEM).expect("the park");

    // The fleet, the player way: the boss invites, the miner accepts.
    let fleet = alice.create_fleet(ORCA).expect("the fleet");
    let seat = alice.fleet_seat(fleet, 0).expect("a seat");
    alice.fleet_invite(fleet, MINER, seat).expect("invites");
    let mut invite = None;
    assert!(until(Duration::from_secs(20), || {
        let _ = miner.collect_pushes(Duration::from_millis(200));
        invite = miner
            .take_pushes()
            .iter()
            .find_map(|(n, a)| FleetInvite::from_push(n, a));
        invite.is_some()
    }));
    miner
        .accept_fleet_invite(invite.expect("invited").fleet_id, SMALL)
        .expect("the miner joins");

    // A kilometre apart, at rest: inside the compressor's and the hangar's
    // reach.
    place(&world, 1_000.0);
    Scene {
        world,
        orca: alice.session().ship_id().expect("Alice's ship"),
        small: miner.session().ship_id().expect("the miner's ship"),
        alice,
        miner,
    }
}

/// Both at rest, the miner `dx` metres from Alice along x.
fn place(world: &std::sync::RwLock<eve_server::service::Registry>, dx: f64) {
    let mut w = world.write().expect("the world");
    let a = w.roster().get(ALICE).expect("Alice").motion.position;
    for (who, dx) in [(ALICE, 0.0), (MINER, dx)] {
        let p = w.roster_mut().get_mut(who).expect("in space");
        p.motion.velocity = [0.0; 3];
        p.motion.position = [a[0] + dx, a[1], a[2]];
    }
}

fn open_to_fleet(alice: &mut Conn) {
    let params = alice.session().location_moniker().expect("in space");
    let changes = PyValue::Dict(vec![(PyValue::Bool(true), text(KEY_FLEET_HANGAR_FLEET))]);
    let answer = alice
        .moniker_call("ship", params, "ConfigureShip", vec![changes], vec![])
        .expect("ConfigureShip is answered");
    assert_eq!(answer.outcome.refusal(), None);
}

/// **The drop**: the boss's switch on, the miner compresses at the boss's
/// compressor and the compressed ore lands in the Orca's fleet hangar as the
/// boss lists it; the miner's hold carries no ore.
#[test]
fn a_miner_compresses_at_the_boss_and_fills_his_fleet_hangar() {
    fills_from(eve_bot::FLAG_CARGO);
}

/// The same from the **ore hold** (a barge's own bay, flag 134), where a
/// real miner's ore is.
#[test]
fn a_miners_ore_hold_empties_into_the_boss_fleet_hangar_too() {
    fills_from(eve_bot::brain::FLAG_MINING_HOLD);
}

fn fills_from(hold: i64) {
    let mut s = scene(hold);
    open_to_fleet(&mut s.alice);
    assert_eq!(
        held(&mut s.miner, s.small, None, ORE_MIXED),
        ORE_UNITS,
        "the hold carries raw ore to begin with"
    );
    let mut memory = Memory::new("Miner1", Policy::fast());
    let moved = fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect("drops");
    assert_eq!(moved, 1, "one stack moved");
    assert_eq!(
        held(
            &mut s.alice,
            s.orca,
            Some(FLAG_FLEET_HANGAR),
            ORE_MIXED_COMPRESSED
        ),
        ORE_UNITS,
        "the boss lists the compressed ore, unit for unit, in the fleet hangar"
    );
    assert_eq!(
        held(&mut s.alice, s.orca, Some(FLAG_FLEET_HANGAR), ORE_MIXED),
        0,
        "no raw ore went to the hangar: it was compressed first"
    );
    assert_eq!(
        held(&mut s.miner, s.small, None, ORE_MIXED)
            + held(&mut s.miner, s.small, None, ORE_MIXED_COMPRESSED),
        0,
        "the miner's hold is empty"
    );
    assert_eq!(memory.hangar_drop.stacks, 1);
    assert!(memory.hangar_drop.since.is_none() && memory.homing.is_none());
}

/// **Refused**: the boss's switch is off (its default). Nothing arrives in
/// the hangar, the drop is given up and the miner goes home.
#[test]
fn a_boss_whose_switch_is_off_refuses_the_drop_and_the_miner_goes_home() {
    let mut s = scene(eve_bot::FLAG_CARGO);
    let mut memory = Memory::new("Miner1", Policy::fast());
    memory.hangar_drop.since = Some(std::time::Instant::now());
    let refused = fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect_err("refused");
    let why = refused.to_string();
    assert!(
        why.contains("FleetHangarAccessDenied") && why.contains("0 moved"),
        "the act log names the server's key: {why}"
    );
    assert_eq!(
        held(
            &mut s.alice,
            s.orca,
            Some(FLAG_FLEET_HANGAR),
            ORE_MIXED_COMPRESSED
        ) + held(&mut s.alice, s.orca, Some(FLAG_FLEET_HANGAR), ORE_MIXED),
        0,
        "the fleet hangar is empty"
    );
    assert_eq!(memory.homing, Some(Homing::HoldFull), "it goes home");
    assert!(memory.hangar_drop.refused_at.is_some());
    assert!(memory.hangar_drop.since.is_none());
}

/// **Too early**: the bot's model of a follow puts it in range at once, the
/// real ship is still far (client run 11-05). The server's range refusal
/// (`CustomNotify`) does not end the drop: nothing is given up, the miner
/// waits and tries again, and once the ship is in reach the ore moves.
#[test]
fn a_drop_made_before_the_ship_is_in_reach_waits_and_lands_when_it_is() {
    let mut s = scene(eve_bot::FLAG_CARGO);
    open_to_fleet(&mut s.alice);
    place(
        &s.world,
        eve_server::ship_config::MAX_TRANSFER_DISTANCE * 20.0,
    );
    let mut memory = Memory::new("Miner1", Policy::fast());
    memory.hangar_drop.since = Some(std::time::Instant::now());
    let waits =
        fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect_err("a refusal, not ok");
    assert!(
        waits.to_string().contains("out of range"),
        "logged as a range refusal: {waits}"
    );
    assert_eq!(
        memory.hangar_drop.last_refusal.as_deref(),
        Some("CustomNotify")
    );
    assert!(memory.homing.is_none(), "not sent home for being early");
    assert!(memory.hangar_drop.since.is_some(), "the drop is still on");
    assert_eq!(memory.hangar_drop.range_tries, 1);
    place(&s.world, 1_000.0);
    let moved = fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect("drops");
    assert_eq!(moved, 1, "in reach now: one stack");
    assert_eq!(
        held(
            &mut s.alice,
            s.orca,
            Some(FLAG_FLEET_HANGAR),
            ORE_MIXED_COMPRESSED
        ),
        ORE_UNITS
    );
    assert_eq!(memory.hangar_drop.range_tries, 0);
}

/// The wait is bounded: a boss that never lets the ship near sends it home,
/// with the key in the act log.
#[test]
fn a_boss_who_stays_out_of_reach_is_given_up_after_the_tries() {
    let mut s = scene(eve_bot::FLAG_CARGO);
    open_to_fleet(&mut s.alice);
    place(
        &s.world,
        eve_server::ship_config::MAX_TRANSFER_DISTANCE * 20.0,
    );
    let mut memory = Memory::new("Miner1", Policy::fast());
    memory.hangar_drop.since = Some(std::time::Instant::now());
    memory.hangar_drop.range_tries = fleet_hangar::MAX_RANGE_TRIES;
    let refused =
        fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect_err("out of tries");
    assert!(refused.to_string().contains("CustomNotify"), "{refused}");
    assert_eq!(memory.homing, Some(Homing::HoldFull));
}

/// The timeout after a refusal gives up with the last key in the act log.
#[test]
fn a_timeout_after_a_refusal_logs_the_last_key() {
    let mut s = scene(eve_bot::FLAG_CARGO);
    open_to_fleet(&mut s.alice);
    let mut memory = Memory::new("Miner1", Policy::fast());
    memory.hangar_drop.since = Some(std::time::Instant::now());
    memory.hangar_drop.last_refusal = Some("CustomNotify".to_owned());
    memory.hangar_drop.timed_out = true;
    let refused =
        fleet_hangar::drop_into(&mut s.miner, &mut memory, s.orca).expect_err("timed out");
    assert!(refused.to_string().contains("CustomNotify"), "{refused}");
    assert!(refused.to_string().contains("gave up"), "{refused}");
    assert_eq!(memory.homing, Some(Homing::HoldFull));
}
