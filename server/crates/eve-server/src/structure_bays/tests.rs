//! Tests of the deed and fuel bays (STR-CORE). Every id is invented; the
//! client's own ids appear only through the named consts of the module.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::space_state::{Motion, Presence};
use crate::structure::State;

const BOSS: i64 = 7;
const PAL: i64 = 8;
const FOE: i64 = 9;
const CORP: i64 = 11;
const FOE_CORP: i64 = 13;
const SYSTEM: i64 = 31;
const HARBOUR: i64 = 6_001;
const ASTRAHUS: i64 = crate::structure_space::ASTRAHUS;
/// An invented hull carrying an invented core type.
const HULL: i64 = 9_402_001;
const CORE: i64 = 9_402_090;
/// An invented core of no hull under test.
const OTHER_CORE: i64 = 9_402_091;
/// An invented hull that names no core.
const BARE_HULL: i64 = 9_402_002;
/// An invented fuel type, in the group the module takes as fuel.
const FUEL: i64 = 9_402_100;
/// An invented type of no bay's.
const JUNK: i64 = 9_402_110;
const JUNK_GROUP: i64 = 9_402_900;
const BAY_ATTRS: &str = "1137=3.0;2056=3.0;2339=-25.0";

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{fixture}attribute\t{ATTR_REQUIRES_DEED_TYPE}\t0.0\trequiresDeed\n\
         type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Spawn Hull\n\
         type\t{HULL}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t{ATTR_REQUIRES_DEED_TYPE}={CORE}.0;{BAY_ATTRS}\tInvented Hull\n\
         type\t{BARE_HULL}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Bare Hull\n\
         type\t{CORE}\t{GROUP_QUANTUM_CORE}\t66\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Core\n\
         type\t{OTHER_CORE}\t{GROUP_QUANTUM_CORE}\t66\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Other Core\n\
         type\t{FUEL}\t{GROUP_FUEL_BLOCK}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Fuel\n\
         type\t{JUNK}\t{JUNK_GROUP}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Junk\n",
        fixture = crate::typedata::FIXTURE
    );
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn config() -> SandboxConfig {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
        corporation_id: corp,
        solar_system_id: Some(SYSTEM),
        station_id: Some(HARBOUR),
        ship_id: 1_000 + id,
        ship_type_id: crate::service::TYPE_CAPSULE,
        ..Character::default()
    };
    SandboxConfig {
        characters: vec![ch(BOSS, CORP), ch(PAL, CORP), ch(FOE, FOE_CORP)],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
        )),
        ..SandboxConfig::default()
    }
}

/// A world with a structure of `hull` waiting in state 102 for its deed, and
/// the pilots in space beside it. BOSS is a director of the owning corp, PAL
/// a member with no role, FOE of another corp.
fn world_of(hull: i64) -> (Registry, i64) {
    let mut reg = Registry::new(config()).with_content(std::sync::Arc::new(content()));
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
    // Spawned the GM's way, then put back to waiting for its deed as the
    // hull under test.
    crate::service::structures::slash(
        &mut reg,
        Some(BOSS),
        &["spawn", "astrahus", "Invented", "Home"],
    )
    .expect("spawns");
    let id = structure::all(reg.deltas()).last().expect("spawned").id;
    let mut s = structure::get(reg.deltas(), id).expect("the row");
    let e = crate::structure_space::env(&reg, crate::pyutil::filetime_now());
    s.type_id = hull;
    s.deed_present = false;
    s.set_state(State::OnliningVulnerable, &e);
    s.take_events();
    assert_eq!(s.state, State::OnliningVulnerable, "waits for its deed");
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_pending();
    (reg, id)
}

