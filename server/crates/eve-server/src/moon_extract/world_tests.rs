//! #53 MOON-SCHEDULE: the world part of [`super`], through the real docking,
//! controls, fitting and wire. Every id and every attribute value is invented;
//! the client's constants appear through the named consts of the modules.
//! The deadlines, the yield and the volumes are compared with what the module's
//! own pure functions say of the row, never with a number. The few checks that
//! pin an `[A]` of the module (who is mailed, which ends mail) say `PINNED [A]`
//! where they are, and name what would falsify the rule.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::notifications as notif;
use crate::pyutil::dict_get;
use crate::service::Request;
use crate::space_state::{Motion, Presence};
use crate::structure::{State, Structure};
use crate::structure_services::{
    ATTR_SERVICE_SLOTS, CATEGORY_STRUCTURE_MODULE, EFFECT_SERVICE_SLOT,
};

const BOSS: i64 = 7;
const PAL: i64 = 8;
const FOE: i64 = 9;
const CORP: i64 = 11;
const FOE_CORP: i64 = 13;
/// An invented-id system of `Universe::fixture`: a null-sec one, which has a
/// moon makeup.
const SYSTEM: i64 = 39_000_003;
const HARBOUR: i64 = 6_001;
const SID: i64 = 9_530_100;
/// Invented hulls: a drilling platform (the client's own group id, named) and
/// one that is not.
const HULL: i64 = 9_530_001;
const OTHER_HULL: i64 = 9_530_002;
const OTHER_GROUP: i64 = 9_530_050;
const MODULE_GROUP: i64 = 9_530_060;
const MOON: i64 = 9_530_201;
/// The invented hull's automatic fracture delay (seconds) and yield multiplier.
const HULL_AUTO_S: f64 = 7_200.0;
const HULL_YIELD: f64 = 1.5;
/// Where the invented beacon is, in metres from the structure at the origin.
const BEACON_AT: f64 = 100_000.0;
const MOON_AT: f64 = 4.0e7;

fn content(beacon: bool) -> crate::content::Content {
    let hull_group = crate::structure::GROUP_STRUCTURE_DRILLING_PLATFORM;
    let mut c = crate::content::Content::fixture();
    let mut rows = format!(
        "{}{}",
        crate::typedata::FIXTURE,
        crate::moon::tests::fixture_text()
    );
    rows.push_str(&format!(
        "type\t{HULL}\t{hull_group}\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t{ATTR_SERVICE_SLOTS}=3.0;{ATTR_AUTO_FRACTURE_DELAY}={HULL_AUTO_S};{ATTR_YIELD_MULTIPLIER}={HULL_YIELD}\tInvented Hull\n\
         type\t{OTHER_HULL}\t{OTHER_GROUP}\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t{ATTR_SERVICE_SLOTS}=3.0\tInvented Other Hull\n\
         type\t{TYPE_STANDUP_MOON_DRILL}\t{MODULE_GROUP}\t{CATEGORY_STRUCTURE_MODULE}\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t{EFFECT_SERVICE_SLOT}\t\tInvented Drill\n"
    ));
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    if beacon {
        let beacons = crate::moon_beacon::Beacons::parse(&[format!(
            "{MOON}\t{SYSTEM}\t{BEACON_AT}\t0.0\t0.0\t0"
        )])
        .expect("parses");
        c.insert("moon_beacons", beacons);
    }
    c
}

/// Three pilots docked at an NPC station of the system (two of one corporation,
/// the boss a director; one of another), a moon with a beacon, and a drilling
/// structure of the corporation at the origin.
fn world_of(hull: i64, beacon: bool) -> Registry {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
        name: format!("Pilot {id}"),
        corporation_id: corp,
        solar_system_id: Some(SYSTEM),
        station_id: Some(HARBOUR),
        ship_id: 1_000 + id,
        ship_type_id: crate::service::TYPE_CAPSULE,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![ch(BOSS, CORP), ch(PAL, CORP), ch(FOE, FOE_CORP)],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
             moon\t{MOON}\t990021\t1500000.0\t{MOON_AT}\t0.0\t0.0\t{SYSTEM}\n"
        )),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content(beacon)));
    reg.corporations_mut()
        .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    reg.corporations_mut().enrol_for_test(PAL, CORP, 0);
    reg.corporations_mut().enrol_for_test(FOE, FOE_CORP, 0);
    for (n, who) in [BOSS, PAL, FOE].into_iter().enumerate() {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            1_000 + who,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    let profile = crate::structure_profile::ensure_default(&mut reg, CORP);
    let e = structure::Env::new(crate::pyutil::filetime_now());
    let mut s = Structure::anchored(
        SID, hull, CORP, SYSTEM, [0.0; 3], 0.0, "Invented", profile, &e,
    );
    s.set_state(State::ShieldVulnerable, &e);
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_pending();
    reg
}

