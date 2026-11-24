//! The Proving Grounds arena's tests. **Every id here is invented**: pilots
//! 990,1xx, types 990,9xx, dungeons 990,95x, void systems 32,885,xxx, the home
//! system 30,885,001. The client's constants appear by name only
//! ([`RING`], [`ENTRY_LOCATOR`], the gate and locator types, the timer's
//! number 14 as [`TIMER_PVP_EXPIRATION`]); no `[A]` value is asserted by its
//! number: the lengths are the client's own `[V]` 15 minutes and 18 seconds.

use super::*;
use crate::celestial::Universe;
use crate::content::Content;
use crate::dungeon_data::{Object, Room};
use crate::item::Item;
use crate::model::{Character, SandboxConfig};
use crate::proving::matchmaker::Team;
use crate::proving::queue::Entry;
use crate::proving::{Catalogue, MatchType};
use crate::space_state::{Motion, Presence};
use std::sync::Arc;

const HOME: i64 = 30_885_001;
const VOID: [i64; 3] = [32_885_001, 32_885_002, 32_885_003];
const ALICE: i64 = 990_101;
const BOB: i64 = 990_102;
const CAROL: i64 = 990_103;
const ARENA_ROOM: i64 = 990_951;
const FENCE_ROOM: i64 = 990_952;
const PVE_ROOM: i64 = 990_953;
const ENTRY_TYPE: i64 = 990_910;
/// A hull whose death leaves a wreck (the combat fixture's first), so the
/// tests can see wrecks; the one client type id these tests use, taken from
/// the code's own table.
const HULL: i64 = crate::combat::FIXTURE_WRECKS[0].0;
const HULL_GROUP: i64 = 990_931;
const MATCH: i64 = 885_001;
const SCHED: i64 = 885_002;
const FIL: i64 = 990_885_001;
const NOW: BlueTime = 133_000_000_000_000_000;
const T: i64 = FILETIME_PER_S;

/// The arena room's entry locators sit at 10, 20, ... 50 km on x.
const ENTRY_AT: [[f64; 3]; 5] = [
    [10_000.0, 0.0, 0.0],
    [20_000.0, 0.0, 0.0],
    [30_000.0, 0.0, 0.0],
    [40_000.0, 0.0, 0.0],
    [50_000.0, 0.0, 0.0],
];
/// The exit locators: the lower id is at the first position.
const EXIT_AT: [f64; 3] = [0.0, 0.0, 25_000.0];
const OTHER_EXIT_AT: [f64; 3] = [9.0, 9.0, 9.0];

fn ship(who: i64) -> i64 {
    100_000_000 + who
}

fn pod(who: i64) -> i64 {
    100_800_000 + who
}

fn obj(id: i64, type_id: i64, pos: [f64; 3]) -> Object {
    Object {
        room: 1,
        id,
        type_id,
        pos,
        ..Object::default()
    }
}

/// An arena room with the centre piece `centre`, five entry locators and two
/// exits (the lower id at [`EXIT_AT`]).
fn arena_room(id: i64, centre: i64) -> Dungeon {
    let mut objects = vec![obj(1, centre, [0.0; 3])];
    for (i, at) in ENTRY_AT.iter().enumerate() {
        objects.push(obj(20 + i as i64, ENTRY_LOCATOR, *at));
    }
    objects.push(obj(40, abyss::EXIT_LOCATOR, OTHER_EXIT_AT));
    objects.push(obj(30, abyss::EXIT_LOCATOR, EXIT_AT));
    Dungeon {
        id,
        archetype: abyss::ARCHETYPE,
        entry_object: Some(20),
        entry_type: Some(ENTRY_TYPE),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects,
        ..Dungeon::default()
    }
}

/// A PvE abyssal room: one area-of-effect locator makes it one.
fn pve_room(id: i64) -> Dungeon {
    let mut d = arena_room(id, 990_911);
    d.objects.retain(|o| o.type_id != ENTRY_LOCATOR);
    d.objects.push(obj(10, ENTRY_TYPE, [0.0, 0.0, -35.0]));
    d.objects
        .push(obj(14, *abyss::AOE_LOCATORS.start(), [5.0, 5.0, 5.0]));
    d.entry_object = Some(10);
    d
}

