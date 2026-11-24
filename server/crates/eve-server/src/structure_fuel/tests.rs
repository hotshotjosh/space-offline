//! #42 part b STR-FUEL. Every id is invented; the client's own ids appear
//! only through the named consts of the modules (the fuel group, the ozone
//! type, the service attributes). No number below is an `[A]` of the module:
//! the thresholds and the switch's default are read from its own consts.

use super::*;
use crate::item::Slot;
use crate::model::{Character, SandboxConfig};
use crate::notifications as notif;
use crate::structure::{State, Upkeep};
use crate::structure_notify as nt;
use crate::structure_services::{
    ATTR_SERVICE_SLOTS, CATEGORY_STRUCTURE_MODULE, EFFECT_SERVICE_SLOT,
};

const BOSS: i64 = 7;
const MEMBER: i64 = 8;
const CORP: i64 = 11;
const SYSTEM: i64 = 31;
/// An invented hull with three service slots and the role bonus.
const HULL: i64 = 9_420_001;
const SID: i64 = 1_042_001;
/// An invented module group the hull's role bonus names.
const GROUP_BONUSED: i64 = 9_420_030;
/// An invented module group it does not.
const GROUP_PLAIN: i64 = 9_420_031;
/// Burn 12 and online cost 40 before the bonus.
const MOD_A: i64 = 9_420_010;
/// Burn 3 and online cost 10, no bonus.
const MOD_B: i64 = 9_420_011;
/// A service module that uses no fuel.
const MOD_FREE: i64 = 9_420_012;
/// The Metenox drill's own type (a named client id) with an invented row.
const MOD_METENOX: i64 = svc::TYPE_STANDUP_METENOX_MOON_DRILL;
/// An invented fuel type in the fuel group; a second one.
const FUEL: i64 = 9_420_100;
const FUEL_2: i64 = 9_420_101;
/// The invented effect carrying the hull's role bonus.
const BONUS_EFFECT: i64 = 9_420_200;
/// The invented role bonus, in percent.
const BONUS: f64 = -25.0;
/// Invented services for the invented modules.
const TABLE: svc::ServiceTable<'static> = &[
    (MOD_A, &[crate::structure_profile::SERVICE_MEDICAL]),
    (MOD_B, &[crate::structure_profile::SERVICE_REPROCESSING]),
    (MOD_FREE, &[crate::structure_profile::SERVICE_MEDICAL]),
];
const T0: i64 = 134_000_000_000_000_000 / HOUR * HOUR;

fn module_row(id: i64, group: i64, attrs: &str) -> String {
    format!(
        "type\t{id}\t{group}\t{CATEGORY_STRUCTURE_MODULE}\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t{EFFECT_SERVICE_SLOT}\t{attrs}\tInvented Module {id}\n"
    )
}