fn world() -> Registry {
    world_of(HULL, true)
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

fn dock_and_control(reg: &mut Registry, who: i64) {
    let ship = reg.character(who).expect("pilot").ship_id;
    call(
        reg,
        who,
        "structureDocking",
        "Dock",
        &[PyValue::Int(SID), PyValue::Int(ship)],
    )
    .expect("docks");
    call(
        reg,
        who,
        "structureControl",
        "TakeControl",
        &[PyValue::Int(SID)],
    )
    .expect("takes the controls");
}

/// The drill fitted and online.
fn fit_drill(reg: &mut Registry) -> i64 {
    let line = svc::gm_fit(reg, BOSS, SID, TYPE_STANDUP_MOON_DRILL, 0);
    assert!(line.ends_with(", online"), "{line}");
    svc::fitted(reg, &structure::get(reg.deltas(), SID).expect("structure"))
        .first()
        .expect("fitted")
        .item_id
}

/// BOSS docked, at the controls, with the drill online.
fn ready() -> (Registry, i64) {
    let mut reg = world();
    let module = fit_drill(&mut reg);
    dock_and_control(&mut reg, BOSS);
    drain(&mut reg);
    (reg, module)
}

/// Everything pushed since the last call, by name and arguments.
fn drain(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
    let mut out: Vec<(&'static str, Vec<PyValue>)> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push())
        .collect();
    out.extend(
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| p.to_push()),
    );
    out
}

/// The `OnMoonExtractionUpdate` arguments among `pushes`.
fn updates(pushes: &[(&'static str, Vec<PyValue>)]) -> Vec<PyValue> {
    pushes
        .iter()
        .filter(|(n, _)| *n == "OnMoonExtractionUpdate")
        .map(|(_, a)| a[0].clone())
        .collect()
}

fn mails(reg: &Registry, who: i64, type_id: i64) -> Vec<notif::Notification> {
    notif::of(reg.deltas(), who)
        .into_iter()
        .filter(|n| n.type_id == type_id)
        .collect()
}

fn field<'a>(n: &'a notif::Notification, name: &str) -> &'a Value {
    &n.data
        .iter()
        .find(|(k, _)| k == name)
        .unwrap_or_else(|| panic!("{name} missing"))
        .1
}

fn now() -> i64 {
    crate::pyutil::filetime_now()
}

const LONG: i64 = MINIMUM_EXTRACTION_DURATION + 3_600;

fn start_it(reg: &mut Registry) -> Extraction {
    start(reg, BOSS, SID, LONG, true, now()).expect("starts")
}

// -- the refusals ------------------------------------------------------------------------

#[test]
fn start_refuses_without_control() {
    let (mut reg, _) = ready();
    // PAL is not at the controls; FOE is not in the corporation.
    for who in [PAL, FOE] {
        assert_eq!(
            start(&mut reg, who, SID, LONG, true, now()),
            Err(Denied::NotControlling)
        );
    }
    // Nor does the boss, asking about a structure that is not the one he controls.
    assert_eq!(
        start(&mut reg, BOSS, SID + 1, LONG, true, now()),
        Err(Denied::NotControlling)
    );
    assert!(all(reg.deltas()).is_empty(), "nothing was written");
}

#[test]
fn start_refuses_a_structure_that_is_not_a_drilling_platform() {
    let mut reg = world_of(OTHER_HULL, true);
    // A hull that is no drilling platform has no drill to online, so seat
    // the pilot without one: the group check comes first.
    dock_and_control(&mut reg, BOSS);
    assert!(!crate::structure::is_drilling_platform(OTHER_GROUP));
    assert_eq!(
        start(&mut reg, BOSS, SID, LONG, true, now()),
        Err(Denied::NotDrillingPlatform)
    );
}

#[test]
fn start_refuses_with_the_drill_offline() {
    let mut reg = world();
    dock_and_control(&mut reg, BOSS);
    // No drill fitted at all.
    assert_eq!(
        start(&mut reg, BOSS, SID, LONG, true, now()),
        Err(Denied::DrillOffline)
    );
    // Fitted, then taken offline.
    let module = fit_drill(&mut reg);
    svc::set_online(&mut reg, BOSS, SID, module, false, now()).expect("offline");
    assert!(!svc::online_with(&reg, SID, SERVICE_MOONMINING));
    assert_eq!(
        start(&mut reg, BOSS, SID, LONG, true, now()),
        Err(Denied::DrillOffline)
    );
    svc::set_online(&mut reg, BOSS, SID, module, true, now()).expect("online");
    assert!(svc::online_with(&reg, SID, SERVICE_MOONMINING));
    assert!(start(&mut reg, BOSS, SID, LONG, true, now()).is_ok());
}

#[test]
fn start_refuses_far_from_a_beacon() {
    let (mut reg, _) = ready();
    // The drill went online by the beacon; the structure is then moved out of
    // its reach (the beacon check at start reads the structure's position now).
    let mut s = structure::get(reg.deltas(), SID).expect("structure");
    s.pos = [MAXIMUM_MINING_BEACON_DISTANCE as f64 * 2.0, 0.0, 0.0];
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(
        start(&mut reg, BOSS, SID, LONG, true, now()),
        Err(Denied::NoBeacon)
    );
}

#[test]
fn start_refuses_a_second_extraction() {
    let (mut reg, _) = ready();
    let first = start_it(&mut reg);
    assert_eq!(
        start(&mut reg, BOSS, SID, LONG, true, now()),
        Err(Denied::Busy)
    );
    assert_eq!(all(reg.deltas()), vec![first], "the first is untouched");
}

#[test]
fn start_refuses_a_duration_the_pure_rule_refuses() {
    let (mut reg, _) = ready();
    let too_short = MINIMUM_EXTRACTION_DURATION - 1;
    let too_long = MAXIMUM_EXTRACTION_DURATION + 1;
    assert_eq!(check_duration(too_short), Err(Refusal::TooShort));
    assert_eq!(
        start(&mut reg, BOSS, SID, too_short, true, now()),
        Err(Denied::Duration(Refusal::TooShort))
    );
    assert_eq!(
        start(&mut reg, BOSS, SID, too_long, true, now()),
        Err(Denied::Duration(Refusal::TooLong))
    );
    assert!(all(reg.deltas()).is_empty());
    // Both ends are allowed.
    for ok in [MINIMUM_EXTRACTION_DURATION, MAXIMUM_EXTRACTION_DURATION] {
        let e = start(&mut reg, BOSS, SID, ok, false, now()).expect("allowed");
        end(&mut reg, SID, Reason::Manual, Some(BOSS), now());
        assert_eq!(e.duration_s, ok);
    }
}

#[test]
fn a_pilot_at_the_controls_may_start_without_the_role_but_not_stop() {
    let (mut reg, _) = ready();
    // PAL takes the controls from the boss (the owner corporation's control
    // bypass) and has no role: the client lets him start after its own
    // warning (`schedulingSvc.py:61-63`), and only stopping needs the role.
    dock_and_control(&mut reg, PAL);
    let e = start(&mut reg, PAL, SID, LONG, true, now()).expect("starts");
    assert_eq!(e.started_by, PAL);
    assert_eq!(cancel(&mut reg, PAL, now()), Err(Denied::NotAllowed));
    assert!(get(reg.deltas(), SID).is_some());
}

// -- start -------------------------------------------------------------------------------

#[test]
fn start_writes_the_row_the_calendar_event_and_a_202() {
    let (mut reg, _) = ready();
    let t = now();
    let e = start(&mut reg, BOSS, SID, LONG, true, t).expect("starts");
    // The row is the one the pure functions describe.
    let clock = crate::game_clock::of(&reg);
    let p = plan(t, LONG, HULL_AUTO_S as i64, |s| clock.span(s * SECOND));
    assert_eq!(
        (e.start_move, e.chunk_available, e.natural_decay),
        (p.start_move, p.chunk_available, p.natural_decay)
    );
    assert_eq!(
        (
            e.structure_id,
            e.moon_id,
            e.system_id,
            e.owner_corp,
            e.started_by
        ),
        (SID, MOON, SYSTEM, CORP, BOSS)
    );
    assert_eq!(e.phase, Phase::Travel);
    assert_eq!(get(reg.deltas(), SID), Some(e.clone()));
    assert_eq!(of_corp(reg.deltas(), CORP), vec![e.clone()]);
    assert!(of_corp(reg.deltas(), FOE_CORP).is_empty());
    // The calendar event: the corporation's, at the arrival, of the auto type.
    let id = e.calendar_event.expect("an event id");
    let ev = calendar::get(reg.deltas(), id).expect("the event");
    assert_eq!(ev.owner_id, CORP);
    assert_eq!(ev.scope, calendar::Scope::Corp);
    assert_eq!(ev.date_time, e.chunk_available);
    assert_eq!(
        ev.auto_event_type,
        Some(CALENDAR_AUTO_EVENT_MOON_EXTRACTION)
    );
    assert!(!ev.deleted);
    // The 202 reaches the corporation's director, with the keys the formatter
    // reads (who else is mailed is an [A] of the module, not asserted here).
    let got = mails(&reg, BOSS, NOTE_STARTED);
    assert_eq!(got.len(), 1);
    let n = &got[0];
    assert_eq!(n.sender, CORP);
    assert_eq!(field(n, "structureID"), &Value::Int(SID));
    assert_eq!(field(n, "moonID"), &Value::Int(MOON));
    assert_eq!(field(n, "solarSystemID"), &Value::Int(SYSTEM));
    assert_eq!(field(n, "startedBy"), &Value::Int(BOSS));
    assert_eq!(field(n, "readyTime"), &Value::Int(e.chunk_available));
    assert_eq!(field(n, "autoTime"), &Value::Int(e.natural_decay));
    // oreVolumeByType: integer keys, the moon's ores, summing to the yield.
    let Value::IntDict(volumes) = field(n, "oreVolumeByType") else {
        panic!("an integer-keyed dict");
    };
    let shares = crate::moon_scan::makeup(&reg, MOON).expect("placed");
    assert_eq!(
        volumes.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        shares.iter().map(|s| s.basic).collect::<Vec<_>>()
    );
    let sum: f64 = volumes
        .iter()
        .map(|(_, v)| match v {
            Value::Real(x) => *x,
            other => panic!("{other:?}"),
        })
        .sum();
    let total = total_yield_m3(LONG, e.yield_mult);
    assert!((sum - total).abs() <= total * 1e-9, "{sum} vs {total}");
    // The update pushed to the pilot at the controls carries the row.
    let pushes = drain(&mut reg);
    assert_eq!(updates(&pushes), vec![to_keyval(&e)]);
}

#[test]
fn start_without_the_calendar_writes_no_event() {
    let (mut reg, _) = ready();
    let e = start(&mut reg, BOSS, SID, LONG, false, now()).expect("starts");
    assert_eq!(e.calendar_event, None);
    assert!(calendar::all(reg.deltas()).is_empty());
}

#[test]
fn the_yield_is_the_structures_attribute_times_the_world_setting() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    assert_eq!(
        e.yield_mult,
        HULL_YIELD * crate::rules::DEFAULT_MOON_YIELD_MULT
    );
    end(&mut reg, SID, Reason::Manual, Some(BOSS), now());
    reg.rules_mut().moon_yield_mult = 3.0;
    let e = start_it(&mut reg);
    assert_eq!(e.yield_mult, HULL_YIELD * 3.0);
    // A running extraction keeps the value it started with.
    reg.rules_mut().moon_yield_mult = 5.0;
    assert_eq!(
        get(reg.deltas(), SID).expect("row").yield_mult,
        HULL_YIELD * 3.0
    );
}

