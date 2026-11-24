//! #41 UPWELL-FIT: fit, online, offline and unfit at the controls, and the
//! services that follow. Every id and every table is invented; the service
//! and setting ids are the protocol's, named by their constants.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::service::Request;
use crate::space_state::{Motion, Presence};

const BOSS: i64 = 7;
const PAL: i64 = 8;
const FOE: i64 = 9;
const CORP: i64 = 11;
const FOE_CORP: i64 = 13;
const SYSTEM: i64 = 31;
const HARBOUR: i64 = 6_001;
/// An invented hull with four service slots, a 100 CPU and 50 power budget,
/// a hull hit-point multiplier of 1.5 and a core type.
const HULL: i64 = 9_402_001;
const SID: i64 = 9_402_100;
const GROUP_MODULE: i64 = 9_404_001;
const CLONE: i64 = 9_403_001;
const REPROC: i64 = 9_403_002;
const MARKET: i64 = 9_403_003;
const OTHER_HULL_ONLY: i64 = 9_403_004;
const SINGLE: i64 = 9_403_005;
const NO_HISEC: i64 = 9_403_006;
const DRILL: i64 = 9_403_007;
const HOG_CPU: i64 = 9_403_008;
const HOG_POWER: i64 = 9_403_009;
const A_ROCK: i64 = 9_402_555;
const MOON: i64 = 9_405_001;
/// Invented systems of `Universe::fixture` (a high-sec and a null-sec one).
const HIGH_SYSTEM: i64 = 39_000_001;
const NULL_SYSTEM: i64 = 39_000_003;
/// What the invented clone module's 2744 says.
const CLONE_MULT: f64 = 2.5;
const HULL_MULT: f64 = 1.5;

const TABLE: ServiceTable<'static> = &[
    (CLONE, &[SERVICE_MEDICAL]),
    (REPROC, &[SERVICE_REPROCESSING]),
    (MARKET, &[SERVICE_MARKET]),
    (OTHER_HULL_ONLY, &[SERVICE_MEDICAL]),
    (SINGLE, &[SERVICE_MEDICAL]),
    (NO_HISEC, &[SERVICE_REPROCESSING]),
    (DRILL, &[SERVICE_MOONMINING]),
    (HOG_CPU, &[SERVICE_REPROCESSING]),
    (HOG_POWER, &[SERVICE_REPROCESSING]),
];

fn module_row(id: i64, attrs: &str) -> String {
    format!(
        "type\t{id}\t{GROUP_MODULE}\t{CATEGORY_STRUCTURE_MODULE}\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t{EFFECT_SERVICE_SLOT}\t{attrs}\tInvented Module {id}\n"
    )
}

fn content(beacon: bool) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let mut rows = crate::typedata::FIXTURE.to_owned();
    rows.push_str(&format!(
        "type\t{HULL}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t{ATTR_CPU_OUTPUT}=100.0;{ATTR_POWER_OUTPUT}=50.0;{ATTR_SERVICE_SLOTS}=4.0;{ATTR_HULL_HP_MULTIPLIER}={HULL_MULT};3101=9.0\tInvented Hull\n"
    ));
    rows.push_str(&module_row(
        CLONE,
        &format!("{ATTR_CPU}=40.0;{ATTR_POWER}=20.0;{ATTR_SERVICE_HP_MULTIPLIER}={CLONE_MULT}"),
    ));
    rows.push_str(&module_row(
        REPROC,
        &format!("{ATTR_CPU}=40.0;{ATTR_POWER}=20.0"),
    ));
    rows.push_str(&module_row(MARKET, &format!("{ATTR_CPU}=1.0")));
    // The client's own Cloning Center type, with an invented row.
    rows.push_str(&module_row(
        TYPE_STANDUP_CLONING_CENTER,
        &format!("{ATTR_CPU}=1.0;{ATTR_POWER}=1.0"),
    ));
    rows.push_str(&module_row(OTHER_HULL_ONLY, "1302=9402999.0"));
    rows.push_str(&module_row(
        SINGLE,
        &format!(
            "{ATTR_CPU}=1.0;{}=1.0",
            crate::fitting::ATTR_MAX_TYPE_FITTED
        ),
    ));
    rows.push_str(&module_row(NO_HISEC, &format!("{ATTR_HIGH_SEC_BAN}=1.0")));
    rows.push_str(&module_row(DRILL, &format!("{ATTR_CPU}=1.0")));
    rows.push_str(&module_row(HOG_CPU, &format!("{ATTR_CPU}=30.0")));
    rows.push_str(&module_row(HOG_POWER, &format!("{ATTR_POWER}=40.0")));
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    if beacon {
        let beacons =
            crate::moon_beacon::Beacons::parse(&[format!("{MOON}\t{SYSTEM}\t0.0\t0.0\t0.0\t0")])
                .expect("parses");
        c.insert("moon_beacons", beacons);
    }
    c
}