fn content(bonus: bool) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let mut rows = crate::typedata::FIXTURE.to_owned();
    let effect = if bonus {
        BONUS_EFFECT.to_string()
    } else {
        String::new()
    };
    rows.push_str(&format!(
        "type\t{HULL}\t9420002\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t{effect}\t{ATTR_SERVICE_SLOTS}=3.0;{ATTR_ROLE_BONUS}={BONUS}\tInvented Hull\n"
    ));
    if bonus {
        // The shape of a ship's role bonus: a passive hull effect, operation 6
        // on the modules of one group, from the hull's own bonus attribute.
        rows.push_str(&format!(
            "effect\t{BONUS_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tinventedRoleBonus\t\n\
             modifier\t{BONUS_EFFECT}\tshipID\tLocationGroupModifier\t{ATTR_FUEL_PER_HOUR}\t{ATTR_ROLE_BONUS}\t6\t\t{GROUP_BONUSED}\n\
             modifier\t{BONUS_EFFECT}\tshipID\tLocationGroupModifier\t{ATTR_ONLINE_AMOUNT}\t{ATTR_ROLE_BONUS}\t6\t\t{GROUP_BONUSED}\n"
        ));
    }
    rows.push_str(&module_row(
        MOD_A,
        GROUP_BONUSED,
        &format!("{ATTR_FUEL_PER_HOUR}=12.0;{ATTR_ONLINE_AMOUNT}=40.0"),
    ));
    rows.push_str(&module_row(
        MOD_B,
        GROUP_PLAIN,
        &format!("{ATTR_FUEL_PER_HOUR}=3.0;{ATTR_ONLINE_AMOUNT}=10.0"),
    ));
    rows.push_str(&module_row(MOD_FREE, GROUP_PLAIN, "50=1.0"));
    rows.push_str(&module_row(
        MOD_METENOX,
        GROUP_PLAIN,
        &format!("{ATTR_FUEL_PER_HOUR}=100.0"),
    ));
    rows.push_str(&format!(
        "type\t{FUEL}\t{GROUP_FUEL_BLOCK}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Fuel\n\
         type\t{FUEL_2}\t{GROUP_FUEL_BLOCK}\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Fuel Two\n\
         type\t{TYPE_LIQUID_OZONE}\t9420040\t4\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n"
    ));
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn world_with(c: crate::content::Content) -> Registry {
    let ch = |id: i64| Character {
        character_id: id,
        corporation_id: CORP,
        solar_system_id: Some(SYSTEM),
        ship_id: 1_000 + id,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![ch(BOSS), ch(MEMBER)],
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(c));
    reg.corporations_mut()
        .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    reg.corporations_mut().enrol_for_test(MEMBER, CORP, 0);
    let profile = crate::structure_profile::ensure_default(&mut reg, CORP);
    // Built on the world's own clock, as the other structure tests do: its
    // upkeep and abandon timers are judged against the ticks that follow.
    // It starts at full power (a module "online" at its birth), so the first
    // change a test sees is the one it causes.
    let e = structure::Env {
        has_service_module: true,
        ..structure::Env::new(crate::pyutil::filetime_now())
    };
    let mut s = Structure::anchored(
        SID, HULL, CORP, SYSTEM, [0.0; 3], 0.0, "Invented", profile, &e,
    );
    s.set_state(State::ShieldVulnerable, &e);
    s.take_events();
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_pending();
    reg
}

/// The burn switched on, the hull's bonus present.
fn world() -> Registry {
    let mut reg = world_with(content(true));
    reg.rules_mut().structure_fuel_burn = true;
    reg
}

fn the_structure(reg: &Registry) -> Structure {
    structure::get(reg.deltas(), SID).expect("the structure")
}

/// `units` fuel blocks of `type_id` in the fuel bay, the corporation's.
fn load(reg: &mut Registry, type_id: i64, units: i64) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity: units,
            to: Slot::new(CORP, SID, FLAG_FUEL),
        }])
        .expect("created");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("a new stack")
}

/// Fit and online `type_id` the GM's way (the online cost is asked), and say
/// what happened.
fn fit_online(reg: &mut Registry, type_id: i64, now: i64) -> String {
    svc::gm_fit_with(TABLE, reg, BOSS, SID, type_id, now)
}

/// Whether a [`fit_online`] answer says the module is online.
fn onlined(line: &str) -> bool {
    line.ends_with(", online")
}

fn module_of(reg: &Registry, type_id: i64) -> Fitted {
    svc::fitted(reg, &the_structure(reg))
        .into_iter()
        .find(|f| f.type_id == type_id)
        .expect("fitted")
}

fn online_types(reg: &Registry) -> Vec<i64> {
    svc::fitted(reg, &the_structure(reg))
        .iter()
        .filter(|f| f.online)
        .map(|f| f.type_id)
        .collect()
}

/// Take `n` units out of the bay (the test's own drain).
fn drain(reg: &mut Registry, n: i64) -> Result<(), String> {
    let s = the_structure(reg);
    take_units(reg, &s, n)
}

fn units(reg: &Registry) -> i64 {
    fuel_units(reg, &the_structure(reg))
}