#[test]
fn start_past_the_fuel_expiry_is_allowed() {
    let (mut reg, _) = ready();
    // The burn on and an empty fuel bay: the structure's fuel is already out
    // (the client asked the pilot; the server does not refuse).
    reg.rules_mut().structure_fuel_burn = true;
    assert!(start(&mut reg, BOSS, SID, LONG, true, now()).is_ok());
}

#[test]
fn the_calendar_event_is_pushed_when_made_and_when_removed() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    let id = e.calendar_event.expect("event");
    let pushes = drain(&mut reg);
    let new: Vec<_> = pushes
        .iter()
        .filter(|(n, _)| *n == "OnNewCalendarEvent")
        .collect();
    assert_eq!(new.len(), 1, "{pushes:?}");
    assert_eq!(new[0].1[0], PyValue::Int(id));
    assert_eq!(new[0].1[1], PyValue::Int(CORP));
    assert_eq!(new[0].1[2], PyValue::Int(e.chunk_available));
    cancel(&mut reg, BOSS, now()).expect("cancels");
    let pushes = drain(&mut reg);
    let gone: Vec<_> = pushes
        .iter()
        .filter(|(n, _)| *n == "OnRemoveCalendarEvent")
        .collect();
    assert_eq!(gone.len(), 1, "{pushes:?}");
    assert_eq!(gone[0].1[0], PyValue::Int(id));
    // Without the calendar there is nothing to push.
    start(&mut reg, BOSS, SID, LONG, false, now()).expect("starts");
    assert!(
        drain(&mut reg)
            .iter()
            .all(|(n, _)| !n.contains("CalendarEvent"))
    );
}