fn world() -> (Registry, i64) {
    world_of(HULL)
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

/// Dock `who` in `id` and, when `control`, take its controls.
fn seat(reg: &mut Registry, who: i64, id: i64, control: bool) {
    let ship = reg.character(who).unwrap().ship_id;
    call(
        reg,
        who,
        "structureDocking",
        "Dock",
        &[PyValue::Int(id), PyValue::Int(ship)],
    )
    .expect("docks");
    if control {
        call(
            reg,
            who,
            "structureControl",
            "TakeControl",
            &[PyValue::Int(id)],
        )
        .expect("takes the controls");
    }
}

/// `qty` units of `type_id` in `who`'s hangar at `id`.
fn stock(reg: &mut Registry, who: i64, id: i64, type_id: i64, qty: i64) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity: qty,
            to: Slot::new(who, id, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("an id")
}

/// The call the client makes for a drop on a bay: an `Add` on the
/// structure's inventory with the bay's flag.
fn add_to_bay(
    reg: &mut Registry,
    who: i64,
    id: i64,
    item: i64,
    qty: Option<i64>,
    flag: i64,
) -> Result<PyValue, ServiceError> {
    let bound = PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]);
    let mut kwargs = vec![(PyValue::Int(flag), PyValue::Bytes(b"flag".to_vec()))];
    if let Some(q) = qty {
        kwargs.push((PyValue::Int(q), PyValue::Bytes(b"qty".to_vec())));
    }
    reg.dispatch(&Request {
        service: "invbroker::inventory",
        method: "Add",
        args: &[PyValue::Int(item), PyValue::Int(id)],
        kwargs: &kwargs,
        bind_params: Some(&bound),
        actor: Some(who),
    })
}

/// The call the client makes to take an item out of a bay into the hangar.
fn add_to_hangar(
    reg: &mut Registry,
    who: i64,
    id: i64,
    item: i64,
    qty: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let bound = PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]);
    let kwargs: Vec<(PyValue, PyValue)> = qty
        .map(|q| vec![(PyValue::Int(q), PyValue::Bytes(b"qty".to_vec()))])
        .unwrap_or_default();
    reg.dispatch(&Request {
        service: "invbroker::inventory",
        method: "Add",
        args: &[PyValue::Int(item), PyValue::Int(id)],
        kwargs: &kwargs,
        bind_params: Some(&bound),
        actor: Some(who),
    })
}

/// The world clock, four ticks of 1,000 s: long enough for the onlining
/// repair that starts when the deed is in (`structure_space`'s own tests walk
/// it the same way).
fn run_clock(reg: &mut Registry) {
    let mut at = crate::pyutil::filetime_now();
    for _ in 0..4 {
        at += 1_000 * structure::SECOND;
        crate::structure_space::tick(reg, std::time::Instant::now(), at);
    }
}

fn key_of(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        return format!("{e:?}");
    };
    match &**header {
        PyValue::Tuple(h) => match h.get(1) {
            Some(PyValue::Tuple(a)) => match a.first() {
                Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                other => format!("{other:?}"),
            },
            other => format!("{other:?}"),
        },
        other => format!("{other:?}"),
    }
}

fn state_of(reg: &Registry, id: i64) -> State {
    structure::get(reg.deltas(), id).expect("the row").state
}

fn in_bay(reg: &Registry, id: i64, flag: i64) -> Vec<crate::item::Item> {
    reg.store()
        .contents_of(id)
        .filter(|i| i.flag_id == flag)
        .cloned()
        .collect()
}

#[test]
fn the_deed_bay_takes_only_the_hulls_core() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let junk = stock(&mut reg, BOSS, id, JUNK, 1);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 5);
    for item in [junk, fuel] {
        let e = add_to_bay(&mut reg, BOSS, id, item, None, FLAG_DEED).expect_err("not a core");
        assert_eq!(key_of(&e), "CannotPutNonDeedInDeedSlot");
    }
    assert!(in_bay(&reg, id, FLAG_DEED).is_empty());
    assert_eq!(state_of(&reg, id), State::OnliningVulnerable);
    let core = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, core, Some(1), FLAG_DEED).expect("the core goes in");
    let held = in_bay(&reg, id, FLAG_DEED);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].type_id, CORE);
}