fn mails(reg: &Registry, who: i64, type_id: i64) -> Vec<notif::Notification> {
    notif::of(reg.deltas(), who)
        .into_iter()
        .filter(|n| n.type_id == type_id)
        .collect()
}

fn field<'a>(n: &'a notif::Notification, name: &str) -> &'a notif::Value {
    &n.data
        .iter()
        .find(|(k, _)| k == name)
        .unwrap_or_else(|| panic!("{name} missing"))
        .1
}

// -- the burn --------------------------------------------------------------------------

#[test]
fn burn_reads_the_dogma_modified_amount() {
    let mut reg = world();
    load(&mut reg, FUEL, 200);
    assert!(onlined(&fit_online(&mut reg, MOD_A, T0)), "burn 12");
    let a = uses(&reg, &the_structure(&reg));
    // The hull's invented -25 on the invented group: 12 -> 9 (and the online
    // cost 40 -> 30), the server's dogma engine on a category-65 hull.
    assert_eq!(a[0].per_hour, 12.0 * (100.0 + BONUS) / 100.0);
    assert_eq!(a[0].per_hour, 9.0);
    assert_eq!(a[0].online, 30.0);
    assert_eq!(burn_per_hour(&reg, &the_structure(&reg)), 9);
    // A module of another group is not touched, and the two add up.
    assert!(onlined(&fit_online(&mut reg, MOD_B, T0)), "burn 3");
    assert_eq!(burn_per_hour(&reg, &the_structure(&reg)), 9 + 3);
    // Without the hull's effect the same modules read their own numbers: the
    // bonus is the engine's, not the test's.
    let mut bare = world_with(content(false));
    bare.rules_mut().structure_fuel_burn = true;
    load(&mut bare, FUEL, 100);
    assert!(onlined(&fit_online(&mut bare, MOD_A, T0)));
    assert_eq!(burn_per_hour(&bare, &the_structure(&bare)), 12);
}

#[test]
fn an_offline_module_a_free_module_and_the_metenox_do_not_burn() {
    let mut reg = world();
    load(&mut reg, FUEL, 1_000);
    fit_online(&mut reg, MOD_B, T0);
    fit_online(&mut reg, MOD_FREE, T0);
    assert_eq!(burn_per_hour(&reg, &the_structure(&reg)), 3);
    // Offline: nothing.
    let m = module_of(&reg, MOD_B);
    svc::force_offline(&mut reg, SID, m.item_id, T0).expect("offline");
    assert_eq!(burn_per_hour(&reg, &the_structure(&reg)), 0);
    // The Metenox drill, fitted and online, is #55's burn, not this one (F13).
    let id = reg
        .apply_items(&[Edit::CreateAssembled {
            type_id: MOD_METENOX,
            to: Slot::new(CORP, SID, svc::FIRST_SLOT + 2),
        }])
        .expect("fitted")
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("created");
    reg.deltas_mut().put(
        svc::KIND,
        vec![SID.to_string(), id.to_string()],
        vec!["1".to_owned()],
    );
    assert_eq!(burn_per_hour(&reg, &the_structure(&reg)), 0);
}

#[test]
fn ozone_never_counts() {
    let mut reg = world();
    // Exactly the online cost (10), so no block is left.
    load(&mut reg, FUEL, 10);
    assert!(onlined(&fit_online(&mut reg, MOD_B, T0)));
    load(&mut reg, TYPE_LIQUID_OZONE, 1_000);
    let s = the_structure(&reg);
    assert_eq!(fuel_units(&reg, &s), 0);
    assert_eq!(ozone_units(&reg, &s), 1_000);
    assert_eq!(expires_at(&reg, &s, T0), Some(T0), "no fuel: no hours");
    load(&mut reg, FUEL, 9);
    assert_eq!(expires_at(&reg, &s, T0), Some(T0 + 3 * HOUR));
    // The burn takes the blocks and leaves the ozone.
    let r = step(&mut reg, SID, T0 + HOUR).expect("steps");
    assert_eq!((r.paid, r.burned), (1, 3));
    let r = step(&mut reg, SID, T0 + 2 * HOUR).expect("steps");
    assert_eq!((r.paid, r.burned), (1, 3));
    let s = the_structure(&reg);
    assert_eq!((fuel_units(&reg, &s), ozone_units(&reg, &s)), (3, 1_000));
}