fn universe(systems: &[i64]) -> Universe {
    let sys = |id: i64, region: i64, sec: &str, name: &str| {
        format!("{id}\t{region}\t22885001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let mut rows = vec![sys(HOME, 10_885_001, "-0.45", "Invented Home")];
    for (n, id) in systems.iter().enumerate().rev() {
        rows.push(sys(*id, 12_885_001, "-0.99", &format!("Invented AD-{n}")));
    }
    Universe::parse(
        &rows,
        &[
            "10885001\t\t\t\t\tInvented Home".to_owned(),
            "12885001\t\t\t\t\tInvented Depths".to_owned(),
        ],
        &["22885001\t12885001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses")
}

fn event() -> Event {
    let text = format!(
        "{}{SCHED}\t2x1\t\t\t\t1\ttest\n",
        crate::proving::tests::header_for_queue()
    );
    let mt = MatchType {
        id: MATCH,
        filament_type_id: FIL,
        number_of_fleets: 2,
        fleet_size: 1,
        ..MatchType::default()
    };
    Catalogue::parse(&text, vec![mt])
        .expect("the rows parse")
        .active(0)
        .remove(0)
}

fn world_in(systems: &[i64], rooms: Vec<Dungeon>) -> Registry {
    let mut c = Content::fixture();
    c.insert("universe", universe(systems));
    c.insert(
        "dungeons",
        Table {
            dungeons: rooms.into_iter().map(|d| (d.id, d)).collect(),
            ..Table::default()
        },
    );
    let mut reg = Registry::new(SandboxConfig {
        characters: [ALICE, BOB, CAROL]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: ship(id),
                ship_type_id: HULL,
                solar_system_id: Some(HOME),
                ..Character::default()
            })
            .collect(),
        ..SandboxConfig::default()
    })
    .with_content(Arc::new(c));
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id >= ship(ALICE)) {
        i.type_id = HULL;
        i.group_id = HULL_GROUP;
    }
    for who in [ALICE, BOB, CAROL] {
        items.push(Item {
            item_id: pod(who),
            type_id: crate::damage::CAPSULE_TYPE_ID,
            owner_id: who,
            location_id: ship(who),
            flag_id: crate::fitting::FLAG_CAPSULE,
            quantity: 1,
            group_id: crate::fitting::GROUP_CAPSULE,
            category_id: 6,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_885_000,
        ..crate::store::Snapshot::default()
    });
    for (client, who) in [(1, ALICE), (2, BOB), (3, CAROL)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
    }
    reg
}

fn world() -> Registry {
    world_in(
        &VOID,
        vec![
            arena_room(ARENA_ROOM, RING),
            arena_room(FENCE_ROOM, FENCE),
            pve_room(PVE_ROOM),
        ],
    )
}

fn stand(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// A jump that has landed: the session change gone, the bind taken.
fn land(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    crate::jump::arm_arrival(reg, who, system, at);
    let _ = crate::jump::take_arrival(reg, who);
    stand(reg, who, system, at);
}

fn teams(groups: &[&[i64]]) -> Vec<Team> {
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| Team {
            fleet_id: 0,
            members: g
                .iter()
                .map(|&c| Entry {
                    char_id: c,
                    item_id: 0,
                    type_id: FIL,
                    match_type_id: MATCH,
                    schedule_id: SCHED,
                    ship_id: ship(c),
                    system_id: HOME,
                    since: i as i64,
                    fleet_id: 0,
                })
                .collect(),
        })
        .collect()
}

/// A match of `groups` opened and launched at [`NOW`], everyone standing at
/// home beforehand.
fn begin(reg: &mut Registry, groups: &[&[i64]]) -> u64 {
    for (i, who) in groups.iter().flat_map(|g| g.iter()).enumerate() {
        stand(reg, *who, HOME, [i as f64 * 100.0, 0.0, 0.0]);
    }
    let id = open(reg, &event(), &teams(groups), NOW).expect("the arena opens");
    launch(reg, id, NOW);
    id
}

/// Everyone lands at their spot.
fn arrive_all(reg: &mut Registry, id: u64) {
    let a = get(reg, id).expect("the arena");
    for f in a.fighters() {
        land(reg, f.character, a.system, f.spot);
    }
}

/// A started match with Alice against Bob, both landed, and the first tick.
fn duel(reg: &mut Registry) -> u64 {
    let id = begin(reg, &[&[ALICE], &[BOB]]);
    arrive_all(reg, id);
    tick(reg, Instant::now(), NOW + T);
    id
}

fn at(n: i64) -> BlueTime {
    NOW + n * T
}

type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

fn sent(reg: &mut Registry) -> Vec<Sent> {
    reg.take_pending()
        .into_iter()
        .map(|(_, p)| {
            let (event, args) = p.to_push();
            (event, args, p.audience())
        })
        .collect()
}

/// The scheduled pushes that are due (the slim changes), counted.
fn scheduled(reg: &mut Registry) -> usize {
    reg.take_due(Instant::now() + std::time::Duration::from_secs(60))
        .len()
}

fn ring(reg: &Registry, id: u64) -> i64 {
    ring_item(reg, &get(reg, id).unwrap()).expect("the room has a ring")
}

fn ring_vars(reg: &Registry, id: u64) -> BTreeMap<&'static str, i64> {
    let item = ring(reg, id);
    slim_extras(reg, &[item])
        .into_iter()
        .flat_map(|(_, p)| p)
        .map(|(k, v)| match v {
            PyValue::Int(i) => (k, i),
            other => panic!("{k}: {other:?}"),
        })
        .collect()
}

fn dispatch_gate(reg: &mut Registry, who: i64, item: i64) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service: "pvpFilamentMgr",
        method: "AbyssalPVPEndGateActivation",
        args: &[PyValue::Int(item)],
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

fn origin_row(reg: &Registry, who: i64) -> bool {
    reg.deltas()
        .of(crate::logoff_spot::KIND)
        .contains_key(&vec![who.to_string()])
}

fn killed(reg: &mut Registry, who: i64, ft: BlueTime) {
    assert!(crate::combat::self_destruct_kill(reg, who, ft));
}

// ---- the room and the rules ------------------------------------------------