/// Three pilots in one system: two of one corporation (the boss a director),
/// one of another. A structure of the corporation's is online in the system.
fn world() -> Registry {
    world_with(content(false))
}

fn world_with(c: crate::content::Content) -> Registry {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
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
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
        )),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(c));
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
    let profile = sp::ensure_default(&mut reg, CORP);
    let e = structure::Env::new(crate::pyutil::filetime_now());
    let mut s = Structure::anchored(
        SID, HULL, CORP, SYSTEM, [0.0; 3], 0.0, "Invented", profile, &e,
    );
    s.set_state(structure::State::ShieldVulnerable, &e);
    structure::put(reg.deltas_mut(), &s);
    let _ = reg.take_pending();
    reg
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
    bind: Option<&PyValue>,
    kwargs: &[(PyValue, PyValue)],
) -> Result<PyValue, ServiceError> {
    let req = Request {
        service,
        method,
        args,
        kwargs,
        bind_params: bind,
        actor: Some(who),
    };
    reg.dispatch(&req)
}

fn dock(reg: &mut Registry, who: i64) {
    let ship = reg.character(who).unwrap().ship_id;
    call(
        reg,
        who,
        "structureDocking",
        "Dock",
        &[PyValue::Int(SID), PyValue::Int(ship)],
        None,
        &[],
    )
    .expect("docks");
}

fn take_control(reg: &mut Registry, who: i64) {
    call(
        reg,
        who,
        "structureControl",
        "TakeControl",
        &[PyValue::Int(SID)],
        None,
        &[],
    )
    .expect("takes the controls");
}

/// BOSS docked and at the controls.
fn at_the_controls() -> Registry {
    let mut reg = world();
    dock(&mut reg, BOSS);
    take_control(&mut reg, BOSS);
    let _ = reg.take_pending();
    reg
}

/// One unit of `type_id` in `who`'s hangar of the structure.
fn grant(reg: &mut Registry, who: i64, type_id: i64) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(who, SID, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("a new item")
}

fn key(r: &Result<impl std::fmt::Debug, ServiceError>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(e) => error_key(e),
    }
}

fn names(reg: &mut Registry) -> Vec<&'static str> {
    reg.take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect()
}

fn structure_row(reg: &Registry) -> Structure {
    structure::get(reg.deltas(), SID).expect("the structure")
}

fn online(reg: &Registry, module: i64) -> bool {
    fitted(reg, &structure_row(reg))
        .iter()
        .any(|f| f.item_id == module && f.online)
}

fn set(reg: &mut Registry, who: i64, module: i64, on: bool) -> Result<PyValue, ServiceError> {
    set_online_with(
        TABLE,
        reg,
        who,
        SID,
        module,
        on,
        crate::pyutil::filetime_now(),
    )
}

/// Fit a fresh `type_id` at `flag` as BOSS.
fn fit_new(reg: &mut Registry, type_id: i64, flag: i64) -> Result<i64, ServiceError> {
    let item = grant(reg, BOSS, type_id);
    fit(reg, BOSS, SID, item, flag)?;
    Ok(item)
}

// -- fit ----------------------------------------------------------------------

#[test]
fn fit_moves_the_item_to_the_slot_corp_owned_and_does_not_online() {
    let mut reg = at_the_controls();
    let item = grant(&mut reg, BOSS, CLONE);
    assert_eq!(fit(&mut reg, BOSS, SID, item, FIRST_SLOT), Ok(FIRST_SLOT));
    let i = reg.item(item).expect("still there");
    assert_eq!(
        (i.location_id, i.flag_id, i.owner_id, i.singleton),
        (SID, FIRST_SLOT, CORP, true)
    );
    let s = structure_row(&reg);
    let f = fitted(&reg, &s);
    assert_eq!(f.len(), 1);
    assert!(!f[0].online, "the client's own SetModuleOnline follows");
    assert!(reg.deltas().of(KIND).is_empty(), "no row was written");
    assert!(!has_online_module(&reg, &s));
}

#[test]
fn fit_needs_the_controls_and_the_role() {
    let mut reg = world();
    dock(&mut reg, BOSS);
    dock(&mut reg, PAL);
    // Docked, not at the controls.
    let item = grant(&mut reg, BOSS, CLONE);
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, item, FIRST_SLOT)),
        "CustomNotify"
    );
    // At the controls, but no station manager or director role.
    take_control(&mut reg, PAL);
    let mine = grant(&mut reg, PAL, CLONE);
    assert_eq!(
        key(&fit(&mut reg, PAL, SID, mine, FIRST_SLOT)),
        "CrpAccessDenied"
    );
    assert!(fitted(&reg, &structure_row(&reg)).is_empty());
    assert_eq!(reg.item(mine).unwrap().flag_id, crate::model::FLAG_HANGAR);
}

