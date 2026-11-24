//! #57 UPWELL-INDUSTRY: the structure facility, its rates, the hull's terms,
//! and the jobs through a module going offline, online and unfitted. Every id
//! and table is invented; the service and setting ids and the Standup
//! Manufacturing Plant's type are the protocol's, named by their constants.

use super::*;
use crate::industry::{ACTIVITIES, FILETIME_SECOND, MANUFACTURING, REACTION};
use crate::item::{Edit, Slot};
use crate::model::{Character, SandboxConfig};
use crate::service::{Request, ServiceError};
use crate::space_state::{Motion, Presence};
use crate::structure_services as ss;

const BOSS: i64 = 7;
const FRIEND: i64 = 8;
const STRANGER: i64 = 9;
const CORP: i64 = 11;
const FRIEND_CORP: i64 = 12;
const STRANGER_CORP: i64 = 13;
const SYSTEM: i64 = 31;
const HARBOUR: i64 = 6_001;
const SID: i64 = 9_502_100;
/// An invented hull with two service slots, a 100 CPU and 100 power budget
/// and three invented multiplier attributes for manufacturing.
const HULL: i64 = 9_502_001;
const HULL_GROUP: i64 = 9_501_001;
const ATTR_TIME: i64 = 900_201;
const ATTR_MATERIAL: i64 = 900_202;
const ATTR_COST: i64 = 900_203;
const ATTR_RIG_TIME: i64 = 900_204;
/// An invented type that has modifier rows but no such attribute.
const BARE_HULL: i64 = 9_502_002;
const GROUP_MODULE: i64 = 9_504_001;
/// The invented blueprint of the fixture's afterburner and its mineral.
const BLUEPRINT: i64 = 21_858;
const TRITANIUM: i64 = 34;
/// The invented groups of the profile's two tax rows.
const THREE_PERCENT: f64 = 3.0;
const FIVE_PERCENT: f64 = 5.0;

fn module_row(id: i64, attrs: &str) -> String {
    format!(
        "type\t{id}\t{GROUP_MODULE}\t{}\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t{}\t{attrs}\tInvented Module {id}\n",
        ss::CATEGORY_STRUCTURE_MODULE,
        ss::EFFECT_SERVICE_SLOT,
    )
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let mut rows = crate::typedata::FIXTURE.to_owned();
    rows.push_str(&format!(
        "type\t{HULL}\t{HULL_GROUP}\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t{}=100.0;{}=100.0;{}=2.0;{ATTR_TIME}=0.85;{ATTR_MATERIAL}=0.9;{ATTR_COST}=0.97;{ATTR_RIG_TIME}=0.5\tInvented Hull\n",
        ss::ATTR_CPU_OUTPUT,
        ss::ATTR_POWER_OUTPUT,
        ss::ATTR_SERVICE_SLOTS,
    ));
    rows.push_str(&format!(
        "type\t{BARE_HULL}\t{HULL_GROUP}\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Bare Hull\n"
    ));
    // The client's own plant type, with an invented row.
    rows.push_str(&module_row(
        ss::TYPE_STANDUP_MANUFACTURING_PLANT,
        &format!("{}=10.0;{}=10.0", ss::ATTR_CPU, ss::ATTR_POWER),
    ));
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let mut industry = crate::industry::Table::fixture();
    industry.modifier_sources.insert(
        HULL,
        vec![
            source(MANUFACTURING, industry::ModKind::Time, ATTR_TIME, 0),
            source(MANUFACTURING, industry::ModKind::Material, ATTR_MATERIAL, 0),
            source(MANUFACTURING, industry::ModKind::Cost, ATTR_COST, 0),
            // A rig-style row: names an attribute and a filter, so a hull
            // read skips it.
            source(
                industry::REACTION,
                industry::ModKind::Time,
                ATTR_RIG_TIME,
                990_501,
            ),
        ],
    );
    industry.modifier_sources.insert(
        BARE_HULL,
        vec![source(MANUFACTURING, industry::ModKind::Time, ATTR_TIME, 0)],
    );
    c.insert("industry", industry);
    c
}

fn source(
    activity_id: i64,
    kind: industry::ModKind,
    attribute_id: i64,
    filter_id: i64,
) -> industry::ModifierSource {
    industry::ModifierSource {
        activity_id,
        kind,
        attribute_id,
        filter_id,
    }
}