#[test]
fn a_room_is_an_arena_by_its_centre_its_locators_and_no_aoe() {
    assert!(room_of(&arena_room(ARENA_ROOM, RING)).is_some());
    assert!(room_of(&arena_room(FENCE_ROOM, FENCE)).is_some());
    assert!(room_of(&pve_room(PVE_ROOM)).is_none(), "an AOE room is PvE");
    let no_centre = arena_room(ARENA_ROOM, 990_912);
    assert!(room_of(&no_centre).is_none());
    let mut no_exit = arena_room(ARENA_ROOM, RING);
    no_exit.objects.retain(|o| o.type_id != abyss::EXIT_LOCATOR);
    assert!(room_of(&no_exit).is_none());
    let mut no_entry = arena_room(ARENA_ROOM, RING);
    no_entry.objects.retain(|o| o.type_id != ENTRY_LOCATOR);
    assert!(room_of(&no_entry).is_none());
    let r = room_of(&arena_room(ARENA_ROOM, RING)).unwrap();
    assert_eq!(r.entries, ENTRY_AT.to_vec());
    assert_eq!(r.exit, EXIT_AT, "the lowest-id exit");
}

#[test]
fn team_i_stands_at_locator_2i_and_members_two_km_apart() {
    let e = ENTRY_AT.to_vec();
    assert_eq!(spot_for(&e, 0, 0), e[0]);
    assert_eq!(spot_for(&e, 1, 0), e[2], "two locators along");
    assert_eq!(spot_for(&e, 2, 0), e[4]);
    assert_eq!(spot_for(&e, 0, 1)[0], e[0][0] + 2_000.0);
    // Past n/2 teams the circle is used again, shifted.
    let lap = spot_for(&e, 3, 0);
    assert_eq!(lap[0], e[1][0], "6 mod 5 is locator 1");
    assert_ne!(lap[1], e[1][1], "and 5 km along y");
}

fn fighter(out: Option<Out>, out_at: BlueTime) -> Fighter {
    Fighter {
        character: 0,
        origin_system: 0,
        origin_spot: [0.0; 3],
        spot: [0.0; 3],
        told: true,
        out,
        out_at,
        pod_home_at: None,
        pod_die_at: None,
        absent_since: None,
    }
}

fn arena_of_teams(teams: Vec<Vec<Fighter>>) -> Arena {
    Arena {
        id: 1,
        match_type_id: MATCH,
        schedule_id: SCHED,
        system: VOID[0],
        site_id: 0,
        dungeon: ARENA_ROOM,
        entries: ENTRY_AT.to_vec(),
        exit_at: EXIT_AT,
        conduit: 0,
        formed_at: NOW,
        ends_at: NOW + 900 * T,
        teams,
        outcome: None,
        kill_flash: false,
        flash_at: 0,
        reported: false,
        launched: true,
        expired: false,
        last_player_damage: 0,
    }
}

#[test]
fn the_decision_follows_who_is_left() {
    let alive = || fighter(None, 0);
    let out = |t| fighter(Some(Out::ShipLost), t);
    // Two standing: nothing yet; at the clock's end a draw between them.
    let a = arena_of_teams(vec![vec![alive()], vec![alive()]]);
    assert_eq!(decide(&a, false), None);
    assert_eq!(decide(&a, true), Some(Outcome::Draw(vec![0, 1])));
    // One standing: that team wins, with or without the clock.
    let a = arena_of_teams(vec![vec![alive()], vec![out(5)]]);
    assert_eq!(decide(&a, false), Some(Outcome::Win(0)));
    // A team falls when its last pilot does.
    let a = arena_of_teams(vec![vec![alive(), out(5)], vec![alive()]]);
    assert_eq!(decide(&a, false), None);
    // A mutual kill: the two teams that fell together draw.
    let a = arena_of_teams(vec![vec![out(7)], vec![out(7)]]);
    assert_eq!(decide(&a, false), Some(Outcome::Draw(vec![0, 1])));
    // Three teams, two fall together after the first: the first lost.
    let a = arena_of_teams(vec![vec![out(3)], vec![out(7)], vec![out(7)]]);
    assert_eq!(decide(&a, false), Some(Outcome::Draw(vec![1, 2])));
}

#[test]
fn the_report_names_win_loss_and_draw_by_the_clients_numbers() {
    let mut a = arena_of_teams(vec![vec![fighter(None, 0)], vec![fighter(None, 0)]]);
    a.teams[0][0].character = ALICE;
    a.teams[1][0].character = BOB;
    assert_eq!(finished(&a), None, "nothing to report while it goes on");
    a.outcome = Some(Outcome::Win(1));
    let m = finished(&a).unwrap();
    assert_eq!(
        m.results,
        vec![
            (vec![ALICE], MatchResult::Loss),
            (vec![BOB], MatchResult::Win)
        ]
    );
    assert_eq!(
        (
            MatchResult::Loss as i64,
            MatchResult::Win as i64,
            MatchResult::Draw as i64
        ),
        (0, 1, 2)
    );
    a.outcome = Some(Outcome::Draw(vec![0, 1]));
    assert!(
        finished(&a)
            .unwrap()
            .results
            .iter()
            .all(|(_, r)| *r == MatchResult::Draw)
    );
}

// ---- opening ---------------------------------------------------------------

