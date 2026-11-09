//! The Needlejack tests. **Every id and number here is invented**: the types
//! are 990,880,xxx, the systems 30,880,xxx, the regions 10,880,xxx, the pilots
//! 90,880,xxx. The client's constants appear only by name (the key group, the
//! attribute numbers, the error codes). The spool, the radius and the pilot
//! counts are round values chosen for the test, not the client's.

use super::*;
use crate::celestial::Universe;
use crate::item::Item;
use crate::space_state::{Motion, Presence};
use std::time::Duration;

const ORIGIN: i64 = 30_880_001;
const ORIGIN_NEIGHBOUR: i64 = 30_880_002;
const NULL_A: i64 = 30_880_011;
const NULL_B: i64 = 30_880_012;
const NULL_C: i64 = 30_880_013;
const LOW_SEC: i64 = 30_880_014;
const HIGH_SEC: i64 = 30_880_015;
const HAZARD: i64 = 30_880_016;
const BARE: i64 = 30_880_017;
/// A null-sec system of wormhole class 25 (an invented Pochven).
const RING: i64 = 30_880_018;
const WORMHOLE: i64 = 31_880_001;
const VOID: i64 = 32_880_001;
const R_HOME: i64 = 10_880_001;
const R_OTHER: i64 = 10_880_002;
const R_THIRD: i64 = 10_880_003;

const ALICE: i64 = 90_880_001;
const BOB: i64 = 90_880_002;
const CAROL: i64 = 90_880_003;
const DAN: i64 = 90_880_004;

const FIL_NOISE: i64 = 990_880_001;
const FIL_SIGNAL: i64 = 990_880_002;
const FIL_TRIG: i64 = 990_880_003;
const FIL_NO_COUNT: i64 = 990_880_004;
const NOT_A_KEY: i64 = 990_880_005;
const FRIGATE: i64 = 990_880_010;
const BATTLESHIP_CAP: i64 = 990_880_011;
const GROUP_SHIP: i64 = 990_881;
const GROUP_OTHER: i64 = 990_882;

/// Invented: the filament's pilots, radius and spool, in seconds.
const PILOTS: f64 = 2.0;
const RADIUS: f64 = 6_000.0;
const SPOOL_S: f64 = 30.0;

const FLAG_CARGO: i64 = 5;
const KEY_ALICE: i64 = 100_880_501;
const KEY_BOB: i64 = 100_880_502;

fn ship(who: i64) -> i64 {
    100_880_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    content_with("", &[], &[])
}

/// [`content`] plus extra typedata lines, system rows and constellation rows.
fn content_with(
    extra_types: &str,
    extra_systems: &[String],
    extra_constellations: &[String],
) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let filament = |id: i64, group: i64, attrs: &str, name: &str| {
        format!("type\t{id}\t{group}\t17\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{attrs}\t{name}\n")
    };
    let full = |active: u8| {
        format!(
            "{ATTR_RADIUS}={RADIUS};{ATTR_MAX_SHIPS}={PILOTS};{ATTR_ACTIVE_SYSTEM}={active}.0;\
             {ATTR_SPOOLUP}={SPOOL_S}"
        )
    };
    let hull = |id: i64, extra: &str, name: &str| {
        format!(
            "type\t{id}\t{GROUP_SHIP}\t6\t1\t1\t20000.0\t130.0\t1000000.0\t33.0\t0\t\t{extra}\t{name}\n"
        )
    };
    let rows = format!(
        "{}{}{}{}{}{}{}{}{extra_types}",
        crate::typedata::FIXTURE,
        filament(
            FIL_NOISE,
            GROUP_RANDOM_JUMP_KEYS,
            &full(0),
            "Invented Noise Filament"
        ),
        filament(
            FIL_SIGNAL,
            GROUP_RANDOM_JUMP_KEYS,
            &full(1),
            "Invented Signal Filament"
        ),
        filament(
            FIL_TRIG,
            GROUP_TRIGLAVIAN_JUMP_KEYS,
            &full(0),
            "Invented Other Key"
        ),
        filament(
            FIL_NO_COUNT,
            GROUP_RANDOM_JUMP_KEYS,
            &format!("{ATTR_SPOOLUP}={SPOOL_S}"),
            "Invented Countless Key"
        ),
        filament(NOT_A_KEY, GROUP_OTHER, "", "Invented Trinket"),
        hull(FRIGATE, "", "Invented Frigate"),
        hull(
            BATTLESHIP_CAP,
            &format!("{ATTR_IS_CAPITAL}=1.0"),
            "Invented Capital"
        ),
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, region: i64, sec: &str, scan_off: u8, name: &str| {
        format!("{id}\t{region}\t20880001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t{scan_off}\t\t\t{name}")
    };
    let mut systems = vec![
        sys(ORIGIN, R_HOME, "-0.45", 0, "Invented Origin"),
        sys(ORIGIN_NEIGHBOUR, R_HOME, "-0.45", 0, "Invented Neighbour"),
        sys(NULL_A, R_OTHER, "-0.45", 0, "Invented A"),
        sys(NULL_B, R_OTHER, "-0.15", 0, "Invented B"),
        sys(NULL_C, R_THIRD, "-0.95", 0, "Invented C"),
        sys(LOW_SEC, R_OTHER, "0.25", 0, "Invented Low"),
        sys(HIGH_SEC, R_OTHER, "0.85", 0, "Invented High"),
        sys(HAZARD, R_OTHER, "-1.0", 1, "Invented Hazard"),
        sys(BARE, R_THIRD, "-0.35", 0, "Invented Bare"),
        // Class 25 in the system's own column (13).
        format!(
            "{RING}\t{R_OTHER}\t20880001\t\t\t-0.45\t\t0\t0\t0\t0\t0\t0\t25\t0\t\t\tInvented Ring"
        ),
        sys(WORMHOLE, R_OTHER, "-0.99", 0, "Invented Hole"),
        sys(VOID, R_OTHER, "-0.99", 0, "Invented Void"),
        sys(
            crate::celestial::VOID_SPACE_SYSTEMS.start + 1,
            R_OTHER,
            "-0.99",
            0,
            "Invented Void Space",
        ),
    ];
    systems.extend(extra_systems.iter().cloned());
    let mut constellations = vec!["20880001\t10880001\t\t\t\tInvented Constellation".to_owned()];
    constellations.extend(extra_constellations.iter().cloned());
    let u = Universe::parse(
        &systems,
        &[
            format!("{R_HOME}\t\t\t\t\tInvented Home"),
            format!("{R_OTHER}\t\t\t\t\tInvented Other"),
            format!("{R_THIRD}\t\t\t\t\tInvented Third"),
        ],
        &constellations,
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    c
}

fn key(c: &crate::content::Content, item_id: i64, type_id: i64, owner: i64, qty: i64) -> Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: ship(owner),
        flag_id: FLAG_CARGO,
        quantity: qty,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: false,
    }
}