#[test]
fn missed_hours_are_burned_per_hour_and_never_twice() {
    let mut reg = world();
    load(&mut reg, FUEL, 1_000);
    fit_online(&mut reg, MOD_A, T0);
    assert_eq!(
        last_burn_hour(&reg, SID),
        Some(T0),
        "the module's hour is paid by nothing"
    );
    // Within the hour: nothing.
    let r = step(&mut reg, SID, T0 + HOUR - 1).expect("steps");
    assert_eq!((r.paid, r.burned), (0, 0));
    // Three hours missed and a part: three burns of 9.
    let r = step(&mut reg, SID, T0 + 3 * HOUR + 5).expect("steps");
    assert_eq!((r.paid, r.burned), (3, 27));
    assert_eq!(units(&reg), 1_000 - 30 - 27);
    assert_eq!(last_burn_hour(&reg, SID), Some(T0 + 3 * HOUR));
    // The same hour again, or a restart reading the row: no second burn.
    let before = units(&reg);
    let r = step(&mut reg, SID, T0 + 3 * HOUR + 99).expect("steps");
    assert_eq!((r.paid, r.burned), (0, 0));
    assert_eq!(units(&reg), before);
}

#[test]
fn plan_pays_whole_hours_and_stops_at_the_first_short_one() {
    assert_eq!(
        plan(100, 12, 3),
        Plan {
            paid: 3,
            burned: 36,
            short: false
        }
    );
    assert_eq!(
        plan(30, 12, 5),
        Plan {
            paid: 2,
            burned: 24,
            short: true
        }
    );
    assert_eq!(
        plan(11, 12, 1),
        Plan {
            paid: 0,
            burned: 0,
            short: true
        }
    );
    assert_eq!(
        plan(0, 12, 0),
        Plan {
            paid: 0,
            burned: 0,
            short: false
        }
    );
}

#[test]
fn the_oldest_stack_goes_first() {
    let mut reg = world();
    let old = load(&mut reg, FUEL, 5);
    let new = load(&mut reg, FUEL_2, 50);
    drain(&mut reg, 7).expect("takes");
    assert!(reg.item(old).is_none(), "the old stack went whole");
    assert_eq!(reg.item(new).expect("kept").quantity, 48);
    assert!(drain(&mut reg, 49).is_err());
    assert_eq!(units(&reg), 48, "a short take takes nothing");
}

// -- empty -----------------------------------------------------------------------------------

#[test]
fn a_short_bay_offlines_every_fuel_module_and_sends_198() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    fit_online(&mut reg, MOD_A, T0);
    fit_online(&mut reg, MOD_B, T0);
    fit_online(&mut reg, MOD_FREE, T0);
    // Drain the bay to less than the hour's 12.
    let left = units(&reg);
    drain(&mut reg, left - 5).expect("drained");
    assert_eq!(units(&reg), 5);
    step(&mut reg, SID, T0 + 100).expect("steps");
    let r = step(&mut reg, SID, T0 + HOUR).expect("steps");
    assert_eq!(r.burned, 0, "a short hour takes nothing");
    assert_eq!(units(&reg), 5);
    let mut offlined = r.offlined.clone();
    offlined.sort_unstable();
    assert_eq!(offlined, vec![MOD_A, MOD_B]);
    // The module that uses no fuel stays.
    assert_eq!(online_types(&reg), vec![MOD_FREE]);
    // 198 to the manager, with the module type ids, not to a plain member.
    let got = mails(&reg, BOSS, nt::STRUCTURE_SERVICES_OFFLINE);
    assert_eq!(got.len(), 1);
    let notif::Value::List(ids) = field(&got[0], "listOfServiceModuleIDs") else {
        panic!("a list");
    };
    let mut ids: Vec<i64> = ids
        .iter()
        .map(|v| match v {
            notif::Value::Int(n) => *n,
            other => panic!("{other:?}"),
        })
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![MOD_A, MOD_B]);
    assert_eq!(field(&got[0], "structureID"), &notif::Value::Int(SID));
    assert!(mails(&reg, MEMBER, nt::STRUCTURE_SERVICES_OFFLINE).is_empty());
    // Nothing burns now, so the row goes with the next step.
    step(&mut reg, SID, T0 + 2 * HOUR).expect("steps");
    assert_eq!(last_burn_hour(&reg, SID), None);
    assert_eq!(
        mails(&reg, BOSS, nt::STRUCTURE_SERVICES_OFFLINE).len(),
        1,
        "once"
    );
}