/// Three pilots in one system: BOSS (a director) of the owner corporation,
/// FRIEND of another and STRANGER of a third, each docked in the structure.
fn world() -> Registry {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
        corporation_id: corp,
        solar_system_id: Some(SYSTEM),
        station_id: Some(SID),
        ship_id: 1_000 + id,
        ship_type_id: crate::service::TYPE_CAPSULE,
        balance: 1_000_000_000.0,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            ch(BOSS, CORP),
            ch(FRIEND, FRIEND_CORP),
            ch(STRANGER, STRANGER_CORP),
        ],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
        )),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content()));
    reg.corporations_mut()
        .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    reg.corporations_mut()
        .enrol_for_test(FRIEND, FRIEND_CORP, 0);
    reg.corporations_mut()
        .enrol_for_test(STRANGER, STRANGER_CORP, 0);
    for (n, who) in [BOSS, FRIEND, STRANGER].into_iter().enumerate() {
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
) -> Result<PyValue, ServiceError> {
    let req = Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    };
    reg.dispatch(&req)
}

/// BOSS at the controls of the structure (docked there already, by the
/// character row).
fn at_the_controls(reg: &mut Registry) {
    let ship = reg.character(BOSS).expect("boss").ship_id;
    call(
        reg,
        BOSS,
        "structureDocking",
        "Dock",
        &[PyValue::Int(SID), PyValue::Int(ship)],
    )
    .expect("docks");
    call(
        reg,
        BOSS,
        "structureControl",
        "TakeControl",
        &[PyValue::Int(SID)],
    )
    .expect("takes the controls");
    let _ = reg.take_pending();
}

/// A plant fitted and online, through the GM's route.
fn fit_the_plant(reg: &mut Registry) -> i64 {
    let said = ss::gm_fit(
        reg,
        BOSS,
        SID,
        ss::TYPE_STANDUP_MANUFACTURING_PLANT,
        crate::pyutil::filetime_now(),
    );
    assert!(said.contains("online"), "{said}");
    plant_id(reg)
}

fn plant_id(reg: &Registry) -> i64 {
    let s = structure::get(reg.deltas(), SID).expect("the structure");
    ss::fitted(reg, &s)
        .iter()
        .find(|f| f.type_id == ss::TYPE_STANDUP_MANUFACTURING_PLANT)
        .map(|f| f.item_id)
        .expect("the plant is fitted")
}

fn facility_of(reg: &Registry, who: i64) -> Option<Facility> {
    facility(reg, reg.character(who).expect("character"))
}

/// A profile row that gives `group` the setting at `value`, with the
/// groups `(corp, member)` created for the purpose.
fn tax_groups(reg: &mut Registry, rows: &[(i64, f64)]) {
    let pid = sp::ensure_default(reg, CORP);
    let mut settings = Vec::new();
    for (i, (corp, value)) in rows.iter().enumerate() {
        let group =
            og::create_for(reg, CORP, &format!("tax {i}"), "", &[(*corp, og::MEMBER)]).group_id;
        settings.push((SETTING_MANUFACTURING_TAX, group, *value));
    }
    sp::save_settings(reg.deltas_mut(), pid, &settings);
}

#[test]
fn no_online_industry_service_means_no_facility() {
    let mut reg = world();
    // No module at all: what the server answered before #57.
    assert!(facility_of(&reg, BOSS).is_none());
    assert!(reg.industry_facility(Some(BOSS)).is_none());
    assert_eq!(
        call(&mut reg, BOSS, "facilityManager", "GetFacilities", &[]).expect("answers"),
        PyValue::List(vec![])
    );
    assert!(
        call(
            &mut reg,
            BOSS,
            "facilityManager",
            "GetFacility",
            &[PyValue::Int(SID)]
        )
        .is_err()
    );
    // A module that is fitted but not online is none either.
    let plant = fit_the_plant(&mut reg);
    assert!(facility_of(&reg, BOSS).is_some());
    at_the_controls(&mut reg);
    ss::set_online(
        &mut reg,
        BOSS,
        SID,
        plant,
        false,
        crate::pyutil::filetime_now(),
    )
    .expect("offline");
    assert!(facility_of(&reg, BOSS).is_none());
}

#[test]
fn an_online_plant_gives_manufacturing_only() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    let f = facility_of(&reg, BOSS).expect("the plant is a facility");
    let t = f.structure.expect("a structure's terms");
    assert_eq!(t.activities(), &[MANUFACTURING]);
    assert_eq!(
        f.type_id, HULL,
        "the client reads the category of the facility's type"
    );
    assert_eq!(f.facility_id, SID);
    assert_eq!(f.owner_id, CORP);
    assert_eq!(f.tax, None);
    assert_eq!(
        t.access().iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![SERVICE_MANUFACTURING_BASIC]
    );
    // The same facility through `industry_facility`, which is what the
    // service layer asks.
    assert_eq!(reg.industry_facility(Some(BOSS)), Some(f));
}