fn place(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
}

fn call_fleet(
    reg: &mut Registry,
    actor: i64,
    method: &str,
    args: Vec<PyValue>,
    bind: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let bind = bind.map_or(PyValue::None, PyValue::Int);
    reg.dispatch(&Request {
        service: "fleetObjectHandler",
        method,
        args: &args,
        kwargs: &[],
        bind_params: Some(&bind),
        actor: Some(actor),
    })
}

/// Alice (a noise filament, quantity 3) and Bob (a signal one) in ORIGIN, with
/// Carol 2 km and Dan 20 km away from Alice. Everyone's safety is off. Nobody
/// is in a fleet.
fn world() -> Registry {
    world_in(content())
}

fn world_in(content: crate::content::Content) -> Registry {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(ORIGIN),
        corporation_id: 98_880_001,
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content);
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
            pilot(DAN, "Dan"),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for who in [ALICE, BOB, CAROL, DAN] {
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = FRIGATE;
            i.group_id = GROUP_SHIP;
        }
    }
    items.extend([
        key(&content, KEY_ALICE, FIL_NOISE, ALICE, 3),
        key(&content, KEY_BOB, FIL_SIGNAL, BOB, 1),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_000,
        ..crate::store::Snapshot::default()
    });
    for (client, who) in [(1, ALICE), (2, BOB), (3, CAROL), (4, DAN)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
        reg.character_mut(who).expect("a pilot").ship_type_id = FRIGATE;
        crate::crimewatch::set_safety(&mut reg, who, crate::crimewatch::rules::Safety::None);
    }
    place(&mut reg, ALICE, ORIGIN, [0.0, 0.0, 0.0]);
    place(&mut reg, BOB, ORIGIN, [9.0e7, 0.0, 0.0]);
    place(&mut reg, CAROL, ORIGIN, [2_000.0, 0.0, 0.0]);
    place(&mut reg, DAN, ORIGIN, [20_000.0, 0.0, 0.0]);
    let _ = reg.take_pending();
    reg
}

/// Alice, Carol and Dan in one fleet.
fn fleet_world() -> Registry {
    fleet_world_in(content())
}

fn fleet_world_in(content: crate::content::Content) -> Registry {
    let mut reg = world_in(content);
    call_fleet(&mut reg, ALICE, "CreateFleet", vec![], None).expect("create");
    call_fleet(&mut reg, ALICE, "Init", vec![PyValue::Int(880)], None).expect("init");
    let PyValue::Int(id) = call_fleet(&mut reg, ALICE, "GetFleetID", vec![], None).expect("id")
    else {
        panic!("an integer");
    };
    for who in [CAROL, DAN] {
        call_fleet(&mut reg, ALICE, "Invite", vec![PyValue::Int(who)], Some(id)).expect("invite");
        call_fleet(
            &mut reg,
            who,
            "AcceptInvite",
            vec![PyValue::Int(880)],
            Some(id),
        )
        .expect("accept");
    }
    let _ = reg.take_pending();
    reg
}

fn activate_item(
    reg: &mut Registry,
    who: i64,
    item: i64,
    now: Instant,
) -> Result<PyValue, ServiceError> {
    let args = [PyValue::Int(item)];
    activate(
        reg,
        &Request {
            service: "randomJumpMgr",
            method: "ActivateRandomJumpFilament",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        },
        now,
        crate::pyutil::filetime_now(),
    )
}