#[test]
fn fit_refuses_a_slot_past_the_hulls_count() {
    let mut reg = at_the_controls();
    // Four slots: 164..=167 exist, 168 does not.
    let item = grant(&mut reg, BOSS, CLONE);
    assert_eq!(key(&fit(&mut reg, BOSS, SID, item, 168)), "NoFreeShipSlots");
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, item, 172)),
        "CantFitModuleToThatShip"
    );
    assert_eq!(fit(&mut reg, BOSS, SID, item, 167), Ok(167));
}

#[test]
fn fit_refuses_a_taken_slot() {
    let mut reg = at_the_controls();
    fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    let second = grant(&mut reg, BOSS, REPROC);
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, second, FIRST_SLOT)),
        "SlotAlreadyOccupied"
    );
    // The autofit takes the next one.
    assert_eq!(
        fit(&mut reg, BOSS, SID, second, crate::fitting::FLAG_AUTOFIT),
        Ok(FIRST_SLOT + 1)
    );
}

#[test]
fn fit_refuses_a_module_the_hull_type_is_not_named_for() {
    let mut reg = at_the_controls();
    let named_for_another = grant(&mut reg, BOSS, OTHER_HULL_ONLY);
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, named_for_another, FIRST_SLOT)),
        "CantFitModuleToThatShip"
    );
    // Not a service module at all.
    let ship_module = grant(&mut reg, BOSS, 990_000_003);
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, ship_module, FIRST_SLOT)),
        "CantFitModuleToThatShip"
    );
    assert!(fitted(&reg, &structure_row(&reg)).is_empty());
}

#[test]
fn fit_refuses_a_second_of_a_max_one_type() {
    let mut reg = at_the_controls();
    fit_new(&mut reg, SINGLE, FIRST_SLOT).expect("fitted");
    let second = grant(&mut reg, BOSS, SINGLE);
    assert_eq!(
        key(&fit(&mut reg, BOSS, SID, second, FIRST_SLOT + 1)),
        "CantFitTooManyByType"
    );
}

// -- online and offline -------------------------------------------------------------

#[test]
fn online_answers_none_and_writes_the_row() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    let _ = reg.take_pending();
    assert_eq!(set(&mut reg, BOSS, m, true), Ok(PyValue::None));
    assert!(online(&reg, m));
    assert_eq!(reg.deltas().of(KIND).len(), 1);
    assert!(has_online_module(&reg, &structure_row(&reg)));
    let sent = names(&mut reg);
    assert!(sent.contains(&"OnGodmaShipEffect"), "{sent:?}");
    assert!(sent.contains(&"OnStructureServiceChanged"), "{sent:?}");
}

#[test]
fn online_twice_raises_effect_already_active2() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    set(&mut reg, BOSS, m, true).expect("online");
    assert_eq!(key(&set(&mut reg, BOSS, m, true)), "EffectAlreadyActive2");
    assert!(online(&reg, m), "and it stays online");
}

#[test]
fn online_refused_in_state_102() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    let mut s = structure_row(&reg);
    s.state = structure::State::OnliningVulnerable;
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(
        key(&set(&mut reg, BOSS, m, true)),
        "StructureCannotActivateModuleNotFullyOnline"
    );
    assert!(!online(&reg, m));
}

#[test]
fn online_refused_over_cpu_or_power() {
    let mut reg = at_the_controls();
    // 100 CPU and 50 power: two modules of 40 and 20 fill most of both.
    for (type_id, flag) in [(CLONE, 164), (REPROC, 165)] {
        let m = fit_new(&mut reg, type_id, flag).expect("fitted");
        set(&mut reg, BOSS, m, true).expect("online");
    }
    let cpu = fit_new(&mut reg, HOG_CPU, 166).expect("fitted");
    assert_eq!(key(&set(&mut reg, BOSS, cpu, true)), "NotEnoughCpu");
    let power = fit_new(&mut reg, HOG_POWER, 167).expect("fitted");
    assert_eq!(key(&set(&mut reg, BOSS, power, true)), "NotEnoughPower");
    assert!(!online(&reg, cpu) && !online(&reg, power));
    // Taking one of the two offline makes room.
    let first = fitted(&reg, &structure_row(&reg))[0].item_id;
    set(&mut reg, BOSS, first, false).expect("offline");
    assert_eq!(set(&mut reg, BOSS, cpu, true), Ok(PyValue::None));
}