/// PINNED [A] A10: the owner corporation pays 0. Falsifier: the owner wants
/// its corp taxed by its own profile.
#[test]
fn pinned_a10_the_owner_corp_pays_no_tax() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    // Even with a profile row for everyone else.
    tax_groups(&mut reg, &[(FRIEND_CORP, THREE_PERCENT)]);
    let f = facility_of(&reg, BOSS).expect("facility");
    assert_eq!(
        f.structure
            .expect("terms")
            .rate_of(SERVICE_MANUFACTURING_BASIC),
        Some(0.0)
    );
}

#[test]
fn a_guests_rate_is_one_of_its_groups_values_and_no_row_means_no_access() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    tax_groups(
        &mut reg,
        &[(FRIEND_CORP, FIVE_PERCENT), (FRIEND_CORP, THREE_PERCENT)],
    );
    let rate = facility_of(&reg, FRIEND)
        .and_then(|f| f.structure)
        .and_then(|t| t.rate_of(SERVICE_MANUFACTURING_BASIC))
        .expect("a guest with a row has a rate");
    let values = [FIVE_PERCENT / 100.0, THREE_PERCENT / 100.0];
    assert!(
        values.iter().any(|v| (v - rate).abs() < 1e-12),
        "{rate} is one of the guest's group values"
    );
    // No group holds STRANGER: no `serviceAccess` entry, so no access.
    let f = facility_of(&reg, STRANGER).expect("the structure's facility is the same for all");
    let t = f.structure.expect("terms");
    assert!(t.access().is_empty());
    assert_eq!(t.rate_of(SERVICE_MANUFACTURING_BASIC), None);
}

/// PINNED [A] A11: the lowest of the matching groups' values. Falsifier: TQ
/// charges the first matching group, or the highest.
#[test]
fn pinned_a11_a_guest_pays_the_lowest_matching_group_rate() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    tax_groups(
        &mut reg,
        &[(FRIEND_CORP, FIVE_PERCENT), (FRIEND_CORP, THREE_PERCENT)],
    );
    let rate = facility_of(&reg, FRIEND)
        .and_then(|f| f.structure)
        .and_then(|t| t.rate_of(SERVICE_MANUFACTURING_BASIC))
        .expect("a rate");
    assert!((rate - THREE_PERCENT / 100.0).abs() < 1e-12, "{rate}");
}

/// The structure's activities are its own online services', whoever asks: a
/// guest with no tax row sees manufacturing and is refused as
/// `FACILITY_DENIED`, never `FACILITY_ACTIVITY`.
#[test]
fn a_guest_without_a_tax_row_is_denied_not_refused_for_its_activity() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    let f = facility_of(&reg, STRANGER).expect("a facility");
    assert_eq!(f.structure.expect("terms").activities(), &[MANUFACTURING]);
    let refusal = plan_for(&mut reg, STRANGER, 1, MANUFACTURING).map(|_| ());
    assert_eq!(refusal, Err(industry::Refusal::FacilityDenied));
    // An activity the structure does not run is the activity refusal.
    let other = plan_for(&mut reg, STRANGER, 1, industry::RESEARCH_MATERIAL).map(|_| ());
    assert_eq!(
        other,
        Err(industry::Refusal::FacilityActivity(
            industry::RESEARCH_MATERIAL
        ))
    );
}

#[test]
fn hull_terms_read_the_type_attributes() {
    let reg = world();
    let terms = hull_terms(reg.content(), HULL);
    let at = |a: i64| ACTIVITIES.iter().position(|x| *x == a).expect("activity");
    assert_eq!(terms[at(MANUFACTURING)], (0.85, 0.9, 0.97));
    // The reaction row carries a filter (a rig's kind of row): not a hull's.
    assert_eq!(terms[at(REACTION)], (1.0, 1.0, 1.0));
    // A type whose named attribute is missing, and a type with no rows.
    assert_eq!(terms[at(industry::COPYING)], (1.0, 1.0, 1.0));
    assert_eq!(
        hull_terms(reg.content(), BARE_HULL)[at(MANUFACTURING)],
        (1.0, 1.0, 1.0)
    );
    assert_eq!(hull_terms(reg.content(), 1), [(1.0, 1.0, 1.0); 6]);
}

