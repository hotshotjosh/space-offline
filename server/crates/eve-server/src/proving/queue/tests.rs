//! The Proving Grounds queue's tests. **Every id here is invented**: pilots
//! 90,882,xxx, systems 30,882,xxx, filament types 990,882,xxx, items
//! 100,882,xxx, schedules 882,xxx, match types 883,xxx. The client's own
//! codes ([V]: the order of the base checks, the codes of the client's list)
//! appear as numbers or `err::` names; every `[A]` code is asserted by the
//! name of its constant (`DROP_DOCKED`, `DROP_MOVED`, `DROP_ENDED`,
//! `KEY_NOT_IN_SHIP`, `FLEET_NEEDED`), never as a number.

use super::*;
use crate::item::Item;
use crate::model::{Character, SandboxConfig};
use crate::proving::matchmaker::Team;
use crate::proving::{Event, MatchType};
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_882_001;
const BOB: i64 = 90_882_002;
const CAROL: i64 = 90_882_003;
const DAVE: i64 = 90_882_004;
const HOME: i64 = 30_882_001;
const AWAY: i64 = 30_882_002;
const FIL: i64 = 990_882_001;
const OTHER_FIL: i64 = 990_882_002;
const SCHED: i64 = 882_001;
const MATCH: i64 = 883_001;
const KEY: i64 = 100_882_001;
const HANGAR_KEY: i64 = 100_882_002;
const BOBS_KEY: i64 = 100_882_003;
const CAROLS_KEY: i64 = 100_882_005;
const DAVES_KEY: i64 = 100_882_006;
const NOT_A_KEY: i64 = 100_882_004;
const NOT_A_KEY_TYPE: i64 = 990_882_900;
const GROUP_OTHER: i64 = 990_882_901;
const HANGAR: i64 = 60_882_001;
const DAY: i64 = crate::seed::DAY;
const HOUR: i64 = crate::seed::HOUR;

fn ship(who: i64) -> i64 {
    100_000_000 + who
}

/// Midday UTC of the invented year 3001 (a date the client's table never
/// reaches), `day` days after the new year.
fn noon(day: i64) -> i64 {
    // 3001-01-01 00:00 UTC as a FILETIME, by the same oracle the read-side
    // tests use.
    super::super::tests::ft(3001, 1, 1, 0) + day * DAY + 12 * HOUR
}

fn match_type(fleets: i64, size: i64) -> MatchType {
    MatchType {
        id: MATCH,
        filament_type_id: FIL,
        number_of_fleets: fleets,
        fleet_size: size,
        ..MatchType::default()
    }
}

/// A QA row of the given format, always active.
fn qa_catalogue(fleets: i64, size: i64) -> Catalogue {
    let text = format!(
        "{}{SCHED}\t{fleets}x{size}\t\t\t\t1\ttest\n",
        super::super::tests::header_for_queue()
    );
    Catalogue::parse(&text, vec![match_type(fleets, size)]).expect("the rows parse")
}

/// A real window, 3001-01-10 to 3001-01-12 (days 9 to 11 after the new year).
fn windowed() -> Catalogue {
    let text = format!(
        "{}{SCHED}\t2x1\t\t3001-01-10\t3001-01-12\t0\ttest\n",
        super::super::tests::header_for_queue()
    );
    Catalogue::parse(&text, vec![match_type(2, 1)]).expect("the rows parse")
}

fn cargo(item_id: i64, owner: i64, location: i64, type_id: i64, group: i64, n: i64) -> Item {
    Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: location,
        flag_id: 5,
        quantity: n,
        group_id: group,
        category_id: 4,
        singleton: false,
    }
}

/// Alice and Bob, each with a ship in HOME; Alice's cargo holds a filament
/// (3 of them), a thing that is not one, and a filament in a hangar; Bob's
/// ship holds a filament of his own. Both are docked until `stand`.
fn world(cat: Catalogue) -> Registry {
    let mut content = crate::content::Content::fixture();
    content.insert("proving_events", cat);
    let mut reg = Registry::new(SandboxConfig {
        characters: [ALICE, BOB, CAROL, DAVE]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: ship(id),
                solar_system_id: Some(HOME),
                ..Character::default()
            })
            .collect(),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    let mut items = reg.store().items().to_vec();
    items.push(cargo(KEY, ALICE, ship(ALICE), FIL, FILAMENT_GROUP, 3));
    items.push(cargo(HANGAR_KEY, ALICE, HANGAR, FIL, FILAMENT_GROUP, 1));
    items.push(cargo(BOBS_KEY, BOB, ship(BOB), FIL, FILAMENT_GROUP, 1));
    items.push(cargo(
        CAROLS_KEY,
        CAROL,
        ship(CAROL),
        FIL,
        FILAMENT_GROUP,
        1,
    ));
    items.push(cargo(DAVES_KEY, DAVE, ship(DAVE), FIL, FILAMENT_GROUP, 1));
    items.push(cargo(
        NOT_A_KEY,
        ALICE,
        ship(ALICE),
        NOT_A_KEY_TYPE,
        GROUP_OTHER,
        1,
    ));
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_883_000,
        ..crate::store::Snapshot::default()
    });
    reg
}

