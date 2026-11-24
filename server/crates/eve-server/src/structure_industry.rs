//! #57 UPWELL-INDUSTRY: industry jobs in a structure with an online industry
//! service module.
//!
//! A pilot docked in a structure gets an industry facility there only while
//! the structure has an online industry service the pilot may use (a
//! Standup Manufacturing Plant, a laboratory, a reactor: #41's
//! [`crate::structure_services`]). With none, [`facility`] answers `None`, which
//! is what the server answered for every structure before this lane (#56):
//! `GetFacilities` is empty and `GetFacility` raises the `UserError` its
//! callers catch.
//!
//! # What the client reads **[V]**
//!
//! - The facility row's `typeID` is the structure's type: the client's
//!   `FACILITY_DENIED` test reads `GetCategoryID(facility.typeID) == 65`
//!   (`P:industry/job.pyc:709-719`).
//! - `tax` is `None` and `serviceAccess` is `{serviceID: rate}`; the job's tax
//!   is `serviceAccess[GetServiceID(activity, product)]`, as a fraction
//!   (`industry/activity.py:51-59`, `job.py:442-448`).
//! - `activities` lists the activities of the online services only; an
//!   activity outside it is `FACILITY_ACTIVITY` (33), a service the pilot
//!   has no `serviceAccess` entry for is `FACILITY_DENIED` (41).
//! - `GetServiceID` (`structures/services.py:186-212`) is [`service_id`].
//! - The hull's multipliers are type attributes the client's
//!   `industry_activity_modifier_sources.static` names per activity and kind
//!   (Raitaru: 2600 = 0.99 material, 2601 = 0.97 cost, 2602 = 0.85 time)
//!   **[V-data]**: [`hull_terms`], from the optional bundle files.
//!
//! # What changes against before
//!
//! A structure without an online industry module: nothing (no facility). A
//! structure with one: a facility, and the jobs it allows.
//!
//! # [A]s (falsifier in each)
//!
//! - **A11** a pilot who is not in the owner corporation pays the **lowest**
//!   value among the profile's rows of the service's setting whose group
//!   holds the pilot. Falsifier: TQ charges the first matching group, or the
//!   highest.
//! - **A10** the owner corporation pays 0 (#41's rule). Falsifier: the owner
//!   wants their corp taxed by their own profile.
//! - **A12** a job whose service goes offline is paused (`pause_date` = now)
//!   and resumes with the lost time added to its end; a job whose service has
//!   no fitted module left (the module unfitted) is cancelled. Falsifier: a TQ
//!   job that keeps running with its service offline, or stays paused after an
//!   unfit. Paused-job notifications 210/211 wait on the structure
//!   notification module.
//! - the hull's material multiplier applies as the client's `MaterialModifier`
//!   does, inside the job's rounding **[I]**. Falsifier: the client's quoted
//!   material count differs from ours by one for a Raitaru job.
//! - **Not covered**: a hull going offline or being destroyed does not
//!   pause or cancel its jobs (only a module change does).

use crate::industry::{self, Facility, Job, Status, StructureTerms};
use crate::notify::Push;
use crate::owner_groups as og;
use crate::service::Registry;
use crate::structure::{self, Structure};
use crate::structure_profile::{self as sp, *};
use eve_marshal::PyValue;

/// `capitalShipsGroups` **[V]** (`C:inventorycommon/const.py:2926-2928`).
pub const CAPITAL_GROUPS: [i64; 6] = [1538, 547, 5120, 485, 883, 4594];
/// `superCapitalShipsGroups` **[V]** (same place).
pub const SUPERCAPITAL_GROUPS: [i64; 2] = [659, 30];
/// The reaction product groups of each reaction service **[V]**
/// (`C:structures/services.py:186-212`).
pub const BIOCHEMICAL_GROUPS: [i64; 2] = [712, 4096];
pub const HYBRID_GROUPS: [i64; 1] = [974];
pub const COMPOSITE_GROUPS: [i64; 3] = [428, 429, 4932];

/// The client's `GetServiceID(activity, productGroup)` **[V]** with the
/// capital lists as the client has them.
pub fn service_id(activity: i64, product_group: Option<i64>) -> Option<i64> {
    service_id_in(
        activity,
        product_group,
        &CAPITAL_GROUPS,
        &SUPERCAPITAL_GROUPS,
    )
}