#[test]
fn two_pilots_share_the_highest_free_void_system_at_two_locators_apart() {
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    let a = get(&reg, id).unwrap();
    assert_eq!(a.system, VOID[2], "the highest, the Abyss takes the lowest");
    assert_eq!(a.teams[0][0].spot, ENTRY_AT[0]);
    assert_eq!(a.teams[1][0].spot, ENTRY_AT[2], "two locators along");
    assert!(
        distance(a.teams[0][0].spot, a.teams[1][0].spot) >= 10_000.0,
        "far enough apart to not land on each other"
    );
    for who in [ALICE, BOB] {
        assert!(reg.jumps().contains(who), "{who} is on the jump");
        assert!(origin_row(&reg, who), "{who}'s origin is their logoff spot");
    }
    assert_eq!(a.teams[0][0].origin_system, HOME);
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn a_second_arena_takes_the_next_free_system_and_a_third_finds_none() {
    let mut reg = world_in(
        &VOID[..2],
        vec![arena_room(ARENA_ROOM, RING), arena_room(FENCE_ROOM, FENCE)],
    );
    let t = |a, b| teams(&[&[a], &[b]]);
    let one = open(&mut reg, &event(), &t(ALICE, BOB), NOW).unwrap();
    let two = open(&mut reg, &event(), &t(CAROL, ALICE), NOW).unwrap();
    assert_eq!(get(&reg, one).unwrap().system, VOID[1]);
    assert_eq!(get(&reg, two).unwrap().system, VOID[0]);
    let e = open(&mut reg, &event(), &t(BOB, CAROL), NOW).unwrap_err();
    assert!(e.contains("free"), "{e}");
}

#[test]
fn an_arena_and_an_abyss_pocket_never_share_a_void_system() {
    // One void system, so a shared one would be the only choice. [trap 2]
    let one_system = || {
        world_in(
            &VOID[..1],
            vec![arena_room(ARENA_ROOM, RING), pve_room(PVE_ROOM)],
        )
    };
    let mut reg = one_system();
    stand(&mut reg, CAROL, HOME, [0.0; 3]);
    abyss::enter(&mut reg, CAROL, 0, NOW).expect("control: the pocket takes the system");
    let e = open(&mut reg, &event(), &teams(&[&[ALICE], &[BOB]]), NOW).unwrap_err();
    assert!(
        e.contains("free"),
        "the pocket's system is not the arena's: {e}"
    );
    let mut reg = one_system();
    open(&mut reg, &event(), &teams(&[&[ALICE], &[BOB]]), NOW).unwrap();
    stand(&mut reg, CAROL, HOME, [0.0; 3]);
    let e = abyss::enter(&mut reg, CAROL, 0, NOW).unwrap_err();
    assert!(
        e.contains("free"),
        "the arena's system is not the pocket's: {e}"
    );
}

#[test]
fn a_wreck_in_a_void_system_keeps_it_from_the_arena() {
    let mut reg = world_in(&VOID[..1], vec![arena_room(ARENA_ROOM, RING)]);
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    arrive_all(&mut reg, id);
    killed(&mut reg, BOB, at(1));
    assert!(reg.combat().wrecks().any(|w| w.solar_system_id == VOID[0]));
    // A second arena cannot use the system while the wreck is in it, even
    // after the first is gone from the table.
    reg.ext_mut::<ArenaState>().arenas.clear();
    let e = open(&mut reg, &event(), &teams(&[&[CAROL], &[ALICE]]), at(2)).unwrap_err();
    assert!(e.contains("free"), "{e}");
}

#[test]
fn the_arena_site_is_in_the_abyss_band_and_survives_a_site_pass() {
    // A system with a pilot in it is hot; `site::tick` then vacates every
    // instance whose site the population does not list, unless it is
    // an abyss, wormhole, escalation or mission site. [trap 1]
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    let a = get(&reg, id).unwrap();
    assert!(abyss::is_abyss_site(a.site_id));
    arrive_all(&mut reg, id);
    // A control instance outside every protected band, in the same system.
    let control = 8_100_000_001;
    assert!(!abyss::is_abyss_site(control));
    assert!(site::instantiate(
        &mut reg,
        &abyss::site_of(a.system, control, a.dungeon, NOW)
    ));
    crate::liveness::refresh(&mut reg, Instant::now(), NOW);
    site::tick(&mut reg, Instant::now(), NOW);
    assert!(
        reg.ext::<Sites>().get(control).is_none(),
        "the site pass does vacate a stray instance here"
    );
    assert!(
        reg.ext::<Sites>().get(a.site_id).is_some(),
        "and leaves the arena's alone"
    );
}

#[test]
fn site_ids_come_from_the_abyss_counter_so_two_arenas_differ() {
    let mut reg = world();
    let one = open(&mut reg, &event(), &teams(&[&[ALICE], &[BOB]]), NOW).unwrap();
    let two = open(&mut reg, &event(), &teams(&[&[CAROL], &[ALICE]]), NOW).unwrap();
    let (a, b) = (get(&reg, one).unwrap(), get(&reg, two).unwrap());
    assert_ne!(a.site_id, b.site_id);
    assert!(abyss::is_abyss_site(a.site_id) && abyss::is_abyss_site(b.site_id));
}

#[test]
fn no_arena_room_in_the_bundle_is_an_error_not_a_panic() {
    let mut reg = world_in(&VOID, vec![pve_room(PVE_ROOM)]);
    let e = open(&mut reg, &event(), &teams(&[&[ALICE], &[BOB]]), NOW).unwrap_err();
    assert!(e.contains("arena room"), "{e}");
    assert!(reg.ext::<ArenaState>().arenas.is_empty());
}

#[test]
fn the_clock_is_fifteen_minutes_and_eighteen_seconds_and_the_ring_counts_fifteen() {
    let mut reg = world();
    let id = duel(&mut reg);
    let a = get(&reg, id).unwrap();
    assert_eq!(a.ends_at - a.formed_at, (15 * 60 + 18) * 10_000_000);
    let v = ring_vars(&reg, id);
    assert_eq!(v["endTime"] - v["startTime"], 15 * 60 * 10_000_000);
    assert_eq!(v["endTime"], a.ends_at);
    assert_eq!((v["kill"], v["eventComplete"]), (0, 0));
}

// ---- arrival ---------------------------------------------------------------

#[test]
fn a_pilot_who_landed_is_told_once_and_in_order() {
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    let a = get(&reg, id).unwrap();
    let _ = sent(&mut reg);
    // Alice lands, Bob has not.
    land(&mut reg, ALICE, a.system, a.teams[0][0].spot);
    tick(&mut reg, Instant::now(), at(1));
    let s = sent(&mut reg);
    let alice: Vec<&Sent> = s
        .iter()
        .filter(|p| {
            p.2 == Some(vec![ALICE]) && p.0.starts_with("OnAbyssal")
                || p.2 == Some(vec![ALICE]) && p.0.starts_with("OnAdd")
        })
        .collect();
    assert_eq!(
        alice.iter().map(|p| p.0).collect::<Vec<_>>(),
        vec![
            "OnAbyssalLocationEntered",
            "OnAddExpiryDurationForAbyssalPvpContent"
        ]
    );
    let PyValue::Dict(d) = &alice[0].1[0] else {
        panic!("a dict");
    };
    let get_int = |key: &str| {
        d.iter()
            .find_map(|(v, k)| (*k == crate::pyutil::bstr(key)).then(|| v.clone()))
    };
    assert_eq!(get_int("content_id"), Some(PyValue::Int(a.site_id)));
    assert_eq!(get_int("difficulty_tier"), Some(PyValue::Int(0)));
    assert_eq!(get_int("location_id"), Some(PyValue::Int(1)));
    assert_eq!(
        get_int("nebula_graphic_id"),
        Some(PyValue::Int(abyss::nebula(0)))
    );
    assert_eq!(alice[1].1, vec![PyValue::Int(a.ends_at)]);
    assert!(
        s.iter()
            .all(|p| p.2 != Some(vec![BOB]) || !p.0.starts_with("OnAbyssal")),
        "Bob is not told before he lands"
    );
    // Once only.
    tick(&mut reg, Instant::now(), at(2));
    assert!(
        sent(&mut reg)
            .iter()
            .all(|p| p.2 != Some(vec![ALICE]) || !p.0.starts_with("OnAbyssal")),
        "Alice is not told again"
    );
}

// ---- killing, deciding ------------------------------------------------------

#[test]
fn a_ship_kill_flashes_the_ring_for_one_tick() {
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB, CAROL]]);
    arrive_all(&mut reg, id);
    tick(&mut reg, Instant::now(), at(1));
    assert_eq!(ring_vars(&reg, id)["kill"], 0);
    let _ = sent(&mut reg);
    killed(&mut reg, CAROL, at(2));
    assert_eq!(ring_vars(&reg, id)["kill"], 1, "the explosion");
    assert!(scheduled(&mut reg) > 0, "the slim change went out");
    // The same instant's tick leaves it up; the next one takes it down.
    tick(&mut reg, Instant::now(), at(2));
    assert_eq!(ring_vars(&reg, id)["kill"], 1);
    tick(&mut reg, Instant::now(), at(3));
    let v = ring_vars(&reg, id);
    assert_eq!(v["kill"], 0, "one tick");
    assert_eq!(
        v["eventComplete"], 0,
        "Bob still stands, nothing is decided"
    );
    assert!(scheduled(&mut reg) > 0, "and the change went out");
}