/// The `(code, args)` pairs of a `RandomJumpError`, or a failure of the test.
fn faults_of(e: &ServiceError) -> Vec<(i64, Vec<i64>)> {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("a raised error, got {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("a call header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("user error args")
    };
    assert_eq!(args[0], PyValue::Bytes(ERROR.as_bytes().to_vec()));
    let PyValue::Dict(d) = &args[1] else {
        panic!("a dict")
    };
    let (PyValue::List(list), _) = &d[0] else {
        panic!("a list of errors, the dict's value first")
    };
    list.iter()
        .map(|f| {
            let PyValue::Tuple(t) = f else {
                panic!("a pair")
            };
            let (PyValue::Int(c), PyValue::Tuple(a)) = (&t[0], &t[1]) else {
                panic!("(code, args)")
            };
            (
                *c,
                a.iter()
                    .map(|v| match v {
                        PyValue::Int(i) => *i,
                        _ => panic!("an int arg"),
                    })
                    .collect(),
            )
        })
        .collect()
}

fn count(reg: &Registry, item: i64) -> i64 {
    reg.store().get(item).map_or(0, |i| i.quantity)
}

fn refused_with(reg: &mut Registry, who: i64, item: i64) -> Vec<(i64, Vec<i64>)> {
    let before = (count(reg, item), pending(reg).count());
    let e = activate_item(reg, who, item, Instant::now()).expect_err("refused");
    assert_eq!(
        (count(reg, item), pending(reg).count()),
        before,
        "a refusal spends nothing and starts nothing"
    );
    faults_of(&e)
}

fn codes(f: &[(i64, Vec<i64>)]) -> Vec<i64> {
    f.iter().map(|(c, _)| *c).collect()
}

// ---------------------------------------------------------------------------
// The type and the destination
// ---------------------------------------------------------------------------

#[test]
fn a_filament_is_read_from_its_type_and_never_by_id() {
    let c = content();
    let noise = filament_type(&c, FIL_NOISE).expect("a filament");
    assert_eq!(noise.pilots, PILOTS as i64);
    assert!(!noise.signal);
    assert_eq!(noise.radius_m, RADIUS);
    assert_eq!(noise.spool, Duration::from_secs_f64(SPOOL_S * SPOOL_SCALE));
    assert!(filament_type(&c, FIL_SIGNAL).expect("a filament").signal);
    // Another group, or no pilot count: not served.
    assert!(filament_type(&c, FIL_TRIG).is_none());
    assert!(filament_type(&c, FIL_NO_COUNT).is_none());
    assert!(filament_type(&c, NOT_A_KEY).is_none());
    let all: Vec<i64> = filament_types(&c).iter().map(|f| f.type_id).collect();
    assert_eq!(all, vec![FIL_NOISE, FIL_SIGNAL]);
}

#[test]
fn a_destination_is_null_sec_of_another_region_and_never_banded() {
    let c = content();
    let u = c.get::<Universe>();
    let all = destinations(u, Some(R_HOME), &|_| true);
    assert_eq!(all, vec![NULL_A, NULL_B, NULL_C, BARE]);
    // The client's void space (34xxxxxx) has a null security but is no landing.
    let void_space = crate::celestial::VOID_SPACE_SYSTEMS.start + 1;
    assert_eq!(u.system(void_space).map(|s| s.band()), Some(Band::Void));
    assert!(!all.contains(&void_space));
    for seed in 0..200u64 {
        let d = pick_destination(u, Some(R_HOME), &|_| true, &|_| false, false, seed)
            .expect("somewhere");
        let row = u.system(d).expect("a system");
        assert_eq!(row.band(), Band::Null);
        assert_ne!(row.region_id, R_HOME);
        assert!(!row.disallow_scanning);
        assert_ne!(d, ORIGIN);
    }
    // Pochven (class 25) is null-sec but never a landing: its gates lead
    // only inside it.
    assert_eq!(u.wormhole_class(RING), Some(25));
    assert_eq!(u.system(RING).map(|s| s.band()), Some(Band::Null));
    assert!(!all.contains(&RING));
    // From the other region, its own systems are out.
    let from_other = destinations(u, Some(R_OTHER), &|_| true);
    assert_eq!(from_other, vec![ORIGIN, ORIGIN_NEIGHBOUR, NULL_C, BARE]);
    // A system the catalogue does not serve is out.
    let served = destinations(u, Some(R_HOME), &|s| s != NULL_C);
    assert_eq!(served, vec![NULL_A, NULL_B, BARE]);
    assert_eq!(
        pick_destination(u, Some(R_HOME), &|_| false, &|_| false, false, 1),
        None
    );
}