#[test]
fn the_extraction_ids_count_up_and_survive_a_removal() {
    let (mut reg, _) = ready();
    let a = start_it(&mut reg);
    end(&mut reg, SID, Reason::Manual, Some(BOSS), now());
    let b = start_it(&mut reg);
    assert!(b.id > a.id);
    assert_eq!(
        all(reg.deltas()).len(),
        1,
        "the next-id row is no extraction"
    );
}

// -- cancel ------------------------------------------------------------------------------

#[test]
fn cancel_only_while_travelling() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    // Past the arrival the stage is no longer travel, even before the tick moved it.
    let late = e.chunk_available + 1;
    assert_eq!(stage(&e, late), Phase::Ready);
    assert_eq!(cancel(&mut reg, BOSS, late), Err(Denied::NotInTravel));
    assert!(get(reg.deltas(), SID).is_some());
    assert_eq!(cancel(&mut reg, BOSS, e.start_move), Ok(()));
    assert!(get(reg.deltas(), SID).is_none());
}

#[test]
fn cancel_without_an_extraction_is_refused() {
    let (mut reg, _) = ready();
    assert_eq!(cancel(&mut reg, BOSS, now()), Err(Denied::NoExtraction));
    assert_eq!(cancel(&mut reg, PAL, now()), Err(Denied::NotControlling));
}

