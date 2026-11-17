//! **The steward's upgrades** (SOVX-STEWARD-2, #221): the steward of a hub
//! installs hub upgrades, one at a time, in a fixed preference order, and only
//! those the client's own power rule ([`eve_server::sov_upgrade::simulate`])
//! says would be `Online` with the hub's budget and reagent stock now, and
//! that knock no installed upgrade out of `Online`.
//!
//! The arm runs after the steward's ([`super::ARMS`]) and only for the pilot
//! that holds the hub's claim ([`super::steward::claim`]); it acts when the
//! steward arm had nothing to do. Every act is a client verb: the gateway's
//! reads, the upgrade item carried in the ship, and the hub window's own save
//! (`ProcessConfigurationRequest`, items in the pilot's ship within
//! [`TRANSFER_RANGE`] of the hub). An item comes only from the market (a bid
//! the builders read, [`super::kit::want`]) or the pilot's own hangar.
//!
//! **Wire.** Definitions: `GetDefinitionsRequest` answers `{1* Definition}`
//! (`sov_upgrade.rs` `definition_bytes`: the type, power, workforce, fuel
//! type, burn per hour, startup, exclusive group, power and workforce made).
//! The hub's list: `GetHubUpgradesRequest {1 id(hub)}` answers `{1 {1 hub, 2*
//! {1 {.. 3 id(type)}, 2 {1 definition, 2 state}}, 3 stamp}}`. The wire has no
//! toggle: a slot reads `Offline` when it is configured off. The budget:
//! `hub.api.GetResourcesRequest` (`available` is the budget before upgrades;
//! the workforce the dens use is taken off, as the server's own rule does).
//! The save: `ProcessConfigurationRequest {1* {1 item}, 2* {1 id(type), 2
//! online}, 3 id(hub)}`.
//!
//! **Refusals back off per target** ([`super::steward::Mind::refused`],
//! [`REFUSE_BACKOFF`]): an upgrade type that was refused, found not to come
//! online, could not be loaded or bought is left alone; a hub whose reads are
//! refused is left alone (key `-hub`, hub ids being positive).
//!
//! **[A] B1b-1** preference: producers first (the largest output first), then
//! by type id. Falsified by soak hubs that hold only producers while their
//! power sits unused for days. The cyno jammer service waits on a way to read
//! jammer ownership cheaply (follow-up).
//!
//! **[A] B1b-2** one install per pass. Falsified by hubs taking a week to fill
//! with the reagents there.
//!
//! **[A] B1b-3** a steward never uninstalls (an uninstalled upgrade is
//! destroyed). Falsified by the owner wanting bots to re-plan hubs.
//!
//! **[A] B1b-4** a type whose item is not held is wanted (a bid and a want
//! line) for [`REFUSE_BACKOFF`] and the next candidate is tried meanwhile, so
//! several wants may stand. Falsified by soak bids for upgrades no builder
//! ever makes piling up.

use super::Mind as ExtrasMind;
use super::gw::{self, Gw};
use super::steward::{self, DEFS_TTL, REFUSE_BACKOFF, id_body, reachable_book, soft};
use crate::brain::Memory;
use crate::clock;
use crate::sov_bot::{Ctx, Duty, Plan, RANGE_USE, WALLET_SHARE};
use crate::verbs::ItemRow;
use crate::{Conn, Error, Result};
use eve_server::skyhook::TRANSFER_RANGE;
use eve_server::sov_upgrade::{Def, Power, Slot, UPGRADE_GROUPS, simulate};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Instant;

const DEFINITIONS: &str = "GetDefinitionsRequest";
const HUB_UPGRADES: &str = "GetHubUpgradesRequest";
const RESOURCES: &str = "GetResourcesRequest";
const PROCESS: &str = "ProcessConfigurationRequest";

// ---- what the wire says ------------------------------------------------------------------

/// One upgrade type: the rule's numbers and its exclusive group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defn {
    pub def: Def,
    pub group: String,
}