#[test]
fn service_id_ports_the_client_table() {
    const CAP: i64 = 9_503_001;
    const SUPER: i64 = 9_503_002;
    const BIO: i64 = 712;
    let any = |a, g| service_id_in(a, g, &[CAP], &[SUPER]);
    assert_eq!(any(MANUFACTURING, None), Some(SERVICE_MANUFACTURING_BASIC));
    assert_eq!(
        any(MANUFACTURING, Some(1)),
        Some(SERVICE_MANUFACTURING_BASIC)
    );
    assert_eq!(
        any(MANUFACTURING, Some(CAP)),
        Some(SERVICE_MANUFACTURING_CAPITAL)
    );
    assert_eq!(
        any(MANUFACTURING, Some(SUPER)),
        Some(SERVICE_MANUFACTURING_SUPERCAPITAL)
    );
    assert_eq!(
        any(industry::RESEARCH_MATERIAL, None),
        Some(SERVICE_LABORATORY_RESEARCH_MATERIAL)
    );
    assert_eq!(
        any(industry::RESEARCH_TIME, None),
        Some(SERVICE_LABORATORY_RESEARCH_TIME)
    );
    assert_eq!(
        any(industry::COPYING, None),
        Some(SERVICE_LABORATORY_COPYING)
    );
    assert_eq!(
        any(industry::INVENTION, None),
        Some(SERVICE_LABORATORY_INVENTION)
    );
    // An unknown reaction product is the client's meta service REACTIONS.
    assert_eq!(any(REACTION, None), Some(SERVICE_REACTIONS));
    assert_eq!(any(REACTION, Some(1)), Some(SERVICE_REACTIONS));
    assert_eq!(
        any(REACTION, Some(BIO)),
        Some(SERVICE_REACTIONS_BIOCHEMICAL)
    );
    assert_eq!(any(REACTION, Some(974)), Some(SERVICE_REACTIONS_HYBRID));
    assert_eq!(any(REACTION, Some(428)), Some(SERVICE_REACTIONS_COMPOSITE));
    assert_eq!(any(2, None), None);
    // The inverse.
    for svc in ss::INDUSTRY_SERVICES {
        if let Some(a) = activity_of(svc) {
            assert!(ACTIVITIES.contains(&a), "{svc}");
        }
    }
    assert_eq!(activity_of(SERVICE_MANUFACTURING), None);
    assert_eq!(activity_of(SERVICE_MARKET), None);
    // A basic plant refuses a capital product: no entry for that service.
    let mut terms = StructureTerms::EMPTY;
    terms.access[0] = (SERVICE_MANUFACTURING_BASIC, 0.0);
    terms.access_len = 1;
    let service = any(MANUFACTURING, Some(CAP)).expect("a service");
    assert_eq!(terms.rate_of(service), None);
    assert_eq!(terms.rate_of(SERVICE_MANUFACTURING_BASIC), Some(0.0));
}

/// The plan of one job of `who` in the structure with the REAL facility
/// (`facility()`), from a blueprint and minerals granted to `who` there.
fn plan_for(
    reg: &mut Registry,
    who: i64,
    runs: i64,
    activity: i64,
) -> Result<industry::Install, industry::Refusal> {
    reg.apply_items(&[
        Edit::Create {
            type_id: BLUEPRINT,
            quantity: 1,
            to: Slot::new(who, SID, crate::industry::FLAG_HANGAR),
        },
        Edit::Create {
            type_id: TRITANIUM,
            quantity: 400,
            to: Slot::new(who, SID, crate::industry::FLAG_HANGAR),
        },
    ])
    .expect("items");
    let blueprint_item = reg
        .store()
        .items()
        .iter()
        .rfind(|i| i.type_id == BLUEPRINT && i.owner_id == who)
        .expect("the blueprint")
        .clone();
    let blueprint = reg
        .jobs()
        .blueprint_item(reg.content().get(), &blueprint_item)
        .expect("a blueprint");
    let on_hand: Vec<industry::Stack> = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.owner_id == who && i.location_id == SID)
        .map(industry::Stack::of)
        .collect();
    let skills = crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
        type_id: industry::SKILL_INDUSTRY,
        level: 1,
        points: 0,
        rank: 1,
    }]);
    let pilot = industry::Pilot {
        character_id: who,
        corporation_id: None,
        skills: Some(&skills),
        implants: &[],
        balance: 1_000_000_000.0,
    };
    let f = facility_of(reg, who).expect("a facility");
    let request = industry::InstallRequest {
        activity_id: activity,
        blueprint_id: blueprint.item_id,
        runs,
        input_location_id: SID,
        input_flag_id: industry::FLAG_HANGAR,
        output_location_id: SID,
        output_flag_id: industry::FLAG_HANGAR,
        licensed_runs: 1,
        product_type_id: None,
        decryptor: None,
    };
    reg.jobs().plan_install(
        reg.content().get(),
        reg.content(),
        crate::pyutil::filetime_now(),
        &pilot,
        &f,
        &blueprint,
        &request,
        &on_hand,
    )
}