#[test]
fn cancel_needs_the_manager_roles() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    dock_and_control(&mut reg, PAL);
    assert_eq!(cancel(&mut reg, PAL, now()), Err(Denied::NotAllowed));
    assert_eq!(get(reg.deltas(), SID), Some(e));
    // The role opens it.
    reg.corporations_mut()
        .enrol_for_test(PAL, CORP, crate::sov_hub::CORP_ROLE_STATION_MANAGER);
    assert_eq!(cancel(&mut reg, PAL, now()), Ok(()));
}

#[test]
fn cancel_ends_with_a_203_naming_who_stopped_it_and_deletes_the_event() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    drain(&mut reg);
    cancel(&mut reg, BOSS, now()).expect("cancels");
    assert!(get(reg.deltas(), SID).is_none());
    let ev = calendar::get(reg.deltas(), e.calendar_event.expect("event")).expect("kept");
    assert!(ev.deleted, "the calendar renders a deleted event as such");
    let got = mails(&reg, BOSS, NOTE_CANCELLED);
    assert_eq!(got.len(), 1);
    assert_eq!(field(&got[0], "cancelledBy"), &Value::Int(BOSS));
    assert_eq!(field(&got[0], "structureID"), &Value::Int(SID));
    assert_eq!(updates(&drain(&mut reg)), vec![PyValue::None]);
}

#[test]
fn cancel_then_start_again_works() {
    let (mut reg, _) = ready();
    let first = start_it(&mut reg);
    cancel(&mut reg, BOSS, now()).expect("cancels");
    let second = start_it(&mut reg);
    assert_ne!(first.id, second.id);
    assert_eq!(get(reg.deltas(), SID), Some(second));
}

// -- the drill and the structure ---------------------------------------------------------

#[test]
fn the_drill_going_offline_ends_it_with_offlined() {
    let (mut reg, module) = ready();
    let e = start_it(&mut reg);
    drain(&mut reg);
    // Through the real online change and its hook, not a call of `end`.
    svc::set_online(&mut reg, BOSS, SID, module, false, now()).expect("offline");
    assert!(
        get(reg.deltas(), SID).is_none(),
        "the row ended with the drill"
    );
    let ev = calendar::get(reg.deltas(), e.calendar_event.expect("event")).expect("kept");
    assert!(ev.deleted);
    // PINNED [A]: a 203 for this automatic end, naming nobody (falsifier: a TQ
    // corp receiving no mail when a drill runs dry). The window is cleared.
    let got = mails(&reg, BOSS, NOTE_CANCELLED);
    assert_eq!(got.len(), 1);
    assert_eq!(field(&got[0], "cancelledBy"), &Value::None);
    assert_eq!(
        field(&got[0], "cancelledByLink"),
        &Value::Text(String::new())
    );
    assert!(updates(&drain(&mut reg)).contains(&PyValue::None));
}

#[test]
fn the_tick_ends_one_whose_drill_went_offline_behind_its_back() {
    let (mut reg, module) = ready();
    start_it(&mut reg);
    let t = now();
    // A first tick runs the drill check and finds it online.
    tick(&mut reg, Instant::now(), t);
    assert!(get(reg.deltas(), SID).is_some());
    // The online row removed without the hook (a restore, a forgotten row).
    reg.deltas_mut()
        .del(svc::KIND, &[SID.to_string(), module.to_string()]);
    // Within the check period the tick looks at arrival and the structure only.
    tick(&mut reg, Instant::now(), t + SECOND);
    assert!(
        get(reg.deltas(), SID).is_some(),
        "not looked at every second"
    );
    tick(&mut reg, Instant::now(), t + DRILL_CHECK_EVERY * SECOND);
    assert!(get(reg.deltas(), SID).is_none());
    assert_eq!(mails(&reg, BOSS, NOTE_CANCELLED).len(), 1);
}