// -- the online cost ---------------------------------------------------------------------------

#[test]
fn onlining_pays_the_modified_cost_and_a_short_bay_refuses() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    // 40 before the hull's -25: 30 comes out of the bay.
    assert!(onlined(&fit_online(&mut reg, MOD_A, T0)));
    assert_eq!(units(&reg), 70);
    // A second module whose cost (10) the bay can pay.
    assert!(onlined(&fit_online(&mut reg, MOD_B, T0)));
    assert_eq!(units(&reg), 60);
    // Drain to 4: the next onlining (the free one costs 0) refuses for a
    // module that costs 10.
    drain(&mut reg, 56).expect("drained");
    let m = module_of(&reg, MOD_B);
    svc::force_offline(&mut reg, SID, m.item_id, T0).expect("offline");
    let s = the_structure(&reg);
    let m = module_of(&reg, MOD_B);
    let e = pay_online(&mut reg, &s, &m, T0).expect_err("4 < 10");
    assert_eq!(svc::error_key(&e), "ServiceRequiresFuel");
    assert_eq!(units(&reg), 4, "a refusal takes nothing");
    let line = fit_online(&mut reg, MOD_B, T0);
    assert!(line.contains("ServiceRequiresFuel"), "{line}");
    assert!(!online_types(&reg).contains(&MOD_B));
}

#[test]
fn the_online_cost_starts_the_burn_at_the_next_hour() {
    let mut reg = world();
    load(&mut reg, FUEL, 1_000);
    assert_eq!(last_burn_hour(&reg, SID), None);
    fit_online(&mut reg, MOD_A, T0 + 5);
    assert_eq!(last_burn_hour(&reg, SID), Some(T0));
    let r = step(&mut reg, SID, T0 + 10).expect("steps");
    assert_eq!(r.paid, 0, "the hour it was onlined in is free");
}

// -- the expiry -----------------------------------------------------------------------------------

#[test]
fn fuel_expires_is_the_clients_formula() {
    let mut reg = world();
    // Nothing burns: no expiry, with fuel or without.
    assert_eq!(expires_at(&reg, &the_structure(&reg), T0), None);
    load(&mut reg, FUEL, 100);
    assert_eq!(expires_at(&reg, &the_structure(&reg), T0), None);
    fit_online(&mut reg, MOD_A, T0);
    let s = the_structure(&reg);
    // 70 units at 9 an hour: 7 whole hours from the top of the current hour.
    assert_eq!(units(&reg), 70);
    assert_eq!(expires_at(&reg, &s, T0 + 1_234), Some(T0 + 7 * HOUR));
    assert_eq!(expires_at(&reg, &s, T0 + HOUR - 1), Some(T0 + 7 * HOUR));
    assert_eq!(expires_at(&reg, &s, T0 + HOUR), Some(T0 + 8 * HOUR));
    // The cap.
    load(&mut reg, FUEL, 9 * (MAX_EXPIRY_HOURS + 100));
    assert_eq!(expires_at(&reg, &s, T0), Some(T0 + MAX_EXPIRY_HOURS * HOUR));
    // The browser's answer is the same number, burn on or off.
    let PyValue::Int(v) = expires(&reg, &s) else {
        panic!("an int");
    };
    assert_eq!(v % HOUR, 0);
    reg.rules_mut().structure_fuel_burn = false;
    assert!(matches!(
        expires(&reg, &the_structure(&reg)),
        PyValue::Int(_)
    ));
}