#[test]
fn noise_reaches_everything_and_signal_follows_the_pilots() {
    let c = content();
    let u = c.get::<Universe>();
    let mut seen = BTreeSet::new();
    for seed in 0..400u64 {
        seen.extend(pick_destination(
            u,
            Some(R_HOME),
            &|_| true,
            &|s| s == NULL_B,
            false,
            seed,
        ));
    }
    assert_eq!(seen, BTreeSet::from([NULL_A, NULL_B, NULL_C, BARE]));
    for seed in 0..100u64 {
        assert_eq!(
            pick_destination(u, Some(R_HOME), &|_| true, &|s| s == NULL_B, true, seed),
            Some(NULL_B),
            "signal goes where there is somebody"
        );
    }
    // Nobody anywhere eligible: as noise.
    let mut spread = BTreeSet::new();
    for seed in 0..400u64 {
        spread.extend(pick_destination(
            u,
            Some(R_HOME),
            &|_| true,
            &|s| s == ORIGIN,
            true,
            seed,
        ));
    }
    assert!(spread.len() > 1);
    // A draw is a function of its seed.
    assert_eq!(
        pick_destination(u, Some(R_HOME), &|_| true, &|_| false, false, 7),
        pick_destination(u, Some(R_HOME), &|_| true, &|_| false, false, 7)
    );
}

#[test]
fn a_group_lands_deep_in_space_and_close_together() {
    let reg = world();
    let centre = landing_centre(&reg, NULL_A, 5);
    let origin_dist = crate::space_state::distance(centre, [0.0; 3]);
    assert!((origin_dist - DEEP_SPACE_M).abs() < 1.0, "{origin_dist}");
    assert_eq!(landing_spot(centre, 0, 5), centre);
    for i in 1..6 {
        let at = landing_spot(centre, i, 5);
        let gap = crate::space_state::distance(at, centre);
        assert!(gap > 0.0 && gap <= SPREAD_M, "{gap}");
        assert_eq!(at, landing_spot(centre, i, 5));
    }
}

// ---------------------------------------------------------------------------
// The call
// ---------------------------------------------------------------------------

#[test]
fn activation_spends_one_and_starts_the_spool() {
    let mut reg = world();
    let now = Instant::now();
    assert_eq!(
        activate_item(&mut reg, ALICE, KEY_ALICE, now),
        Ok(PyValue::None)
    );
    assert_eq!(count(&reg, KEY_ALICE), 2, "one of three");
    let p = pending(&reg).next().expect("a group").clone();
    assert_eq!(p.anchor, ALICE);
    assert_eq!(p.origin, ORIGIN);
    assert_eq!(p.key_type, FIL_NOISE);
    assert_eq!(p.due, now + Duration::from_secs_f64(SPOOL_S * SPOOL_SCALE));
    assert_ne!(
        reg.content()
            .get::<Universe>()
            .system(p.dest)
            .unwrap()
            .region_id,
        R_HOME
    );
    // Nobody left yet.
    tick(
        &mut reg,
        now + Duration::from_secs(1),
        crate::pyutil::filetime_now(),
    );
    assert!(reg.roster().get(ALICE).is_some());
    assert!(reg.jumps().is_empty());
    // A second activation while waiting is refused, and costs nothing.
    let f = refused_with(&mut reg, ALICE, KEY_ALICE);
    assert_eq!(codes(&f), vec![code::SESSION_CHANGE_IN_PROGRESS]);
    assert_eq!(count(&reg, KEY_ALICE), 2);
}

#[test]
fn the_last_filament_of_a_stack_is_destroyed() {
    let mut reg = world();
    activate_item(&mut reg, BOB, KEY_BOB, Instant::now()).expect("spent");
    assert!(reg.store().get(KEY_BOB).is_none());
}

#[test]
fn the_group_leaves_together_when_the_spool_ends() {
    let mut reg = fleet_world();
    let now = Instant::now();
    activate_item(&mut reg, ALICE, KEY_ALICE, now).expect("activated");
    let p = pending(&reg).next().unwrap().clone();
    // Alice and Carol (2 km, in range); Dan is 20 km away, out of range.
    assert_eq!(p.pilots, vec![ALICE, CAROL]);
    let late = now + Duration::from_secs_f64(SPOOL_S * SPOOL_SCALE) + Duration::from_millis(5);
    tick(&mut reg, late, crate::pyutil::filetime_now());
    assert_eq!(pending(&reg).count(), 0);
    assert!(reg.roster().get(ALICE).is_none() && reg.roster().get(CAROL).is_none());
    assert!(reg.roster().get(DAN).is_some(), "out of range: left behind");
    let went: Vec<(i64, i64)> = reg.jumps().destinations().collect();
    assert_eq!(
        went,
        vec![(ALICE, p.dest), (CAROL, p.dest)],
        "one system for both"
    );
    // They land in that system, within a short way of one another.
    let later = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    crate::jump::tick(&mut reg, later);
    let a = crate::jump::take_arrival(&mut reg, ALICE).expect("Alice lands");
    let c = crate::jump::take_arrival(&mut reg, CAROL).expect("Carol lands");
    assert_eq!((a.system, c.system), (p.dest, p.dest));
    assert!(
        crate::space_state::distance(a.motion.position, c.motion.position) <= SPREAD_M,
        "together"
    );
}