fn stand(reg: &mut Registry, who: i64, system: i64) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
}

fn codes(e: &[Refusal]) -> Vec<i64> {
    e.iter().map(|(c, _)| *c).collect()
}

fn clean_facts() -> Facts {
    Facts {
        ship_found: true,
        ..Facts::default()
    }
}

fn clean_join() -> JoinFacts {
    JoinFacts {
        key: Some(KeyFacts {
            type_id: FIL,
            group_id: FILAMENT_GROUP,
            owned: true,
            in_ship: true,
            stack: 1,
        }),
        asked_type: FIL,
        fleet_format: false,
        fleet_over: None,
        ship_allowed: true,
        event_active: true,
        already_queued: false,
    }
}

fn failures(reg: &mut Registry) -> Vec<(Vec<i64>, PyValue)> {
    reg.take_due(Instant::now() + Duration::from_secs(60))
        .iter()
        .filter_map(|p| {
            let (name, mut args) = p.to_push();
            (name == "OnMatchMakingFailure").then(|| (p.audience().unwrap(), args.remove(0)))
        })
        .collect()
}

// ---- join_errors: the client's order --------------------------------------

#[test]
fn a_clean_pilot_with_a_key_has_no_errors() {
    assert!(join_errors(ALICE, &clean_facts(), &clean_join()).is_empty());
}

#[test]
fn docked_gives_16_with_char() {
    let f = Facts {
        docked: true,
        ..clean_facts()
    };
    assert_eq!(
        join_errors(ALICE, &f, &clean_join()),
        vec![(err::SHIP_IS_DOCKED, vec![ALICE])]
    );
}

#[test]
fn base_order_matches_client() {
    // `perform_base_validation_checks`: ship (21), docked 16, cloaked 15,
    // invulnerable 17, self-destructing 18, tethered 19, warp 14, abyss
    // system 1, session change 10, PvP timer 8.
    let f = Facts {
        ship_found: false,
        docked: true,
        cloaked: true,
        invulnerable: true,
        self_destructing: true,
        tethered: true,
        warping: true,
        in_abyss_system: true,
        session_change: true,
        pvp_timer: true,
        in_fleet: false,
    };
    let e = join_errors(ALICE, &f, &clean_join());
    assert_eq!(codes(&e), vec![21, 16, 15, 17, 18, 19, 14, 1, 10, 8]);
    assert_eq!(e[0].1, Vec::<i64>::new(), "SHIP_NOT_FOUND has no arguments");
    assert!(e[1..].iter().all(|(_, a)| a == &vec![ALICE]));
}

#[test]
fn the_client_checks_come_before_the_servers() {
    let f = Facts {
        docked: true,
        ..clean_facts()
    };
    let j = JoinFacts {
        key: None,
        event_active: false,
        already_queued: true,
        ..clean_join()
    };
    // 16 (client), then key not found 6, event 33, queued 32.
    assert_eq!(codes(&join_errors(ALICE, &f, &j)), vec![16, 6, 33, 32]);
}

#[test]
fn the_keys_checks_name_their_arguments() {
    let k = |f: fn(&mut KeyFacts)| {
        let mut j = clean_join();
        f(j.key.as_mut().unwrap());
        join_errors(ALICE, &clean_facts(), &j)
    };
    assert_eq!(k(|k| k.in_ship = false), vec![(KEY_NOT_IN_SHIP, vec![FIL])]);
    assert_eq!(
        k(|k| k.group_id = GROUP_OTHER),
        vec![(err::INVALID_KEY_TYPE, vec![ALICE])]
    );
    assert_eq!(
        k(|k| k.type_id = OTHER_FIL),
        vec![(err::INVALID_KEY_TYPE, vec![ALICE])],
        "a filament of another type than the call named"
    );
    assert_eq!(
        k(|k| k.owned = false),
        vec![(err::INVALID_OWNER, vec![ALICE])]
    );
    assert_eq!(k(|k| k.stack = 0), vec![(err::INVALID_KEY_AMOUNT, vec![])]);
    let none = JoinFacts {
        key: None,
        ..clean_join()
    };
    assert_eq!(
        join_errors(ALICE, &clean_facts(), &none),
        vec![(err::KEY_NOT_FOUND, vec![])]
    );
}

#[test]
fn a_fleet_format_needs_a_fleet() {
    let j = JoinFacts {
        fleet_format: true,
        ..clean_join()
    };
    assert_eq!(
        join_errors(ALICE, &clean_facts(), &j),
        vec![(FLEET_NEEDED, vec![])]
    );
    let f = Facts {
        in_fleet: true,
        ..clean_facts()
    };
    assert!(join_errors(ALICE, &f, &j).is_empty());
}