/// [`service_id`] with the capital and supercapital group lists given (the
/// tests pass invented groups).
pub fn service_id_in(
    activity: i64,
    product_group: Option<i64>,
    capital: &[i64],
    supercapital: &[i64],
) -> Option<i64> {
    match activity {
        industry::MANUFACTURING => Some(match product_group {
            Some(g) if supercapital.contains(&g) => SERVICE_MANUFACTURING_SUPERCAPITAL,
            Some(g) if capital.contains(&g) => SERVICE_MANUFACTURING_CAPITAL,
            _ => SERVICE_MANUFACTURING_BASIC,
        }),
        // An unknown group is the client's meta service `REACTIONS` (40),
        // which no module gives, so such a job is refused as denied.
        industry::REACTION => Some(match product_group {
            Some(g) if BIOCHEMICAL_GROUPS.contains(&g) => SERVICE_REACTIONS_BIOCHEMICAL,
            Some(g) if HYBRID_GROUPS.contains(&g) => SERVICE_REACTIONS_HYBRID,
            Some(g) if COMPOSITE_GROUPS.contains(&g) => SERVICE_REACTIONS_COMPOSITE,
            _ => SERVICE_REACTIONS,
        }),
        industry::RESEARCH_MATERIAL => Some(SERVICE_LABORATORY_RESEARCH_MATERIAL),
        industry::RESEARCH_TIME => Some(SERVICE_LABORATORY_RESEARCH_TIME),
        industry::COPYING => Some(SERVICE_LABORATORY_COPYING),
        industry::INVENTION => Some(SERVICE_LABORATORY_INVENTION),
        _ => None,
    }
}

/// [`service_id`] for a job making `product_type`: the product's group from
/// the type table (manufacturing and reactions only).
pub fn service_for_product(
    content: &crate::content::Content,
    activity: i64,
    product_type: i64,
) -> Option<i64> {
    let group = match activity {
        industry::MANUFACTURING | industry::REACTION => {
            crate::item::find_in(content, product_type).map(|i| i.group_id)
        }
        _ => None,
    };
    service_id(activity, group)
}

/// The client's `GetActivityID` **[V]**: the activity a specific industry
/// service runs. The meta services (industry, manufacturing, laboratory,
/// reactions) run none.
pub fn activity_of(service: i64) -> Option<i64> {
    match service {
        SERVICE_MANUFACTURING_BASIC
        | SERVICE_MANUFACTURING_CAPITAL
        | SERVICE_MANUFACTURING_SUPERCAPITAL => Some(industry::MANUFACTURING),
        SERVICE_LABORATORY_RESEARCH_TIME => Some(industry::RESEARCH_TIME),
        SERVICE_LABORATORY_RESEARCH_MATERIAL => Some(industry::RESEARCH_MATERIAL),
        SERVICE_LABORATORY_COPYING => Some(industry::COPYING),
        SERVICE_LABORATORY_INVENTION => Some(industry::INVENTION),
        SERVICE_REACTIONS_COMPOSITE | SERVICE_REACTIONS_BIOCHEMICAL | SERVICE_REACTIONS_HYBRID => {
            Some(industry::REACTION)
        }
        _ => None,
    }
}

/// What `ch` pays for a service whose setting is `setting` on `s`, as a
/// fraction. PINNED **[A]** (A10, A11): the owner corporation pays 0; another pilot
/// pays the lowest value among the profile's rows of `setting` whose group
/// holds them, divided by 100; a service with no setting costs 0. `None`: no
/// row holds the pilot (no `serviceAccess` entry, so `FACILITY_DENIED`).
pub fn rate_for(reg: &Registry, s: &Structure, ch: i64, setting: Option<i64>) -> Option<f64> {
    if reg.character(ch).map(|c| c.corporation_id) == Some(s.owner_corp) {
        return Some(0.0);
    }
    let Some(setting) = setting else {
        return Some(0.0);
    };
    let pid = sp::profile_for(reg.deltas(), s)?;
    sp::settings(reg.deltas(), pid)
        .into_iter()
        .filter(|(set, group, _)| {
            *set == setting && *group != NO_GROUP && og::effective(reg, *group, ch) >= og::MEMBER
        })
        .map(|(_, _, value)| value / 100.0)
        .reduce(f64::min)
}