#[test]
fn a_passenger_who_wandered_off_is_left_and_a_vanished_activator_takes_nobody() {
    let mut reg = fleet_world();
    let now = Instant::now();
    activate_item(&mut reg, ALICE, KEY_ALICE, now).expect("activated");
    // Carol moves out of the radius during the spool.
    place(&mut reg, CAROL, ORIGIN, [RADIUS * 3.0, 0.0, 0.0]);
    let late = now + Duration::from_secs(60);
    tick(&mut reg, late, crate::pyutil::filetime_now());
    assert!(reg.roster().get(CAROL).is_some());
    assert_eq!(reg.jumps().destinations().count(), 1);

    let mut reg = fleet_world();
    activate_item(&mut reg, ALICE, KEY_ALICE, now).expect("activated");
    let _ = reg.roster_mut().leave(ALICE);
    tick(
        &mut reg,
        now + Duration::from_secs(60),
        crate::pyutil::filetime_now(),
    );
    assert!(reg.roster().get(CAROL).is_some(), "the group stayed");
    assert!(reg.jumps().is_empty());
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

#[test]
fn the_key_is_checked() {
    let mut reg = world();
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, 100_889_999)),
        vec![code::KEY_NOT_FOUND]
    );
    // Bob's is not Alice's to spend.
    assert!(codes(&refused_with(&mut reg, ALICE, KEY_BOB)).contains(&code::INVALID_OWNER));
    // A trinket, an item of another key group, a key with no pilot count.
    let content = content();
    let mut items = reg.store().items().to_vec();
    items.extend([
        key(&content, 100_880_601, NOT_A_KEY, ALICE, 1),
        key(&content, 100_880_602, FIL_TRIG, ALICE, 1),
        key(&content, 100_880_603, FIL_NO_COUNT, ALICE, 1),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_000,
        ..crate::store::Snapshot::default()
    });
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, 100_880_601)),
        vec![code::INVALID_KEY_TYPE]
    );
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, 100_880_602)),
        vec![code::NOT_ACTIVE]
    );
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, 100_880_603)),
        vec![code::NOT_ACTIVE]
    );
    // Not in the ship.
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == KEY_ALICE) {
        i.location_id = 60_880_001;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_000,
        ..crate::store::Snapshot::default()
    });
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, KEY_ALICE)),
        vec![code::ITEM_NOT_REACHABLE]
    );
}

#[test]
fn each_of_the_clients_rules_refuses_with_its_code_and_the_pilot() {
    // Safety green.
    let mut reg = world();
    crate::crimewatch::set_safety(&mut reg, ALICE, crate::crimewatch::rules::Safety::Full);
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![(code::SAFETY_ENGAGED, vec![ALICE])]
    );
    // A PvP timer.
    let mut reg = world();
    let ft = crate::pyutil::filetime_now();
    reg.combat_mut()
        .crimewatch_mut()
        .start(ALICE, crate::crimewatch::Timer::Pvp, ft, false);
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![(code::PVP_TIMER_ACTIVE, vec![ALICE])]
    );
    // A capital hull.
    let mut reg = world();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(ALICE)) {
        i.type_id = BATTLESHIP_CAP;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_000,
        ..crate::store::Snapshot::default()
    });
    crate::crimewatch::set_safety(&mut reg, ALICE, crate::crimewatch::rules::Safety::None);
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![(code::INVALID_SHIP_TYPE, vec![ALICE])]
    );
    // Docked: out of the roster.
    let mut reg = world();
    let _ = reg.roster_mut().leave(ALICE);
    let f = refused_with(&mut reg, ALICE, KEY_ALICE);
    assert_eq!(f, vec![(code::SHIP_IS_DOCKED, vec![ALICE])]);
    // Invulnerable (just undocked).
    let mut reg = world();
    crate::invuln::set_enabled(&mut reg, true);
    crate::invuln::grant(&mut reg, ALICE, ship(ALICE), crate::pyutil::filetime_now());
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![(code::SHIP_IS_INVULNERABLE, vec![ALICE])]
    );
    // Already in a jump.
    let mut reg = world();
    crate::jump::arm_at(&mut reg, ALICE, ORIGIN, NULL_A, [0.0; 3]);
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, KEY_ALICE)),
        vec![code::SESSION_CHANGE_IN_PROGRESS]
    );
}

#[test]
fn the_systems_the_client_bans_refuse_it() {
    for (system, want) in [
        (WORMHOLE, vec![code::WORMHOLE_SPACE_DISALLOWED]),
        (VOID, vec![code::IN_ABYSS_SYSTEM]),
        (HAZARD, vec![code::IN_BANNED_SYSTEM]),
    ] {
        let mut reg = world();
        place(&mut reg, ALICE, system, [0.0; 3]);
        assert_eq!(
            codes(&refused_with(&mut reg, ALICE, KEY_ALICE)),
            want,
            "{system}"
        );
    }
    // High and low sec are fine to leave from.
    for system in [HIGH_SEC, LOW_SEC] {
        let mut reg = world();
        place(&mut reg, ALICE, system, [0.0; 3]);
        activate_item(&mut reg, ALICE, KEY_ALICE, Instant::now()).expect("fine");
    }
}

#[test]
fn a_fleet_over_the_count_is_refused_and_each_member_answers_for_themselves() {
    let mut reg = fleet_world();
    // Dan comes within range: three pilots, two seats.
    place(&mut reg, DAN, ORIGIN, [3_000.0, 0.0, 0.0]);
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![(code::TOO_MANY_IN_FLEET, vec![PILOTS as i64])]
    );
    // Dan's safety is green: he is named, not Alice.
    let mut reg = fleet_world();
    place(&mut reg, DAN, ORIGIN, [3_000.0, 0.0, 0.0]);
    crate::crimewatch::set_safety(&mut reg, DAN, crate::crimewatch::rules::Safety::Full);
    crate::crimewatch::set_safety(&mut reg, CAROL, crate::crimewatch::rules::Safety::Full);
    assert_eq!(
        refused_with(&mut reg, ALICE, KEY_ALICE),
        vec![
            (code::SAFETY_ENGAGED, vec![CAROL]),
            (code::SAFETY_ENGAGED, vec![DAN]),
        ]
    );
}