// ---- the wire shapes -------------------------------------------------------

#[test]
fn the_refusal_is_a_pvpjumperror_with_the_error_list() {
    let ServiceError::Raise(v) = raise(&[(err::SHIP_IS_DOCKED, vec![ALICE]), (33, vec![])]) else {
        panic!("not a Raise");
    };
    let expected = eve_proto::user_error(
        "PVPJumpError",
        crate::pyutil::pypairs(vec![(
            "errors",
            PyValue::List(vec![
                PyValue::Tuple(vec![
                    PyValue::Int(16),
                    PyValue::Tuple(vec![PyValue::Int(ALICE)]),
                ]),
                PyValue::Tuple(vec![PyValue::Int(33), PyValue::Tuple(vec![])]),
            ]),
        )]),
    );
    assert_eq!(v, expected);
}

#[test]
fn the_failure_push_carries_the_same_list_to_one_pilot() {
    let p = MatchFailure {
        to: ALICE,
        errors: vec![(err::EVENT_NOT_ACTIVE, vec![])],
    };
    let (name, args) = p.to_push();
    assert_eq!(name, "OnMatchMakingFailure");
    assert_eq!(args, vec![errors_value(&[(33, vec![])])]);
    assert_eq!(p.audience(), Some(vec![ALICE]));
}

// ---- the queue -------------------------------------------------------------

fn entry(who: i64) -> Entry {
    Entry {
        char_id: who,
        item_id: KEY,
        type_id: FIL,
        match_type_id: MATCH,
        schedule_id: SCHED,
        ship_id: ship(who),
        system_id: HOME,
        since: 1,
        fleet_id: 0,
    }
}

#[test]
fn leave_is_idempotent() {
    let mut q = Queue::default();
    assert!(q.add(entry(ALICE)));
    assert!(q.remove(ALICE));
    assert!(
        !q.remove(ALICE),
        "the second leave finds nobody and is quiet"
    );
    assert!(q.is_empty());
    assert!(!q.remove(BOB), "a pilot who never queued");
}

#[test]
fn one_entry_per_pilot() {
    let mut q = Queue::default();
    assert!(q.add(entry(ALICE)));
    assert!(!q.add(entry(ALICE)));
    assert!(q.add(entry(BOB)));
    assert_eq!(q.len(), 2);
}

#[test]
fn drop_reasons_follow_the_documented_order() {
    let e = entry(ALICE);
    let ok = Now {
        docked: false,
        system_id: HOME,
        ship_id: ship(ALICE),
        event: Some((MATCH, SCHED)),
        key_held: true,
        fleet_id: 0,
        fleet_format: false,
    };
    assert_eq!(drop_reason(&e, &ok), None);
    let docked = Now {
        docked: true,
        system_id: 0,
        ..ok.clone()
    };
    assert_eq!(drop_reason(&e, &docked), Some((DROP_DOCKED, vec![ALICE])));
    for moved in [
        Now {
            system_id: AWAY,
            ..ok.clone()
        },
        Now {
            ship_id: ship(BOB),
            ..ok.clone()
        },
        Now {
            key_held: false,
            ..ok.clone()
        },
    ] {
        assert_eq!(drop_reason(&e, &moved), Some((DROP_MOVED, vec![])));
    }
    for ended in [
        Now {
            event: None,
            ..ok.clone()
        },
        Now {
            event: Some((MATCH, SCHED + 1)),
            ..ok.clone()
        },
    ] {
        assert_eq!(drop_reason(&e, &ended), Some((DROP_ENDED, vec![])));
    }
}

// ---- the world -------------------------------------------------------------

#[test]
fn join_in_space_answers_zero_and_queues() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    let e = q.of(ALICE).expect("queued");
    assert_eq!((e.match_type_id, e.schedule_id), (MATCH, SCHED));
    assert_eq!((e.system_id, e.ship_id), (HOME, ship(ALICE)));
}

#[test]
fn join_never_spends_key() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    let before = reg.store().get(KEY).cloned();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    assert_eq!(reg.store().get(KEY).cloned(), before);
    assert_eq!(reg.store().get(KEY).unwrap().quantity, 3);
    // Leaving and sweeping it away change nothing either.
    q.remove(ALICE);
    assert_eq!(reg.store().get(KEY).unwrap().quantity, 3);
}

#[test]
fn a_docked_join_is_refused_16_with_the_char() {
    let reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    assert_eq!(
        join(&reg, &mut q, ALICE, KEY, FIL, noon(0)),
        Err(vec![(err::SHIP_IS_DOCKED, vec![ALICE])])
    );
    assert!(q.is_empty());
}