/// An installed manufacturing job in the structure, charged to BOSS.
fn install_job(reg: &mut Registry) -> i64 {
    let install = plan_for(reg, BOSS, 1, MANUFACTURING).expect("the job installs");
    reg.install_job(install).expect("installed")
}

/// PINNED [A] A12: pause on offline, resume with the lost time added.
/// Falsifier: a TQ job that keeps running with its service offline.
#[test]
fn pinned_a12_service_offline_pauses_and_online_resumes_with_the_lost_time_added() {
    let mut reg = world();
    let plant = fit_the_plant(&mut reg);
    at_the_controls(&mut reg);
    let job_id = install_job(&mut reg);
    let before = *reg.jobs().get(job_id).expect("job");
    assert_eq!(before.status, Status::Installed);
    assert_eq!(before.facility_id, SID);
    // The plant goes offline: the job pauses at once, and its owner is told.
    ss::set_online(
        &mut reg,
        BOSS,
        SID,
        plant,
        false,
        crate::pyutil::filetime_now(),
    )
    .expect("offline");
    let paused = *reg.jobs().get(job_id).expect("job");
    assert_eq!(paused.status, Status::Paused);
    assert!(paused.pause_date.is_some());
    assert_eq!(paused.end_date, before.end_date, "the end waits");
    let pushed: Vec<&'static str> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    assert!(
        pushed.contains(&"OnIndustryJob"),
        "the owner is told of the pause: {pushed:?}"
    );
    // The pilot refetches the facility (`OnFacilitiesUpdated([structure])`).
    assert!(pushed.contains(&"OnFacilitiesUpdated"), "{pushed:?}");
    // Pretend the plant has been offline for ninety seconds.
    let mut back_dated = paused;
    back_dated.pause_date = paused.pause_date.map(|p| p - 90 * FILETIME_SECOND);
    reg.jobs_mut().commit(back_dated);
    ss::set_online(
        &mut reg,
        BOSS,
        SID,
        plant,
        true,
        crate::pyutil::filetime_now(),
    )
    .expect("online");
    let resumed = *reg.jobs().get(job_id).expect("job");
    assert_eq!(resumed.status, Status::Installed);
    assert_eq!(resumed.pause_date, None);
    let added = resumed.end_date - before.end_date;
    assert!(
        (90 * FILETIME_SECOND..91 * FILETIME_SECOND).contains(&added),
        "the lost time is added to the end: {added}"
    );
}

/// PINNED [A] A12: cancel on unfit. Falsifier: TQ keeps it paused.
#[test]
fn pinned_a12_unfitting_the_last_module_cancels_its_jobs() {
    let mut reg = world();
    let plant = fit_the_plant(&mut reg);
    at_the_controls(&mut reg);
    let job_id = install_job(&mut reg);
    ss::unfit(&mut reg, BOSS, SID, plant, crate::pyutil::filetime_now()).expect("unfitted");
    let job = *reg.jobs().get(job_id).expect("job");
    assert_eq!(job.status, Status::Cancelled);
    // And with no module there is no facility again.
    assert!(facility_of(&reg, BOSS).is_none());
}

/// A copy of `job` installed at `facility` under a new id.
fn clone_job_at(reg: &mut Registry, job: industry::Job, id: i64, facility: i64) -> i64 {
    let mut j = job;
    j.job_id = id;
    j.facility_id = facility;
    j.blueprint_id += 10_000 * id;
    reg.jobs_mut().commit(j);
    id
}

