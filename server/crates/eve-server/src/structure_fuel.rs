//! **An Upwell structure's fuel** (STR-FUEL, issue #42 part b; spec
//! `structures-spec.md` P4): the hourly burn from the fuel bay, the cost of
//! onlining a module, what an empty bay does, and the fuel expiry the
//! structure browser shows.
//!
//! # What it does
//!
//! With `/world fuelburn` **on** ([`crate::rules::Rules::structure_fuel_burn`]):
//!
//! - every UTC hour each structure pays [`burn_per_hour`] from its fuel bay
//!   (flag 172, fuel blocks only, the lowest item id first **[A]**), once for
//!   every hour it has missed ([`tick`], [`plan`]);
//! - a bay that cannot pay an hour takes **nothing**, every fuel-using module
//!   goes offline through [`crate::structure_services::force_offline`] (no
//!   controls, no role) and the managers get notice 198 with the module
//!   **type** ids;
//! - onlining a module pays its dogma-modified 2110 from the bay first
//!   ([`pay_online`]), and a short bay refuses with `ServiceRequiresFuel`;
//! - when the fuel left covers fewer than [`LOW_FUEL_HOURS`] hours the managers
//!   get notice 181, once per crossing.
//!
//! With it **off** (the default, today's behaviour) nothing burns and
//! onlining is free. [`expires`] still answers, so the browser stays honest
//! **[I]**; notices 207, 208 and 249 follow the upkeep state machine
//! ([`crate::structure_notify::on_events`]) either way.
//!
//! # State
//!
//! Delta kind [`KIND`]: `structure -> last_burn_hour low_fuel_warned`. The
//! row is made when a module is first onlined with the burn on (so its first
//! hour is the *next* one, **[A]**), moves to the current UTC hour as hours
//! are paid, and goes when nothing burns. A restart reads it back, so no hour
//! is paid twice and none is lost.
//!
//! # What the client reads **[V]**
//!
//! - per hour = the sum over fitted, online modules of the **modified**
//!   `serviceModuleFuelAmount` 2109 (`shipfitting/fittingDogmaLocationUtil.py:467-475`,
//!   `GetFuelUsagePerHour`);
//! - expiry = the top of the current hour plus `min(units / per_hour,
//!   876600)` whole hours, counting group 1136 in flag 172
//!   (`moonmining/schedulingSvc.py:93-108`, `GetFuelExpiryTime`).
//!
//! # The hull's role bonus
//!
//! 2339 `structureServiceRoleBonus` (-25 for a Citadel or Engineering hull)
//! reaches 2109 and 2110 as a hull passive effect with an operation-6
//! modifier on the module group, the same shape a ship's role bonus has.
//! [`uses`] runs the server's dogma engine ([`crate::dogma::evaluate`]) with
//! the structure as the hull, so the bonus is whatever the hull's effect rows
//! say. `burn_reads_the_dogma_modified_amount` proves the engine applies it on
//! a category-65 hull.
//!
//! # [A]s (falsifier in each)
//!
//! - **The burn is off by default** (owner decision D2). Falsifier: the owner
//!   decides D2 otherwise.
//! - **Oldest stack first** (the lowest item id). Falsifier: TQ eating the
//!   newest; the order is invisible to the client's total.
//! - **A just-onlined module's first burn is the next hour.** Falsifier: TQ
//!   charging the hour it started in; the client's expiry cannot tell.
//! - **A short bay offlines every fuel-using module and burns nothing for the
//!   hour** (Eden runs one cycle per module and offlines only the short one).
//!   Falsifier: a TQ structure with fuel for one module only.
//! - **Hours missed while the server was down are paid on the next tick.**
//!   Falsifier: TQ charging nothing for a downtime.
//! - **[`LOW_FUEL_HOURS`] is 24, one mail per crossing.** Falsifier: a TQ
//!   mail's timing.
//! - **The online cost is charged whenever the burn is on** (owner question
//!   Q2). Falsifier: the owner decides otherwise.
//! - **The Metenox drill is skipped** (its reagent cycle is #55's, F13).
//!   Falsifier: the owner folds it into this burn.
//! - **The engine reaches the hull's 2339** (spec A29) when the hull's rows
//!   carry the effect. Falsifier: `GetFuelUsagePerHour` in the client
//!   (`fci_upwell_fuel` step 4) differing from [`burn_per_hour`].
//! - **Audience** of 181 and 198: the owner corporation's managers
//!   ([`crate::structure_notify::tell`]). Falsifier: a TQ recipient list.