#[test]
fn online_refused_on_a_type_without_online_services() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    let mut s = structure_row(&reg);
    s.type_id = TYPE_UPWELL_CYNOSURAL_BEACON;
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(
        key(&set(&mut reg, BOSS, m, true)),
        "StructureServiceUnavailable"
    );
    assert!(!online(&reg, m));
}

#[test]
fn online_refuses_a_service_this_server_does_not_have_yet() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, MARKET, FIRST_SLOT).expect("fitted");
    assert_eq!(key(&set(&mut reg, BOSS, m, true)), "CustomNotify");
    assert!(!online(&reg, m));
}

#[test]
fn online_a_drill_needs_a_beacon() {
    let mut without = at_the_controls();
    let m = fit_new(&mut without, DRILL, FIRST_SLOT).expect("fitted");
    assert_eq!(
        key(&set(&mut without, BOSS, m, true)),
        "CannotOnlineModuleMustBeNearMoonMiningBeacon"
    );
    let mut reg = world_with(content(true));
    dock(&mut reg, BOSS);
    take_control(&mut reg, BOSS);
    let m = fit_new(&mut reg, DRILL, FIRST_SLOT).expect("fitted");
    assert_eq!(set(&mut reg, BOSS, m, true), Ok(PyValue::None));
}

#[test]
fn online_a_highsec_banned_module_in_highsec_is_refused() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, NO_HISEC, FIRST_SLOT).expect("fitted");
    let mut s = structure_row(&reg);
    s.system = HIGH_SYSTEM;
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(key(&set(&mut reg, BOSS, m, true)), "CustomNotify");
    assert!(!online(&reg, m));
    s.system = NULL_SYSTEM;
    structure::put(reg.deltas_mut(), &s);
    assert_eq!(set(&mut reg, BOSS, m, true), Ok(PyValue::None));
}

#[test]
fn offline_drops_the_service_and_pushes_service_changed() {
    let mut reg = at_the_controls();
    reg.rules_mut().upwell_free_clone = false;
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    set(&mut reg, BOSS, m, true).expect("online");
    let s = structure_row(&reg);
    let medical = |reg: &Registry| services_of_with(TABLE, reg, &s)[&SERVICE_MEDICAL];
    assert_eq!(medical(&reg), SERVICE_STATE_ONLINE);
    let _ = reg.take_pending();
    assert_eq!(set(&mut reg, BOSS, m, false), Ok(PyValue::None));
    assert_eq!(medical(&reg), SERVICE_STATE_OFFLINE);
    assert!(reg.deltas().of(KIND).is_empty());
    let sent = names(&mut reg);
    assert!(sent.contains(&"OnStructureServiceChanged"), "{sent:?}");
    // Offline again is nothing.
    assert_eq!(set(&mut reg, BOSS, m, false), Ok(PyValue::None));
}

#[test]
fn unfit_offlines_and_returns_to_the_pilots_hangar() {
    let mut reg = at_the_controls();
    reg.rules_mut().upwell_free_clone = false;
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    set(&mut reg, BOSS, m, true).expect("online");
    let _ = reg.take_pending();
    unfit(&mut reg, BOSS, SID, m, 0).expect("unfitted");
    let i = reg.item(m).expect("kept");
    assert_eq!(
        (i.location_id, i.flag_id, i.owner_id),
        (SID, crate::model::FLAG_HANGAR, BOSS)
    );
    assert!(reg.deltas().of(KIND).is_empty());
    let s = structure_row(&reg);
    assert!(!has_online_module(&reg, &s));
    assert!(!services_of_with(TABLE, &reg, &s).contains_key(&SERVICE_MEDICAL));
    let sent = names(&mut reg);
    assert!(sent.contains(&"OnStructureServiceChanged"), "{sent:?}");
    // A module that is not fitted is refused.
    assert_eq!(
        key(&unfit(&mut reg, BOSS, SID, m, 0).map(|_| ())),
        "CustomNotify"
    );
}

// -- the services ----------------------------------------------------------------------