#[test]
fn the_last_ship_decides_and_the_conduit_stands_at_the_exit() {
    let mut reg = world();
    let id = duel(&mut reg);
    crate::proving::arena::FIRED.with(|f| f.borrow_mut().clear());
    killed(&mut reg, BOB, at(2));
    let a = get(&reg, id).unwrap();
    assert_eq!(a.teams[1][0].out, Some(Out::ShipLost));
    assert_eq!(a.outcome, None, "decided by the stage");
    tick(&mut reg, Instant::now(), at(3));
    let a = get(&reg, id).unwrap();
    assert_eq!(a.outcome, Some(Outcome::Win(0)));
    assert_eq!(ring_vars(&reg, id)["eventComplete"], 1);
    let (g, _) = site::object(&reg, a.conduit).expect("the conduit is in space");
    assert_eq!(g.type_id, abyss::PVP_GATE);
    assert_eq!(g.position, EXIT_AT, "at the lowest-id exit locator");
    // Reported once, however many ticks follow.
    tick(&mut reg, Instant::now(), at(4));
    tick(&mut reg, Instant::now(), at(5));
    let fired = crate::proving::arena::FIRED.with(|f| f.borrow().clone());
    assert_eq!(fired.len(), 1);
    assert_eq!(
        fired[0].results,
        vec![
            (vec![ALICE], MatchResult::Win),
            (vec![BOB], MatchResult::Loss)
        ]
    );
}