#[test]
fn another_hulls_core_is_the_wrong_deed() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let other = stock(&mut reg, BOSS, id, OTHER_CORE, 1);
    let e = add_to_bay(&mut reg, BOSS, id, other, None, FLAG_DEED).expect_err("wrong core");
    assert_eq!(key_of(&e), "WrongDeedTypeForStructure");
    assert!(in_bay(&reg, id, FLAG_DEED).is_empty());
    assert!(!structure::get(reg.deltas(), id).unwrap().deed_present);
}

#[test]
fn a_second_core_is_refused() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let first = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, first, None, FLAG_DEED).unwrap();
    let second = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, second, None, FLAG_DEED).expect_err("full");
    assert_eq!(in_bay(&reg, id, FLAG_DEED).len(), 1);
    assert_eq!(
        reg.store().get(second).map(|i| i.flag_id),
        Some(crate::model::FLAG_HANGAR)
    );
}

#[test]
fn a_stack_of_cores_gives_one() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let stack = stock(&mut reg, BOSS, id, CORE, 3);
    add_to_bay(&mut reg, BOSS, id, stack, None, FLAG_DEED).unwrap();
    assert_eq!(in_bay(&reg, id, FLAG_DEED)[0].quantity, 1);
    assert_eq!(reg.store().get(stack).map(|i| i.quantity), Some(2));
}

#[test]
fn the_core_finishes_onlining() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let core = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, core, None, FLAG_DEED).unwrap();
    assert!(structure::get(reg.deltas(), id).unwrap().deed_present);
    // The onlining repair runs after the deed (the client's state machine),
    // so it is the clock that leaves 102.
    run_clock(&mut reg);
    assert_eq!(state_of(&reg, id), State::ShieldVulnerable);
}

#[test]
fn without_a_core_it_waits_in_the_tick() {
    let (mut reg, id) = world();
    reg.rules_mut().structure_core_required = true;
    run_clock(&mut reg);
    assert_eq!(state_of(&reg, id), State::OnliningVulnerable);
}

#[test]
fn with_the_core_rule_off_the_tick_finishes_onlining() {
    let (mut reg, id) = world();
    reg.rules_mut().structure_core_required = false;
    run_clock(&mut reg);
    assert_eq!(state_of(&reg, id), State::ShieldVulnerable);
}

/// A hull with no attribute 3101 is not waived by that: with cores required it
/// waits in 102 like any other, and under the world's own default it ends where
/// a hull with a core ends (so a missing 3101, or no typedata, onlines nothing).
#[test]
fn a_hull_that_names_no_core_waits_like_any_other() {
    let (mut reg, id) = world_of(BARE_HULL);
    assert_eq!(core_type(reg.content(), BARE_HULL), None);
    reg.rules_mut().structure_core_required = true;
    run_clock(&mut reg);
    assert_eq!(state_of(&reg, id), State::OnliningVulnerable);
    let (mut default_bare, bare_id) = world_of(BARE_HULL);
    let (mut default_cored, cored_id) = world();
    run_clock(&mut default_bare);
    run_clock(&mut default_cored);
    assert_eq!(
        state_of(&default_bare, bare_id),
        state_of(&default_cored, cored_id)
    );
}

/// With `corerule` off the structure's events leave on the clock's queue, not
/// in `pending` until some client's next call (the #44 rule).
#[test]
fn the_waive_is_scheduled_on_the_clock() {
    let (mut reg, id) = world();
    reg.rules_mut().structure_core_required = false;
    let _ = reg.take_pending_raw();
    let queued = reg.scheduled_len();
    run_clock(&mut reg);
    assert_eq!(state_of(&reg, id), State::ShieldVulnerable);
    assert!(
        reg.take_pending_raw().is_empty(),
        "nothing waits for a client call"
    );
    assert!(reg.scheduled_len() > queued, "the change is on the queue");
}

#[test]
fn the_core_cannot_be_taken_out() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let core = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, core, None, FLAG_DEED).unwrap();
    let e = add_to_hangar(&mut reg, BOSS, id, core, None).expect_err("stays");
    assert_eq!(key_of(&e), "CannotRemoveDeedOnceFitted");
    assert_eq!(in_bay(&reg, id, FLAG_DEED).len(), 1);
    assert!(structure::get(reg.deltas(), id).unwrap().deed_present);
}