#[test]
fn a_job_in_another_structure_or_station_is_left_alone() {
    let mut reg = world();
    let plant = fit_the_plant(&mut reg);
    at_the_controls(&mut reg);
    let mine = install_job(&mut reg);
    let template = *reg.jobs().get(mine).expect("job");
    // The same job at an NPC station and at another structure id.
    let at_station = clone_job_at(&mut reg, template, 9_901, HARBOUR);
    let elsewhere = clone_job_at(&mut reg, template, 9_902, SID + 1);
    let before = (
        *reg.jobs().get(at_station).expect("job"),
        *reg.jobs().get(elsewhere).expect("job"),
    );
    // Toggle the plant at SID (pauses the job there), then unfit it.
    let now = crate::pyutil::filetime_now();
    ss::set_online(&mut reg, BOSS, SID, plant, false, now).expect("offline");
    assert_eq!(reg.jobs().get(mine).expect("job").status, Status::Paused);
    ss::set_online(&mut reg, BOSS, SID, plant, true, now).expect("online");
    ss::unfit(&mut reg, BOSS, SID, plant, now).expect("unfitted");
    assert_eq!(reg.jobs().get(mine).expect("job").status, Status::Cancelled);
    // The other two are exactly as they were.
    assert_eq!(*reg.jobs().get(at_station).expect("job"), before.0);
    assert_eq!(*reg.jobs().get(elsewhere).expect("job"), before.1);
}

/// A finished job's product waits to be delivered: an unfit never destroys it.
#[test]
fn unfitting_the_module_leaves_a_finished_job_to_be_delivered() {
    let mut reg = world();
    let plant = fit_the_plant(&mut reg);
    at_the_controls(&mut reg);
    let job_id = install_job(&mut reg);
    let now = crate::pyutil::filetime_now();
    let mut done = *reg.jobs().get(job_id).expect("job");
    done.end_date = now - FILETIME_SECOND;
    reg.jobs_mut().commit(done);
    assert!(reg.jobs().get(job_id).expect("job").ready_at(now));
    ss::unfit(&mut reg, BOSS, SID, plant, now).expect("unfitted");
    let job = *reg.jobs().get(job_id).expect("job");
    assert_ne!(job.status, Status::Cancelled);
    assert!(job.ready_at(now));
}

/// The real facility drives `plan_install`: the hull's time and material
/// terms are in the job, against the same job at a station.
#[test]
fn the_real_facility_puts_the_hull_terms_into_the_job() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    let at_structure = plan_for(&mut reg, BOSS, 1, MANUFACTURING).expect("installs");
    // The same blueprint and skills at a station facility in the same place.
    let station = Facility::npc_station(SID, CORP, 2502, SYSTEM);
    let blueprint_item = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == BLUEPRINT && i.owner_id == BOSS)
        .expect("the blueprint")
        .clone();
    let blueprint = reg
        .jobs()
        .blueprint_item(reg.content().get(), &blueprint_item)
        .expect("a blueprint");
    let on_hand: Vec<industry::Stack> = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.owner_id == BOSS && i.location_id == SID)
        .map(industry::Stack::of)
        .collect();
    let skills = crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
        type_id: industry::SKILL_INDUSTRY,
        level: 1,
        points: 0,
        rank: 1,
    }]);
    let pilot = industry::Pilot {
        character_id: BOSS,
        corporation_id: None,
        skills: Some(&skills),
        implants: &[],
        balance: 1_000_000_000.0,
    };
    let request = industry::InstallRequest {
        activity_id: MANUFACTURING,
        blueprint_id: blueprint.item_id,
        runs: 1,
        input_location_id: SID,
        input_flag_id: industry::FLAG_HANGAR,
        output_location_id: SID,
        output_flag_id: industry::FLAG_HANGAR,
        licensed_runs: 1,
        product_type_id: None,
        decryptor: None,
    };
    let at_station = reg
        .jobs()
        .plan_install(
            reg.content().get(),
            reg.content(),
            crate::pyutil::filetime_now(),
            &pilot,
            &station,
            &blueprint,
            &request,
            &on_hand,
        )
        .expect("installs at the station");
    // Time: the hull's 0.85 (an invented attribute value), within the
    // rounding of a 120 second job.
    let ratio = at_structure.job.time_in_seconds as f64 / at_station.job.time_in_seconds as f64;
    assert!((ratio - 0.85).abs() < 0.02, "{ratio}");
    // Material: nine tenths of the base quantity (invented 0.9).
    let units = |i: &industry::Install| {
        i.materials
            .iter()
            .find(|m| m.type_id == TRITANIUM)
            .map(|m| m.quantity)
            .expect("tritanium")
    };
    assert_eq!(units(&at_structure) * 10, units(&at_station) * 9);
    assert!(at_structure.cost.cost < at_station.cost.cost);
}