#[test]
fn a_pod_kill_counts_and_a_kill_elsewhere_does_not() {
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    arrive_all(&mut reg, id);
    // A death in another system, of a fighter, changes nothing.
    stand(&mut reg, CAROL, HOME, [0.0; 3]);
    killed(&mut reg, CAROL, at(1));
    assert!(
        get(&reg, id)
            .unwrap()
            .teams
            .iter()
            .flatten()
            .all(|f| f.out.is_none())
    );
    // Bob's ship, then his pod.
    killed(&mut reg, BOB, at(2));
    assert_eq!(get(&reg, id).unwrap().teams[1][0].out, Some(Out::ShipLost));
    killed(&mut reg, BOB, at(3));
    let f = get(&reg, id).unwrap().teams[1][0].clone();
    assert_eq!(f.out, Some(Out::PodLost));
    assert_eq!(f.out_at, at(2), "out when the ship died");
    assert_eq!(f.pod_home_at, None, "a dead pod does not go home");
}

#[test]
fn a_mutual_kill_in_one_instant_is_a_draw() {
    let mut reg = world();
    let id = duel(&mut reg);
    killed(&mut reg, ALICE, at(2));
    killed(&mut reg, BOB, at(2));
    tick(&mut reg, Instant::now(), at(3));
    assert_eq!(
        get(&reg, id).unwrap().outcome,
        Some(Outcome::Draw(vec![0, 1]))
    );
}

#[test]
fn the_losers_capsule_goes_home_three_seconds_after_the_ship_and_the_row_clears_on_landing() {
    let mut reg = world();
    let id = duel(&mut reg);
    killed(&mut reg, BOB, at(2));
    assert!(origin_row(&reg, BOB));
    tick(&mut reg, Instant::now(), at(4));
    assert!(!reg.jumps().contains(BOB), "not yet: 2 s after");
    tick(&mut reg, Instant::now(), at(5));
    assert!(reg.jumps().contains(BOB), "3 s after the ship died");
    assert!(in_arena(&reg, BOB));
    assert!(
        reg.roster()
            .get(BOB)
            .is_none_or(|p| p.solar_system_id == get(&reg, id).unwrap().system),
        "still in the arena until the jump takes him"
    );
    // The pod lands home.
    land(&mut reg, BOB, HOME, [0.0; 3]);
    tick(&mut reg, Instant::now(), at(7));
    assert!(
        !origin_row(&reg, BOB),
        "the origin row is cleared on landing"
    );
    assert!(!in_arena(&reg, BOB) || get(&reg, id).is_some());
    assert!(reg.ext::<ArenaState>().returning.is_empty());
}

#[test]
fn the_server_never_kills_a_waiting_capsule() {
    let mut reg = world();
    let _ = duel(&mut reg);
    killed(&mut reg, BOB, at(2));
    // The capsule would die only to a shot; the stage alone leaves it.
    tick(&mut reg, Instant::now(), at(3));
    tick(&mut reg, Instant::now(), at(4));
    assert!(reg.roster().get(BOB).is_some(), "Bob's pod is still there");
}

// ---- the end gate ----------------------------------------------------------

#[test]
fn the_end_gate_refuses_until_decided_and_for_the_wrong_item_or_pilot() {
    let mut reg = world();
    let id = duel(&mut reg);
    let a = get(&reg, id).unwrap();
    assert!(dispatch_gate(&mut reg, ALICE, 12345).is_err(), "undecided");
    killed(&mut reg, BOB, at(2));
    tick(&mut reg, Instant::now(), at(3));
    let a2 = get(&reg, id).unwrap();
    assert_ne!(a2.conduit, 0);
    assert!(
        dispatch_gate(&mut reg, ALICE, a2.conduit + 1).is_err(),
        "not the conduit"
    );
    assert!(
        dispatch_gate(&mut reg, CAROL, a2.conduit).is_err(),
        "Carol is in no arena"
    );
    assert_eq!(a.conduit, 0);
    // The pure refusals, including the clock.
    assert_eq!(
        gate_refusal(Some(&a2), ALICE, a2.conduit, a2.ends_at),
        Some("The arena is collapsing.")
    );
    assert_eq!(
        gate_refusal(Some(&a2), ALICE, a2.conduit, a2.ends_at - 1),
        None
    );
    assert!(gate_refusal(None, ALICE, a2.conduit, 0).is_some());
}