#[test]
fn the_directory_and_the_info_carry_fuel_expires_and_ozone() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    load(&mut reg, TYPE_LIQUID_OZONE, 33);
    fit_online(&mut reg, MOD_A, T0);
    let ask = |reg: &mut Registry, method: &str, args: &[PyValue]| {
        reg.dispatch(&crate::service::Request {
            service: "structureDirectory",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(BOSS),
        })
        .expect("answers")
    };
    let whole_hour = |v: &PyValue| match v {
        PyValue::Int(n) => *n == floor_hour(*n) && *n > 0,
        other => panic!("{other:?}"),
    };
    let dir = ask(&mut reg, "GetMyCorporationStructures", &[]);
    let PyValue::Dict(d) = &dir else {
        panic!("{dir:?}")
    };
    let entry = &d[0].0;
    let pick = |v: &PyValue, k: &str| crate::pyutil::dict_get(v, k.as_bytes()).cloned();
    assert!(whole_hour(&pick(entry, "fuelExpires").expect("key")));
    assert_eq!(pick(entry, "liquidOzoneQty"), Some(PyValue::Int(33)));
    let info = ask(&mut reg, "GetStructureInfo", &[PyValue::Int(SID)]);
    let PyValue::Instance { state, .. } = &info else {
        panic!("an attribute object expected, got {info:?}")
    };
    assert!(whole_hour(&pick(state, "fuelExpires").expect("key")));
}

// -- low fuel -------------------------------------------------------------------------------------------

#[test]
fn low_fuel_is_mailed_once_per_crossing() {
    let mut reg = world();
    // MOD_B burns 3 an hour. The bay holds one hour past the threshold.
    let per = 3;
    // The online cost (10), then one hour more than the threshold.
    load(&mut reg, FUEL, 10 + per * LOW_FUEL_HOURS + per);
    assert!(onlined(&fit_online(&mut reg, MOD_B, T0)));
    let mut at = T0;
    let mut hour = |reg: &mut Registry| {
        at += HOUR;
        step(reg, SID, at).expect("steps");
    };
    hour(&mut reg); // exactly the threshold: not below it
    assert!(mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT).is_empty());
    hour(&mut reg); // below
    let got = mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT);
    assert_eq!(got.len(), 1);
    let left = units(&reg);
    assert_eq!(
        field(&got[0], "listOfTypesAndQty"),
        &notif::Value::List(vec![notif::Value::Tuple(vec![
            notif::Value::Int(left),
            notif::Value::Int(FUEL)
        ])])
    );
    hour(&mut reg);
    hour(&mut reg);
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT).len(), 1, "once");
    // A refill lifts it above the threshold; the next crossing mails again.
    load(&mut reg, FUEL, per * LOW_FUEL_HOURS);
    hour(&mut reg);
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT).len(), 1);
    for _ in 0..LOW_FUEL_HOURS + 1 {
        hour(&mut reg);
    }
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT).len(), 2);
}

// -- the switch ------------------------------------------------------------------------------------------