/// `GetDefinitionsResponse`: `{1* Definition}`.
pub fn decode_definitions(payload: &[u8]) -> BTreeMap<i64, Defn> {
    gw::fields(payload, 1)
        .into_iter()
        .filter_map(|d| {
            let type_id = i64::try_from(gw::read_id(d, 1)?).ok()?;
            let fuel = gw::read_id(d, 4)
                .filter(|f| *f > 0)
                .and_then(|f| i64::try_from(f).ok());
            Some((
                type_id,
                Defn {
                    def: Def {
                        power: gw::uint(d, 2).unwrap_or(0),
                        workforce: gw::uint(d, 3).unwrap_or(0),
                        fuel,
                        per_hour: u32::try_from(gw::uint(d, 5).unwrap_or(0)).unwrap_or(u32::MAX),
                        startup: u32::try_from(gw::uint(d, 6).unwrap_or(0)).unwrap_or(u32::MAX),
                        power_out: gw::uint(d, 8).unwrap_or(0),
                        workforce_out: gw::uint(d, 9).unwrap_or(0),
                    },
                    group: gw::bytes(d, 7)
                        .map(|g| String::from_utf8_lossy(g).into_owned())
                        .unwrap_or_default(),
                },
            ))
        })
        .collect()
}

fn state_of(v: u64) -> Option<Power> {
    match v {
        1 => Some(Power::Offline),
        2 => Some(Power::Online),
        3 => Some(Power::Low),
        4 => Some(Power::Pending),
        _ => None,
    }
}

/// The answer of `GetHubUpgrades` and of the save: `{1 HubUpgrades}`, the
/// installed `(type, state)` in configured order.
pub fn decode_hub_upgrades(payload: &[u8]) -> Vec<(i64, Power)> {
    let Some(hub) = gw::bytes(payload, 1) else {
        return Vec::new();
    };
    gw::fields(hub, 2)
        .into_iter()
        .filter_map(|u| {
            let ident = gw::bytes(u, 1)?;
            let attrs = gw::bytes(u, 2)?;
            let type_id = gw::read_id(ident, 3)
                .or_else(|| gw::read_id(ident, 2))
                .and_then(|t| i64::try_from(t).ok())?;
            Some((type_id, state_of(gw::uint(attrs, 2)?)?))
        })
        .collect()
}

/// The budget the hub reads: `(power available, workforce available,
/// workforce allocated)`.
pub fn decode_resources(payload: &[u8]) -> Option<(u64, u64, u64)> {
    let power = gw::bytes(payload, 1)?;
    let workforce = gw::bytes(payload, 2)?;
    Some((
        gw::uint(power, 1).unwrap_or(0),
        gw::uint(workforce, 1).unwrap_or(0),
        gw::uint(workforce, 2).unwrap_or(0),
    ))
}

/// The `(power, workforce)` budget before upgrades the rule runs against:
/// what the hub harvests, less what the mercenary dens draw (the allocated
/// workforce less what the Online upgrades account for).
pub fn budget(
    available: (u64, u64),
    allocated_workforce: u64,
    installed: &[(i64, Power)],
    defs: &BTreeMap<i64, Defn>,
) -> (u64, u64) {
    let upgrades: u64 = installed
        .iter()
        .filter(|(_, s)| *s == Power::Online)
        .filter_map(|(t, _)| defs.get(t))
        .map(|d| d.def.workforce)
        .sum();
    let dens = allocated_workforce.saturating_sub(upgrades);
    (available.0, available.1.saturating_sub(dens))
}

/// The installed upgrades as the rule's slots: configured online unless the
/// wire says `Offline`, the stored state as `prev`.
pub fn slots(installed: &[(i64, Power)]) -> Vec<Slot> {
    installed
        .iter()
        .map(|(t, s)| Slot {
            type_id: *t,
            online: *s != Power::Offline,
            prev: Some(*s),
            new: false,
        })
        .collect()
}

