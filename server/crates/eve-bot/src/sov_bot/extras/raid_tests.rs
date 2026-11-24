//! Tests of the raiders (#221 B2): the planner's raid candidate, the guards,
//! the link gate, the disposal and the decoders. Ids are invented; no `[A]`
//! constant appears as an expected value, only by name.

use super::gw::{put_bytes, put_uint};
use super::raid::*;
use crate::sov_bot::{
    ATTACK_R, Duty, Facts, Kind, Kit, LEAD_SECS, Phase, Plan, RAID_MIN_LEFT, RaidView, TICKS,
    assign, plan,
};
use crate::verbs::ItemRow;
use eve_server::sov_resources::GROUP_COLONY_REAGENTS;

const ME: i64 = 99_221_001;
const FOE: i64 = 99_221_002;
const BLUE: i64 = 99_221_003;
const MATE: i64 = 99_221_004;
const NOW: i64 = 1_000_000_000_000;
const SYSTEM: i64 = 30_221_001;
const SKYHOOK: i64 = 8_221_001;
const SKYHOOK_B: i64 = 8_221_002;

fn near(_: i64) -> Option<u32> {
    Some(1)
}

fn rel(a: i64) -> f64 {
    match a {
        FOE => -0.9,
        BLUE => 0.8,
        _ => 0.0,
    }
}

fn never(_: i64) -> bool {
    false
}

fn mate(a: i64) -> bool {
    a == MATE
}

fn raid(owner: Option<i64>, opens: i64, closes: i64) -> RaidView {
    RaidView {
        system: SYSTEM,
        skyhook: SKYHOOK,
        owner,
        opens,
        closes,
    }
}

fn open_for(secs: i64) -> (i64, i64) {
    (NOW - 60 * TICKS, NOW + secs * TICKS)
}

fn pick(raids: &[RaidView], alarms: &[(i64, i64)]) -> Option<crate::sov_bot::Target> {
    plan(&Facts {
        me: ME,
        hubs: &[],
        jumps: &near,
        relation: &rel,
        bloc_enemy: &never,
        bloc_mate: &mate,
        now: NOW,
        linkers: 0,
        previous: None,
        raids,
        alarms,
    })
}

#[test]
fn an_open_window_of_an_enemy_is_a_raid() {
    let (o, c) = open_for(2 * RAID_MIN_LEFT);
    let t = pick(&[raid(Some(FOE), o, c)], &[]).expect("a target");
    assert_eq!(t.kind, Kind::Raid);
    assert_eq!((t.system, t.hub_id, t.holder), (SYSTEM, SKYHOOK, Some(FOE)));
}

#[test]
fn a_closed_window_is_never_a_target() {
    // Ended a minute ago.
    assert!(
        pick(
            &[raid(Some(FOE), NOW - 7_200 * TICKS, NOW - 60 * TICKS)],
            &[]
        )
        .is_none()
    );
    // Open, but too little of it left for a link.
    let (o, c) = open_for(RAID_MIN_LEFT / 2);
    assert!(pick(&[raid(Some(FOE), o, c)], &[]).is_none());
    // Opens later than the lead time.
    let opens = NOW + (LEAD_SECS + 600) * TICKS;
    assert!(pick(&[raid(Some(FOE), opens, opens + 7_200 * TICKS)], &[]).is_none());
    // Opens within the lead time: waited for at the skyhook.
    let opens = NOW + (LEAD_SECS / 2) * TICKS;
    let t = pick(&[raid(Some(FOE), opens, opens + 7_200 * TICKS)], &[]).expect("due");
    assert!(matches!(t.phase, Phase::Shut { .. }));
}

#[test]
fn a_blue_an_own_a_mates_and_an_unowned_skyhook_is_never_a_target() {
    let (o, c) = open_for(2 * RAID_MIN_LEFT);
    for owner in [Some(BLUE), Some(ME), Some(MATE), None, Some(99_221_099)] {
        assert!(
            pick(&[raid(owner, o, c)], &[]).is_none(),
            "owner {owner:?} was raided"
        );
    }
    assert!(rel(BLUE) > ATTACK_R);
}

#[test]
fn an_alliance_the_bloc_named_an_enemy_is_raided_whatever_its_relation() {
    let (o, c) = open_for(2 * RAID_MIN_LEFT);
    let enemy = |a: i64| a == 99_221_099;
    let raids = [raid(Some(99_221_099), o, c)];
    let f = Facts {
        me: ME,
        hubs: &[],
        jumps: &near,
        relation: &rel,
        bloc_enemy: &enemy,
        bloc_mate: &never,
        now: NOW,
        linkers: 0,
        previous: None,
        raids: &raids,
        alarms: &[],
    };
    assert_eq!(plan(&f).map(|t| t.kind), Some(Kind::Raid));
}

#[test]
fn a_raid_plan_carries_a_guard_when_guards_exist_and_no_linker() {
    let (o, c) = open_for(2 * RAID_MIN_LEFT);
    let target = pick(&[raid(Some(FOE), o, c)], &[]).expect("a raid");
    let plan = Plan {
        target: Some(target),
        fuel: Vec::new(),
    };
    let roster = [
        (
            1,
            Kit {
                link: false,
                guns: true,
                hauler: false,
            },
        ),
        (
            2,
            Kit {
                link: true,
                guns: false,
                hauler: false,
            },
        ),
        (
            3,
            Kit {
                link: false,
                guns: false,
                hauler: true,
            },
        ),
    ]
    .into_iter()
    .collect();
    let duties = assign(&plan, &roster);
    assert!(duties.values().any(|d| *d == Duty::Guard), "no guard");
    assert!(
        !duties.values().any(|d| *d == Duty::Entosis),
        "a raid links no hub"
    );
    // The pilot with a link raids: it is left idle.
    assert_eq!(duties[&2], Duty::Idle);
}