#[test]
fn a_second_join_is_refused_32() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    assert_eq!(
        join(&reg, &mut q, ALICE, KEY, FIL, noon(0)),
        Err(vec![(err::ALREADY_IN_QUEUE, vec![])])
    );
    assert_eq!(q.len(), 1);
}

#[test]
fn a_filament_with_no_active_event_is_refused_33() {
    let mut reg = world(windowed());
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    // Before the window opens, the event is not active.
    assert_eq!(
        join(&reg, &mut q, ALICE, KEY, FIL, noon(2)),
        Err(vec![(err::EVENT_NOT_ACTIVE, vec![])])
    );
    // In the window it is.
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(10)), Ok(0));
}

#[test]
fn a_key_that_is_not_the_pilots_or_not_in_the_ship_is_refused() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    let refused = |reg: &Registry, q: &mut Queue, item: i64, ty: i64| {
        join(reg, q, ALICE, item, ty, noon(0)).unwrap_err()
    };
    assert_eq!(
        refused(&reg, &mut q, 100_882_999, FIL),
        vec![(err::KEY_NOT_FOUND, vec![])]
    );
    assert_eq!(
        refused(&reg, &mut q, HANGAR_KEY, FIL),
        vec![(KEY_NOT_IN_SHIP, vec![FIL])]
    );
    assert_eq!(
        refused(&reg, &mut q, BOBS_KEY, FIL),
        vec![
            (KEY_NOT_IN_SHIP, vec![FIL]),
            (err::INVALID_OWNER, vec![ALICE])
        ]
    );
    assert_eq!(
        codes(&refused(&reg, &mut q, NOT_A_KEY, NOT_A_KEY_TYPE)),
        vec![err::INVALID_KEY_TYPE, err::EVENT_NOT_ACTIVE]
    );
    assert!(q.is_empty());
}

#[test]
fn a_fleet_format_refuses_a_pilot_without_a_fleet() {
    let mut reg = world(qa_catalogue(2, 4));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(
        join(&reg, &mut q, ALICE, KEY, FIL, noon(0)),
        Err(vec![(FLEET_NEEDED, vec![])])
    );
}

#[test]
fn sweep_drops_a_docked_pilot_and_tells_them() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    stand(&mut reg, BOB, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    assert_eq!(join(&reg, &mut q, BOB, BOBS_KEY, FIL, noon(0)), Ok(0));
    assert!(
        sweep(&mut reg, &mut q, noon(0)).is_empty(),
        "nobody has moved"
    );
    assert!(failures(&mut reg).is_empty());
    let _ = reg.roster_mut().leave(ALICE);
    assert_eq!(sweep(&mut reg, &mut q, noon(0)), vec![ALICE]);
    assert_eq!(q.len(), 1);
    assert!(q.of(BOB).is_some(), "the others stay queued");
    let pushed = failures(&mut reg);
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].0, vec![ALICE]);
    assert_eq!(pushed[0].1, errors_value(&[(DROP_DOCKED, vec![ALICE])]));
}

#[test]
fn sweep_drops_a_pilot_who_moved() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    stand(&mut reg, ALICE, AWAY);
    assert_eq!(sweep(&mut reg, &mut q, noon(0)), vec![ALICE]);
    let pushed = failures(&mut reg);
    assert_eq!(pushed[0].1, errors_value(&[(DROP_MOVED, vec![])]));
}

#[test]
fn sweep_drops_a_pilot_whose_event_ended() {
    let mut reg = world(windowed());
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(10)), Ok(0));
    assert!(sweep(&mut reg, &mut q, noon(10)).is_empty(), "still open");
    assert_eq!(sweep(&mut reg, &mut q, noon(11)), vec![ALICE]);
    let pushed = failures(&mut reg);
    assert_eq!(pushed[0].1, errors_value(&[(DROP_ENDED, vec![])]));
    assert_eq!(reg.store().get(KEY).unwrap().quantity, 3);
}

#[test]
fn sweep_drops_a_pilot_whose_key_is_gone() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    // Remove the filament: the pilot no longer holds it.
    let mut items = reg.store().items().to_vec();
    items.retain(|i| i.item_id != KEY);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_883_000,
        ..crate::store::Snapshot::default()
    });
    assert_eq!(sweep(&mut reg, &mut q, noon(0)), vec![ALICE]);
    let pushed = failures(&mut reg);
    assert_eq!(pushed[0].1, errors_value(&[(DROP_MOVED, vec![])]));
}

#[test]
fn sweep_drops_a_pilot_whose_key_left_the_ship() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    // The filament is still hers, but now in a hangar, not in the ship.
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == KEY) {
        i.location_id = HANGAR;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_883_000,
        ..crate::store::Snapshot::default()
    });
    assert_eq!(sweep(&mut reg, &mut q, noon(0)), vec![ALICE]);
    let pushed = failures(&mut reg);
    assert_eq!(pushed[0].1, errors_value(&[(DROP_MOVED, vec![])]));
}