#[test]
fn the_structure_gone_ends_it_with_invalid() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    reg.deltas_mut().del(structure::KIND, &[SID.to_string()]);
    tick(&mut reg, Instant::now(), now());
    assert!(get(reg.deltas(), SID).is_none());
    let ev = calendar::get(reg.deltas(), e.calendar_event.expect("event")).expect("kept");
    assert!(ev.deleted);
    // PINNED [A]: a 203 for this automatic end too (as above), and it still
    // names the structure that is gone.
    let got = mails(&reg, BOSS, NOTE_CANCELLED);
    assert_eq!(got.len(), 1);
    assert_eq!(field(&got[0], "cancelledBy"), &Value::None);
    assert_eq!(
        field(&got[0], "structureName"),
        &Value::Text("Invented".to_owned())
    );
    assert_eq!(field(&got[0], "structureTypeID"), &Value::Int(HULL));
}

#[test]
fn the_structure_no_longer_a_drilling_platform_ends_it_with_invalid() {
    let (mut reg, _) = ready();
    start_it(&mut reg);
    let mut s = structure::get(reg.deltas(), SID).expect("structure");
    s.type_id = OTHER_HULL;
    structure::put(reg.deltas_mut(), &s);
    tick(&mut reg, Instant::now(), now());
    assert!(get(reg.deltas(), SID).is_none());
}

// -- arrival -----------------------------------------------------------------------------

#[test]
fn arrival_moves_it_to_ready_and_sends_204() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    drain(&mut reg);
    tick(&mut reg, Instant::now(), e.chunk_available - 1);
    assert_eq!(get(reg.deltas(), SID).expect("row").phase, Phase::Travel);
    assert!(mails(&reg, BOSS, NOTE_FINISHED).is_empty());
    tick(&mut reg, Instant::now(), e.chunk_available);
    let row = get(reg.deltas(), SID).expect("row");
    assert_eq!(row.phase, Phase::Ready);
    let got = mails(&reg, BOSS, NOTE_FINISHED);
    assert_eq!(got.len(), 1);
    assert_eq!(field(&got[0], "autoTime"), &Value::Int(e.natural_decay));
    assert!(matches!(field(&got[0], "oreVolumeByType"), Value::IntDict(v) if !v.is_empty()));
    assert_eq!(updates(&drain(&mut reg)), vec![to_keyval(&row)]);
    // Once only.
    tick(&mut reg, Instant::now(), e.chunk_available + SECOND);
    assert_eq!(mails(&reg, BOSS, NOTE_FINISHED).len(), 1);
    assert!(updates(&drain(&mut reg)).is_empty());
    // The tick records its stamp.
    assert_eq!(reg.ext::<Cache>().last_tick, e.chunk_available + SECOND);
}

#[test]
fn the_tick_drops_a_row_that_does_not_parse() {
    let mut reg = world();
    reg.deltas_mut()
        .put(KIND, vec!["77".to_owned()], vec!["garbage".to_owned()]);
    assert!(all(reg.deltas()).is_empty());
    tick(&mut reg, Instant::now(), now());
    assert!(reg.deltas().of(KIND).is_empty());
}

// -- what the client sees ----------------------------------------------------------------

#[test]
fn the_crater_slim_names_the_chunk_and_faces_the_structure() {
    let (mut reg, _) = ready();
    assert!(slim_extras(&reg, &[MOON]).is_empty(), "no crater before");
    let e = start_it(&mut reg);
    let x = slim_extras(&reg, &[MOON, MOON + 1]);
    assert_eq!(x.len(), 1);
    let (id, extras) = &x[0];
    assert_eq!(*id, MOON);
    assert_eq!(extras[0].0, MOON_CRATER_SLIMITEM_NAME);
    let PyValue::Tuple(t) = &extras[0].1 else {
        panic!("a tuple");
    };
    let PyValue::Tuple(dir) = &t[0] else {
        panic!("a 3-tuple direction");
    };
    // From the moon (out on +x) to the structure (at the origin): -x, unit length.
    assert_eq!(
        dir,
        &vec![PyValue::Real(-1.0), PyValue::Real(0.0), PyValue::Real(0.0)]
    );
    assert_eq!(t[1], PyValue::Long(e.started));
    assert_eq!(t[2], PyValue::Int(CHUNK_BASE + e.id));
    // The crater goes when the extraction does.
    end(&mut reg, SID, Reason::Manual, Some(BOSS), now());
    assert!(slim_extras(&reg, &[MOON]).is_empty());
}