#[test]
fn a_running_link_on_an_own_silo_is_defended_before_any_raid() {
    let (o, c) = open_for(2 * RAID_MIN_LEFT);
    let t = pick(&[raid(Some(FOE), o, c)], &[(SYSTEM, SKYHOOK_B)]).expect("a target");
    assert_eq!((t.kind, t.hub_id), (Kind::Rescue, SKYHOOK_B));
    assert!(!t.kind.takes_links());
}

#[test]
fn only_a_strangers_running_link_raises_the_alarm() {
    let running = Some((STATE_RUNNING, Some(7_001)));
    assert!(is_theft(running, Some(FOE), ME));
    assert!(is_theft(running, None, ME), "an unseen ship is a stranger");
    assert!(!is_theft(running, Some(ME), ME), "our own secure link");
    assert!(!is_theft(Some((STATE_IDLE, None)), None, ME));
    assert!(!is_theft(Some((STATE_COMPLETED, Some(7_001))), None, ME));
    assert!(!is_theft(None, None, ME));
}

fn view() -> LinkView {
    LinkView {
        hull_ok: true,
        gap: Some(0.0),
        silo: Some(STATE_IDLE),
        hold_free: HOLD_MIN_M3,
    }
}

#[test]
fn the_link_is_called_only_in_range_in_an_allowed_hull_on_an_idle_silo() {
    assert!(may_link(&view()));
    assert!(!may_link(&LinkView {
        hull_ok: false,
        ..view()
    }));
    assert!(!may_link(&LinkView {
        gap: Some(eve_server::skyhook_theft::MAX_LINK_RANGE + 1.0),
        ..view()
    }));
    assert!(!may_link(&LinkView {
        gap: None,
        ..view()
    }));
    assert!(!may_link(&LinkView {
        silo: None,
        ..view()
    }));
    assert!(!may_link(&LinkView {
        silo: Some(STATE_RUNNING),
        ..view()
    }));
    assert!(!may_link(&LinkView {
        hold_free: HOLD_MIN_M3 - 1.0,
        ..view()
    }));
}

#[test]
fn only_a_frigate_class_hull_raids() {
    for g in RAIDER_GROUPS {
        assert!(hull_allowed(g));
    }
    // An invented group that is no frigate's.
    assert!(!hull_allowed(990_221_900));
}

fn row(group: i64, qty: i64) -> ItemRow {
    ItemRow {
        item_id: 1,
        type_id: 990_221_101,
        owner_id: 0,
        location_id: 0,
        flag_id: 5,
        quantity: qty,
        group_id: group,
        category_id: 0,
        singleton: false,
    }
}

#[test]
fn the_hold_gains_only_what_the_link_delivered() {
    let before = [row(GROUP_COLONY_REAGENTS, 5), row(990_221_902, 40)];
    let after = [row(GROUP_COLONY_REAGENTS, 5), row(990_221_902, 40)];
    // A link that moved nothing leaves nothing to dispose of.
    let delivered = reagent_units(&after) - reagent_units(&before);
    assert_eq!(delivered, 0);
    assert_eq!(
        disposal(delivered, &[(SYSTEM, 8_221_900)]),
        Disposal::Nothing
    );
    // Other cargo never counts.
    assert_eq!(reagent_units(&[row(990_221_902, 99)]), 0);
    // What was delivered goes to an own hub, else to the market.
    let after = [row(GROUP_COLONY_REAGENTS, 35)];
    let delivered = reagent_units(&after) - reagent_units(&before);
    assert_eq!(delivered, 30);
    assert_eq!(
        disposal(delivered, &[(SYSTEM, 8_221_900)]),
        Disposal::Deposit(SYSTEM, 8_221_900)
    );
    assert_eq!(disposal(delivered, &[]), Disposal::Sell);
}

#[test]
fn a_link_is_over_unless_it_runs() {
    assert!(!link_over(Some(STATE_RUNNING)));
    assert!(link_over(Some(STATE_COMPLETED)));
    assert!(link_over(Some(STATE_IDLE)));
    assert!(link_over(None));
}

fn id_msg(n: u64) -> Vec<u8> {
    let mut m = Vec::new();
    put_uint(&mut m, 1, n);
    m
}

#[test]
fn the_map_filter_answers_decode() {
    let mut systems = Vec::new();
    put_bytes(&mut systems, 1, &id_msg(SYSTEM as u64));
    put_bytes(&mut systems, 1, &id_msg(30_221_002));
    assert_eq!(decode_systems(&systems), vec![SYSTEM, 30_221_002]);

    let mut entry = Vec::new();
    put_bytes(&mut entry, 1, &id_msg(SKYHOOK as u64));
    put_bytes(&mut entry, 2, &id_msg(40_221_001));
    put_bytes(&mut entry, 3, &id_msg(2_000));
    put_bytes(&mut entry, 4, &id_msg(1_000));
    let mut payload = Vec::new();
    put_bytes(&mut payload, 1, &entry);
    assert_eq!(decode_windows(&payload), vec![(SKYHOOK, 1_000, 2_000)]);
    // Truncation never panics.
    for n in 0..payload.len() {
        let _ = decode_windows(&payload[..n]);
    }
}