#[test]
fn a_fleet_member_in_another_system_is_not_carried() {
    let mut reg = fleet_world();
    place(&mut reg, CAROL, ORIGIN_NEIGHBOUR, [10.0, 0.0, 0.0]);
    assert_eq!(carried(&reg, ALICE, RADIUS), vec![ALICE]);
    assert_eq!(carried(&reg, BOB, RADIUS), vec![BOB]);
    activate_item(&mut reg, ALICE, KEY_ALICE, Instant::now()).expect("alone");
    assert_eq!(pending(&reg).next().unwrap().pilots, vec![ALICE]);
}

#[test]
fn nowhere_to_go_spends_nothing() {
    // The only null-sec systems are in the pilots' own region.
    let mut c = content();
    let sys = |id: i64, name: &str| {
        format!("{id}\t{R_HOME}\t20880001\t\t\t-0.45\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = Universe::parse(
        &[
            sys(ORIGIN, "Invented Origin"),
            sys(ORIGIN_NEIGHBOUR, "Invented Neighbour"),
        ],
        &[format!("{R_HOME}\t\t\t\t\tInvented Home")],
        &["20880001\t10880001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("parses");
    c.insert("universe", u);
    let mut reg = world_in(c);
    assert_eq!(
        codes(&refused_with(&mut reg, ALICE, KEY_ALICE)),
        vec![code::NOT_ACTIVE]
    );
    assert_eq!(count(&reg, KEY_ALICE), 3);
}

#[test]
fn the_error_is_the_shape_the_client_adds_to_a_list() {
    let e = refusal(&[
        Fault::about(code::SAFETY_ENGAGED, ALICE),
        Fault::bare(code::KEY_NOT_FOUND),
    ]);
    assert_eq!(
        faults_of(&e),
        vec![
            (code::SAFETY_ENGAGED, vec![ALICE]),
            (code::KEY_NOT_FOUND, vec![])
        ]
    );
}

#[test]
fn the_service_answers_by_its_name() {
    let mut reg = world();
    let args = [PyValue::Int(KEY_ALICE)];
    let r = reg.dispatch(&Request {
        service: "randomJumpMgr",
        method: "ActivateRandomJumpFilament",
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: Some(ALICE),
    });
    assert_eq!(r, Ok(PyValue::None));
    assert!(is_waiting(&reg, ALICE));
}

// ---------------------------------------------------------------------------
// Triglavian keys (PV-3). Invented types 990,880,1xx, invented Pochven systems
// 30,880,1xx in constellations 20,880,101/102, and a light-year grid the test
// chooses; the faction is the named constant, never its number.
// ---------------------------------------------------------------------------

const POCH_A1: i64 = 30_880_101;
const POCH_A2: i64 = 30_880_102;
const POCH_B1: i64 = 30_880_103;
const R_POCH: i64 = 10_880_004;
const KRAI_A: i64 = 20_880_101;
const KRAI_B: i64 = 20_880_102;

const TK_REACH: i64 = 990_880_101;
const TK_ALPHA: i64 = 990_880_102;
const TK_BETA: i64 = 990_880_103;
const TK_MID: i64 = 990_880_104;
const TK_SOLO: i64 = 990_880_105;
const TK_DUP: i64 = 990_880_106;
const TK_PLAIN: i64 = 990_880_107;
const TK_BARE_REACH: i64 = 990_880_108;

/// The reach the test gives its proximity key, light years.
const TEST_REACH_LY: f64 = 2.7;

fn trig_content() -> crate::content::Content {
    let attrs = |count: f64| {
        format!(
            "{ATTR_RADIUS}={RADIUS};{ATTR_MAX_SHIPS}={count};{ATTR_ACTIVE_SYSTEM}=0.0;\
             {ATTR_SPOOLUP}={SPOOL_S}"
        )
    };
    let row = |id: i64, attrs: &str, name: &str| {
        format!(
            "type\t{id}\t{GROUP_TRIGLAVIAN_JUMP_KEYS}\t17\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{attrs}\t{name}\n"
        )
    };
    let types = [
        row(
            TK_REACH,
            &format!(
                "{};{}={TEST_REACH_LY}",
                attrs(PILOTS),
                filaments::ATTR_LY_MAX
            ),
            "Invented Reach Key",
        ),
        row(TK_ALPHA, &attrs(PILOTS), "Invented Alpha Key"),
        row(TK_BETA, &attrs(PILOTS), "Invented Beta Key"),
        row(TK_MID, &attrs(PILOTS), "Invented Mid Key"),
        row(TK_SOLO, &attrs(1.0), "Invented Alpha Solo Key"),
        row(TK_DUP, &attrs(PILOTS), "Invented Alpha Invented Reach Key"),
        row(TK_PLAIN, &attrs(PILOTS), "Invented Plain Key"),
        row(TK_BARE_REACH, &attrs(PILOTS), "Invented Reach Bare Key"),
    ]
    .concat();
    let pochven = |id: i64, krai: i64, name: &str| {
        format!(
            "{id}\t{R_POCH}\t{krai}\t{}\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t{}\t0\t\t\t{name}",
            crate::pochven::FACTION_TRIGLAVIAN,
            crate::pochven::CLASS
        )
    };
    let mut c = content_with(
        &types,
        &[
            pochven(POCH_A1, KRAI_A, "Invented Poch A1"),
            pochven(POCH_A2, KRAI_A, "Invented Poch A2"),
            pochven(POCH_B1, KRAI_B, "Invented Poch B1"),
        ],
        &[
            format!("{KRAI_A}\t{R_POCH}\t\t\t\tInvented Alpha Krai"),
            format!("{KRAI_B}\t{R_POCH}\t\t\t\tInvented Beta Krai"),
        ],
    );
    c.insert("pochven_filaments", filaments::Table::fixture());
    // Pochven sits at the grid's zero; A, B, C are 1, 2 and 3 light years out,
    // and the origin well beyond any reach.
    let ly = |n: f64| n * crate::celestial::LIGHTYEAR;
    let lines: Vec<String> = [
        (ORIGIN, 5.0),
        (POCH_A1, 0.0),
        (POCH_A2, 0.2),
        (POCH_B1, 0.4),
        (NULL_A, 1.0),
        (HAZARD, 1.5),
        (NULL_B, 2.0),
        (NULL_C, 3.0),
    ]
    .iter()
    .map(|(id, n)| format!("s\t{id}\t{}\t0.0\t0.0", ly(*n)))
    .collect();
    c.insert(
        "centers",
        crate::celestial::Centers::parse(&lines).expect("invented centres parse"),
    );
    c
}

fn give(reg: &mut Registry, item_id: i64, type_id: i64, who: i64, qty: i64) {
    let item = key(reg.content(), item_id, type_id, who, qty);
    let mut items = reg.store().items().to_vec();
    items.push(item);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_881_900,
        ..crate::store::Snapshot::default()
    });
    // A snapshot resets the safety.
    for p in [ALICE, BOB, CAROL, DAN] {
        crate::crimewatch::set_safety(reg, p, crate::crimewatch::rules::Safety::None);
    }
}

fn trig_world() -> Registry {
    world_in(trig_content())
}

/// The destination a pending group draws, after one activation.
fn dest_of(reg: &Registry) -> i64 {
    pending(reg).next().expect("a group in its spool").dest
}

fn dest_by_seed(reg: &Registry, type_id: i64, origin: i64, seed: u64) -> Option<i64> {
    let k = triglavian_type(reg.content(), type_id).expect("a served Triglavian key");
    trig_destination(reg, &k, origin, seed)
}

#[test]
fn an_inbound_key_inside_pochven_is_refused_with_31_and_nothing_is_spent() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_101, TK_ALPHA, ALICE, 2);
    place(&mut reg, ALICE, POCH_A1, [0.0; 3]);
    let f = refused_with(&mut reg, ALICE, 100_881_101);
    assert_eq!(f, vec![(code::TRIGLAVIAN_SPACE_DISALLOWED, vec![ALICE])]);
    assert_eq!(count(&reg, 100_881_101), 2);
}