#[test]
fn the_fuel_bay_takes_fuel_and_refuses_other_items() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 40);
    add_to_bay(&mut reg, BOSS, id, fuel, None, FLAG_FUEL).expect("fuel goes in");
    let held = in_bay(&reg, id, FLAG_FUEL);
    assert_eq!(
        (held.len(), held[0].type_id, held[0].quantity),
        (1, FUEL, 40)
    );
    let junk = stock(&mut reg, BOSS, id, JUNK, 1);
    let e = add_to_bay(&mut reg, BOSS, id, junk, None, FLAG_FUEL).expect_err("not fuel");
    assert_eq!(key_of(&e), "CannotPlaceTypeInFuelBay");
    assert_eq!(in_bay(&reg, id, FLAG_FUEL).len(), 1);
    // The core is no fuel either.
    let core = stock(&mut reg, BOSS, id, CORE, 1);
    let e = add_to_bay(&mut reg, BOSS, id, core, None, FLAG_FUEL).expect_err("not fuel");
    assert_eq!(key_of(&e), "CannotPlaceTypeInFuelBay");
}

#[test]
fn the_content_rule_knows_ozone_and_the_metenox_reagent() {
    let content = content();
    // Liquid Ozone for any hull but the Metenox, lava only for the Metenox.
    assert!(may_hold(&content, HULL, FLAG_FUEL, TYPE_LIQUID_OZONE, JUNK_GROUP).is_ok());
    assert!(
        may_hold(
            &content,
            HULL,
            FLAG_FUEL,
            TYPE_COLONY_REAGENT_LAVA,
            JUNK_GROUP
        )
        .is_err()
    );
    let metenox = structure::TYPE_UPWELL_AUTO_MOON_MINER;
    assert!(
        may_hold(
            &content,
            metenox,
            FLAG_FUEL,
            TYPE_COLONY_REAGENT_LAVA,
            JUNK_GROUP
        )
        .is_ok()
    );
    assert!(may_hold(&content, metenox, FLAG_FUEL, TYPE_LIQUID_OZONE, JUNK_GROUP).is_err());
    assert!(may_hold(&content, metenox, FLAG_FUEL, FUEL, GROUP_FUEL_BLOCK).is_ok());
    // A bay no module owns takes whatever the caller's own rules let in.
    assert!(may_hold(&content, HULL, 5, JUNK, JUNK_GROUP).is_ok());
}

#[test]
fn a_pilot_without_the_role_cannot_fill_the_bay() {
    let (mut reg, id) = world();
    // PAL may take the controls (the owning corp's own), but holds no role.
    seat(&mut reg, PAL, id, true);
    let fuel = stock(&mut reg, PAL, id, FUEL, 5);
    // The gate is [A]; the test follows the module's own predicate.
    let s = structure::get(reg.deltas(), id).unwrap();
    let allowed = may_use(&reg, PAL, &s).is_ok();
    let done = add_to_bay(&mut reg, PAL, id, fuel, None, FLAG_FUEL);
    assert_eq!(done.is_ok(), allowed);
    if let Err(e) = &done {
        assert_eq!(key_of(e), "CrpAccessDenied");
    }
    assert_eq!(in_bay(&reg, id, FLAG_FUEL).is_empty(), !allowed);
}

#[test]
fn a_pilot_not_at_the_controls_falls_through() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, false);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 5);
    let req = Request {
        service: "invbroker::inventory",
        method: "Add",
        args: &[PyValue::Int(fuel), PyValue::Int(id)],
        kwargs: &[(PyValue::Int(FLAG_FUEL), PyValue::Bytes(b"flag".to_vec()))],
        bind_params: Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None])),
        actor: Some(BOSS),
    };
    assert!(inventory_call(&mut reg, &req).is_none());
    // Neither is it the bay's call for another method.
    let list = Request {
        method: "List",
        ..req.clone()
    };
    assert!(inventory_call(&mut reg, &list).is_none());
    assert!(in_bay(&reg, id, FLAG_FUEL).is_empty());
}