#[test]
fn the_end_gate_sends_the_winner_home_with_the_timer_finished() {
    let mut reg = world();
    let id = duel(&mut reg);
    killed(&mut reg, BOB, at(2));
    tick(&mut reg, Instant::now(), at(3));
    let a = get(&reg, id).unwrap();
    let _ = sent(&mut reg);
    assert_eq!(dispatch_gate(&mut reg, ALICE, a.conduit), Ok(PyValue::None));
    let s = sent(&mut reg);
    let done: Vec<&Sent> = s
        .iter()
        .filter(|p| p.0 == "OnAbyssalContentFinished")
        .collect();
    assert_eq!(done.len(), 1);
    assert_eq!(
        done[0].1,
        vec![PyValue::Int(a.site_id), PyValue::Int(TIMER_PVP_EXPIRATION)]
    );
    assert_eq!(done[0].2, Some(vec![ALICE]));
    assert!(reg.jumps().contains(ALICE), "on the way home");
    assert!(in_arena(&reg, ALICE), "until she lands");
    land(&mut reg, ALICE, HOME, [0.0; 3]);
    tick(&mut reg, Instant::now(), at(5));
    assert!(!origin_row(&reg, ALICE));
}

// ---- the clock's end, absence -------------------------------------------------

#[test]
fn at_the_clocks_end_two_standing_teams_draw_and_everyone_inside_dies() {
    let mut reg = world();
    let id = duel(&mut reg);
    let a = get(&reg, id).unwrap();
    crate::proving::arena::FIRED.with(|f| f.borrow_mut().clear());
    let _ = sent(&mut reg);
    tick(&mut reg, Instant::now(), a.ends_at);
    let a = get(&reg, id).unwrap();
    assert_eq!(a.outcome, Some(Outcome::Draw(vec![0, 1])));
    assert_eq!(a.conduit, 0, "no conduit when the clock has run out");
    let s = sent(&mut reg);
    for who in [ALICE, BOB] {
        let finished = s
            .iter()
            .filter(|p| p.0 == "OnAbyssalContentFinished" && p.2 == Some(vec![who]));
        assert_eq!(finished.count(), 1, "{who} is told once");
        assert!(
            reg.store().get(ship(who)).is_none(),
            "{who}'s ship is destroyed"
        );
        assert!(reg.roster().get(who).is_some(), "the pod is still there");
    }
    assert!(
        s.iter()
            .filter(|p| p.0 == "OnAbyssalContentFinished")
            .all(|p| p.1[1] == PyValue::Int(TIMER_PVP_EXPIRATION))
    );
    // A moment later the pods die too, and nobody goes home.
    tick(&mut reg, Instant::now(), a.ends_at + 3 * T);
    for who in [ALICE, BOB] {
        assert!(reg.roster().get(who).is_none(), "{who} is in a clone");
        assert!(!reg.jumps().contains(who));
    }
    let fired = crate::proving::arena::FIRED.with(|f| f.borrow().clone());
    assert_eq!(fired.len(), 1);
    assert!(
        fired[0]
            .results
            .iter()
            .all(|(_, r)| *r == MatchResult::Draw)
    );
    // And the arena closes, its wrecks with it.
    tick(&mut reg, Instant::now(), a.ends_at + 4 * T);
    assert!(get(&reg, id).is_none());
    assert!(reg.combat().wrecks().all(|w| w.solar_system_id != a.system));
}

#[test]
fn a_pilot_absent_for_sixty_seconds_outside_a_jump_is_out() {
    let mut reg = world();
    let id = duel(&mut reg);
    // Alice's ship leaves the roster, as a released log-off hold does.
    let _ = reg.roster_mut().leave(ALICE);
    tick(&mut reg, Instant::now(), at(10));
    tick(&mut reg, Instant::now(), at(10 + ABSENT_S - 1));
    assert_eq!(get(&reg, id).unwrap().teams[0][0].out, None, "59 s");
    tick(&mut reg, Instant::now(), at(10 + ABSENT_S));
    let a = get(&reg, id).unwrap();
    assert_eq!(a.teams[0][0].out, Some(Out::Left));
    assert_eq!(a.outcome, Some(Outcome::Win(1)), "Bob's team is the last");
}

#[test]
fn a_pilot_in_a_jump_is_never_absent() {
    let mut reg = world();
    let id = begin(&mut reg, &[&[ALICE], &[BOB]]);
    tick(&mut reg, Instant::now(), at(1));
    tick(&mut reg, Instant::now(), at(1 + 10 * ABSENT_S));
    assert!(
        get(&reg, id)
            .unwrap()
            .teams
            .iter()
            .flatten()
            .all(|f| f.out.is_none()),
        "both are on the jump in"
    );
}

#[test]
fn a_log_off_mid_jump_keeps_the_origin_row() {
    // `logoff_spot::record` clears the row of a pilot with no roster entry,
    // and a pilot between the start of the jump and the landing has none.
    // [trap 3]
    let mut reg = world();
    let _ = begin(&mut reg, &[&[ALICE], &[BOB]]);
    assert!(reg.roster().get(ALICE).is_none());
    assert!(origin_row(&reg, ALICE));
    crate::logoff_spot::record(&mut reg, ALICE);
    assert!(origin_row(&reg, ALICE), "the arena guard keeps it");
    // The control: the same call for a pilot in no arena clears a row.
    crate::logoff_spot::set(
        &mut reg,
        CAROL,
        crate::logoff_spot::Spot {
            ship_id: ship(CAROL),
            system: HOME,
            position: [0.0; 3],
        },
    );
    let _ = reg.roster_mut().leave(CAROL);
    crate::logoff_spot::record(&mut reg, CAROL);
    assert!(
        !origin_row(&reg, CAROL),
        "an ordinary pilot's row is cleared"
    );
}

// ---- closing, the room coming back ---------------------------------------------