/// A guest's tax is credited to the owner corporation's Master Wallet as a
/// transfer, journalled on both sides; the rest of the fee is a sink; the
/// counterparty is the structure's owner, and the owner pays no tax.
#[test]
fn a_guests_tax_is_credited_to_the_owner_corp_and_the_rest_is_a_sink() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    tax_groups(&mut reg, &[(FRIEND_CORP, THREE_PERCENT)]);
    let corp_before =
        crate::corp_wallet::balance(reg.deltas(), CORP, crate::corp_wallet::MASTER_KEY);
    let guest_before = reg.character(FRIEND).expect("guest").balance;
    let install = plan_for(&mut reg, FRIEND, 1, MANUFACTURING).expect("installs");
    let (cost, tax) = (install.cost, install.cost.tax);
    assert!(tax > 0, "a taxed job");
    let job_id = reg.install_job(install).expect("installed");
    let total = reg.jobs().get(job_id).expect("job").cost;
    assert_eq!(total, cost.total());
    // The guest paid the whole fee.
    let guest_after = reg.character(FRIEND).expect("guest").balance;
    assert!((guest_before - guest_after - total as f64).abs() < 1e-6);
    // The owner corp received exactly the tax.
    let corp_after =
        crate::corp_wallet::balance(reg.deltas(), CORP, crate::corp_wallet::MASTER_KEY);
    assert_eq!(
        corp_after - corp_before,
        crate::corp_wallet::to_cents(tax as f64)
    );
    // Conservation: guest debit == owner credit + the non-tax cost (the sink).
    let sink = (cost.cost + cost.scc_surcharge + cost.security_tax) as f64;
    assert!(
        ((guest_before - guest_after)
            - (crate::corp_wallet::to_isk(corp_after - corp_before) + sink))
            .abs()
            < 1e-6
    );
    // Both journals, with the job as the reference and the right owners.
    let ref_type = industry::Cost::ref_type(MANUFACTURING).expect("a ref type");
    let corp_rows = crate::corp_wallet::all_rows(reg.deltas(), CORP);
    let row = corp_rows
        .iter()
        .find(|r| r.reference == job_id)
        .expect("the corp's journal row");
    assert_eq!(
        (row.ref_type, row.owner1, row.owner2, row.amount),
        (
            ref_type,
            FRIEND,
            CORP,
            crate::corp_wallet::to_cents(tax as f64)
        )
    );
    let mine = reg
        .journal()
        .entries()
        .iter()
        .find(|e| e.reference_id == job_id && e.amount < 0.0)
        .expect("the guest's journal row");
    assert_eq!(
        (mine.entry_type_id, mine.owner_id_1, mine.owner_id_2),
        (ref_type, FRIEND, CORP)
    );
    // The owner pays no tax: its job moves nothing into the corp wallet.
    let before = crate::corp_wallet::balance(reg.deltas(), CORP, crate::corp_wallet::MASTER_KEY);
    let own = plan_for(&mut reg, BOSS, 1, MANUFACTURING).expect("installs");
    assert_eq!(own.cost.tax, 0);
    reg.install_job(own).expect("installed");
    assert_eq!(
        crate::corp_wallet::balance(reg.deltas(), CORP, crate::corp_wallet::MASTER_KEY),
        before
    );
}

#[test]
fn a_paused_job_cannot_be_delivered_and_can_be_cancelled() {
    let mut reg = world();
    let plant = fit_the_plant(&mut reg);
    at_the_controls(&mut reg);
    let job_id = install_job(&mut reg);
    ss::set_online(
        &mut reg,
        BOSS,
        SID,
        plant,
        false,
        crate::pyutil::filetime_now(),
    )
    .expect("offline");
    assert_eq!(reg.jobs().get(job_id).expect("job").status, Status::Paused);
    let args = [PyValue::Int(job_id), PyValue::Int(SYSTEM)];
    let delivered = call(&mut reg, BOSS, "industryManager", "CompleteJob", &args);
    assert!(delivered.is_err(), "a paused job is not ready");
    assert_eq!(reg.jobs().get(job_id).expect("job").status, Status::Paused);
    call(&mut reg, BOSS, "industryManager", "CancelJob", &args).expect("a paused job cancels");
    assert_eq!(
        reg.jobs().get(job_id).expect("job").status,
        Status::Cancelled
    );
}