/// **[V]** the client's `invControllers` `StructureBay.IsItemHere` /
/// `GetOwnerID` read a bay's items as the structure's corporation's.
#[test]
fn bay_contents_are_owned_by_the_corp() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 5);
    add_to_bay(&mut reg, BOSS, id, fuel, None, FLAG_FUEL).unwrap();
    let core = stock(&mut reg, BOSS, id, CORE, 1);
    add_to_bay(&mut reg, BOSS, id, core, None, FLAG_DEED).unwrap();
    for flag in [FLAG_FUEL, FLAG_DEED] {
        let held = in_bay(&reg, id, flag);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].owner_id, CORP, "flag {flag}");
    }
    // The controls list shows them: the bay listing filters on the corp.
    let bound = PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]);
    let kwargs = [(PyValue::Int(FLAG_FUEL), PyValue::Bytes(b"flag".to_vec()))];
    let PyValue::List(rows) = reg
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "List",
            args: &[],
            kwargs: &kwargs,
            bind_params: Some(&bound),
            actor: Some(BOSS),
        })
        .expect("lists")
    else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
}

#[test]
fn fuel_stacks_merge_in_the_bay() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let a = stock(&mut reg, BOSS, id, FUEL, 30);
    let b = stock(&mut reg, BOSS, id, FUEL, 12);
    add_to_bay(&mut reg, BOSS, id, a, None, FLAG_FUEL).unwrap();
    add_to_bay(&mut reg, BOSS, id, b, None, FLAG_FUEL).unwrap();
    let held = in_bay(&reg, id, FLAG_FUEL);
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].quantity, 42);
    assert!(
        reg.store().get(b).is_none(),
        "the source stack is folded in"
    );
}

#[test]
fn taking_fuel_out_gives_it_to_the_pilot() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 40);
    add_to_bay(&mut reg, BOSS, id, fuel, None, FLAG_FUEL).unwrap();
    let bay_stack = in_bay(&reg, id, FLAG_FUEL)[0].item_id;
    add_to_hangar(&mut reg, BOSS, id, bay_stack, None).expect("fuel comes out");
    assert!(in_bay(&reg, id, FLAG_FUEL).is_empty());
    let it = reg.store().get(bay_stack).expect("the stack");
    // Whose it becomes and where in the structure is [A]: not asserted.
    assert_eq!((it.location_id, it.quantity), (id, 40));
    assert!(!is_owned_bay(it.flag_id));
}

#[test]
fn a_partial_stack_moves_qty_units() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let fuel = stock(&mut reg, BOSS, id, FUEL, 40);
    add_to_bay(&mut reg, BOSS, id, fuel, Some(15), FLAG_FUEL).unwrap();
    assert_eq!(in_bay(&reg, id, FLAG_FUEL)[0].quantity, 15);
    assert_eq!(reg.store().get(fuel).map(|i| i.quantity), Some(25));
    // More of the same: it grows the bay's stack, and a part comes out.
    add_to_bay(&mut reg, BOSS, id, fuel, Some(5), FLAG_FUEL).unwrap();
    assert_eq!(in_bay(&reg, id, FLAG_FUEL)[0].quantity, 20);
    assert_eq!(reg.store().get(fuel).map(|i| i.quantity), Some(20));
    let bay_stack = in_bay(&reg, id, FLAG_FUEL)[0].item_id;
    add_to_hangar(&mut reg, BOSS, id, bay_stack, Some(8)).unwrap();
    assert_eq!(in_bay(&reg, id, FLAG_FUEL)[0].quantity, 12);
    let out: i64 = reg
        .store()
        .contents_of(id)
        .filter(|i| i.type_id == FUEL && !is_owned_bay(i.flag_id))
        .map(|i| i.quantity)
        .sum();
    assert_eq!(out, 20 + 8);
}