/// The `ProcessConfigurationRequest` body: the item, every installed
/// `(type, online)` in configured order and the new one last (online), the
/// hub.
pub fn configuration_body(
    item: i64,
    installed: &[(i64, bool)],
    new_type: i64,
    hub: i64,
) -> Vec<u8> {
    let mut b = Vec::new();
    gw::put_bytes(&mut b, 1, &gw::id(item as u64));
    for (t, online) in installed.iter().copied().chain([(new_type, true)]) {
        let mut e = Vec::new();
        gw::put_bytes(&mut e, 1, &gw::id(t as u64));
        gw::put_uint(&mut e, 2, u64::from(online));
        gw::put_bytes(&mut b, 2, &e);
    }
    gw::put_bytes(&mut b, 3, &gw::id(hub as u64));
    b
}

// ---- the plan (pure) -----------------------------------------------------------------------

/// What to do about the next upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// The item is held: carry it to the hub and save the configuration.
    Install(i64),
    /// It would come online but no item is held: want it, buy it.
    Buy(i64),
    /// Nothing the rule would bring online.
    None,
}

/// The upgrade types in the order they are tried (**[A] B1b-1**): producers
/// first, the largest output first, then the rest by type id; without the
/// installed types and the types of an installed exclusive group.
pub fn preference(defs: &BTreeMap<i64, Defn>, installed: &[Slot]) -> Vec<i64> {
    let groups: BTreeSet<&str> = installed
        .iter()
        .filter_map(|s| defs.get(&s.type_id))
        .map(|d| d.group.as_str())
        .collect();
    let mut out: Vec<(&i64, &Defn)> = defs
        .iter()
        .filter(|(t, d)| {
            !installed.iter().any(|s| s.type_id == **t) && !groups.contains(d.group.as_str())
        })
        .collect();
    let made = |d: &Defn| d.def.power_out.saturating_add(d.def.workforce_out);
    out.sort_by(|a, b| {
        (made(b.1) > 0)
            .cmp(&(made(a.1) > 0))
            .then(made(b.1).cmp(&made(a.1)).then(a.0.cmp(b.0)))
    });
    out.into_iter().map(|(t, _)| *t).collect()
}

/// **The next upgrade** (pure): the first of `candidates` that the rule would
/// leave `Online` as a new last slot with `power`, `workforce` and the reagent
/// stock `fuel`, and that changes no installed slot's state (an Online upgrade
/// is never knocked Low). [`Want::Install`] when its item is among `held`,
/// else [`Want::Buy`].
pub fn next_upgrade(
    installed: &[Slot],
    defs: &dyn Fn(i64) -> Option<Def>,
    power: u64,
    workforce: u64,
    fuel: &dyn Fn(i64) -> u64,
    candidates: &[i64],
    held: &[i64],
) -> Want {
    let base = simulate(installed, defs, power, workforce, fuel);
    for &t in candidates {
        if installed.iter().any(|s| s.type_id == t) {
            continue;
        }
        let mut with = installed.to_vec();
        with.push(Slot {
            type_id: t,
            online: true,
            prev: None,
            new: true,
        });
        let sim = simulate(&with, defs, power, workforce, fuel);
        let Some((last, _)) = sim.last() else {
            continue;
        };
        let n = installed.len();
        let knocks = sim[..n]
            .iter()
            .zip(&base)
            .any(|((after, _), (before, _))| after != before);
        if *last != Power::Online || knocks {
            continue;
        }
        return if held.contains(&t) {
            Want::Install(t)
        } else {
            Want::Buy(t)
        };
    }
    Want::None
}

/// The `(system, hub)` a pilot of `duty` stewards, if any: a hauler of a hub
/// in the plan.
pub fn hub_of(plan: &Plan, duty: Duty) -> Option<(i64, i64)> {
    let Duty::Haul(hub) = duty else {
        return None;
    };
    plan.fuel
        .iter()
        .find(|(_, h)| *h == hub)
        .map(|(system, h)| (*system, *h))
}