/// `serviceAccess` for `ch`: each online specific industry service the pilot
/// may use, with its rate.
pub fn access(reg: &Registry, s: &Structure, ch: i64) -> Vec<(i64, f64)> {
    let usable = sp::services_for(reg, s, ch);
    crate::structure_services::INDUSTRY_SERVICES
        .iter()
        .filter(|svc| activity_of(**svc).is_some() && usable.contains_key(svc))
        .filter_map(|svc| rate_for(reg, s, ch, setting_of_service(*svc)).map(|rate| (*svc, rate)))
        .collect()
}

/// The hull's `(time, material, cost)` multiplier per activity (indexed like
/// [`industry::ACTIVITIES`]) from the bundle's modifier sources for
/// `type_id` that carry no filter, and the type's value of each named
/// attribute; 1.0 where there is none **[V-data]** (the attribute holds the
/// multiplier itself). A bundle without `industry/modifier-sources.tsv` gives
/// all 1.0.
pub fn hull_terms(content: &crate::content::Content, type_id: i64) -> [(f64, f64, f64); 6] {
    let mut out = [(1.0, 1.0, 1.0); 6];
    let table: &industry::Table = content.get();
    for src in table.modifier_sources.get(&type_id).into_iter().flatten() {
        if src.filter_id != 0 {
            continue;
        }
        let Some(i) = industry::ACTIVITIES
            .iter()
            .position(|a| *a == src.activity_id)
        else {
            continue;
        };
        let Some(v) = crate::typedata::own_attribute_in(content, type_id, src.attribute_id)
            .filter(|v| v.is_finite() && *v > 0.0)
        else {
            continue;
        };
        match src.kind {
            industry::ModKind::Time => out[i].0 *= v,
            industry::ModKind::Material => out[i].1 *= v,
            industry::ModKind::Cost => out[i].2 *= v,
        }
    }
    out
}

/// The activities the structure's own ONLINE services run, in
/// [`industry::ACTIVITIES`] order, whoever asks.
pub fn online_activities(reg: &Registry, s: &Structure) -> Vec<i64> {
    let states = crate::structure_services::services_of(reg, s);
    industry::ACTIVITIES
        .iter()
        .copied()
        .filter(|a| {
            states
                .iter()
                .any(|(svc, state)| *state == SERVICE_STATE_ONLINE && activity_of(*svc) == Some(*a))
        })
        .collect()
}

/// The facility a pilot docked in structure `c.station_id` gets: `None` when
/// the structure has no online industry service (the #56 behaviour, kept).
/// The activities are the structure's (so a guest with no tax row still sees
/// manufacturing and is refused as `FACILITY_DENIED`, not as
/// `FACILITY_ACTIVITY`); `serviceAccess` is the pilot's.
pub fn facility(reg: &Registry, c: &crate::model::Character) -> Option<Facility> {
    let sid = c.station_id?;
    let s = structure::get(reg.deltas(), sid)?;
    let activities = online_activities(reg, &s);
    if activities.is_empty() {
        return None;
    }
    let mut terms = StructureTerms::EMPTY;
    for (svc, rate) in access(reg, &s, c.character_id)
        .iter()
        .take(terms.access.len())
    {
        terms.access[terms.access_len] = (*svc, *rate);
        terms.access_len += 1;
    }
    for a in activities {
        terms.activities[terms.activities_len] = a;
        terms.activities_len += 1;
    }
    terms.hull = hull_terms(reg.content(), s.type_id);
    Some(Facility {
        facility_id: sid,
        type_id: s.type_id,
        owner_id: s.owner_corp,
        solar_system_id: s.system,
        tax: None,
        scc_tax_modifier: industry::DEFAULT_SCC_TAX_MODIFIER,
        online: s.is_online(),
        time_multiplier: 1.0,
        material_multiplier: 1.0,
        cost_multiplier: 1.0,
        distance: 0,
        safe_sec: false,
        structure: Some(terms),
    })
}

/// The service a running job of this structure needs.
fn job_service(reg: &Registry, j: &Job) -> Option<i64> {
    service_for_product(reg.content(), j.activity_id, j.product_type_id)
}