#[test]
fn the_fleet_check_comes_before_the_key_checks() {
    let j = JoinFacts {
        fleet_format: true,
        key: None,
        ..clean_join()
    };
    assert_eq!(
        codes(&join_errors(ALICE, &clean_facts(), &j)),
        vec![FLEET_NEEDED, err::KEY_NOT_FOUND]
    );
}

#[test]
fn a_pilot_between_gates_is_in_a_jump_not_docked() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    // A gate jumper leaves the roster but is not in a station.
    let _ = reg.roster_mut().leave(ALICE);
    crate::jump::arm_at(&mut reg, ALICE, HOME, AWAY, [0.0; 3]);
    let f = facts(&reg, ALICE, noon(0));
    assert!(!f.docked);
    assert!(f.session_change);
    let e = q.of(ALICE).unwrap().clone();
    let now = now_of(&reg, &e, noon(0));
    assert!(!now.docked);
    assert_eq!(drop_reason(&e, &now), Some((DROP_MOVED, vec![])));
    assert_eq!(sweep(&mut reg, &mut q, noon(0)), vec![ALICE]);
}

#[test]
fn the_empty_queue_is_a_no_op() {
    let mut reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    assert!(sweep(&mut reg, &mut q, noon(0)).is_empty());
    assert_eq!(reg.scheduled_len(), 0);
}

// ---- through the slot ------------------------------------------------------
//
// These go through the real registry with `queue::SLOT` in `world_ext::SLOTS`.