// ---- the arm ---------------------------------------------------------------------------------

type Defs = BTreeMap<Option<SocketAddr>, (Instant, BTreeMap<i64, Defn>)>;

static DEFS: Mutex<Defs> = Mutex::new(BTreeMap::new());

/// One request; `None` when the gateway answers a transport error or a status
/// other than 200 (the status is returned in the `Err` for the caller).
fn read(
    g: &mut Gw,
    mind: &mut ExtrasMind,
    ty: &str,
    body: &[u8],
) -> std::result::Result<Vec<u8>, Option<u32>> {
    match g.ask(mind, ty, body) {
        Ok((200, payload)) => Ok(payload),
        Ok((status, _)) => Err(Some(status)),
        Err(_) => Err(None),
    }
}

fn upgrade_rows(rows: &[ItemRow], defs: &BTreeMap<i64, Defn>) -> Vec<ItemRow> {
    rows.iter()
        .filter(|r| UPGRADE_GROUPS.contains(&r.group_id) && defs.contains_key(&r.type_id))
        .cloned()
        .collect()
}

/// **The upgrades arm** ([`super::ARMS`]): `Ok(None)` for a pilot that is not
/// the hub's steward, a hub with nothing the rule would bring online, a refused
/// hub, or a gateway that is gone.
pub fn arm(
    c: &mut Conn,
    m: &mut Memory,
    ctx: &Ctx<'_>,
    plan: &Plan,
    duty: Duty,
) -> Result<Option<String>> {
    let Some((system, hub)) = hub_of(plan, duty) else {
        return Ok(None);
    };
    let (Some(map), Some(_)) = (ctx.map, ctx.content) else {
        return Ok(None);
    };
    let Some(who) = c.session().char_id() else {
        return Ok(None);
    };
    let now = clock::instant();
    let gateway = gw::gateway_of(c);
    {
        let st = &mut m.sov.extras.steward;
        st.refused
            .retain(|_, at| now.saturating_duration_since(*at) < REFUSE_BACKOFF);
        // A pilot waiting for a role cannot save a configuration; a hub whose
        // reads were refused lately is left alone.
        if st.no_role_until.is_some_and(|t| now < t) || st.refused.contains_key(&-hub) {
            return Ok(None);
        }
    }
    if steward::held_by_other(gateway, hub, who, now) {
        return Ok(None);
    }
    let Some(mut g) = Gw::open(c, &mut m.sov.extras) else {
        return Ok(None);
    };

    // The reads; the claim is taken once they have all been answered.
    let defs = {
        let cached = DEFS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&gateway)
            .filter(|(at, _)| now.saturating_duration_since(*at) < DEFS_TTL)
            .map(|(_, d)| d.clone());
        match cached {
            Some(d) => d,
            None => {
                let ty = format!("{}{DEFINITIONS}", gw::UPGRADE_PKG);
                let Ok(payload) = read(&mut g, &mut m.sov.extras, &ty, &[]) else {
                    return Ok(None);
                };
                let d = decode_definitions(&payload);
                DEFS.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(gateway, (now, d.clone()));
                d
            }
        }
    };
    if defs.is_empty() {
        return Ok(None);
    }
    let reads = [
        (gw::UPGRADE_PKG, HUB_UPGRADES),
        (gw::HUB_PKG, RESOURCES),
        (gw::FUEL_PKG, "GetRequest"),
    ];
    let mut raw: Vec<Vec<u8>> = Vec::new();
    for (pkg, name) in reads {
        match read(
            &mut g,
            &mut m.sov.extras,
            &format!("{pkg}{name}"),
            &id_body(hub),
        ) {
            Ok(p) => raw.push(p),
            Err(status) => {
                // A refusal of the hub backs off; a lost gateway does not
                // refuse anything.
                if status.is_some() {
                    m.sov.extras.steward.refused.insert(-hub, now);
                }
                return Ok(None);
            }
        }
    }
    if !steward::claim(gateway, hub, who, now) {
        return Ok(None);
    }
    let installed = decode_hub_upgrades(&raw[0]);
    let Some((power, workforce, allocated)) = decode_resources(&raw[1]) else {
        return Ok(None);
    };
    let bay = steward::decode_bay(&raw[2], clock::wall_unix_secs());
    let (power, workforce) = budget((power, workforce), allocated, &installed, &defs);
    let slot_list = slots(&installed);

    let docked = c.session().station_id().is_some();
    let cargo = c.cargo()?;
    let hangar = if docked {
        c.hangar_items()?
    } else {
        Vec::new()
    };
    let in_cargo = upgrade_rows(&cargo, &defs);
    let in_hangar = upgrade_rows(&hangar, &defs);
    let held: Vec<i64> = in_cargo
        .iter()
        .chain(&in_hangar)
        .map(|r| r.type_id)
        .collect();
    let refused = &m.sov.extras.steward.refused;
    let candidates: Vec<i64> = preference(&defs, &slot_list)
        .into_iter()
        .filter(|t| !refused.contains_key(t))
        .collect();
    let stock = |r: i64| {
        bay.iter()
            .find(|(t, _, _)| *t == r)
            .map_or(0, |(_, amount, _)| *amount)
    };
    let next = next_upgrade(
        &slot_list,
        &|t| defs.get(&t).map(|d| d.def),
        power,
        workforce,
        &stock,
        &candidates,
        &held,
    );
    let mut lines: Vec<String> = Vec::new();
    match next {
        Want::None => return Ok(None),
        Want::Buy(ty) => {
            // Wanting and buying are docked acts.
            if !docked {
                return Ok(None);
            }
            lines.push(acquire(c, map, &mut m.sov.extras.steward, ty)?);
        }
        Want::Install(ty) => {
            let toggles: Vec<(i64, bool)> =
                slot_list.iter().map(|s| (s.type_id, s.online)).collect();
            lines.push(install(
                c,
                m,
                &mut g,
                map,
                system,
                hub,
                ty,
                !in_cargo.iter().any(|r| r.type_id == ty),
                &toggles,
            )?);
        }
    }
    Ok(Some(format!("sov steward: {}", lines.join("; "))))
}