use crate::damage::BlueTime;
use crate::dogma::{self, Inputs, ModuleInput, State};
use crate::item::Edit;
use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};
use crate::structure::{self, SECOND, Structure};
use crate::structure_bays::{GROUP_FUEL_BLOCK, TYPE_LIQUID_OZONE};
use crate::structure_dock::control::FLAG_FUEL;
use crate::structure_services::{self as svc, Fitted};
use eve_marshal::PyValue;
use std::time::Instant;

/// The delta kind: `structure -> last_burn_hour low_fuel_warned`.
pub const KIND: &str = "strfuel";

/// `serviceModuleFuelAmount` **[V]** (`dogma/const.py:878`): the fuel a
/// module burns each hour.
pub const ATTR_FUEL_PER_HOUR: i64 = 2109;
/// `serviceModuleOnlineAmount` **[V]** (`dogma/const.py:879`): the fuel
/// onlining a module costs.
pub const ATTR_ONLINE_AMOUNT: i64 = 2110;
/// `structureServiceRoleBonus` **[V-data]**: the hull's bonus to both.
pub const ATTR_ROLE_BONUS: i64 = 2339;

/// One hour, in FILETIME ticks.
pub const HOUR: i64 = 3_600 * SECOND;
/// The client's cap on the expiry, in hours **[V]** (`schedulingSvc.py:93-108`).
pub const MAX_EXPIRY_HOURS: i64 = 876_600;
/// The low-fuel mail goes out below this many hours of fuel **[A]**.
pub const LOW_FUEL_HOURS: i64 = 24;

/// Whether the hourly burn and the online cost are on (`/world fuelburn`).
pub fn enabled(reg: &Registry) -> bool {
    reg.config().rules.structure_fuel_burn
}

/// The top of the UTC hour `t` falls in. A FILETIME counts from midnight UTC
/// of 1601-01-01, so its hours are the UTC hours.
pub fn floor_hour(t: BlueTime) -> BlueTime {
    t - t.rem_euclid(HOUR)
}

// -- what a module uses --------------------------------------------------------

/// One fitted module and what the dogma engine says it uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Use {
    pub module: Fitted,
    /// The modified 2109.
    pub per_hour: f64,
    /// The modified 2110.
    pub online: f64,
}

fn whole(v: f64) -> u64 {
    if v.is_finite() && v > 0.0 {
        v as u64
    } else {
        0
    }
}

/// Every fitted module of `s` (the Metenox drill apart) with its modified 2109
/// and 2110: the server's dogma engine with the structure as the hull, every
/// fitted module in it (online ones online), so the hull's role bonus is in
/// the numbers. A hull the type table lacks gives the modules' own values.
pub fn uses(reg: &Registry, s: &Structure) -> Vec<Use> {
    let fitted = svc::fitted(reg, s);
    if fitted.is_empty() {
        return Vec::new();
    }
    let content = reg.content();
    let mut modules: Vec<ModuleInput> = fitted
        .iter()
        .map(|f| ModuleInput {
            item_id: f.item_id,
            type_id: f.type_id,
            state: if f.online {
                State::Online
            } else {
                State::Offline
            },
            charge_type: None,
        })
        .collect();
    modules.sort_by_key(|m| m.item_id);
    let sheet = dogma::evaluate(
        crate::typedata::installed_in(content).table(),
        &Inputs {
            ship_type: s.type_id,
            modules,
            ..Inputs::default()
        },
    );
    fitted
        .into_iter()
        .filter(|f| f.type_id != svc::TYPE_STANDUP_METENOX_MOON_DRILL)
        .map(|f| {
            let read = |attr: i64| match &sheet {
                Some(sh) => sh.module(f.item_id, attr).unwrap_or(0.0),
                None => crate::typedata::own_attribute_in(content, f.type_id, attr).unwrap_or(0.0),
            };
            Use {
                module: f,
                per_hour: read(ATTR_FUEL_PER_HOUR),
                online: read(ATTR_ONLINE_AMOUNT),
            }
        })
        .collect()
}

/// The client's `GetFuelUsagePerHour`: the sum of the online modules' modified
/// 2109, as the client's `int(...)` takes it (the sum first, then the whole
/// part).
pub fn burn_per_hour(reg: &Registry, s: &Structure) -> u64 {
    whole(
        uses(reg, s)
            .iter()
            .filter(|u| u.module.online)
            .map(|u| u.per_hour)
            .sum(),
    )
}