#[test]
fn services_with_maps_online_modules_and_hull_services() {
    let got = services_with(TABLE, HULL, true, &[(CLONE, true), (REPROC, true)], false);
    assert_eq!(
        got,
        BTreeMap::from([
            (SERVICE_DOCKING, SERVICE_STATE_ONLINE),
            (SERVICE_FITTING, SERVICE_STATE_ONLINE),
            (SERVICE_MEDICAL, SERVICE_STATE_ONLINE),
            (SERVICE_JUMP_CLONE, SERVICE_STATE_ONLINE),
            (SERVICE_REPROCESSING, SERVICE_STATE_ONLINE),
            (SERVICE_REPAIR, SERVICE_STATE_ONLINE),
            (SERVICE_INSURANCE, SERVICE_STATE_ONLINE),
        ])
    );
    // OFFICES is not a hull service yet; a hull that is offline has all
    // offline; the client's types with no online services have none.
    assert!(!got.contains_key(&SERVICE_OFFICES));
    let off = services_with(TABLE, HULL, false, &[(CLONE, true)], false);
    assert!(off.values().all(|s| *s == SERVICE_STATE_OFFLINE));
    for t in WITHOUT_ONLINE_SERVICES {
        assert!(services_with(TABLE, t, true, &[(CLONE, true)], true).is_empty());
    }
    // Two modules of one service: online wins.
    let two = services_with(
        TABLE,
        HULL,
        true,
        &[(REPROC, false), (HOG_CPU, true)],
        false,
    );
    assert_eq!(two[&SERVICE_REPROCESSING], SERVICE_STATE_ONLINE);
}

#[test]
fn an_offline_module_lists_its_service_offline() {
    let got = services_with(TABLE, HULL, true, &[(REPROC, false), (CLONE, false)], false);
    assert_eq!(got[&SERVICE_REPROCESSING], SERVICE_STATE_OFFLINE);
    assert_eq!(got[&SERVICE_MEDICAL], SERVICE_STATE_OFFLINE);
    assert_eq!(got[&SERVICE_JUMP_CLONE], SERVICE_STATE_OFFLINE);
    assert_eq!(got[&SERVICE_REPAIR], SERVICE_STATE_ONLINE, "the hull's own");
}

#[test]
fn free_clone_adds_medical_and_jump_clone() {
    let with = services_with(TABLE, HULL, true, &[], true);
    assert_eq!(with[&SERVICE_MEDICAL], SERVICE_STATE_ONLINE);
    assert_eq!(with[&SERVICE_JUMP_CLONE], SERVICE_STATE_ONLINE);
    let without = services_with(TABLE, HULL, true, &[], false);
    assert!(!without.contains_key(&SERVICE_MEDICAL));
    assert!(!without.contains_key(&SERVICE_JUMP_CLONE));
    // At the hull's state, as before there were modules.
    let offline_hull = services_with(TABLE, HULL, false, &[], true);
    assert_eq!(offline_hull[&SERVICE_MEDICAL], SERVICE_STATE_OFFLINE);
    // And the world's switch is what `services_of` reads: on, today's six.
    let reg = world();
    let s = structure_row(&reg);
    assert_eq!(
        services_of(&reg, &s).keys().copied().collect::<Vec<_>>(),
        vec![
            SERVICE_DOCKING,
            SERVICE_FITTING,
            SERVICE_MEDICAL,
            SERVICE_REPAIR,
            SERVICE_INSURANCE,
            SERVICE_JUMP_CLONE
        ]
    );
}

#[test]
fn the_owner_passes_every_setting() {
    let reg = world();
    let s = structure_row(&reg);
    for setting in [
        SETTING_MARKET_TAX,
        SETTING_REPROCESSING_TAX,
        SETTING_CLONINGBAY_TAX,
        SETTING_INDUSTRY_TAX,
        SETTING_JUMP_BRIDGE_ACTIVATION,
    ] {
        assert!(sp::has_access(&reg, &s, PAL, setting), "{setting}");
        assert!(!sp::has_access(&reg, &s, FOE, setting), "{setting}");
    }
}

