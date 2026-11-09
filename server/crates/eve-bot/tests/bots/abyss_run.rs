//! BOT-ABYSS over a real socket: three bot frigates -- brain, driver, verbs
//! -- run a co-op abyssal pocket: the leader forms the fleet, deploys a key
//! it holds, the three take the entrance gate, each room is cleared and its
//! gate taken, the cache's wreck is looted, and the three come home.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): an invented home system
//! (the harness's), six invented abyssal systems in the void band, two
//! invented archetype-51 rooms with entry and exit locators, an invented key
//! type with invented tier and weather attribute values, an invented cache
//! and wreck. The ship lists are the bundle's typelists by their ids, here
//! naming the bots' hull by type. **No `[A]` value is an expected value**:
//! the assertions are about kinds and order.
//!
//! **The server's wave table** is the shipped one in the co-op test: its
//! types do not resolve in this invented bundle, the rooms put no wave in
//! space, and each room is clear at once. The waves test starts its own
//! server on a bundle with an `abyss::WaveTable` (invented rows of the server's
//! fixture NPC; no other test's server sees it), so the bots fight real waves before each gate.

use super::common::{SYSTEM, config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::abyss_run;
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Entry, Handle, Pace, Verdict};
use eve_server::abyss::{self, ARCHETYPE, EXIT_LOCATOR, Mode, TARGETS_LIST};
use eve_server::celestial::Universe;
use eve_server::content::Content;
use eve_server::dungeon_data::{self, Dungeon, Object, Room, TypeList};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const RUNNER1: i64 = 90_238_001;
const RUNNER2: i64 = 90_238_002;
const RUNNER3: i64 = 90_238_003;
const REGION: i64 = 19_238_001;
const DEPTHS: i64 = 19_238_002;
const CONSTELLATION: i64 = 29_238_001;
const ABY: [i64; 6] = [
    32_238_001, 32_238_002, 32_238_003, 32_238_004, 32_238_005, 32_238_006,
];
const ROOM_A: i64 = 990_981;
const ROOM_B: i64 = 990_982;
const ENTRY_TYPE: i64 = 990_910;
const OTHER_TYPE: i64 = 990_911;
const KEY_TYPE: i64 = 990_941;
const WEATHER: f64 = 990_950.0;
const CACHE: i64 = 990_960;

fn typedata() -> eve_server::typedata::Installed {
    let key = format!(
        "type\t{KEY_TYPE}\t{}\t4\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{}=0.0;{}={WEATHER}\tInvented Filament\n",
        abyss::KEY_GROUP,
        abyss::ATTR_TIER,
        abyss::ATTR_WEATHER
    );
    let cache = format!(
        "type\t{CACHE}\t{}\t11\t0\t1\t100.0\t0.0\t1000.0\t50.0\t0\t\t9=500.0\tInvented Bioadaptive Cache\n",
        abyss::loot::GROUP_CACHE
    );
    let rows = [eve_server::typedata::FIXTURE.to_owned(), key, cache].concat();
    eve_server::typedata::Installed::build(
        eve_server::typedata::Table::parse(&rows).expect("the invented rows parse"),
    )
}