// -- the bay -----------------------------------------------------------------------

/// The stacks `keep` accepts in the fuel bay of `s`, as `(item id, quantity)`,
/// lowest item id first.
fn bay(
    reg: &Registry,
    s: &Structure,
    keep: impl Fn(&crate::item::Item) -> bool,
) -> Vec<(i64, i64)> {
    let mut stacks: Vec<(i64, i64)> = reg
        .store()
        .contents_of(s.id)
        .filter(|i| i.flag_id == FLAG_FUEL && keep(i))
        .map(|i| (i.item_id, i.quantity))
        .collect();
    stacks.sort_unstable();
    stacks
}

/// The fuel blocks (group 1136) in the fuel bay: what the burn and the expiry
/// count **[V]**.
pub fn fuel_units(reg: &Registry, s: &Structure) -> i64 {
    bay(reg, s, |i| i.group_id == GROUP_FUEL_BLOCK)
        .iter()
        .map(|(_, q)| q)
        .sum()
}

/// The Liquid Ozone in the fuel bay: the browser's `liquidOzoneQty`. It never
/// burns and never counts toward the expiry.
pub fn ozone_units(reg: &Registry, s: &Structure) -> i64 {
    bay(reg, s, |i| i.type_id == TYPE_LIQUID_OZONE)
        .iter()
        .map(|(_, q)| q)
        .sum()
}

/// The fuel-block stacks of the bay as `(quantity, type)` per type, for 181.
fn stacks_by_type(reg: &Registry, s: &Structure) -> Vec<(i64, i64)> {
    let mut by_type: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for i in reg
        .store()
        .contents_of(s.id)
        .filter(|i| i.flag_id == FLAG_FUEL && i.group_id == GROUP_FUEL_BLOCK)
    {
        *by_type.entry(i.type_id).or_default() += i.quantity;
    }
    by_type.into_iter().map(|(t, q)| (q, t)).collect()
}

/// Take `n` units of fuel blocks out of the bay, the lowest item id first
/// **[A]**. `Err` (nothing taken) when the bay holds fewer.
fn take_units(reg: &mut Registry, s: &Structure, n: i64) -> Result<(), String> {
    if n <= 0 {
        return Ok(());
    }
    let mut left = n;
    let mut edits = Vec::new();
    for (item_id, quantity) in bay(reg, s, |i| i.group_id == GROUP_FUEL_BLOCK) {
        if left == 0 {
            break;
        }
        let take = left.min(quantity);
        edits.push(Edit::Consume {
            item_id,
            quantity: take,
        });
        left -= take;
    }
    if left > 0 {
        return Err("the fuel bay holds too little".to_owned());
    }
    reg.apply_items(&edits)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

// -- the plan -------------------------------------------------------------------------

/// What `missed` hours of `per_hour` do to a bay of `units`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// The hours the bay paid for.
    pub paid: i64,
    /// The units they took.
    pub burned: i64,
    /// An hour was left unpaid: the modules go offline.
    pub short: bool,
}

/// Pay `missed` hours one by one: each takes `per_hour`, and the first hour
/// the bay cannot pay takes nothing and ends the run **[A]**.
pub fn plan(units: i64, per_hour: u64, missed: i64) -> Plan {
    let per = i64::try_from(per_hour).unwrap_or(i64::MAX).max(1);
    let paid = (units.max(0) / per).min(missed.max(0));
    Plan {
        paid,
        burned: paid.saturating_mul(per),
        short: paid < missed,
    }
}

// -- the row ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    last: BlueTime,
    warned: bool,
}

fn key(sid: i64) -> Vec<String> {
    vec![sid.to_string()]
}

fn row_of(reg: &Registry, sid: i64) -> Option<Row> {
    let v = reg.deltas().of(KIND).get(&key(sid))?;
    Some(Row {
        last: v.first()?.parse().ok()?,
        warned: v.get(1).is_some_and(|w| w == "1"),
    })
}

fn put_row(reg: &mut Registry, sid: i64, row: Row) {
    let value = vec![row.last.to_string(), u8::from(row.warned).to_string()];
    if reg.deltas().of(KIND).get(&key(sid)) != Some(&value) {
        reg.deltas_mut().put(KIND, key(sid), value);
        reg.mark_dirty();
    }
}