#[test]
fn an_arena_closes_when_empty_and_decided_and_takes_its_wrecks() {
    let mut reg = world();
    let id = duel(&mut reg);
    let a = get(&reg, id).unwrap();
    killed(&mut reg, BOB, at(2));
    tick(&mut reg, Instant::now(), at(3));
    assert!(get(&reg, id).is_some(), "Alice is inside");
    assert!(reg.combat().wrecks().any(|w| w.solar_system_id == a.system));
    let conduit = get(&reg, id).unwrap().conduit;
    dispatch_gate(&mut reg, ALICE, conduit).unwrap();
    // Bob's pod goes home on its own timer.
    tick(&mut reg, Instant::now(), at(6));
    assert!(reg.jumps().contains(BOB));
    // Both are on their way out: nobody present.
    tick(&mut reg, Instant::now(), at(7));
    assert!(get(&reg, id).is_none(), "closed");
    assert!(
        reg.ext::<Sites>().get(a.site_id).is_none(),
        "the room left space"
    );
    assert!(
        reg.combat().wrecks().all(|w| w.solar_system_id != a.system),
        "the wrecks went with it"
    );
}

#[test]
fn an_undecided_arena_with_a_pilot_inside_stays_open() {
    let mut reg = world();
    let id = duel(&mut reg);
    tick(&mut reg, Instant::now(), at(30));
    assert!(get(&reg, id).is_some());
}

#[test]
fn a_room_the_site_stage_dropped_comes_back_with_its_conduit() {
    let mut reg = world();
    let id = duel(&mut reg);
    killed(&mut reg, BOB, at(2));
    tick(&mut reg, Instant::now(), at(3));
    let a = get(&reg, id).unwrap();
    assert!(site::vacate(&mut reg, a.site_id));
    tick(&mut reg, Instant::now(), at(4));
    let b = get(&reg, id).unwrap();
    assert!(reg.ext::<Sites>().get(b.site_id).is_some(), "re-made");
    assert_ne!(b.conduit, 0);
    assert!(site::object(&reg, b.conduit).is_some(), "with its conduit");
}

// ---- the GM door and the registrations ------------------------------------------

fn say(r: Result<PyValue, ServiceError>) -> String {
    match r.expect("an answer") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("text, got {other:?}"),
    }
}

#[test]
fn pg_expire_moves_the_clock_and_decide_settles_it() {
    let mut reg = world();
    let id = duel(&mut reg);
    assert!(say(slash(&mut reg, Some(CAROL), &["status"])).contains("not in an arena"));
    assert!(say(slash(&mut reg, Some(ALICE), &["status"])).contains("undecided"));
    assert!(say(slash(&mut reg, Some(ALICE), &["bogus"])).starts_with("usage"));
    let _ = sent(&mut reg);
    assert!(say(slash(&mut reg, Some(ALICE), &["expire", "30"])).contains("30 s"));
    let a = get(&reg, id).unwrap();
    let moved = a.ends_at - (NOW + T);
    assert!(
        (30 * T..31 * T).contains(&moved),
        "30 s from the stage's clock"
    );
    let s = sent(&mut reg);
    assert_eq!(
        s.iter()
            .filter(|p| p.0 == "OnAddExpiryDurationForAbyssalPvpContent")
            .count(),
        2,
        "both pilots are told the new end"
    );
    assert_eq!(ring_vars(&reg, id)["endTime"], a.ends_at);
    assert_eq!(say(slash(&mut reg, Some(ALICE), &["decide"])), "decided");
    let a = get(&reg, id).unwrap();
    assert_eq!(a.outcome, Some(Outcome::Draw(vec![0, 1])));
    assert_ne!(a.conduit, 0);
    assert_eq!(ring_vars(&reg, id)["eventComplete"], 1);
}

#[test]
fn every_registration_row_exists() {
    assert!(
        crate::tick::STAGES
            .iter()
            .any(|s| s.name == "proving_arena")
    );
    let site = crate::tick::STAGES
        .iter()
        .position(|s| s.name == "site")
        .unwrap();
    let arena = crate::tick::STAGES
        .iter()
        .position(|s| s.name == "proving_arena")
        .unwrap();
    assert!(arena < site, "before `site`, like the Abyss's pockets");
    assert!(
        crate::kill_hooks::PLAYER
            .iter()
            .any(|r| r.name == "proving_arena_on_kill")
    );
    assert!(
        crate::slim_extra::SOURCES
            .iter()
            .any(|s| *s as *const () == slim_extras as *const ())
    );
    assert!(crate::slash::COMMANDS.iter().any(|c| c.name == "pg"));
    assert!(
        crate::world_ext::SLOTS
            .iter()
            .any(|s| s.name == "proving_arena")
    );
}

#[test]
fn the_wire_arm_is_routed() {
    let mut reg = world();
    // No character: the arm is reached and answers its own refusal, not
    // `no such method`.
    let r = reg.dispatch(&Request {
        service: "pvpFilamentMgr",
        method: "AbyssalPVPEndGateActivation",
        args: &[PyValue::Int(1)],
        kwargs: &[],
        bind_params: None,
        actor: None,
    });
    assert!(
        matches!(r, Err(ServiceError::Raise(_))),
        "the arm's own refusal, not no-such-method: {r:?}"
    );
}