fn universe() -> Universe {
    let sys = |id: i64, region: i64, sec: &str, name: &str| {
        format!("{id}\t{region}\t{CONSTELLATION}\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let mut rows = vec![sys(SYSTEM, REGION, "-0.45", "Invented Home")];
    for (n, id) in ABY.iter().enumerate().rev() {
        rows.push(sys(*id, DEPTHS, "-0.99", &format!("Invented AD-{n}")));
    }
    Universe::parse(
        &rows,
        &[
            format!("{REGION}\t\t\t\t\tInvented Home"),
            format!("{DEPTHS}\t\t\t\t\tInvented Depths"),
        ],
        &[format!(
            "{CONSTELLATION}\t{DEPTHS}\t\t\t\tInvented Constellation"
        )],
    )
    .expect("the invented universe parses")
}

fn room(id: i64) -> Dungeon {
    let object = |obj: i64, type_id: i64, pos: [f64; 3]| Object {
        room: 1,
        id: obj,
        type_id,
        pos,
        ..Object::default()
    };
    Dungeon {
        id,
        archetype: ARCHETYPE,
        entry_object: Some(10),
        entry_type: Some(ENTRY_TYPE),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![
            object(10, ENTRY_TYPE, [0.0, 0.0, -35.0]),
            object(12, EXIT_LOCATOR, [9.0, 9.0, 9.0]),
            object(11, EXIT_LOCATOR, [0.0, 0.0, 24.0]),
            object(13, OTHER_TYPE, [1.0, 2.0, 3.0]),
            object(14, *abyss::AOE_LOCATORS.start(), [5.0, 5.0, 5.0]),
        ],
        ..Dungeon::default()
    }
}

/// The bundle's dungeons: two rooms and the ship lists, by their ids. The
/// co-op list names the bots' hull by type.
fn dungeons() -> dungeon_data::Table {
    let mut t = dungeon_data::Table {
        dungeons: [room(ROOM_A), room(ROOM_B)]
            .into_iter()
            .map(|d| (d.id, d))
            .collect(),
        ..dungeon_data::Table::default()
    };
    let by_type = |ids: &[i64]| TypeList {
        inc_types: ids.to_vec(),
        ..TypeList::default()
    };
    let rifter = vec![eve_bot::escort::RIFTER];
    t.typelists.insert(Mode::Solo.ship_list(), by_type(&[1]));
    t.typelists.insert(Mode::CoOp.ship_list(), by_type(&rifter));
    t.typelists
        .insert(Mode::TwoPlayer.ship_list(), by_type(&[1]));
    t.typelists.insert(
        TARGETS_LIST,
        TypeList {
            inc_groups: vec![eve_server::npc::FIXTURE_GROUP_FRIGATE],
            ..TypeList::default()
        },
    );
    t
}

fn content() -> Arc<Content> {
    let mut c = Content::fixture();
    c.insert("universe", universe());
    c.insert("dungeon_data", dungeons());
    c.insert("typedata", typedata());
    // The wreck is the server's hand wreck, a type its store knows.
    let wreck = eve_server::npc::HAND_SMALL_WRECK;
    let market = vec![format!(
        "{CACHE}\t{}\t11\t\t0.0\t\t\t\t\t\t\t{wreck}\t1\t0",
        abyss::loot::GROUP_CACHE
    )];
    c.insert(
        "abyss_loot",
        abyss::loot::Table::parse(&format!("{}\n", abyss::loot::HEADER), &market)
            .expect("the invented loot table parses"),
    );
    Arc::new(c)
}

fn start(runners: &[(i64, &str)]) -> (SocketAddr, Arc<Content>) {
    start_with(runners, content())
}

/// [`start`] on a content bundle the caller built (its own wave table, say).
fn start_with(runners: &[(i64, &str)], content: Arc<Content>) -> (SocketAddr, Arc<Content>) {
    let mut config = config();
    config.sandbox.characters = runners
        .iter()
        .enumerate()
        .map(|(i, (id, name))| pilot(*id, name, 100_238_001 + i as i64))
        .collect();
    config.content = Some(Arc::clone(&content));
    (serve(config), content)
}

fn runner(addr: SocketAddr, name: &str, content: &Arc<Content>) -> Handle {
    let mut config = BotConfig::new(name, addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&abyss_run::ROLE);
    config.policy.accept_fleet = AcceptFleet::Any;
    config.content = Some(Arc::clone(content));
    driver::spawn(config).0
}

fn abyss_ok(e: &Entry, what: &str) -> bool {
    ok(e, ActKind::Abyss) && e.what.starts_with(what)
}

fn first(log: &[Entry], f: impl Fn(&Entry) -> bool) -> Option<std::time::Duration> {
    log.iter().find(|e| f(e)).map(|e| e.at)
}

/// **The row's test**: three bot frigates, the leader holding three keys,
/// run a co-op pocket and come home.
#[test]
fn three_bot_frigates_run_a_co_op_pocket_loot_the_cache_and_come_home() {
    let (addr, content) = start(&[
        (RUNNER1, "Runner1"),
        (RUNNER2, "Runner2"),
        (RUNNER3, "Runner3"),
    ]);
    {
        let mut c = login(addr, "Runner1");
        gm(&mut c, &format!("/createitem {KEY_TYPE} 3"));
    }
    let bots = [
        runner(addr, "Runner1", &content),
        runner(addr, "Runner2", &content),
        runner(addr, "Runner3", &content),
    ];
    let deployed = |b: &Handle| b.log().iter().any(|e| abyss_ok(e, "abyss: deploy"));
    assert!(
        until(Duration::from_secs(240), || deployed(&bots[0])),
        "the leader deploys:\n{}",
        dump(&bots[0])
    );
    let entered = |b: &Handle| b.log().iter().any(|e| abyss_ok(e, "abyss: take"));
    assert!(
        until(Duration::from_secs(240), || bots.iter().all(entered)),
        "all three take the entrance gate:\n{}\n--\n{}\n--\n{}",
        dump(&bots[0]),
        dump(&bots[1]),
        dump(&bots[2])
    );
    // Each room's gate, three of them (two encounter gates and the exit): the
    // exit sends each pilot home.
    let gates = |b: &Handle| {
        b.log()
            .iter()
            .filter(|e| abyss_ok(e, "abyss: take the room's gate"))
            .count()
    };
    assert!(
        until(Duration::from_secs(300), || bots
            .iter()
            .all(|b| gates(b) >= abyss::ROOMS)),
        "each room's gate, three times over:\n{}\n--\n{}\n--\n{}",
        dump(&bots[0]),
        dump(&bots[1]),
        dump(&bots[2])
    );
    for bot in &bots {
        let log = bot.log();
        let at = |f: &dyn Fn(&Entry) -> bool| first(&log, f);
        let enter = at(&|e| abyss_ok(e, "abyss: take") && !e.what.contains("room's"));
        let gate = at(&|e| abyss_ok(e, "abyss: take the room's gate"));
        assert!(
            matches!((enter, gate), (Some(a), Some(b)) if a < b),
            "the entrance gate before the room's:\n{}",
            dump(bot)
        );
    }
    // The leader formed the fleet and deployed before any mate went in.
    let log0 = bots[0].log();
    let gather = first(&log0, |e| abyss_ok(e, "abyss: form the fleet"));
    let deploy = first(&log0, |e| abyss_ok(e, "abyss: deploy"));
    assert!(
        matches!((gather, deploy), (Some(g), Some(d)) if g < d),
        "the fleet, then the deploy:\n{}",
        dump(&bots[0])
    );
    // A wreck was looted by somebody in the pocket.
    assert!(
        until(Duration::from_secs(60), || bots.iter().any(|b| {
            let log = b.log();
            let gate = first(&log, |e| abyss_ok(e, "abyss: take the room's gate"));
            let loot = log.iter().find(|e| ok(e, ActKind::Loot)).map(|e| e.at);
            matches!((loot, gate), (Some(l), Some(g)) if l < g)
        })),
        "a wreck looted before the first gate:\n{}",
        dump(&bots[0])
    );
    // Home: no bot is in the void band, and the keys are spent.
    for bot in &bots {
        bot.stop();
    }
    std::thread::sleep(Duration::from_secs(2));
    let mut leader = login(addr, "Runner1");
    let keys: i64 = leader
        // Home may be docked or just back out in space: the hangar answers
        // only when docked, the hold always does.
        .hangar_items()
        .unwrap_or_default()
        .iter()
        .chain(leader.cargo().expect("the hold lists").iter())
        .filter(|r| r.type_id == KEY_TYPE)
        .map(|r| r.quantity)
        .sum();
    assert_eq!(keys, 0, "the three keys were spent");
    for name in ["Runner1", "Runner2", "Runner3"] {
        let c = login(addr, name);
        let system = c
            .session()
            .solar_system_id2()
            .or(c.session().solar_system_id());
        assert!(
            system.is_some_and(|s| !abyss_run_void(s)),
            "{name} is back in known space"
        );
    }
    for bot in &bots {
        assert!(
            !bot.log()
                .iter()
                .any(|e| e.kind == ActKind::Abyss && matches!(e.verdict, Verdict::Failed(_))),
            "no abyss act failed on the connection:\n{}",
            dump(bot)
        );
    }
}

fn abyss_run_void(system: i64) -> bool {
    eve_server::celestial::VOID_SYSTEMS.contains(&system)
}

/// **No key, no run**: the same gang, with nothing in any hangar, never
/// deploys (nothing from nowhere).
#[test]
fn a_gang_with_no_key_never_deploys() {
    let (addr, content) = start(&[
        (RUNNER1, "Runner1"),
        (RUNNER2, "Runner2"),
        (RUNNER3, "Runner3"),
    ]);
    let bots = [
        runner(addr, "Runner1", &content),
        runner(addr, "Runner2", &content),
        runner(addr, "Runner3", &content),
    ];
    // They go about the site runner's day: undock, rat. Give them time to
    // have deployed if anything made them want to.
    assert!(
        until(Duration::from_secs(120), || bots
            .iter()
            .all(|b| b.log().iter().any(|e| e.kind == ActKind::Undock))),
        "they undock:\n{}",
        dump(&bots[0])
    );
    std::thread::sleep(Duration::from_secs(20));
    for bot in &bots {
        let log = bot.log();
        assert!(
            !log.iter().any(|e| e.kind == ActKind::Abyss
                && (e.what.starts_with("abyss: deploy")
                    || e.what.starts_with("abyss: form")
                    || e.what.starts_with("abyss: take"))),
            "no deploy, fleet or gate without a key:\n{}",
            dump(bot)
        );
        bot.stop();
    }
}

/// **Looted keys form a stack at the dock**: the server does not merge stacks,
/// so keys that came in one at a time sit in three rows of one. The leader
/// stacks its hangar (`StackAll`) and deploys the co-op run all the same.
#[test]
fn keys_in_rows_of_one_are_stacked_at_the_dock_and_the_run_deploys() {
    let (addr, content) = start(&[
        (RUNNER1, "Runner1"),
        (RUNNER2, "Runner2"),
        (RUNNER3, "Runner3"),
    ]);
    {
        let mut c = login(addr, "Runner1");
        for _ in 0..3 {
            gm(&mut c, &format!("/createitem {KEY_TYPE} 1"));
        }
        let rows = c
            .hangar_items()
            .expect("the hangar lists")
            .iter()
            .filter(|r| r.type_id == KEY_TYPE)
            .count();
        assert_eq!(rows, 3, "the server left the keys in three rows");
    }
    let bots = [
        runner(addr, "Runner1", &content),
        runner(addr, "Runner2", &content),
        runner(addr, "Runner3", &content),
    ];
    let deployed = |b: &Handle| b.log().iter().any(|e| abyss_ok(e, "abyss: deploy"));
    assert!(
        until(Duration::from_secs(240), || deployed(&bots[0])),
        "the leader stacks its keys and deploys:\n{}",
        dump(&bots[0])
    );
    let log = bots[0].log();
    let stacked = first(&log, |e| abyss_ok(e, "abyss: stack"));
    let deploy = first(&log, |e| abyss_ok(e, "abyss: deploy"));
    assert!(
        matches!((stacked, deploy), (Some(s), Some(d)) if s < d),
        "the stack, then the deploy:\n{}",
        dump(&bots[0])
    );
    for bot in &bots {
        bot.stop();
    }
}

/// The content with one wave of `count` fixture rats in every room of every
/// tier, as the bundle's own [`abyss::WaveTable`]: only the server started on
/// it sees them.
fn content_with_waves(count: usize) -> Arc<Content> {
    let rat = eve_server::npc::FIXTURE_RAT.type_id;
    let rows = abyss::TIERS
        .flat_map(|tier| {
            (1..=abyss::ROOMS).map(move |room| abyss::WaveRow {
                tier,
                room,
                wave: 1,
                type_id: rat,
                count,
            })
        })
        .collect();
    let mut c = Arc::try_unwrap(content()).unwrap_or_else(|_| unreachable!("a fresh Arc"));
    c.insert("abyss_waves", abyss::WaveTable(rows));
    Arc::new(c)
}

/// **The fight, the split and the clock** with real waves: rats stand in
/// every room, the gang kills them, loots their wrecks and only then takes
/// the gate.
#[test]
fn a_gang_fights_the_waves_of_each_room_before_its_gate() {
    let (addr, content) = start_with(
        &[
            (RUNNER1, "Runner1"),
            (RUNNER2, "Runner2"),
            (RUNNER3, "Runner3"),
        ],
        content_with_waves(WAVE_RATS),
    );
    {
        let mut c = login(addr, "Runner1");
        gm(&mut c, &format!("/createitem {KEY_TYPE} 3"));
    }
    let bots = [
        runner(addr, "Runner1", &content),
        runner(addr, "Runner2", &content),
        runner(addr, "Runner3", &content),
    ];
    let gates = |b: &Handle| {
        b.log()
            .iter()
            .filter(|e| abyss_ok(e, "abyss: take the room's gate"))
            .count()
    };
    assert!(
        until(Duration::from_secs(420), || bots
            .iter()
            .all(|b| gates(b) >= abyss::ROOMS)),
        "each room's gate, with the waves between:\n{}\n--\n{}\n--\n{}",
        dump(&bots[0]),
        dump(&bots[1]),
        dump(&bots[2])
    );
    // Wrecks looted: the cache's, and a rat's for every rat of every room.
    let looted: std::collections::BTreeSet<String> = bots
        .iter()
        .flat_map(|b| b.log())
        .filter(|e| ok(e, ActKind::Loot))
        .map(|e| e.what.clone())
        .collect();
    assert!(
        looted.len() >= abyss::ROOMS * WAVE_RATS,
        "{} wrecks looted:\n{looted:?}\n{}",
        looted.len(),
        dump(&bots[0])
    );
    for bot in &bots {
        bot.stop();
    }
}

const WAVE_RATS: usize = 2;

/// **No gate on the grid is a failed try**, not a done one: `Op::Gate` with
/// no room gate to show says so, or the log would read a gate taken.
#[test]
fn a_gate_try_with_no_gate_on_the_grid_fails() {
    let (addr, _content) = start(&[(RUNNER1, "Runner1")]);
    let mut c = login(addr, "Runner1");
    let mut m = eve_bot::brain::Memory::new("Runner1", eve_bot::brain::Policy::fast());
    let tried = abyss_run::act(&mut c, &mut m, abyss_run::Op::Gate);
    assert!(
        matches!(&tried, Err(eve_bot::Error::NotSent(w)) if abyss_run::is_no_gate(w)),
        "a try with no gate is not done: {tried:?}"
    );
}