fn drop_row(reg: &mut Registry, sid: i64) {
    if reg.deltas_mut().del(KIND, &key(sid)).is_some() {
        reg.mark_dirty();
    }
}

/// The last UTC hour `sid` paid for, `None` when nothing burns there.
pub fn last_burn_hour(reg: &Registry, sid: i64) -> Option<BlueTime> {
    row_of(reg, sid).map(|r| r.last)
}

// -- onlining ---------------------------------------------------------------------------------

/// What onlining `module` costs **[A]**: with the burn on, the module's
/// modified 2110 from the fuel bay, refused with `ServiceRequiresFuel` (the
/// client's key **[V]**, used here **[I]**) when the bay holds less; free with
/// it off. The structure's burn starts at the next hour **[A]**.
pub fn pay_online(
    reg: &mut Registry,
    s: &Structure,
    module: &Fitted,
    now: BlueTime,
) -> Result<(), ServiceError> {
    if !enabled(reg) || module.type_id == svc::TYPE_STANDUP_METENOX_MOON_DRILL {
        return Ok(());
    }
    let cost = uses(reg, s)
        .iter()
        .find(|u| u.module.item_id == module.item_id)
        .map_or(0, |u| whole(u.online)) as i64;
    if cost > 0 {
        if fuel_units(reg, s) < cost {
            return Err(crate::modules::user_error("ServiceRequiresFuel", vec![]));
        }
        take_units(reg, s, cost).map_err(|e| {
            crate::modules::user_error(
                "CustomNotify",
                vec![(
                    "notify",
                    bstr(&format!("The fuel could not be taken ({e}).")),
                )],
            )
        })?;
    }
    if row_of(reg, s.id).is_none() {
        put_row(
            reg,
            s.id,
            Row {
                last: floor_hour(now),
                warned: false,
            },
        );
    }
    Ok(())
}

// -- the expiry ------------------------------------------------------------------------------

/// The client's `GetFuelExpiryTime` for `s` at `now`: the top of the current
/// hour plus `min(units / per_hour, 876600)` whole hours (FILETIME), `None`
/// when nothing burns **[V]**. Computed with the burn off too **[I]**.
pub fn expires_at(reg: &Registry, s: &Structure, now: BlueTime) -> Option<BlueTime> {
    let per = i64::try_from(burn_per_hour(reg, s)).unwrap_or(i64::MAX);
    if per == 0 {
        return None;
    }
    let hours = (fuel_units(reg, s) / per).min(MAX_EXPIRY_HOURS);
    Some(floor_hour(now) + hours * HOUR)
}

/// [`expires_at`] now, as the browser's `fuelExpires`.
pub fn expires(reg: &Registry, s: &Structure) -> PyValue {
    expires_at(reg, s, crate::pyutil::filetime_now()).map_or(PyValue::None, PyValue::Int)
}

// -- the clock -------------------------------------------------------------------------------

/// What one structure's step did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Hours paid.
    pub paid: i64,
    /// Units taken.
    pub burned: i64,
    /// The type ids of the modules the server took offline.
    pub offlined: Vec<i64>,
}