#[test]
fn the_switch_is_off_by_default_and_off_changes_nothing() {
    let mut reg = world_with(content(true));
    assert!(!crate::rules::Rules::default().structure_fuel_burn);
    assert!(!enabled(&reg));
    // Free to online with an empty bay.
    let line = fit_online(&mut reg, MOD_A, T0);
    assert!(onlined(&line), "{line}");
    assert_eq!(online_types(&reg), vec![MOD_A]);
    assert_eq!(last_burn_hour(&reg, SID), None);
    // The clock burns nothing and offlines nothing, hours later.
    load(&mut reg, FUEL, 50);
    tick(&mut reg, Instant::now(), T0 + 10 * HOUR);
    assert_eq!(units(&reg), 50);
    assert_eq!(online_types(&reg), vec![MOD_A]);
    assert_eq!(last_burn_hour(&reg, SID), None);
    assert!(mails(&reg, BOSS, nt::STRUCTURE_FUEL_ALERT).is_empty());
    assert!(mails(&reg, BOSS, nt::STRUCTURE_SERVICES_OFFLINE).is_empty());
    // The GM's seam says so.
    assert!(gm_hours(&mut reg, SID, 3, T0).contains("off"));
    assert_eq!(units(&reg), 50);
    // The expiry is still computed, so the browser stays honest [I]: 50 at 9.
    assert_eq!(
        expires_at(&reg, &the_structure(&reg), T0),
        Some(T0 + 5 * HOUR)
    );
}

#[test]
fn turning_the_burn_off_drops_its_rows() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    fit_online(&mut reg, MOD_A, T0);
    assert!(last_burn_hour(&reg, SID).is_some());
    reg.rules_mut().structure_fuel_burn = false;
    tick(&mut reg, Instant::now(), T0 + HOUR);
    assert_eq!(last_burn_hour(&reg, SID), None);
    // And back on: the burn starts at the hour of the next step, not from the
    // hours it was off.
    reg.rules_mut().structure_fuel_burn = true;
    tick(&mut reg, Instant::now(), T0 + 50 * HOUR);
    assert_eq!(last_burn_hour(&reg, SID), Some(T0 + 50 * HOUR));
    assert_eq!(units(&reg), 100 - 30, "nothing for the hours it was off");
}

#[test]
fn the_tick_burns_every_structure_and_forgets_a_gone_one() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    fit_online(&mut reg, MOD_A, T0);
    tick(&mut reg, Instant::now(), T0 + 2 * HOUR);
    assert_eq!(units(&reg), 70 - 18);
    // A row with no structure behind it goes.
    reg.deltas_mut().put(
        KIND,
        vec!["555".to_owned()],
        vec![T0.to_string(), "0".to_owned()],
    );
    tick(&mut reg, Instant::now(), T0 + 3 * HOUR);
    assert!(reg.deltas().of(KIND).get(&vec!["555".to_owned()]).is_none());
}

#[test]
fn the_gm_seam_runs_hours_that_have_not_passed() {
    let mut reg = world();
    load(&mut reg, FUEL, 100);
    fit_online(&mut reg, MOD_A, T0);
    let line = gm_hours(&mut reg, SID, 2, T0);
    assert!(
        line.starts_with("2 of 2 hours paid, 18 units burned, 52 left"),
        "{line}"
    );
    assert_eq!(units(&reg), 52);
    // It does not borrow from the real clock: the row is on this hour.
    assert_eq!(last_burn_hour(&reg, SID), Some(T0));
    let r = step(&mut reg, SID, T0 + 10).expect("steps");
    assert_eq!(r.burned, 0);
    // The command parses, and refuses what it cannot read.
    let usage = |args: &[&str]| match slash(&mut world(), Some(BOSS), args) {
        Ok(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(usage(&["x"]).starts_with("/strfuel"));
    assert!(usage(&["1", "hours", "0"]).starts_with("/strfuel"));
    assert!(usage(&["1", "hours", "x"]).starts_with("/strfuel"));
    assert_eq!(usage(&["999", "hours", "1"]), "no structure 999");
}

// -- the notices that follow the upkeep, with the burn off -------------------------------------------------

/// One world-clock tick of the structures at `at`.
fn space_tick(reg: &mut Registry, at: i64) {
    crate::structure_space::tick(reg, Instant::now(), at);
}

#[test]
fn notices_207_208_follow_upkeep_with_the_burn_off() {
    let mut reg = world_with(content(true));
    assert!(!enabled(&reg));
    reg.rules_mut().upwell_free_upkeep = false;
    let at = crate::pyutil::filetime_now();
    // Full power at its birth; with `freeupkeep` off the tick finds no module.
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Full));
    assert!(mails(&reg, BOSS, nt::STRUCTURE_WENT_LOW_POWER).is_empty());
    space_tick(&mut reg, at);
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Low));
    let low = mails(&reg, BOSS, nt::STRUCTURE_WENT_LOW_POWER);
    assert_eq!(low.len(), 1);
    assert_eq!(field(&low[0], "structureID"), &notif::Value::Int(SID));
    assert!(mails(&reg, MEMBER, nt::STRUCTURE_WENT_LOW_POWER).is_empty());
    // A module online (free with the burn off) brings it back.
    assert!(onlined(&fit_online(&mut reg, MOD_A, at)));
    space_tick(&mut reg, at + structure::SECOND);
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Full));
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_WENT_HIGH_POWER).len(), 1);
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_WENT_LOW_POWER).len(), 1);
}