#[test]
fn setting_of_service_ports_the_client_table() {
    // `structures/services.py` `SERVICES_ACCESS_SETTINGS`, row by row.
    let table = [
        (SERVICE_FITTING, Some(SETTING_HOUSING_CAN_DOCK)),
        (SERVICE_DOCKING, Some(SETTING_HOUSING_CAN_DOCK)),
        (SERVICE_OFFICES, Some(SETTING_CORP_RENT_OFFICE)),
        (SERVICE_REPROCESSING, Some(SETTING_REPROCESSING_TAX)),
        (SERVICE_MARKET, Some(SETTING_MARKET_TAX)),
        (SERVICE_MEDICAL, Some(SETTING_CLONINGBAY_TAX)),
        (SERVICE_INDUSTRY, None),
        (SERVICE_INSURANCE, Some(SETTING_HOUSING_CAN_DOCK)),
        (SERVICE_REPAIR, Some(SETTING_HOUSING_CAN_DOCK)),
        (
            SERVICE_MOONMINING,
            Some(SETTING_DEFENSE_CAN_CONTROL_STRUCTURE),
        ),
        (SERVICE_JUMP_BRIDGE, Some(SETTING_JUMP_BRIDGE_ACTIVATION)),
        (SERVICE_CYNO_BEACON, Some(SETTING_CYNO_BEACON)),
        (SERVICE_MANUFACTURING, None),
        (SERVICE_MANUFACTURING_BASIC, Some(SETTING_MANUFACTURING_TAX)),
        (
            SERVICE_MANUFACTURING_CAPITAL,
            Some(SETTING_MANUFACTURING_CAPITAL_TAX),
        ),
        (
            SERVICE_MANUFACTURING_SUPERCAPITAL,
            Some(SETTING_MANUFACTURING_SUPERCAPITAL_TAX),
        ),
        (SERVICE_LABORATORY, None),
        (SERVICE_LABORATORY_RESEARCH_TIME, Some(SETTING_RESEARCH_TAX)),
        (
            SERVICE_LABORATORY_RESEARCH_MATERIAL,
            Some(SETTING_RESEARCH_TAX),
        ),
        (SERVICE_LABORATORY_COPYING, Some(SETTING_RESEARCH_TAX)),
        (SERVICE_LABORATORY_INVENTION, Some(SETTING_INVENTION_TAX)),
        (SERVICE_REACTIONS, None),
        (
            SERVICE_REACTIONS_COMPOSITE,
            Some(SETTING_REACTION_COMPOSITE_TAX),
        ),
        (
            SERVICE_REACTIONS_BIOCHEMICAL,
            Some(SETTING_REACTION_BIOCHEMICAL_TAX),
        ),
        (SERVICE_REACTIONS_HYBRID, Some(SETTING_REACTION_HYBRID_TAX)),
        (SERVICE_LOYALTY_STORE, Some(SETTING_HOUSING_CAN_DOCK)),
        (SERVICE_AUTOMOONMINING, Some(SETTING_AUTOMOONMINING)),
        // The client has no row for these.
        (SERVICE_JUMP_CLONE, None),
        (SERVICE_CYNO_JAMMER, None),
        (SERVICE_MISSION, None),
    ];
    for (service, want) in table {
        assert_eq!(sp::setting_of_service(service), want, "service {service}");
    }
}

// -- upkeep and hit points ---------------------------------------------------------------

fn tick(reg: &mut Registry) {
    let mut s = structure_row(reg);
    let e = crate::structure_space::env_for(reg, &s, crate::pyutil::filetime_now());
    crate::structure_space::settle(&mut s, &e);
    structure::put(reg.deltas_mut(), &s);
}

fn upkeep(reg: &Registry) -> Option<structure::Upkeep> {
    structure_row(reg).upkeep
}

#[test]
fn has_online_module_drives_upkeep_to_low_power_when_freeupkeep_off() {
    // On (today): full power with nothing fitted.
    let mut on = at_the_controls();
    tick(&mut on);
    assert_eq!(upkeep(&on), Some(structure::Upkeep::Full));
    // Off: low power, then full with a module online, low again when it goes.
    let mut reg = at_the_controls();
    reg.rules_mut().upwell_free_upkeep = false;
    tick(&mut reg);
    assert_eq!(upkeep(&reg), Some(structure::Upkeep::Low));
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    tick(&mut reg);
    assert_eq!(
        upkeep(&reg),
        Some(structure::Upkeep::Low),
        "offline: no help"
    );
    set(&mut reg, BOSS, m, true).expect("online");
    tick(&mut reg);
    assert_eq!(upkeep(&reg), Some(structure::Upkeep::Full));
    set(&mut reg, BOSS, m, false).expect("offline");
    tick(&mut reg);
    assert_eq!(upkeep(&reg), Some(structure::Upkeep::Low));
}

#[test]
fn rows_survive_a_restart_and_a_stale_row_is_ignored() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    set(&mut reg, BOSS, m, true).expect("online");
    let mut again = world_with(content(false));
    again.apply_snapshot(reg.to_snapshot());
    assert!(online(&again, m), "online after a restart");
    // A row for a module that is not fitted is no module online.
    again
        .deltas_mut()
        .put(KIND, row_key(SID, 9_999_999), vec!["1".to_owned()]);
    let s = structure_row(&again);
    assert_eq!(fitted(&again, &s).len(), 1);
    // The next write for the structure purges it.
    let second = grant(&mut again, BOSS, REPROC);
    let _ = second;
    purge_stale(&mut again, &s);
    assert_eq!(again.deltas().of(KIND).len(), 1);
    assert!(online(&again, m));
}

#[test]
fn destroy_drops_the_rows() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    set(&mut reg, BOSS, m, true).expect("online");
    reg.deltas_mut()
        .put(KIND, row_key(A_ROCK, m), vec!["1".to_owned()]);
    assert_eq!(reg.deltas().of(KIND).len(), 2);
    forget(&mut reg, SID);
    assert_eq!(reg.deltas().of(KIND).len(), 1, "only the other's remains");
    forget(&mut reg, A_ROCK);
    assert!(reg.deltas().of(KIND).is_empty());
}