/// The structure ids that have an online module or a fuel row.
fn candidates(reg: &Registry) -> Vec<i64> {
    let mut ids: Vec<i64> = reg
        .deltas()
        .of(svc::KIND)
        .keys()
        .chain(reg.deltas().of(KIND).keys())
        .filter_map(|k| k.first()?.parse().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The `tick::Stage` row: with the burn on, every structure with an online
/// module pays the hours it has missed; with it off the rows go.
pub fn tick(reg: &mut Registry, _: Instant, now: BlueTime) {
    if !enabled(reg) {
        let stale: Vec<Vec<String>> = reg.deltas().of(KIND).keys().cloned().collect();
        for k in stale {
            reg.deltas_mut().del(KIND, &k);
            reg.mark_dirty();
        }
        return;
    }
    for sid in candidates(reg) {
        step(reg, sid, now);
    }
}

/// One structure: pay its missed hours, take the modules offline when it is
/// short, and warn when the fuel runs low. `None` when it is gone.
pub fn step(reg: &mut Registry, sid: i64, now: BlueTime) -> Option<Report> {
    let Some(s) = structure::get(reg.deltas(), sid) else {
        drop_row(reg, sid);
        return None;
    };
    let users = uses(reg, &s);
    let per = whole(
        users
            .iter()
            .filter(|u| u.module.online)
            .map(|u| u.per_hour)
            .sum(),
    );
    if per == 0 {
        drop_row(reg, sid);
        return Some(Report::default());
    }
    let hour = floor_hour(now);
    let mut row = row_of(reg, sid).unwrap_or(Row {
        last: hour,
        warned: false,
    });
    let mut report = Report::default();
    let missed = (hour - row.last).max(0) / HOUR;
    if missed > 0 {
        let plan = plan(fuel_units(reg, &s), per, missed);
        if let Err(e) = take_units(reg, &s, plan.burned) {
            crate::errln!("[strfuel] {sid}: burning {} units: {e}", plan.burned);
            return Some(report);
        }
        report.paid = plan.paid;
        report.burned = plan.burned;
        row.last = hour;
        if plan.short {
            report.offlined = offline_all(reg, &s, &users, now);
            // Nothing burns now; the row goes with the next step.
            row.warned = false;
            put_row(reg, sid, row);
            if !report.offlined.is_empty() {
                crate::structure_notify::services_offline(reg, &s, &report.offlined);
            }
            return Some(report);
        }
    }
    low_fuel(reg, &s, per, &mut row);
    put_row(reg, sid, row);
    Some(report)
}

/// Take every online fuel-using module of `s` offline, and answer their
/// **type** ids (what 198 names).
fn offline_all(reg: &mut Registry, s: &Structure, users: &[Use], now: BlueTime) -> Vec<i64> {
    let mut types = Vec::new();
    for u in users.iter().filter(|u| u.module.online && u.per_hour > 0.0) {
        match svc::force_offline(reg, s.id, u.module.item_id, now) {
            Ok(_) => types.push(u.module.type_id),
            Err(e) => crate::errln!(
                "[strfuel] {}: module {} would not go offline ({})",
                s.id,
                u.module.item_id,
                svc::error_key(&e)
            ),
        }
    }
    types
}

/// 181 once per crossing below [`LOW_FUEL_HOURS`]; a refill above it re-arms.
fn low_fuel(reg: &mut Registry, s: &Structure, per: u64, row: &mut Row) {
    let per = i64::try_from(per).unwrap_or(i64::MAX).max(1);
    let low = fuel_units(reg, s) / per < LOW_FUEL_HOURS;
    if low && !row.warned {
        row.warned = true;
        let left = stacks_by_type(reg, s);
        crate::structure_notify::fuel_alert(reg, s, &left);
    } else if !low {
        row.warned = false;
    }
}

// -- the GM's seam ------------------------------------------------------------------------------

/// `/strfuel <structureID> hours <n>`: run the burn as if `n` more hours had
/// passed (the row is moved back `n` hours and the step runs now; there is no
/// `/world tick`). The line the GM reads.
pub fn gm_hours(reg: &mut Registry, sid: i64, hours: i64, now: BlueTime) -> String {
    if structure::get(reg.deltas(), sid).is_none() {
        return format!("no structure {sid}");
    }
    if !enabled(reg) {
        return "the burn is off: /world fuelburn 1".to_owned();
    }
    let hour = floor_hour(now);
    let row = row_of(reg, sid).unwrap_or(Row {
        last: hour,
        warned: false,
    });
    put_row(
        reg,
        sid,
        Row {
            last: row.last - hours * HOUR,
            ..row
        },
    );
    let Some(r) = step(reg, sid, now) else {
        return format!("no structure {sid}");
    };
    let left = structure::get(reg.deltas(), sid).map_or(0, |s| fuel_units(reg, &s));
    let mut line = format!(
        "{} of {hours} hours paid, {} units burned, {left} left",
        r.paid, r.burned
    );
    if !r.offlined.is_empty() {
        line.push_str(&format!("; modules offline: {:?}", r.offlined));
    }
    line
}

/// The `/strfuel` command.
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    const USAGE: &str = "/strfuel <structureID> hours <n>";
    if actor.is_none() {
        return Ok(bstr("no character"));
    }
    let parsed = match args {
        [id, "hours", n] => id.parse::<i64>().ok().zip(n.parse::<i64>().ok()),
        _ => None,
    };
    let Some((sid, n)) = parsed.filter(|(_, n)| (1..=100_000).contains(n)) else {
        return Ok(bstr(USAGE));
    };
    Ok(bstr(&gm_hours(reg, sid, n, crate::pyutil::filetime_now())))
}

#[cfg(test)]
mod tests;