/// The unfinished jobs installed in `sid`.
fn live_jobs(reg: &Registry, sid: i64) -> Vec<Job> {
    reg.jobs()
        .all()
        .iter()
        .filter(|j| j.facility_id == sid && !j.completed())
        .copied()
        .collect()
}

/// Whether `type_id` is a module that gives an industry service.
fn is_industry_module(type_id: i64) -> bool {
    crate::structure_services::module_services(crate::structure_services::MODULE_SERVICES, type_id)
        .iter()
        .any(|svc| activity_of(*svc).is_some())
}

/// Tell the pilots docked in `sid` to refetch the facility.
fn refetch(reg: &mut Registry, sid: i64) {
    let to: Vec<i64> = reg
        .characters_at_station(sid)
        .iter()
        .map(|c| c.character_id)
        .collect();
    if !to.is_empty() {
        reg.push(FacilitiesUpdated { to, ids: vec![sid] });
    }
}

/// `OnFacilitiesUpdated(facilityIDs)`: the client drops and refetches each
/// id with `facilityManager.GetFacility` **[V]** (`facilitySvc.py:50-60`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FacilitiesUpdated {
    pub to: Vec<i64>,
    pub ids: Vec<i64>,
}

impl Push for FacilitiesUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnFacilitiesUpdated",
            vec![PyValue::List(
                self.ids.iter().map(|i| PyValue::Int(*i)).collect(),
            )],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// #41's hook: a service of `sid` changed (fitted, onlined, offlined,
/// unfitted). A running job whose service is no longer online is **paused**
/// (`pause_date` = `now`) and a paused one whose service is back resumes with
/// the lost time added to its end **[A]** (A12); each goes through
/// `commit_job`, which pushes `OnIndustryJob`. Then the docked pilots refetch
/// the facility. A structure with no job and nobody docked costs two scans.
pub fn on_services_changed(reg: &mut Registry, sid: i64, now: i64) {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return;
    };
    let states = crate::structure_services::services_of(reg, &s);
    let mut changed = false;
    for mut job in live_jobs(reg, sid) {
        let Some(service) = job_service(reg, &job) else {
            continue;
        };
        let online = states.get(&service) == Some(&SERVICE_STATE_ONLINE);
        match job.status {
            Status::Installed if !online && !job.ready_at(now) => {
                job.status = Status::Paused;
                job.pause_date = Some(now);
                reg.commit_job(job);
                changed = true;
            }
            Status::Paused if online => {
                if let Some(paused) = job.pause_date {
                    job.end_date += (now - paused).max(0);
                }
                job.status = Status::Installed;
                job.pause_date = None;
                reg.commit_job(job);
                changed = true;
            }
            _ => {}
        }
    }
    // The facility changed for the docked pilots when a job moved or an
    // industry module is fitted (it may have gone on or off line). A change
    // to a structure with neither (a clone bay, say) tells nobody.
    let industry_fitted = crate::structure_services::fitted(reg, &s)
        .iter()
        .any(|f| is_industry_module(f.type_id));
    if changed || industry_fitted {
        refetch(reg, sid);
    }
}

/// #41's unfit hook: `removed` (a module type) left `sid`. A job whose
/// service has no fitted module left (online or not) is cancelled **[A]**
/// (A12), through `cancel_job`; a finished job, whose product waits to be
/// delivered, is never cancelled. The docked pilots refetch the facility when
/// the module was an industry one.
pub fn on_module_removed(reg: &mut Registry, sid: i64, removed: i64, now: i64) {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        return;
    };
    let fitted: Vec<i64> = crate::structure_services::fitted(reg, &s)
        .iter()
        .map(|f| f.type_id)
        .collect();
    for job in live_jobs(reg, sid) {
        let Some(service) = job_service(reg, &job) else {
            continue;
        };
        let kept = fitted.iter().any(|t| {
            crate::structure_services::module_services(
                crate::structure_services::MODULE_SERVICES,
                *t,
            )
            .contains(&service)
        });
        if kept || job.ready_at(now) {
            continue;
        }
        if let Ok(done) = reg.jobs().plan_cancel(job.job_id, now) {
            reg.cancel_job(done);
        }
    }
    if is_industry_module(removed) {
        refetch(reg, sid);
    }
}

#[cfg(test)]
mod tests;