#[test]
fn hp_multiplier_follows_the_online_module() {
    let mut reg = at_the_controls();
    // At full power (an anchored row starts at low until it ticks).
    let mut full = structure_row(&reg);
    full.upkeep = Some(structure::Upkeep::Full);
    structure::put(reg.deltas_mut(), &full);
    let s = |reg: &Registry| structure_row(reg);
    // Nothing online, `freeupkeep` on: the old full-power number.
    assert_eq!(hp_multiplier(&reg, &s(&reg)), FULL_POWER_HP_MULTIPLIER);
    // Off: the hull's own.
    reg.rules_mut().upwell_free_upkeep = false;
    assert_eq!(hp_multiplier(&reg, &s(&reg)), HULL_MULT);
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    assert_eq!(
        hp_multiplier(&reg, &s(&reg)),
        HULL_MULT,
        "fitted, not online"
    );
    set(&mut reg, BOSS, m, true).expect("online");
    assert_eq!(hp_multiplier(&reg, &s(&reg)), CLONE_MULT);
    // Low power is the hull's again.
    let mut low = s(&reg);
    low.upkeep = Some(structure::Upkeep::Low);
    assert_eq!(hp_multiplier(&reg, &low), HULL_MULT);
}

#[test]
fn the_hull_attributes_carry_slots_and_core_type() {
    let reg = world();
    let base = PyValue::Dict(vec![(PyValue::Real(1.0), PyValue::Int(9))]);
    let PyValue::Dict(d) = hull_attributes(reg.content(), HULL, base) else {
        panic!("a dict")
    };
    let get = |a: i64| {
        d.iter().find_map(|(v, k)| match (v, k) {
            (PyValue::Real(v), PyValue::Int(k)) if *k == a => Some(*v),
            _ => None,
        })
    };
    assert_eq!(get(ATTR_SERVICE_SLOTS), Some(4.0));
    assert_eq!(get(ATTR_CPU_OUTPUT), Some(100.0));
    assert_eq!(get(ATTR_POWER_OUTPUT), Some(50.0));
    assert_eq!(get(3101), Some(9.0));
    assert_eq!(get(9), Some(1.0), "the base is kept");
    assert_eq!(get(1137), None, "only what the hull row has");
}

// -- the wire ------------------------------------------------------------------------------

#[test]
fn the_clients_calls_fit_online_and_unfit_through_dispatch() {
    let mut reg = at_the_controls();
    // The wire onlines under the client's own table, so the client's own type.
    let item = grant(&mut reg, BOSS, TYPE_STANDUP_CLONING_CENTER);
    let bound = PyValue::Tuple(vec![PyValue::Int(SID), PyValue::None]);
    let flag =
        |f: i64| crate::pyutil::pypairs(vec![("flag", PyValue::Int(f)), ("qty", PyValue::Int(1))]);
    // Fit: `Add(itemID, source, qty=1, flag=slot)` on the structure's inventory.
    call(
        &mut reg,
        BOSS,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(item), PyValue::Int(SID)],
        Some(&bound),
        &flag(FIRST_SLOT),
    )
    .expect("fitted");
    assert_eq!(reg.item(item).unwrap().flag_id, FIRST_SLOT);
    assert!(!online(&reg, item));
    // Online: `SetModuleOnline(structureID, moduleID)`.
    let args = [PyValue::Int(SID), PyValue::Int(item)];
    assert_eq!(
        call(
            &mut reg,
            BOSS,
            "dogmaIM",
            "SetModuleOnline",
            &args,
            None,
            &[]
        ),
        Ok(PyValue::None)
    );
    assert!(online(&reg, item));
    assert_eq!(
        key(&call(
            &mut reg,
            BOSS,
            "dogmaIM",
            "SetModuleOnline",
            &args,
            None,
            &[]
        )),
        "EffectAlreadyActive2"
    );
    // godma meets the module and asks for it; the structure's dogma lists it.
    let info = call(
        &mut reg,
        BOSS,
        "dogmaIM",
        "ItemGetInfo",
        &[PyValue::Int(item)],
        None,
        &[],
    )
    .expect("answered");
    assert_ne!(info, PyValue::None, "the row, not the miss");
    assert_eq!(info_entries(&reg, SID).len(), 1);
    // Offline, then unfit: `Add(moduleID, structureID, qty=None, flag=4)`.
    assert_eq!(
        call(
            &mut reg,
            BOSS,
            "dogmaIM",
            "TakeModuleOffline",
            &args,
            None,
            &[]
        ),
        Ok(PyValue::None)
    );
    assert!(!online(&reg, item));
    call(
        &mut reg,
        BOSS,
        "invbroker::inventory",
        "Add",
        &[PyValue::Int(item), PyValue::Int(SID)],
        Some(&bound),
        &crate::pyutil::pypairs(vec![
            ("flag", PyValue::Int(crate::model::FLAG_HANGAR)),
            ("qty", PyValue::None),
        ]),
    )
    .expect("unfitted");
    let i = reg.item(item).unwrap();
    assert_eq!((i.location_id, i.flag_id), (SID, crate::model::FLAG_HANGAR));
    assert_eq!(i.owner_id, BOSS);
}