/// `OnFacilitiesUpdated` goes out when an industry module is involved, and
/// not for a change in a structure with neither a module nor a job of its own.
#[test]
fn the_facility_refetch_is_sent_for_industry_changes_only() {
    let names = |reg: &mut Registry| -> Vec<&'static str> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push().0)
            .collect()
    };
    let mut reg = world();
    at_the_controls(&mut reg);
    on_services_changed(&mut reg, SID, crate::pyutil::filetime_now());
    assert!(!names(&mut reg).contains(&"OnFacilitiesUpdated"));
    fit_the_plant(&mut reg);
    let _ = names(&mut reg);
    on_services_changed(&mut reg, SID, crate::pyutil::filetime_now());
    assert!(names(&mut reg).contains(&"OnFacilitiesUpdated"));
    // Unfitting the plant tells the docked pilots too.
    let plant = plant_id(&reg);
    let _ = names(&mut reg);
    ss::unfit(&mut reg, BOSS, SID, plant, crate::pyutil::filetime_now()).expect("unfitted");
    assert!(names(&mut reg).contains(&"OnFacilitiesUpdated"));
}

fn row_of(reg: &mut Registry, who: i64) -> PyValue {
    call(
        reg,
        who,
        "facilityManager",
        "GetFacility",
        &[PyValue::Int(SID)],
    )
    .expect("a facility row")
}

#[test]
fn a_structure_facility_row_has_tax_none_and_service_access() {
    let mut reg = world();
    fit_the_plant(&mut reg);
    tax_groups(&mut reg, &[(FRIEND_CORP, THREE_PERCENT)]);
    let row = row_of(&mut reg, FRIEND);
    let get = |k: &[u8]| crate::pyutil::dict_get(&row, k).cloned().expect("key");
    assert_eq!(get(b"tax"), PyValue::None);
    assert_eq!(get(b"typeID"), PyValue::Int(HULL));
    assert_eq!(get(b"facilityID"), PyValue::Int(SID));
    let access = get(b"serviceAccess");
    let access = access.dict_as_kv().expect("a dict");
    assert_eq!(access.len(), 1);
    assert_eq!(*access[0].0, PyValue::Int(SERVICE_MANUFACTURING_BASIC));
    assert_eq!(*access[0].1, PyValue::Real(0.03));
    // The activities: manufacturing only, with the hull's three modifiers as
    // `(amount, None, None, None, HULL)`.
    let activities = get(b"activities");
    let kv = activities.dict_as_kv().expect("a dict");
    assert_eq!(kv.len(), 1);
    assert_eq!(*kv[0].0, PyValue::Int(MANUFACTURING));
    let PyValue::Tuple(six) = kv[0].1 else {
        panic!("a 6-tuple")
    };
    assert_eq!(six.len(), 6);
    let hull = |i: usize, amount: f64| {
        assert_eq!(
            six[i],
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Real(amount),
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::Int(12),
            ])])
        );
    };
    hull(0, 0.85);
    hull(1, 0.9);
    hull(2, 0.97);
    // `globalModifiers['system']` still holds a float for every activity
    // listed, and `rigModifiers` stays falsy.
    assert_eq!(get(b"rigModifiers"), PyValue::List(vec![]));
    let global = get(b"globalModifiers");
    let system = crate::pyutil::dict_get(&global, b"system").expect("system");
    let system = system.dict_as_kv().expect("a dict");
    assert!(
        system
            .iter()
            .any(|(k, _)| **k == PyValue::Int(MANUFACTURING))
    );
}

#[test]
fn a_structure_without_the_bundle_files_has_no_hull_terms_in_its_row() {
    // The old-bundle fallback: no modifier sources, so no 5-tuples.
    let mut c = content();
    c.insert("industry", industry::Table::fixture());
    let mut reg = world();
    reg = reg.with_content(std::sync::Arc::new(c));
    fit_the_plant(&mut reg);
    let row = row_of(&mut reg, BOSS);
    let activities = crate::pyutil::dict_get(&row, b"activities")
        .cloned()
        .expect("key");
    let kv = activities.dict_as_kv().expect("a dict");
    let PyValue::Tuple(six) = kv[0].1 else {
        panic!("a 6-tuple")
    };
    assert_eq!(six[0], PyValue::List(vec![]));
    assert_eq!(six[1], PyValue::List(vec![]));
    assert_eq!(six[2], PyValue::List(vec![]));
    // And the job's time is the plain one: no hull multiplier.
    assert_eq!(
        facility_of(&reg, BOSS)
            .expect("facility")
            .multipliers(MANUFACTURING),
        (1.0, 1.0, 1.0)
    );
}