fn call(
    reg: &mut Registry,
    who: i64,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    reg.dispatch(&crate::service::Request {
        service: "pvpFilamentMgr",
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

#[test]
fn the_wire_joins_refuses_a_second_and_leaves() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    let args = [PyValue::Int(KEY), PyValue::Int(FIL)];
    assert_eq!(
        call(&mut reg, ALICE, "JoinPVPQueue", &args).unwrap(),
        PyValue::Int(0)
    );
    let Err(ServiceError::Raise(v)) = call(&mut reg, ALICE, "JoinPVPQueue", &args) else {
        panic!("a second join did not raise");
    };
    let ServiceError::Raise(expected) = raise(&[(err::ALREADY_IN_QUEUE, vec![])]) else {
        unreachable!()
    };
    assert_eq!(v, expected);
    assert_eq!(
        call(&mut reg, ALICE, "LeavePVPQueue", &args).unwrap(),
        PyValue::None
    );
    assert_eq!(
        call(&mut reg, ALICE, "LeavePVPQueue", &args).unwrap(),
        PyValue::None,
        "a second leave is quiet"
    );
    assert_eq!(
        call(&mut reg, ALICE, "JoinPVPQueue", &args).unwrap(),
        PyValue::Int(0)
    );
    assert_eq!(reg.store().get(KEY).unwrap().quantity, 3);
}

#[test]
fn the_registered_tick_stage_and_logoff_row_empty_the_queue() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    stand(&mut reg, BOB, HOME);
    for (who, key) in [(ALICE, KEY), (BOB, BOBS_KEY)] {
        let args = [PyValue::Int(key), PyValue::Int(FIL)];
        call(&mut reg, who, "JoinPVPQueue", &args).unwrap();
    }
    assert_eq!(reg.ext::<Queue>().len(), 2);
    // The real tables: the rows must exist, or the queue is never swept.
    let stage = crate::tick::STAGES
        .iter()
        .find(|s| s.name == "proving_queue")
        .expect("tick::STAGES has a proving_queue row");
    let row = crate::session_hooks::ON_LOGOFF
        .iter()
        .find(|r| r.name == "proving_queue")
        .expect("session_hooks::ON_LOGOFF has a proving_queue row");
    (stage.run)(&mut reg, Instant::now(), noon(0));
    assert_eq!(reg.ext::<Queue>().len(), 2, "nobody moved");
    let _ = reg.roster_mut().leave(ALICE);
    (stage.run)(&mut reg, Instant::now(), noon(0));
    assert_eq!(reg.ext::<Queue>().len(), 1);
    (row.run)(&mut reg, BOB);
    assert!(reg.ext::<Queue>().is_empty());
    assert_eq!(failures(&mut reg).len(), 1, "only the docked pilot is told");
}

// ---- the match (PG-2a) ------------------------------------------------------

thread_local! {
    /// The teams `fake_open` was asked to open, as characters.
    static OPENED: std::cell::RefCell<Vec<Vec<Vec<i64>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static LAUNCHED: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

const ARENA: u64 = 7;

fn fake_open(_: &mut Registry, _: &Event, t: &[Team], _: i64) -> Result<u64, String> {
    OPENED.with(|o| {
        o.borrow_mut().push(
            t.iter()
                .map(|t| t.members.iter().map(|e| e.char_id).collect())
                .collect(),
        )
    });
    Ok(ARENA)
}

fn fake_launch(_: &mut Registry, id: u64, _: i64) {
    LAUNCHED.with(|l| l.borrow_mut().push(id));
}

fn no_room(_: &mut Registry, _: &Event, _: &[Team], _: i64) -> Result<u64, String> {
    Err("no free system".to_owned())
}

fn opened() -> Vec<Vec<Vec<i64>>> {
    OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

fn launched() -> Vec<u64> {
    LAUNCHED.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn join_all(reg: &mut Registry, q: &mut Queue, who: &[(i64, i64)], at: i64) {
    for (i, (c, key)) in who.iter().enumerate() {
        stand(reg, *c, HOME);
        join(reg, q, *c, *key, FIL, at + i as i64).expect("the join is accepted");
    }
}

fn four(reg: &mut Registry, q: &mut Queue) {
    join_all(
        reg,
        q,
        &[
            (ALICE, KEY),
            (BOB, BOBS_KEY),
            (CAROL, CAROLS_KEY),
            (DAVE, DAVES_KEY),
        ],
        noon(0),
    );
}

/// Everything pushed: what `push` queued, then what `schedule` did.
fn sent(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>)> {
    let mut all: Vec<Box<dyn crate::notify::Push>> =
        reg.take_pending().into_iter().map(|(_, p)| p).collect();
    all.extend(reg.take_due(Instant::now() + Duration::from_secs(60)));
    all.iter()
        .map(|p| (p.to_push().0, p.audience().unwrap()))
        .collect()
}

fn make_fleet(reg: &mut Registry, boss: i64, others: &[i64]) -> i64 {
    let id = reg.fleets_mut().create(boss).expect("created");
    reg.fleets_mut()
        .init(boss, id, Some(NOT_A_KEY_TYPE), None, None)
        .expect("init");
    for who in others {
        reg.fleets_mut()
            .invite(boss, id, *who, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(*who, id, Some(NOT_A_KEY_TYPE), None, None)
            .expect("accepted");
    }
    id
}

/// Four pilots, a keyed filament each, in a world of one QA event of
/// `format` over `mt` and the given typelist table.
fn custom_world(mt: MatchType, format: &str, table: crate::dungeon_data::Table) -> Registry {
    let text = format!(
        "{}{SCHED}\t{format}\t\t\t\t1\ttest\n",
        super::super::tests::header_for_queue()
    );
    let mut content = crate::content::Content::fixture();
    content.insert(
        "proving_events",
        Catalogue::parse(&text, vec![mt]).expect("the rows parse"),
    );
    content.insert("dungeons", table);
    let mut reg = Registry::new(SandboxConfig {
        characters: [ALICE, BOB, CAROL, DAVE]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: ship(id),
                solar_system_id: Some(HOME),
                ..Character::default()
            })
            .collect(),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    let mut items = reg.store().items().to_vec();
    for (id, key) in [
        (ALICE, KEY),
        (BOB, BOBS_KEY),
        (CAROL, CAROLS_KEY),
        (DAVE, DAVES_KEY),
    ] {
        items.push(cargo(key, id, ship(id), FIL, FILAMENT_GROUP, 1));
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_883_000,
        ..crate::store::Snapshot::default()
    });
    reg
}

fn set_hull(reg: &mut Registry, who: i64, type_id: i64, group: i64) {
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
        i.type_id = type_id;
        i.group_id = group;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_883_000,
        ..crate::store::Snapshot::default()
    });
}

#[test]
fn a_formed_match_takes_one_key_each_at_the_match_and_never_at_the_join() {
    let stack = |reg: &Registry, id: i64| reg.store().get(id).map(|i| i.quantity);
    let mut reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    join_all(&mut reg, &mut q, &[(ALICE, KEY), (BOB, BOBS_KEY)], noon(0));
    assert_eq!(stack(&reg, KEY), Some(3), "the join spends nothing");
    assert_eq!(stack(&reg, BOBS_KEY), Some(1));
    let _ = (opened(), launched(), sent(&mut reg));
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert_eq!(opened(), vec![vec![vec![ALICE], vec![BOB]]]);
    assert_eq!(launched(), vec![ARENA]);
    assert_eq!(
        stack(&reg, KEY),
        Some(2),
        "a stack of three is reduced by one"
    );
    assert_eq!(stack(&reg, BOBS_KEY), None, "a stack of one is destroyed");
    assert!(q.is_empty(), "both leave the queue");
    let told: Vec<_> = sent(&mut reg)
        .into_iter()
        .filter(|(n, _)| n.contains("Match"))
        .collect();
    assert_eq!(
        told,
        vec![
            ("OnMatchFound", vec![ALICE]),
            ("OnProvingGroundsMatchFound", vec![ALICE]),
            ("OnMatchFound", vec![BOB]),
            ("OnProvingGroundsMatchFound", vec![BOB]),
        ]
    );
}

#[test]
fn a_match_that_cannot_open_keeps_everyone_queued_with_their_keys() {
    let mut reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    join_all(&mut reg, &mut q, &[(ALICE, KEY), (BOB, BOBS_KEY)], noon(0));
    let _ = (sent(&mut reg), launched());
    match_round(&mut reg, &mut q, noon(0), no_room, fake_launch);
    assert_eq!(q.len(), 2);
    assert_eq!(reg.store().get(KEY).map(|i| i.quantity), Some(3));
    assert!(reg.store().get(BOBS_KEY).is_some());
    assert!(sent(&mut reg).is_empty(), "nobody is told");
    assert!(launched().is_empty());
}

#[test]
fn one_pilot_alone_forms_no_match() {
    let mut reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    join_all(&mut reg, &mut q, &[(ALICE, KEY)], noon(0));
    let _ = opened();
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert!(opened().is_empty());
    assert_eq!(q.len(), 1);
}

#[test]
fn a_match_time_failure_drops_that_pilot_only() {
    let mut reg = world(qa_catalogue(2, 1));
    let mut q = Queue::default();
    join_all(&mut reg, &mut q, &[(ALICE, KEY), (BOB, BOBS_KEY)], noon(0));
    // Alice changed ship after queueing.
    reg.character_mut(ALICE).unwrap().ship_id = ship(BOB);
    let _ = (opened(), failures(&mut reg));
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert!(opened().is_empty(), "no match without her");
    assert!(q.of(ALICE).is_none());
    assert!(q.of(BOB).is_some(), "the other pilot waits on");
    let f = failures(&mut reg);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].0, vec![ALICE]);
    assert_eq!(f[0].1, errors_value(&[(DROP_MOVED, vec![])]));
    assert_eq!(
        reg.store().get(KEY).map(|i| i.quantity),
        Some(3),
        "nothing was paid"
    );
}