#[test]
fn an_outbound_key_outside_pochven_is_refused_with_30() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_102, TK_REACH, ALICE, 1);
    let f = refused_with(&mut reg, ALICE, 100_881_102);
    assert_eq!(f, vec![(code::TRIGLAVIAN_SPACE_ONLY, vec![ALICE])]);
    assert_eq!(count(&reg, 100_881_102), 1);
}

#[test]
fn a_krai_key_lands_in_that_krai() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_103, TK_ALPHA, ALICE, 1);
    give(&mut reg, 100_881_104, TK_BETA, BOB, 1);
    // The draw, over many seeds: only that Krai's systems, and both of them.
    let mut seen = BTreeSet::new();
    for seed in 0..64 {
        seen.insert(dest_by_seed(&reg, TK_ALPHA, ORIGIN, seed).expect("a landing"));
        assert_eq!(dest_by_seed(&reg, TK_BETA, ORIGIN, seed), Some(POCH_B1));
    }
    assert_eq!(seen, BTreeSet::from([POCH_A1, POCH_A2]));
    // And through the call: spent, and one of that Krai's.
    activate_item(&mut reg, ALICE, 100_881_103, Instant::now()).expect("accepted");
    assert!([POCH_A1, POCH_A2].contains(&dest_of(&reg)));
    assert!(
        reg.store().get(100_881_103).is_none(),
        "the one key is spent"
    );
}

#[test]
fn a_class_key_lands_in_a_system_of_that_class() {
    let reg = trig_world();
    // No gates in the catalogue and no homes: every system is Internal.
    for seed in 0..32 {
        let d = dest_by_seed(&reg, TK_MID, ORIGIN, seed).expect("a landing");
        assert!([POCH_A1, POCH_A2, POCH_B1].contains(&d));
    }
}