/// **Want and buy** the upgrade item (docked): the want on the board, the item
/// from an ask in reach (here, or a seller a trip away within the wallet's
/// share), else a standing bid. A bid that stands, or a failed trip, backs the
/// type off.
fn acquire(
    c: &mut Conn,
    map: &crate::travel::Map,
    st: &mut steward::Mind,
    ty: i64,
) -> Result<String> {
    let Some(station) = c.session().station_id() else {
        return Ok("acquire: not docked".to_owned());
    };
    super::kit::want(ty);
    let owned = |c: &mut Conn| -> Result<bool> {
        Ok(c.hangar_items()?
            .iter()
            .any(|r| r.type_id == ty && UPGRADE_GROUPS.contains(&r.group_id)))
    };
    let mut notes: Vec<String> = Vec::new();
    if !owned(c)?
        && let Some(book) = reachable_book(c, map, ty)?
    {
        let budget = crate::acquire::spendable(c)? * WALLET_SHARE;
        if book.asks.iter().any(|a| a.station_id == station) {
            let got = crate::capital::buy_units(c, &book, ty, 1, budget)?;
            notes.push(format!("bought {got} here"));
        } else if let Some(system) = map.system_of(station) {
            let trips = crate::travel::Trips {
                map,
                floor: crate::sov_bot::SOV_FLOOR,
            };
            if let Some(src) =
                crate::travel::sources(&book, trips, system, station, Some(budget), 1)
                    .into_iter()
                    .next()
            {
                match soft(crate::travel::fetch(
                    c,
                    trips,
                    &src,
                    ty,
                    1,
                    Some(budget),
                    crate::travel::Haul::Cargo,
                ))? {
                    Some(f) => notes.push(f.summary()),
                    None => notes.push("the trip failed".to_owned()),
                }
            }
        }
    }
    if owned(c)? {
        // The next pass carries it to the hub.
        return Ok(format!("upgrade {ty} in the hangar ({})", notes.join("; ")));
    }
    st.refused.insert(ty, clock::instant());
    let wallet = crate::acquire::spendable(c)? * WALLET_SHARE;
    let posted = soft(crate::hull_choice::bid_for(c, ty, 1, wallet))?;
    Ok(format!(
        "MISSING upgrade {ty} ({})",
        if posted == Some(true) {
            "bid posted"
        } else {
            "bid stands"
        }
    ))
}