#[test]
fn another_pilots_items_cannot_be_put_in() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let theirs = stock(&mut reg, PAL, id, FUEL, 5);
    let e = add_to_bay(&mut reg, BOSS, id, theirs, None, FLAG_FUEL).expect_err("not his");
    assert_eq!(key_of(&e), "CustomNotify");
    assert!(in_bay(&reg, id, FLAG_FUEL).is_empty());
}

#[test]
fn flags_after_a_wrap_and_what_dies() {
    use crate::structure_services::{FIRST_SLOT, LAST_SLOT};
    let bays = [FLAG_FUEL, FLAG_DEED];
    for flag in (FIRST_SLOT..=LAST_SLOT).chain(bays) {
        // Whatever the flag is mapped to (the target is [A]), it is no bay.
        assert!(control::is_bay_flag(flag));
        assert!(!control::is_bay_flag(flag_after_wrap(flag)), "flag {flag}");
        assert!(dies_with_hull(flag));
    }
    for flag in RIG_SLOTS {
        assert!(dies_with_hull(flag), "a rig");
        assert_eq!(
            flag_after_wrap(flag),
            flag,
            "a rig is destroyed, not remapped"
        );
    }
    // The hangar itself is neither remapped nor destroyed.
    assert_eq!(
        flag_after_wrap(crate::model::FLAG_HANGAR),
        crate::model::FLAG_HANGAR
    );
    assert!(!dies_with_hull(crate::model::FLAG_HANGAR));
}

#[test]
fn the_dogma_row_carries_the_core_type() {
    let content = content();
    let base = PyValue::Dict(vec![(PyValue::Real(1.0), PyValue::Int(9))]);
    let PyValue::Dict(rows) =
        crate::structure_services::hull_attributes(&content, HULL, base.clone())
    else {
        panic!("a dictionary")
    };
    let value_of = |attr: i64| {
        rows.iter().find_map(|(v, k)| match (v, k) {
            (PyValue::Real(v), PyValue::Int(a)) if *a == attr => Some(*v),
            _ => None,
        })
    };
    assert_eq!(value_of(ATTR_REQUIRES_DEED_TYPE), Some(CORE as f64));
    assert_eq!(value_of(1_137), Some(3.0));
    assert_eq!(value_of(9), Some(1.0), "the base stays");
    // A hull that names none sends none; an attribute is never sent twice.
    let PyValue::Dict(bare) = crate::structure_services::hull_attributes(&content, BARE_HULL, base)
    else {
        panic!("a dictionary")
    };
    assert_eq!(bare.len(), 1);
    let PyValue::Dict(twice) =
        crate::structure_services::hull_attributes(&content, HULL, PyValue::Dict(rows.clone()))
    else {
        panic!("a dictionary")
    };
    assert_eq!(twice.len(), rows.len());
}

/// #41 owns the slot flags 164-171: the bay hook answers neither a drop on a
/// slot nor the take of a fitted module, so `structure_module_call` gets both.
#[test]
fn the_slot_flags_stay_with_the_service_modules() {
    let (mut reg, id) = world();
    seat(&mut reg, BOSS, id, true);
    let item = stock(&mut reg, BOSS, id, FUEL, 1);
    let bound = PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]);
    let slot_flag = [(
        PyValue::Int(crate::structure_services::FIRST_SLOT),
        PyValue::Bytes(b"flag".to_vec()),
    )];
    let drop = Request {
        service: "invbroker::inventory",
        method: "Add",
        args: &[PyValue::Int(item), PyValue::Int(id)],
        kwargs: &slot_flag,
        bind_params: Some(&bound),
        actor: Some(BOSS),
    };
    assert!(
        inventory_call(&mut reg, &drop).is_none(),
        "a drop on a slot"
    );
    // A module sitting in the slot, and the call that unfits it.
    reg.apply_items(&[Edit::Move {
        item_id: item,
        to: Slot::new(CORP, id, crate::structure_services::FIRST_SLOT),
    }])
    .expect("moved");
    let take = Request {
        kwargs: &[],
        ..drop
    };
    assert!(inventory_call(&mut reg, &take).is_none(), "an unfit");
}