#[test]
fn slash_advance_ready_brings_the_arrival_to_now() {
    let (mut reg, _) = ready();
    let e = start_it(&mut reg);
    drain(&mut reg);
    let out = slash(&mut reg, Some(BOSS), &["advance", "ready"]).expect("answers");
    assert!(
        matches!(out, PyValue::Bytes(ref b) if String::from_utf8_lossy(b).contains("chunk ready"))
    );
    let row = get(reg.deltas(), SID).expect("row");
    assert!(
        row.chunk_available <= now(),
        "{} vs {}",
        row.chunk_available,
        now()
    );
    assert!(row.chunk_available < e.chunk_available);
    assert_eq!(updates(&drain(&mut reg)), vec![to_keyval(&row)]);
    // The tick that follows is the arrival.
    tick(&mut reg, Instant::now(), now() + SECOND);
    assert_eq!(get(reg.deltas(), SID).expect("row").phase, Phase::Ready);
    assert_eq!(mails(&reg, BOSS, NOTE_FINISHED).len(), 1);
}

#[test]
fn slash_refuses_without_the_controls_a_row_or_a_mode() {
    let (mut reg, _) = ready();
    let text = |r: Result<PyValue, ServiceError>| match r.expect("answers") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(text(slash(&mut reg, Some(BOSS), &["advance", "ready"])).contains("no extraction"));
    assert!(text(slash(&mut reg, Some(PAL), &["advance", "ready"])).contains("controls"));
    assert!(text(slash(&mut reg, Some(BOSS), &["advance", "sideways"])).starts_with("/extraction"));
    assert!(text(slash(&mut reg, Some(BOSS), &[])).starts_with("/extraction"));
    start_it(&mut reg);
    assert!(text(slash(&mut reg, Some(BOSS), &["advance", "stop"])).contains("not modelled"));
}

// -- the wire ----------------------------------------------------------------------------

fn err_key(r: Result<PyValue, ServiceError>) -> String {
    svc::error_key(&r.expect_err("refused"))
}

#[test]
fn the_refusals_use_the_client_keys() {
    // The keys of `findings/user-errors.tsv`, spelled out so a rename of ours
    // cannot pass its own test.
    for (d, key) in [
        (Denied::NotControlling, "StructureMoonminingDenied"),
        (Denied::NotAllowed, "StructureMoonminingDenied"),
        (Denied::NoExtraction, "StructureMoonminingDenied"),
        (Denied::NotInTravel, "StructureMoonminingDenied"),
        (Denied::NotDrillingPlatform, "StructureServiceUnavailable"),
        (
            Denied::DrillOffline,
            "CannotStartExtractionMustHaveOnlineMoonDrill",
        ),
        (
            Denied::NoBeacon,
            "CannotStartExtractionMustBeNearMoonMiningBeacon",
        ),
        (
            Denied::Busy,
            "CannotStartExtractionAnotherExtractionInProgress",
        ),
        (
            Denied::Duration(Refusal::TooShort),
            "MoonminingEventTooShort",
        ),
        (Denied::Duration(Refusal::TooLong), "CustomNotify"),
        (Denied::NothingToExtract, "CustomNotify"),
        (
            Denied::ResourcesNeedDrill,
            "CannotGetResourcesMustHaveOnlineMoonDrill",
        ),
        (
            Denied::ResourcesNeedBeacon,
            "CannotGetResourcesMustBeNearMoonMiningBeacon",
        ),
        (
            Denied::NotYetFracturable,
            "ExtractionNotAvailableForFracture",
        ),
    ] {
        assert_eq!(d.key(), key);
        assert_eq!(svc::error_key(&d.into_error()), key, "{d:?}");
    }
    // And through the wire, in the order of the table.
    let mut reg = world();
    let start_args = [PyValue::Int(SID), PyValue::Int(LONG), PyValue::Bool(true)];
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "StartNewExtraction",
            &start_args
        )),
        "StructureMoonminingDenied"
    );
    dock_and_control(&mut reg, BOSS);
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "StartNewExtraction",
            &start_args
        )),
        "CannotStartExtractionMustHaveOnlineMoonDrill"
    );
    fit_drill(&mut reg);
    let short = [PyValue::Int(SID), PyValue::Int(3_600), PyValue::Bool(true)];
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "StartNewExtraction",
            &short
        )),
        "MoonminingEventTooShort"
    );
    call(
        &mut reg,
        BOSS,
        "moonExtractions",
        "StartNewExtraction",
        &start_args,
    )
    .expect("starts");
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "StartNewExtraction",
            &start_args
        )),
        "CannotStartExtractionAnotherExtractionInProgress"
    );
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "FractureChunkWithStructure",
            &[]
        )),
        "ExtractionNotAvailableForFracture"
    );
}