/// **Install** upgrade `ty`: into the hold (from the hangar when docked), to
/// the hub, one `ProcessConfigurationRequest` with the installed upgrades in
/// order and the new one last, then home. The answer says whether the slot is
/// `Online`.
#[allow(clippy::too_many_arguments)]
fn install(
    c: &mut Conn,
    m: &mut Memory,
    g: &mut Gw,
    map: &crate::travel::Map,
    system: i64,
    hub: i64,
    ty: i64,
    from_hangar: bool,
    installed: &[(i64, bool)],
) -> Result<String> {
    let now = clock::instant();
    let held_in_hold = |c: &mut Conn| -> Result<Option<ItemRow>> {
        Ok(c.cargo()?
            .into_iter()
            .find(|r| r.type_id == ty && UPGRADE_GROUPS.contains(&r.group_id)))
    };
    if from_hangar {
        let row = c
            .hangar_items()?
            .into_iter()
            .find(|r| r.type_id == ty && UPGRADE_GROUPS.contains(&r.group_id));
        let Some(row) = row else {
            m.sov.extras.steward.refused.insert(ty, now);
            return Ok(format!("upgrade {ty} is not in the hangar"));
        };
        let loaded = c.load_cargo(
            row.item_id,
            (!row.singleton && row.quantity > 1).then_some(1),
        );
        let why = match loaded {
            Ok(out) => out.refusal(),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(e) => Some(e.to_string()),
        };
        if let Some(why) = why {
            m.sov.extras.steward.refused.insert(ty, now);
            return Ok(format!(
                "MISSING a hull whose hold takes upgrade {ty} ({why})"
            ));
        }
    }
    let Some(item) = held_in_hold(c)? else {
        m.sov.extras.steward.refused.insert(ty, now);
        return Ok(format!("upgrade {ty} is not in the hold"));
    };
    let mut lines = Vec::new();
    crate::sov_bot::fly_to_system(c, map, system)?;
    crate::sov_bot::warp_near(c, hub, TRANSFER_RANGE * RANGE_USE)?;
    let ty_name = format!("{}{PROCESS}", gw::UPGRADE_PKG);
    let body = configuration_body(item.item_id, installed, ty, hub);
    lines.push(match g.ask(&mut m.sov.extras, &ty_name, &body) {
        Ok((200, payload)) => {
            let after = decode_hub_upgrades(&payload);
            match after.iter().find(|(t, _)| *t == ty) {
                Some((_, Power::Online)) => format!("upgrade {ty} online"),
                Some((_, state)) => {
                    m.sov.extras.steward.refused.insert(ty, now);
                    format!("upgrade {ty} installed but {state:?}")
                }
                None => {
                    m.sov.extras.steward.refused.insert(ty, now);
                    format!("upgrade {ty} is not in the saved list")
                }
            }
        }
        Ok((status, _)) => {
            m.sov.extras.steward.refused.insert(ty, now);
            format!("upgrade {ty} refused ({status})")
        }
        Err(_) => {
            m.sov.extras.steward.refused.insert(ty, now);
            "MISSING gateway".to_owned()
        }
    });
    lines.push(crate::sov_bot::go_home(c, m, map)?);
    Ok(lines.join("; "))
}