/// The fuel stage and the structure stage, as the clock runs them.
fn run(reg: &mut Registry, at: i64) {
    tick(reg, Instant::now(), at);
    space_tick(reg, at);
}

#[test]
fn an_empty_bay_takes_the_structure_to_low_power_and_back() {
    let mut reg = world();
    reg.rules_mut().upwell_free_upkeep = false;
    load(&mut reg, FUEL, 100);
    let at = floor_hour(crate::pyutil::filetime_now());
    assert!(onlined(&fit_online(&mut reg, MOD_A, at)));
    run(&mut reg, at + structure::SECOND);
    run(&mut reg, at + 2 * structure::SECOND);
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Full));
    let left = units(&reg);
    drain(&mut reg, left - 1).expect("drained");
    run(&mut reg, at + HOUR);
    run(&mut reg, at + HOUR + structure::SECOND);
    assert!(online_types(&reg).is_empty(), "the module went offline");
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Low));
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_SERVICES_OFFLINE).len(), 1);
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_WENT_LOW_POWER).len(), 1);
    // Refilled and onlined again: full power, mail 208.
    load(&mut reg, FUEL, 500);
    assert!(onlined(&fit_online(
        &mut reg,
        MOD_B,
        at + HOUR + 2 * structure::SECOND
    )));
    run(&mut reg, at + HOUR + 3 * structure::SECOND);
    assert_eq!(the_structure(&reg).upkeep, Some(Upkeep::Full));
    assert_eq!(mails(&reg, BOSS, nt::STRUCTURE_WENT_HIGH_POWER).len(), 1);
}

#[test]
fn notice_249_follows_the_abandon_clock() {
    let mut reg = world_with(content(true));
    reg.rules_mut().upwell_free_upkeep = false;
    let at = crate::pyutil::filetime_now();
    let mut s = the_structure(&reg);
    s.upkeep = Some(Upkeep::Low);
    // Two and a half days left: inside the warning window.
    s.abandon_at = Some(at + (2 * 86_400 + 43_200) * structure::SECOND);
    s.abandon_warned = false;
    structure::put(reg.deltas_mut(), &s);
    crate::structure_space::tick(&mut reg, Instant::now(), at);
    let got = mails(&reg, BOSS, nt::STRUCTURE_IMPENDING_ABANDONMENT);
    assert_eq!(got.len(), 1);
    assert_eq!(field(&got[0], "isCorpOwned"), &notif::Value::Bool(true));
    assert_eq!(field(&got[0], "daysUntilAbandon"), &notif::Value::Int(2));
    // Once, until the warning is cleared.
    crate::structure_space::tick(&mut reg, Instant::now(), at + structure::SECOND);
    assert_eq!(
        mails(&reg, BOSS, nt::STRUCTURE_IMPENDING_ABANDONMENT).len(),
        1
    );
}