#[test]
fn set_module_online_on_a_ship_still_takes_the_ship_path() {
    let mut reg = at_the_controls();
    let m = fit_new(&mut reg, CLONE, FIRST_SLOT).expect("fitted");
    let ship = reg.character(BOSS).unwrap().ship_id;
    // The ship's id: the ship path, which has no such module on the ship.
    let on_ship = call(
        &mut reg,
        BOSS,
        "dogmaIM",
        "SetModuleOnline",
        &[PyValue::Int(ship), PyValue::Int(m)],
        None,
        &[],
    );
    assert_eq!(key(&on_ship), "CustomNotify");
    assert!(!online(&reg, m), "the structure's module was not touched");
    // A pilot who is not at the controls reaches the ship path with the
    // structure's id too.
    take_control_released(&mut reg);
    let not_controlling = call(
        &mut reg,
        BOSS,
        "dogmaIM",
        "SetModuleOnline",
        &[PyValue::Int(SID), PyValue::Int(m)],
        None,
        &[],
    );
    assert_eq!(key(&not_controlling), "CustomNotify");
    assert!(!online(&reg, m));
}

fn take_control_released(reg: &mut Registry) {
    call(
        reg,
        BOSS,
        "structureControl",
        "ReleaseControl",
        &[],
        None,
        &[],
    )
    .expect("released");
}

#[test]
fn the_lobby_follows_the_clients_own_module_table() {
    let mut reg = at_the_controls();
    reg.rules_mut().upwell_free_clone = false;
    let services = |reg: &mut Registry| {
        let PyValue::Dict(d) = call(
            reg,
            BOSS,
            "structureSettings",
            "CharacterGetServices",
            &[PyValue::Int(SID)],
            None,
            &[],
        )
        .expect("answered") else {
            panic!("a dict")
        };
        d.into_iter()
            .filter_map(|(_, k)| match k {
                PyValue::Int(i) => Some(i),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(!services(&mut reg).contains(&SERVICE_MEDICAL));
    let m = fit_new(&mut reg, TYPE_STANDUP_CLONING_CENTER, FIRST_SLOT).expect("fitted");
    assert!(!services(&mut reg).contains(&SERVICE_MEDICAL), "offline");
    let now = crate::pyutil::filetime_now();
    assert_eq!(
        set_online(&mut reg, BOSS, SID, m, true, now),
        Ok(PyValue::None)
    );
    let on = services(&mut reg);
    assert!(on.contains(&SERVICE_MEDICAL) && on.contains(&SERVICE_JUMP_CLONE));
    set_online(&mut reg, BOSS, SID, m, false, now).expect("offline");
    assert!(!services(&mut reg).contains(&SERVICE_MEDICAL));
}

// -- the GM's tools --------------------------------------------------------------------------

#[test]
fn gm_fit_fits_and_onlines_without_the_role_but_keeps_the_other_checks() {
    let mut reg = world();
    let say = |reg: &mut Registry, who: i64, type_id: i64| {
        gm_fit_with(TABLE, reg, who, SID, type_id, crate::pyutil::filetime_now())
    };
    // PAL has no role and is not at the controls.
    let out = say(&mut reg, PAL, CLONE);
    assert!(out.contains("in slot 164, online"), "{out}");
    assert!(online(&reg, fitted(&reg, &structure_row(&reg))[0].item_id));
    // The other corporation's structure is not his.
    assert!(say(&mut reg, FOE, REPROC).contains("not your corporation's"));
    // Type, slot and the rest are still checked.
    assert!(say(&mut reg, PAL, OTHER_HULL_ONLY).contains("CantFitModuleToThatShip"));
    assert!(say(&mut reg, PAL, 424_242).contains("no such type"));
    let out = say(&mut reg, PAL, MARKET);
    assert!(out.contains("online refused (CustomNotify)"), "{out}");
    // The services line names the fitted modules.
    assert!(services_text(&reg, SID).contains("slot 164"));
}