#[test]
fn a_proximity_key_never_lands_beyond_its_reach() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_105, TK_REACH, ALICE, 1);
    place(&mut reg, ALICE, POCH_A1, [0.0; 3]);
    let mut seen = BTreeSet::new();
    for seed in 0..64 {
        let d = dest_by_seed(&reg, TK_REACH, POCH_A1, seed).expect("a landing");
        assert_ne!(d, HAZARD, "seed {seed}");
        seen.insert(d);
    }
    // 1 and 2 light years are inside the test's reach; 3 is not; the hazard
    // system (1.5, `disallow_scanning`) is in reach but never drawn.
    assert_eq!(seen, BTreeSet::from([NULL_A, NULL_B]));
    activate_item(&mut reg, ALICE, 100_881_105, Instant::now()).expect("accepted");
    assert!([NULL_A, NULL_B].contains(&dest_of(&reg)));
}

#[test]
fn a_proximity_key_with_no_reach_lands_nowhere_and_spends_nothing() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_106, TK_BARE_REACH, ALICE, 1);
    place(&mut reg, ALICE, POCH_A1, [0.0; 3]);
    let f = refused_with(&mut reg, ALICE, 100_881_106);
    assert_eq!(codes(&f), vec![code::NOT_ACTIVE]);
}

#[test]
fn a_needlejack_still_never_lands_in_pochven() {
    let reg = trig_world();
    let u = reg.content().get::<Universe>();
    for seed in 0..200 {
        let d = pick_destination(u, Some(R_HOME), &|_| true, &|_| false, false, seed)
            .expect("somewhere");
        assert!(
            ![POCH_A1, POCH_A2, POCH_B1].contains(&d),
            "seed {seed} -> {d}"
        );
    }
}

#[test]
fn triglavian_keys_are_kept_apart_from_needlejacks() {
    let c = trig_content();
    let trig: BTreeSet<i64> = triglavian_types(&c)
        .iter()
        .map(|k| k.filament.type_id)
        .collect();
    // The type that matches two families, and the one that matches none, are not served.
    assert_eq!(
        trig,
        BTreeSet::from([TK_REACH, TK_ALPHA, TK_BETA, TK_MID, TK_SOLO, TK_BARE_REACH])
    );
    assert!(
        filament_types(&c)
            .iter()
            .all(|f| !trig.contains(&f.type_id))
    );
    assert!(
        filament_type(&c, TK_ALPHA).is_none(),
        "the Needlejack reader is unchanged"
    );
    let k = triglavian_type(&c, TK_ALPHA).expect("served");
    assert!(!k.filament.signal);
    assert_eq!(k.filament.pilots, PILOTS as i64);
    assert_eq!(k.dir, Dir::In);
}

#[test]
fn a_type_matching_two_families_or_none_is_not_served() {
    let mut reg = trig_world();
    assert!(
        triglavian_type(reg.content(), TK_DUP).is_none(),
        "two families"
    );
    assert!(
        triglavian_type(reg.content(), TK_PLAIN).is_none(),
        "no family"
    );
    give(&mut reg, 100_881_107, TK_DUP, ALICE, 1);
    give(&mut reg, 100_881_108, TK_PLAIN, ALICE, 1);
    for item in [100_881_107, 100_881_108] {
        assert_eq!(
            codes(&refused_with(&mut reg, ALICE, item)),
            vec![code::NOT_ACTIVE]
        );
    }
}

#[test]
fn the_fleet_cap_and_radius_apply_to_triglavian_keys() {
    let mut reg = fleet_world_in(trig_content());
    give(&mut reg, 100_881_109, TK_ALPHA, ALICE, 2);
    give(&mut reg, 100_881_110, TK_SOLO, ALICE, 1);
    // One pilot only: Alice and Carol (2 km) are over the cap, and Alice answers for it.
    let f = refused_with(&mut reg, ALICE, 100_881_110);
    assert_eq!(f, vec![(code::TOO_MANY_IN_FLEET, vec![1])]);
    // Two pilots: Alice and Carol go; Dan, 20 km out, is beyond the 6 km radius.
    activate_item(&mut reg, ALICE, 100_881_109, Instant::now()).expect("accepted");
    let group = pending(&reg).next().expect("a group");
    assert_eq!(group.pilots, vec![ALICE, CAROL]);
    assert_eq!(count(&reg, 100_881_109), 1);
}

#[test]
fn the_group_leaves_for_pochven_when_the_spool_ends() {
    let mut reg = trig_world();
    give(&mut reg, 100_881_111, TK_BETA, ALICE, 1);
    let t0 = Instant::now();
    activate_item(&mut reg, ALICE, 100_881_111, t0).expect("accepted");
    let dest = dest_of(&reg);
    assert!([POCH_B1].contains(&dest));
    tick(
        &mut reg,
        t0 + Duration::from_secs_f64(SPOOL_S * SPOOL_SCALE) + Duration::from_millis(5),
        crate::pyutil::filetime_now(),
    );
    assert_eq!(pending(&reg).count(), 0, "the group has left");
    let went: Vec<(i64, i64)> = reg.jumps().destinations().collect();
    assert_eq!(went, vec![(ALICE, POCH_B1)]);
}