#[test]
fn a_teammate_of_a_failed_pilot_is_told_the_team_broke_and_the_other_team_waits() {
    let mut reg = world(qa_catalogue(2, 2));
    let mut q = Queue::default();
    make_fleet(&mut reg, ALICE, &[BOB]);
    make_fleet(&mut reg, CAROL, &[DAVE]);
    four(&mut reg, &mut q);
    assert_eq!(q.len(), 4);
    // Bob docks.
    let _ = reg.roster_mut().leave(BOB);
    let _ = (opened(), failures(&mut reg));
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert!(opened().is_empty());
    let mut f = failures(&mut reg);
    f.sort_by_key(|(to, _)| to[0]);
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].0, vec![ALICE]);
    assert_eq!(
        f[0].1,
        errors_value(&[(FLEET_SIZE_CHANGED, vec![])]),
        "the teammate"
    );
    assert_eq!(f[1].0, vec![BOB]);
    assert_eq!(
        f[1].1,
        errors_value(&[(DROP_DOCKED, vec![BOB])]),
        "the failing pilot"
    );
    assert_eq!(q.len(), 2, "the other fleet stays");
    assert!(q.of(CAROL).is_some() && q.of(DAVE).is_some());
}

#[test]
fn a_full_2x2_forms_with_both_fleets_as_teams() {
    let mut reg = world(qa_catalogue(2, 2));
    let mut q = Queue::default();
    make_fleet(&mut reg, ALICE, &[BOB]);
    make_fleet(&mut reg, CAROL, &[DAVE]);
    four(&mut reg, &mut q);
    let _ = opened();
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert_eq!(opened(), vec![vec![vec![ALICE, BOB], vec![CAROL, DAVE]]]);
    assert!(q.is_empty());
}

#[test]
fn a_fleet_change_drops_with_31_in_a_fleet_format_only() {
    let e = Entry {
        fleet_id: 7,
        ..entry(ALICE)
    };
    let ok = Now {
        system_id: HOME,
        ship_id: ship(ALICE),
        event: Some((MATCH, SCHED)),
        key_held: true,
        fleet_id: 7,
        fleet_format: true,
        docked: false,
    };
    assert_eq!(drop_reason(&e, &ok), None);
    for fleet_id in [0, 8] {
        let changed = Now {
            fleet_id,
            ..ok.clone()
        };
        assert_eq!(
            drop_reason(&e, &changed),
            Some((FLEET_SIZE_CHANGED, vec![])),
            "fleet {fleet_id}"
        );
    }
    let solo = Now {
        fleet_id: 0,
        fleet_format: false,
        ..ok
    };
    assert_eq!(drop_reason(&e, &solo), None, "a solo format ignores fleets");
}