#[test]
fn the_wire_answers_the_extraction_the_corporation_list_and_the_resources() {
    let (mut reg, _) = ready();
    let who = |reg: &mut Registry, w: i64, m: &str| call(reg, w, "moonExtractions", m, &[]);
    // Nothing yet: `(None, None)` for the unpacking, an empty list.
    assert_eq!(
        who(&mut reg, BOSS, "GetExtractionAndEventIDFromStructureID").expect("answers"),
        PyValue::Tuple(vec![PyValue::None, PyValue::None])
    );
    assert_eq!(
        who(&mut reg, BOSS, "GetExtractionsForCorp").expect("answers"),
        PyValue::List(vec![])
    );
    let started = call(
        &mut reg,
        BOSS,
        "moonExtractions",
        "StartNewExtraction",
        &[PyValue::Int(SID), PyValue::Int(LONG), PyValue::Bool(true)],
    )
    .expect("starts");
    assert_eq!(started, PyValue::None);
    let e = get(reg.deltas(), SID).expect("row");
    assert_eq!(
        who(&mut reg, BOSS, "GetExtractionAndEventIDFromStructureID").expect("answers"),
        PyValue::Tuple(vec![
            to_keyval(&e),
            PyValue::Int(e.calendar_event.expect("event"))
        ])
    );
    assert_eq!(
        who(&mut reg, BOSS, "GetExtractionsForCorp").expect("answers"),
        PyValue::List(vec![to_keyval(&e)])
    );
    // A pilot without the role is answered an empty list, another corporation's too.
    for w in [PAL, FOE] {
        assert_eq!(
            who(&mut reg, w, "GetExtractionsForCorp").expect("answers"),
            PyValue::List(vec![])
        );
    }
    // The resources are the moon's makeup as `{ore: fraction}`.
    let shares = crate::moon_scan::makeup(&reg, MOON).expect("placed");
    assert_eq!(
        who(&mut reg, BOSS, "GetMoonResourcesForStructure").expect("answers"),
        crate::moon_scan::results(&shares)
    );
    // The extraction's keyval is the one the client reads.
    let PyValue::Instance { state, .. } = to_keyval(&e) else {
        panic!("a keyval");
    };
    assert_eq!(dict_get(&state, b"structureID"), Some(&PyValue::Int(SID)));
}

#[test]
fn the_resources_need_an_online_drill_and_a_beacon() {
    let mut reg = world();
    dock_and_control(&mut reg, BOSS);
    let ask = |reg: &mut Registry| {
        call(
            reg,
            BOSS,
            "moonExtractions",
            "GetMoonResourcesForStructure",
            &[],
        )
    };
    assert_eq!(
        err_key(ask(&mut reg)),
        "CannotGetResourcesMustHaveOnlineMoonDrill"
    );
    fit_drill(&mut reg);
    assert!(ask(&mut reg).is_ok());
    let mut s = structure::get(reg.deltas(), SID).expect("structure");
    s.pos = [MAXIMUM_MINING_BEACON_DISTANCE as f64 * 2.0, 0.0, 0.0];
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(
        err_key(ask(&mut reg)),
        "CannotGetResourcesMustBeNearMoonMiningBeacon"
    );
    // Not at the controls at all.
    assert_eq!(
        err_key(call(
            &mut reg,
            PAL,
            "moonExtractions",
            "GetMoonResourcesForStructure",
            &[]
        )),
        "StructureMoonminingDenied"
    );
}

#[test]
fn the_wire_cancel_and_a_bad_call() {
    let (mut reg, _) = ready();
    start_it(&mut reg);
    assert_eq!(
        call(&mut reg, BOSS, "moonExtractions", "CancelExtraction", &[]).expect("cancels"),
        PyValue::None
    );
    assert!(get(reg.deltas(), SID).is_none());
    assert_eq!(
        err_key(call(
            &mut reg,
            BOSS,
            "moonExtractions",
            "CancelExtraction",
            &[]
        )),
        "StructureMoonminingDenied"
    );
    assert!(matches!(
        call(&mut reg, BOSS, "moonExtractions", "NoSuchMethodHere", &[]),
        Err(ServiceError::NoSuchMethod { .. })
    ));
    assert!(matches!(
        call(&mut reg, BOSS, "moonExtractions", "StartNewExtraction", &[]),
        Err(ServiceError::BadArguments { .. })
    ));
}

#[test]
fn the_service_is_registered_and_the_upwell_list_names_the_rows() {
    assert!(crate::service::structures::SERVICES.contains(&"moonExtractions"));
    let (mut reg, _) = ready();
    let list = |reg: &mut Registry| match crate::service::structures::slash(
        reg,
        Some(BOSS),
        &["moon", "list"],
    )
    .expect("answers")
    {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert_eq!(list(&mut reg), "no extractions");
    let e = start_it(&mut reg);
    assert!(list(&mut reg).contains(&format!("structure {SID} moon {MOON}")));
    assert_eq!(e.structure_id, SID);
}
