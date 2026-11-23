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
use crate::proving::MatchType;
use crate::space_state::{Motion, Presence};

const ALICE: i64 = 90_882_001;
const BOB: i64 = 90_882_002;
const HOME: i64 = 30_882_001;
const AWAY: i64 = 30_882_002;
const FIL: i64 = 990_882_001;
const OTHER_FIL: i64 = 990_882_002;
const SCHED: i64 = 882_001;
const MATCH: i64 = 883_001;
const KEY: i64 = 100_882_001;
const HANGAR_KEY: i64 = 100_882_002;
const BOBS_KEY: i64 = 100_882_003;
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
        characters: [ALICE, BOB]
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
fn the_tick_and_the_logoff_hook_empty_the_queue() {
    let mut reg = world(qa_catalogue(2, 1));
    stand(&mut reg, ALICE, HOME);
    stand(&mut reg, BOB, HOME);
    for (who, key) in [(ALICE, KEY), (BOB, BOBS_KEY)] {
        let args = [PyValue::Int(key), PyValue::Int(FIL)];
        call(&mut reg, who, "JoinPVPQueue", &args).unwrap();
    }
    assert_eq!(reg.ext::<Queue>().len(), 2);
    tick(&mut reg, Instant::now(), noon(0));
    assert_eq!(reg.ext::<Queue>().len(), 2, "nobody moved");
    let _ = reg.roster_mut().leave(ALICE);
    tick(&mut reg, Instant::now(), noon(0));
    assert_eq!(reg.ext::<Queue>().len(), 1);
    on_logoff(&mut reg, BOB);
    assert!(reg.ext::<Queue>().is_empty());
    assert_eq!(failures(&mut reg).len(), 1, "only the docked pilot is told");
}