#[test]
fn the_sweep_drops_a_pilot_who_left_the_fleet_they_queued_in() {
    let mut reg = world(qa_catalogue(2, 2));
    let mut q = Queue::default();
    let fleet = make_fleet(&mut reg, ALICE, &[BOB]);
    join_all(&mut reg, &mut q, &[(ALICE, KEY), (BOB, BOBS_KEY)], noon(0));
    assert_eq!(q.of(BOB).map(|e| e.fleet_id), Some(fleet));
    reg.fleets_mut().leave(BOB, fleet).expect("left");
    let _ = failures(&mut reg);
    let gone = sweep(&mut reg, &mut q, noon(0));
    assert_eq!(gone, vec![BOB]);
    assert_eq!(
        failures(&mut reg)[0].1,
        errors_value(&[(FLEET_SIZE_CHANGED, vec![])])
    );
}

#[test]
fn a_fleet_larger_than_the_format_is_refused_with_its_size() {
    let j = JoinFacts {
        fleet_format: true,
        fleet_over: Some(2),
        ..clean_join()
    };
    let f = Facts {
        in_fleet: true,
        ..clean_facts()
    };
    assert_eq!(join_errors(ALICE, &f, &j), vec![(FLEET_TOO_BIG, vec![2])]);
    // And through the world: three pilots in a fleet for a 2x2.
    let mut reg = world(qa_catalogue(2, 2));
    make_fleet(&mut reg, ALICE, &[BOB, CAROL]);
    stand(&mut reg, ALICE, HOME);
    let mut q = Queue::default();
    assert_eq!(
        join(&reg, &mut q, ALICE, KEY, FIL, noon(0)),
        Err(vec![(FLEET_TOO_BIG, vec![2])])
    );
}

#[test]
fn a_hull_off_the_list_is_refused_with_the_pilot_at_join_and_at_match() {
    const LIST: i64 = 990_903;
    const GOOD: i64 = 990_904;
    const BAD: i64 = 990_905;
    let j = JoinFacts {
        ship_allowed: false,
        ..clean_join()
    };
    assert_eq!(
        join_errors(ALICE, &clean_facts(), &j),
        vec![(INVALID_SHIP_TYPE, vec![ALICE])]
    );
    let mut table = crate::dungeon_data::Table::default();
    table.typelists.insert(
        LIST,
        crate::dungeon_data::TypeList {
            inc_types: vec![GOOD],
            ..Default::default()
        },
    );
    let mt = MatchType {
        ship_restrictions: Some(LIST),
        ..match_type(2, 1)
    };
    let mut reg = custom_world(mt, "2x1", table);
    set_hull(&mut reg, ALICE, GOOD, 0);
    set_hull(&mut reg, BOB, BAD, 0);
    stand(&mut reg, ALICE, HOME);
    stand(&mut reg, BOB, HOME);
    let mut q = Queue::default();
    assert_eq!(
        join(&reg, &mut q, BOB, BOBS_KEY, FIL, noon(0)),
        Err(vec![(INVALID_SHIP_TYPE, vec![BOB])]),
        "at the join"
    );
    assert_eq!(join(&reg, &mut q, ALICE, KEY, FIL, noon(0)), Ok(0));
    // A modified client that skipped the join's check is caught at the match.
    q.add(Entry {
        item_id: BOBS_KEY,
        since: noon(0) + 1,
        ..entry(BOB)
    });
    let _ = (opened(), failures(&mut reg));
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert!(opened().is_empty());
    let f = failures(&mut reg);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].0, vec![BOB]);
    assert_eq!(f[0].1, errors_value(&[(INVALID_SHIP_TYPE, vec![BOB])]));
    assert!(q.of(ALICE).is_some());
}

#[test]
fn a_team_over_the_composition_cap_fails_as_a_team() {
    const SAME: i64 = 990_906;
    const OTHER_A: i64 = 990_907;
    const OTHER_B: i64 = 990_908;
    let mt = MatchType {
        max_of_ship_type_restriction: Some(1),
        ..match_type(2, 2)
    };
    let mut reg = custom_world(mt, "2x2", crate::dungeon_data::Table::default());
    // Alice's and Bob's hulls are one type; Carol's and Dave's differ.
    for (who, t) in [
        (ALICE, SAME),
        (BOB, SAME),
        (CAROL, OTHER_A),
        (DAVE, OTHER_B),
    ] {
        set_hull(&mut reg, who, t, 0);
    }
    make_fleet(&mut reg, ALICE, &[BOB]);
    make_fleet(&mut reg, CAROL, &[DAVE]);
    let mut q = Queue::default();
    four(&mut reg, &mut q);
    let _ = (opened(), failures(&mut reg));
    match_round(&mut reg, &mut q, noon(0), fake_open, fake_launch);
    assert!(opened().is_empty());
    let mut f = failures(&mut reg);
    f.sort_by_key(|(to, _)| to[0]);
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].0, vec![ALICE]);
    assert_eq!(f[1].0, vec![BOB]);
    for (_, errors) in &f {
        assert_eq!(*errors, errors_value(&[(36, vec![1, SAME])]));
    }
    assert_eq!(q.len(), 2, "the other team waits");
}
