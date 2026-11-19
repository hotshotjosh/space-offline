//! Sov hub upgrades: install, online, offline and remove (SOV-UPG-1, #79, item
//! L6 of the sov parity spec (unpublished); the wire is
//! `docs/re/sov-gateway.md` §2.1 and §4, the state rule §5.2).
//!
//! **What an upgrade is here.** An item of one of the four upgrade groups
//! ([`UPGRADE_GROUPS`], `sovereignty/upgrades/const.py:16-20` **[V]**) that a
//! pilot drops into the hub window and saves. The item is *consumed* on
//! install **[A]** (falsifier: a retail hub that hands the item back on
//! uninstall; the client's `ConfirmDestroyUpgrade` wording says it is
//! destroyed **[V]**): the stored row *is* the upgrade. It is lost with the hub
//! **[A]**.
//!
//! **State** (delta kind [`KIND`], `delta_kinds::KINDS` row `sovupg`):
//!
//! ```text
//! sovupg <hubID> -> type:online:state,type:online:state,...
//! ```
//!
//! in configured order (order is priority: the rule walks top-down). `online`
//! is `0` or `1`, `state` is [`Power`] `1..=4`.
//!
//! **The rule** is [`simulate`], pure: the client's
//! `hubUtil.UpdateEffectiveStatesForUpgrades` (`sovereignty/client/sovHub/
//! hubUtil.py:15-72` **[V]**) with `hasUnsavedChanges = false`. The window
//! compares the server's answer with its own run of that function
//! (`hubController.py:349-362`, `:164-168`), so a rule that drifts shows as
//! "configuration saved not same".
//!
//! **The Pending state cannot be reached by that rule.** `lackingStartupCost`
//! is `startup + used > stock`, but the fuel cost the same function tests
//! afterwards (`lackingFuel`) *includes* the startup, so a startup shortfall
//! is always a fuel shortfall too, and `lackingFuel` wins (`:60-64`: Low). The
//! spec's "stock = startup - 1 gives Pending" is therefore not what the client
//! computes; this port does what the client does and says Low. [`Power::Pending`]
//! exists for the wire and for a stored row that says it.
//!
//! **Reagents** are a [`Bay`]: the stock a hub holds, the startup paid out of
//! it and the burn per hour set on it. The production bay is [`FuelBay`], a bay
//! over `sov_hub_fuel` (#218); [`NoBay`] (an empty one) is for tests.
//! The wiring: `sov_hub_fuel`'s resimulate slot defaults to [`resimulate`]
//! ([`install_hook`] sets it by name), `sov_resources::allocated` sums the
//! Online slots, `skyhook::changed` calls [`resimulate`], and `/sov upgrade`
//! ([`slash`]) seeds an install. The stamp `GetHubUpgrades` and its notice
//! carry is the bay's oldest stamp ([`Bay::oldest_stamp`]).
//!
//! **Registration (coordinator).** [`UPGRADES`] in `content_tables::TABLES`,
//! [`gateway`] in `gateway_ext::CHAIN` (above `sov_gw_stub::gateway`, which
//! refuses the two writes this module answers), `pub mod sov_upgrade;` in
//! `lib.rs` and `("sovupg", "sov_upgrade")` in `delta_kinds::KINDS`.
//!
//! **System effects (SOV-UPG-4)**: an Online upgrade's `dbuff:` tokens (the
//! `enables` column) are held on every character ship in its system by
//! `sov_effect`; see [`online_effects`].
//!
//! **Clock:** reagent amounts are asked at wall-clock seconds
//! (`sov_proto::unix_now`), the sov-held rule at the game clock.

use crate::content::{Boxed, Content, Sources, TableSpec};
use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, field, get_bytes, get_uint, put_bytes};
use crate::service::Registry;
use crate::sov;
use crate::sov_hub::{self, Hub};
use crate::sov_proto::{id, put_uint, read_id, status, timestamp, typed, unix_now};
use std::collections::{BTreeMap, BTreeSet};

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovupg";

/// The upgrade table, relative to `--authored`.
pub const UPGRADES_FILE: &str = "sov/upgrades.tsv";
/// Its header: the nine `Definition` fields (RE §2.1) and the reason.
pub const UPGRADES_HEADER: &str = "type_id\tpower\tworkforce\tfuel_type\tfuel_per_hour\tstartup\texclusive_group\tpower_out\tworkforce_out\tenables\twhy";
const UPGRADES_SHIPPED: &str = include_str!("../../../../content/sov/upgrades.tsv");

/// `groupSovHubAnomalyDetectionUpgrades`, `...ServiceInfrastructureUpgrades`,
/// `...SystemEffectGeneratorUpgrades` and `...ColonyResourcesManagementUpgrades`
/// **[V]** (`inventorycommon/const.py:1113-1116`, listed by
/// `sovereignty/upgrades/const.py:16-20`).
pub const UPGRADE_GROUPS: [i64; 4] = [4768, 4772, 4839, 4838];

const PKG: &str = "eve_public.sovereignty.hub.upgrade.api.";
const NOTICE_SIMULATED: &str = "eve_public.sovereignty.hub.upgrade.api.HubUpgradesSimulatedNotice";

// ---------------------------------------------------------------------------
// The rule (pure)
// ---------------------------------------------------------------------------

/// `PowerState` (`hub/upgrade/upgrade.proto`, upgrade_pb2.py:19-23 **[V]**).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    Offline = 1,
    Online = 2,
    Low = 3,
    Pending = 4,
}

impl Power {
    fn from_u8(v: u8) -> Option<Power> {
        match v {
            1 => Some(Power::Offline),
            2 => Some(Power::Online),
            3 => Some(Power::Low),
            4 => Some(Power::Pending),
            _ => None,
        }
    }
}

/// One upgrade type's numbers: the `Definition` fields the rule reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Def {
    pub power: u64,
    pub workforce: u64,
    /// The reagent type it burns, if any.
    pub fuel: Option<i64>,
    pub per_hour: u32,
    pub startup: u32,
    pub power_out: u64,
    pub workforce_out: u64,
}

/// One upgrade in configured order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    pub type_id: i64,
    /// Configured online (the toggle).
    pub online: bool,
    /// The stored state; `None` for an upgrade that is being installed.
    pub prev: Option<Power>,
    /// Installed in this very save (the client's `isNewUpgrade`).
    pub new: bool,
}

/// The state of every slot, and the startup reagent the slot pays: its
/// `startup` when it ends Online with the startup due, else 0
/// (`hubUtil.GetStartupCostForUpgrades` with `includePending=false`, `:137-153`).
///
/// `power` and `workforce` are the hub's budget before any upgrade; `fuel` is
/// the stock of a reagent now. A slot whose type `defs` does not know is
/// Offline and uses nothing.
pub fn simulate(
    slots: &[Slot],
    defs: &dyn Fn(i64) -> Option<Def>,
    power: u64,
    workforce: u64,
    fuel: &dyn Fn(i64) -> u64,
) -> Vec<(Power, u64)> {
    let mut power_used = 0u64;
    let mut workforce_used = 0u64;
    let mut fuel_used: BTreeMap<i64, u64> = BTreeMap::new();
    let mut total_power = power;
    let mut total_workforce = workforce;
    let mut out = Vec::with_capacity(slots.len());
    for s in slots {
        let Some(d) = defs(s.type_id) else {
            out.push((Power::Offline, 0));
            continue;
        };
        let lack_power = d.power.saturating_add(power_used) > total_power;
        let lack_workforce = d.workforce.saturating_add(workforce_used) > total_workforce;
        let mut lack_startup = false;
        let mut lack_fuel = false;
        let mut cost = 0u64;
        let mut startup_due = false;
        if let Some(ft) = d.fuel {
            let used = fuel_used.get(&ft).copied().unwrap_or(0);
            let stock = fuel(ft);
            startup_due = s.new || s.prev == Some(Power::Pending) || s.prev == Some(Power::Offline);
            if startup_due {
                let startup = u64::from(d.startup);
                lack_startup = startup.saturating_add(used) > stock;
                cost += startup;
            }
            cost += u64::from(d.per_hour > 0);
            lack_fuel = cost.saturating_add(used) > stock;
        }
        if s.online && !lack_power && !lack_workforce && !lack_fuel {
            power_used = power_used.saturating_add(d.power);
            workforce_used = workforce_used.saturating_add(d.workforce);
            if let (Some(ft), true) = (d.fuel, cost > 0) {
                *fuel_used.entry(ft).or_insert(0) += cost;
            }
        }
        let state = if !s.online {
            Power::Offline
        } else if lack_fuel || lack_power || lack_workforce {
            Power::Low
        } else if lack_startup {
            Power::Pending
        } else {
            total_power = total_power.saturating_add(d.power_out);
            total_workforce = total_workforce.saturating_add(d.workforce_out);
            Power::Online
        };
        let paid = if state == Power::Online && startup_due {
            u64::from(d.startup)
        } else {
            0
        };
        out.push((state, paid));
    }
    out
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The services an Online upgrade can switch on (the `enables` column, SOV-UPG-2
/// #80). **[A]**: the names are ours; falsifier: the client names a service
/// gate of its own for the Ansiblex or the Tenebrex.
pub const SERVICES: [&str; 3] = ["jump_bridge", "cyno_beacon", "cyno_jammer"];

/// One authored row: the numbers, the exclusive group and what it enables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub def: Def,
    /// Two installed upgrades never share it (`hubController.py:220-227`).
    pub group: String,
    /// The services of [`SERVICES`] it switches on while Online.
    pub services: BTreeSet<String>,
    /// The site tier it opens: a `population.tsv` row of band
    /// `sovupg:<group>:<tier>` is live while an Online upgrade of that
    /// exclusive group carries that tier **[A]**.
    pub tier: Option<u32>,
    /// The dbuffs it holds on every pilot's ship in the system while Online
    /// (SOV-UPG-4): (collection id, value in thousandths), in column order.
    /// Thousandths keep the row `Eq`.
    pub effects: Vec<(i64, i64)>,
}

/// What [`parse_enables`] reads: services, site tier, dbuff effects.
type Enables = (BTreeSet<String>, Option<u32>, Vec<(i64, i64)>);

/// The `enables` column: `-`, or a comma list of [`SERVICES`] names,
/// `sites:<tier>` (at most one) and `dbuff:<collection>=<value>` (SOV-UPG-4).
fn parse_enables(field: &str) -> Result<Enables, String> {
    let mut services = BTreeSet::new();
    let mut tier = None;
    let mut effects: Vec<(i64, i64)> = Vec::new();
    if dash(field) {
        return Ok((services, tier, effects));
    }
    for tok in field.split(',').map(str::trim) {
        if let Some(t) = tok.strip_prefix("sites:") {
            let t = t
                .parse::<u32>()
                .ok()
                .filter(|t| *t > 0)
                .ok_or_else(|| format!("enables: {tok:?} needs a positive tier"))?;
            if tier.replace(t).is_some() {
                return Err("enables: two sites: tiers".into());
            }
        } else if let Some(d) = tok.strip_prefix("dbuff:") {
            let bad = || format!("enables: {tok:?} is not dbuff:<collection>=<value>");
            let (c, v) = d.split_once('=').ok_or_else(bad)?;
            let c = c
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|c| *c > 0)
                .ok_or_else(bad)?;
            let v = v
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && v.abs() <= 1000.0)
                .ok_or_else(bad)?;
            if effects.iter().any(|(e, _)| *e == c) {
                return Err(format!("enables: dbuff:{c} twice"));
            }
            effects.push((c, (v * 1000.0).round() as i64));
        } else if SERVICES.contains(&tok) {
            services.insert(tok.to_owned());
        } else {
            return Err(format!(
                "enables: {tok:?} is not sites:<tier>, dbuff:<collection>=<value> or one of {SERVICES:?}"
            ));
        }
    }
    Ok((services, tier, effects))
}

/// `content/sov/upgrades.tsv`, by upgrade type id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpgradeTable {
    rows: BTreeMap<i64, Row>,
}

fn dash(s: &str) -> bool {
    s.is_empty() || s == "-"
}

/// One row. A fuel burn or startup needs a fuel type; the exclusive group may
/// not be empty (the client compares groups as strings, so two upgrades with
/// the empty group would exclude each other, `hubController.py:222-225`).
pub fn parse_row(r: &crate::authored::Row) -> Result<(i64, Row), String> {
    let type_id = r.i64(0)?;
    if type_id <= 0 {
        return Err("type_id is not a positive type id".into());
    }
    let num = |i: usize| -> Result<u64, String> {
        let v = r.i64(i)?;
        u64::try_from(v).map_err(|_| format!("column {} is negative", i + 1))
    };
    let fuel_key = r.field(3);
    let fuel = if dash(fuel_key) || fuel_key == "0" {
        None
    } else {
        Some(
            fuel_key
                .parse::<i64>()
                .ok()
                .filter(|t| *t > 0)
                .ok_or("fuel_type is not a positive type id")?,
        )
    };
    let per_hour = u32::try_from(num(4)?).map_err(|_| "fuel_per_hour too large")?;
    let startup = u32::try_from(num(5)?).map_err(|_| "startup too large")?;
    if fuel.is_none() && (per_hour > 0 || startup > 0) {
        return Err("a burn or startup needs a fuel_type".into());
    }
    let group = r.field(6).trim();
    if dash(group) {
        return Err("exclusive_group is empty".into());
    }
    let (services, tier, effects) = parse_enables(r.field(9).trim())?;
    Ok((
        type_id,
        Row {
            def: Def {
                power: num(1)?,
                workforce: num(2)?,
                fuel,
                per_hour,
                startup,
                power_out: num(7)?,
                workforce_out: num(8)?,
            },
            group: group.to_owned(),
            services,
            tier,
            effects,
        },
    ))
}

impl UpgradeTable {
    /// A table from parsed rows; the last row of a type wins and a repeat is
    /// reported.
    pub fn new(rows: Vec<(i64, Row)>) -> (UpgradeTable, Vec<String>) {
        let mut map = BTreeMap::new();
        let mut dup = Vec::new();
        for (t, row) in rows {
            if map.insert(t, row).is_some() {
                dup.push(format!("upgrade type {t} appears twice; the last row wins"));
            }
        }
        (UpgradeTable { rows: map }, dup)
    }

    pub fn get(&self, type_id: i64) -> Option<&Row> {
        self.rows.get(&type_id)
    }

    pub fn def(&self, type_id: i64) -> Option<Def> {
        self.rows.get(&type_id).map(|r| r.def)
    }

    /// Every row, in type order.
    pub fn rows(&self) -> impl Iterator<Item = (i64, &Row)> {
        self.rows.iter().map(|(t, r)| (*t, r))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn load_upgrades(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(UPGRADES_FILE, UPGRADES_SHIPPED, UPGRADES_HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_row);
    let (table, dups) = UpgradeTable::new(rows);
    for d in dups {
        crate::outln!("[sov_upgrade] {UPGRADES_FILE}: {d}");
    }
    Ok(Box::new(table))
}

/// Invented reagent type of the fixture.
pub const FIXTURE_REAGENT: i64 = 9_101;

/// Invented rows `9_201..=9_205`, never the real ones. 9_201 and 9_202 share
/// a group; 9_203 makes power; 9_204 needs a lot; 9_205 burns a reagent.
pub fn fixture_upgrades() -> UpgradeTable {
    let row = |power, fuel, per_hour, startup, group: &str, power_out, enables: &str| Row {
        def: Def {
            power,
            workforce: 0,
            fuel,
            per_hour,
            startup,
            power_out,
            workforce_out: 0,
        },
        group: group.to_owned(),
        services: parse_enables(enables).expect("a fixture enables").0,
        tier: parse_enables(enables).expect("a fixture enables").1,
        effects: parse_enables(enables).expect("a fixture enables").2,
    };
    UpgradeTable::new(vec![
        (9_201, row(20, None, 0, 0, "ga", 0, "sites:1")),
        (9_202, row(20, None, 0, 0, "ga", 0, "sites:2")),
        (9_203, row(0, None, 0, 0, "gb", 10, "cyno_jammer")),
        (9_204, row(30, None, 0, 0, "gc", 0, "jump_bridge")),
        (
            9_205,
            row(5, Some(FIXTURE_REAGENT), 3, 2, "gd", 0, "cyno_beacon"),
        ),
    ])
    .0
}

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const UPGRADES: TableSpec = TableSpec {
    name: "sov upgrades",
    load: load_upgrades,
    fixture: || Box::new(fixture_upgrades()) as Boxed,
};

static NO_TABLE: UpgradeTable = UpgradeTable {
    rows: BTreeMap::new(),
};

/// The upgrade table in `content`; empty when none is registered.
pub fn table(content: &Content) -> &UpgradeTable {
    content.try_get::<UpgradeTable>().unwrap_or(&NO_TABLE)
}

// ---------------------------------------------------------------------------
// The stored state
// ---------------------------------------------------------------------------

/// One installed upgrade as stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Installed {
    pub type_id: i64,
    pub online: bool,
    pub state: Power,
}

fn key(hub: i64) -> Vec<String> {
    vec![hub.to_string()]
}

/// The hub's upgrades in configured order. A malformed entry is skipped.
pub fn installed(d: &Deltas, hub: i64) -> Vec<Installed> {
    let Some(v) = d.of(KIND).get(&key(hub)) else {
        return Vec::new();
    };
    v.iter()
        .filter_map(|e| {
            let mut p = e.split(':');
            let type_id = p.next()?.parse().ok()?;
            let online = match p.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            let state = Power::from_u8(p.next()?.parse().ok()?)?;
            Some(Installed {
                type_id,
                online,
                state,
            })
        })
        .collect()
}

/// Store the hub's upgrades; an empty list removes the row.
pub fn put_installed(d: &mut Deltas, hub: i64, list: &[Installed]) {
    if list.is_empty() {
        d.del(KIND, &key(hub));
        return;
    }
    let v = list
        .iter()
        .map(|i| format!("{}:{}:{}", i.type_id, u8::from(i.online), i.state as u8))
        .collect();
    d.put(KIND, key(hub), v);
}

/// What a caller asks of [`upgrade_online`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want<'a> {
    /// This upgrade type is Online.
    Type(i64),
    /// An Online upgrade enables this service (one of [`SERVICES`]).
    Service(&'a str),
    /// An Online upgrade of this exclusive group carries this site tier
    /// (`population.tsv` band `sovupg:<group>:<tier>`).
    Site(&'a str, u32),
}

/// **What an online upgrade does (SOV-UPG-2, #80).** Whether `system`'s hub,
/// held by an alliance, has an upgrade that is Online and gives `want`.
/// Needs the table `t` (a `Deltas` alone does not know what a type enables):
/// `table(reg.content())`. A neutral hub's upgrades do nothing **[A]**
/// (falsifier: a retail neutral hub whose installed upgrades still run).
/// The spec's `Want::Group(i64)` is not offered: the table carries no
/// inventory group, only the exclusive group, which [`Want::Site`] uses.
pub fn upgrade_online(d: &Deltas, t: &UpgradeTable, system: i64, want: Want) -> bool {
    let Some(h) = sov_hub::hub(d, system) else {
        return false;
    };
    if sov::holder(d, system).is_none() {
        return false;
    }
    installed(d, h.hub_id)
        .into_iter()
        .filter(|i| i.state == Power::Online)
        .any(|i| match want {
            Want::Type(ty) => i.type_id == ty,
            Want::Service(s) => t.get(i.type_id).is_some_and(|r| r.services.contains(s)),
            Want::Site(g, tier) => t
                .get(i.type_id)
                .is_some_and(|r| r.group == g && r.tier == Some(tier)),
        })
}

/// `attributeDistributionID{Faction}01`, the first of ten per-faction slots, for the
/// six pirate factions: Angel 500011, Blood Raiders 500012, Guristas 500010, Rogue
/// Drones 500025, Sansha 500019, Serpentis 500020 [V] dogma/const.py:317-376,
/// appConst.py:577-603.
pub const DIST_BY_FACTION: [(i64, i64); 6] = [
    (500011, 1695),
    (500012, 1705),
    (500010, 1715),
    (500025, 1725),
    (500019, 1735),
    (500020, 1745),
];
/// `attributeDistributionID01`, the first of ten universal slots [V]
/// dogma/const.py:307-316.
pub const DIST_UNIVERSAL: i64 = 1755;
/// Slots per attribute family [V] dogma/const.py:307-376.
pub const DIST_SLOTS: i64 = 10;

/// The non-zero values of the ten attributes from `base`, in slot order.
fn distribution_slots(content: &Content, type_id: i64, base: i64) -> Vec<i64> {
    let Some(td) = content.try_get::<crate::typedata::Installed>() else {
        return Vec::new();
    };
    let Some(row) = td.table().get(type_id) else {
        return Vec::new();
    };
    (0..DIST_SLOTS)
        .filter_map(|i| row.attribute(base + i))
        .map(|v| v as i64)
        .filter(|v| *v != 0)
        .collect()
}

/// [`distributions`] with the attribute bases given: `by_faction` maps a faction id
/// to its family's first attribute id, `universal` is the family used when the faction
/// has no map entry or the type has no value in its family.
pub(crate) fn distributions_with(
    content: &Content,
    type_id: i64,
    faction: Option<i64>,
    by_faction: &[(i64, i64)],
    universal: i64,
) -> Vec<i64> {
    let own = faction
        .and_then(|f| by_faction.iter().find(|(id, _)| *id == f))
        .map(|(_, base)| distribution_slots(content, type_id, *base))
        .unwrap_or_default();
    if own.is_empty() {
        distribution_slots(content, type_id, universal)
    } else {
        own
    }
}

/// The distribution ids an upgrade type names for `faction` (its ten per-faction
/// attributes, non-zero, slot order), else its universal list. Empty without a bundle.
/// [I] the per-faction family is chosen by the system's faction, as the hub's server does
/// (`radius_dungeon_distribution/static_data_loader.py:64`, `get_distributions_for_faction`).
pub fn distributions(content: &Content, type_id: i64, faction: Option<i64>) -> Vec<i64> {
    distributions_with(content, type_id, faction, &DIST_BY_FACTION, DIST_UNIVERSAL)
}

/// Every `(exclusive group, tier, live cap)` a held hub in `system` has Online, computed
/// once: [`upgrade_online`]'s [`Want::Site`] for all of them. The cap is how many sites
/// the upgrade keeps live: one per distribution id it names for `pirate`. A type that names
/// none for this faction but carries a distribution family (some other faction's, or the
/// universal one) has cap 0, no sites; a type with no distribution attribute at all (or no
/// bundle) has cap `u32::MAX`: the population row's own count [A] (falsifier: an owner
/// wanting no sites without data).
pub fn online_site_tiers(
    d: &Deltas,
    t: &UpgradeTable,
    content: &Content,
    pirate: Option<i64>,
    system: i64,
) -> Vec<(String, u32, u32)> {
    online_site_tiers_with(
        d,
        t,
        content,
        pirate,
        system,
        &DIST_BY_FACTION,
        DIST_UNIVERSAL,
    )
}

/// [`online_site_tiers`] with the attribute bases given (see [`distributions_with`]).
pub(crate) fn online_site_tiers_with(
    d: &Deltas,
    t: &UpgradeTable,
    content: &Content,
    pirate: Option<i64>,
    system: i64,
    by_faction: &[(i64, i64)],
    universal: i64,
) -> Vec<(String, u32, u32)> {
    let Some(h) = sov_hub::hub(d, system) else {
        return Vec::new();
    };
    if sov::holder(d, system).is_none() {
        return Vec::new();
    }
    installed(d, h.hub_id)
        .into_iter()
        .filter(|i| i.state == Power::Online)
        .filter_map(|i| Some((i.type_id, t.get(i.type_id)?)))
        .filter_map(|(ty, r)| {
            let named = distributions_with(content, ty, pirate, by_faction, universal).len() as u32;
            let any = by_faction
                .iter()
                .map(|(_, base)| *base)
                .chain([universal])
                .any(|base| !distribution_slots(content, ty, base).is_empty());
            let cap = match (named, any) {
                (0, false) => u32::MAX,
                (n, _) => n,
            };
            Some((r.group.clone(), r.tier?, cap))
        })
        .collect()
}

/// The dbuffs `system`'s claimed hub holds on the ships in it: the `dbuff:` tokens of
/// its Online upgrades, in configured order, values as given (SOV-UPG-4). `None` for a
/// system with no hub, a neutral hub, or no Online upgrade with effects. A collection
/// two Online upgrades name keeps the larger absolute value.
/// [V] the collections a generator names (`buffBarConst.py:432-445`);
/// [A] the values (`content/sov/upgrades.tsv`).
pub fn online_effects(d: &Deltas, t: &UpgradeTable, system: i64) -> Option<(i64, Vec<(i64, f64)>)> {
    let h = sov_hub::hub(d, system)?;
    sov::holder(d, system)?;
    let mut out: Vec<(i64, f64)> = Vec::new();
    for i in installed(d, h.hub_id)
        .into_iter()
        .filter(|i| i.state == Power::Online)
    {
        let Some(r) = t.get(i.type_id) else { continue };
        for (c, v) in &r.effects {
            let v = *v as f64 / 1000.0;
            match out.iter_mut().find(|(e, _)| e == c) {
                Some(e) if v.abs() > e.1.abs() => e.1 = v,
                Some(_) => {}
                None => out.push((*c, v)),
            }
        }
    }
    (!out.is_empty()).then_some((h.hub_id, out))
}

/// Whether any row carries a dbuff effect (the effect stage's fast path).
pub fn any_effects(t: &UpgradeTable) -> bool {
    t.rows.values().any(|r| !r.effects.is_empty())
}

/// The types of `system`'s hub's upgrades that are Online (L7's export).
pub fn online_types(d: &Deltas, system: i64) -> Vec<i64> {
    let Some(h) = sov_hub::hub(d, system) else {
        return Vec::new();
    };
    installed(d, h.hub_id)
        .into_iter()
        .filter(|i| i.state == Power::Online)
        .map(|i| i.type_id)
        .collect()
}

// ---------------------------------------------------------------------------
// The reagent bay
// ---------------------------------------------------------------------------

/// What this module needs of a hub's reagent stock. `now` is unix seconds.
pub trait Bay {
    /// Units of `reagent` the hub holds at `now`.
    fn amount(&self, d: &Deltas, hub: i64, reagent: i64, now: u64) -> u64;
    /// Spend `n` units (a startup cost); `false`, nothing spent, when short.
    fn take(&self, d: &mut Deltas, hub: i64, reagent: i64, n: u64, now: u64) -> bool;
    /// Burn `per_hour` units an hour from `now` (settle first).
    fn set_burn(&self, d: &mut Deltas, hub: i64, reagent: i64, per_hour: u32, now: u64);
    /// The oldest stamp the hub's stock rows carry, if it has any: the time
    /// the client counts the burn from (`GetHubUpgrades.last_updated`).
    fn oldest_stamp(&self, _d: &Deltas, _hub: i64) -> Option<u64> {
        None
    }
}

/// An empty bay: holds nothing, spends nothing. For tests only; the
/// production bay is [`FuelBay`].
#[derive(Debug, Clone, Copy)]
pub struct NoBay;

impl Bay for NoBay {
    fn amount(&self, _: &Deltas, _: i64, _: i64, _: u64) -> u64 {
        0
    }
    fn take(&self, _: &mut Deltas, _: i64, _: i64, n: u64, _: u64) -> bool {
        n == 0
    }
    fn set_burn(&self, _: &mut Deltas, _: i64, _: i64, _: u32, _: u64) {}
}

/// The bay over `sov_hub_fuel`'s rows (#218): amounts are the client's own
/// burn rule at `now`, a startup is a `take`, a burn is a `set_burn`.
#[derive(Debug, Clone, Copy)]
pub struct FuelBay;

impl Bay for FuelBay {
    fn amount(&self, d: &Deltas, hub: i64, reagent: i64, now: u64) -> u64 {
        crate::sov_hub_fuel::amount_at(&crate::sov_hub_fuel::stock(d, hub, reagent), now)
    }
    fn take(&self, d: &mut Deltas, hub: i64, reagent: i64, n: u64, now: u64) -> bool {
        crate::sov_hub_fuel::take(d, hub, reagent, n, now)
    }
    fn set_burn(&self, d: &mut Deltas, hub: i64, reagent: i64, per_hour: u32, now: u64) {
        crate::sov_hub_fuel::set_burn(d, hub, reagent, per_hour, now);
    }
    fn oldest_stamp(&self, d: &Deltas, hub: i64) -> Option<u64> {
        crate::sov_hub_fuel::stocks(d, hub)
            .iter()
            .map(|(_, s)| s.last_updated)
            .min()
    }
}

/// The bay the gateway and the resimulate hook use.
pub fn production_bay() -> &'static dyn Bay {
    &FuelBay
}

/// Fill `sov_hub_fuel`'s resimulate slot with [`resimulate`]. The slot also
/// defaults to it, so a call is only needed to be explicit; `false` when the
/// slot was already filled.
pub fn install_hook() -> bool {
    crate::sov_hub_fuel::set_resimulate_hook(resimulate)
}

// ---------------------------------------------------------------------------
// Running the rule on a hub
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Entry {
    type_id: i64,
    online: bool,
    prev: Option<Power>,
    new: bool,
}

/// What [`plan`] decided: the entries as saved (a startup-owing slot that
/// would end Low is stored Offline), the numbers they ran against, the result.
struct Plan {
    entries: Vec<Entry>,
    defs: BTreeMap<i64, Def>,
    reagents: BTreeSet<i64>,
    sim: Vec<(Power, u64)>,
    list: Vec<Installed>,
}

/// Run [`simulate`] over `entries` for `hub` without writing anything.
///
/// A slot that owes its startup (new, or Offline or Pending before) and burns
/// a reagent, and would end Low, is saved Offline instead. Low never pays the
/// startup (`hubUtil.py:36-41` adds it only for a slot that ends Online), and a
/// Low slot later goes Online with `prev = Low`, which owes none: left as Low it
/// would be a way to start up for free. Offline owes it again on the next switch.
/// [I] The client's own rule agrees that a configured-online slot that lacks power
/// reads Offline in the unsaved-changes branch (`hubUtil.py:58-62`, `hubController.py:349-366`).
fn plan(
    reg: &Registry,
    hub: &Hub,
    entries: &[Entry],
    also: &[i64],
    bay: &dyn Bay,
    now: u64,
) -> Plan {
    let mut entries = entries.to_vec();
    let defs: BTreeMap<i64, Def> = {
        let t = table(reg.content());
        entries
            .iter()
            .filter_map(|e| t.def(e.type_id).map(|d| (e.type_id, d)))
            .collect()
    };
    let reagents: BTreeSet<i64> = defs
        .values()
        .filter_map(|d| d.fuel)
        .chain(also.iter().copied())
        .collect();
    let (power, workforce) = crate::sov_resources::harvested(reg, hub.system_id);
    // [A] dens take precedence over upgrades: their draw comes off the budget first;
    // falsifier: TQ keeping upgrades online while a den is over budget.
    let workforce =
        workforce.saturating_sub(crate::merc_den::workforce_use(reg, hub.system_id, now));
    let stock: BTreeMap<i64, u64> = reagents
        .iter()
        .map(|r| (*r, bay.amount(reg.deltas(), hub.hub_id, *r, now)))
        .collect();
    let owes = |e: &Entry| {
        let startup = defs
            .get(&e.type_id)
            .is_some_and(|d| d.fuel.is_some() && d.startup > 0);
        startup && (e.new || matches!(e.prev, Some(Power::Pending | Power::Offline)))
    };
    let sim = loop {
        let slots: Vec<Slot> = entries
            .iter()
            .map(|e| Slot {
                type_id: e.type_id,
                online: e.online,
                prev: e.prev,
                new: e.new,
            })
            .collect();
        let sim = simulate(&slots, &|t| defs.get(&t).copied(), power, workforce, &|r| {
            stock.get(&r).copied().unwrap_or(0)
        });
        let bypass = entries
            .iter()
            .zip(&sim)
            .position(|(e, (state, _))| e.online && *state == Power::Low && owes(e));
        match bypass {
            Some(i) => entries[i].online = false,
            None => break sim,
        }
    };
    let list: Vec<Installed> = entries
        .iter()
        .zip(&sim)
        .map(|(e, (state, _))| Installed {
            type_id: e.type_id,
            online: e.online,
            state: *state,
        })
        .collect();
    Plan {
        entries,
        defs,
        reagents,
        sim,
        list,
    }
}

/// Run [`plan`], then (when `force`, or when a state moved) pay the startup,
/// set the burns and store the result.
/// `also` names reagents to re-burn that no entry uses any more (an upgrade
/// just removed). Returns the stored list.
fn settle(
    reg: &mut Registry,
    hub: &Hub,
    entries: &[Entry],
    also: &[i64],
    bay: &dyn Bay,
    now: u64,
    force: bool,
) -> Vec<Installed> {
    let asked: Vec<bool> = entries.iter().map(|e| e.online).collect();
    let Plan {
        entries,
        defs,
        reagents,
        sim,
        list,
    } = plan(reg, hub, entries, also, bay, now);
    // The toggle itself moved (a slot that owed its startup was saved Offline).
    let moved_toggle = asked.iter().zip(&list).any(|(a, i)| *a != i.online);
    let moved = entries
        .iter()
        .zip(&list)
        .any(|(e, i)| e.prev != Some(i.state))
        || moved_toggle;
    if !force && !moved {
        return list;
    }
    let d = reg.deltas_mut();
    let mut startup: BTreeMap<i64, u64> = BTreeMap::new();
    for (e, (_, paid)) in entries.iter().zip(&sim) {
        if let (Some(ft), true) = (defs.get(&e.type_id).and_then(|d| d.fuel), *paid > 0) {
            *startup.entry(ft).or_insert(0) += paid;
        }
    }
    for (ft, n) in startup {
        if !bay.take(d, hub.hub_id, ft, n, now) {
            crate::errln!(
                "[sov_upgrade] hub {}: the bay could not pay {n} startup of {ft} it had counted",
                hub.hub_id
            );
        }
    }
    for r in &reagents {
        let per_hour: u64 = list
            .iter()
            .filter(|i| i.state == Power::Online)
            .filter_map(|i| defs.get(&i.type_id))
            .filter(|def| def.fuel == Some(*r))
            .map(|def| u64::from(def.per_hour))
            .sum();
        bay.set_burn(
            d,
            hub.hub_id,
            *r,
            u32::try_from(per_hour).unwrap_or(u32::MAX),
            now,
        );
    }
    put_installed(d, hub.hub_id, &list);
    list
}

/// The stored upgrades as entries that change nothing.
fn stored_entries(list: &[Installed]) -> Vec<Entry> {
    list.iter()
        .map(|i| Entry {
            type_id: i.type_id,
            online: i.online,
            prev: Some(i.state),
            new: false,
        })
        .collect()
}

/// Re-run the rule on `hub_id`'s stored upgrades against `bay` at `now`
/// (`new = false`): the inputs (reagents, budget) may have changed. Stores,
/// re-burns and announces only when a state moved. `true` when one did.
pub fn resimulate_with(reg: &mut Registry, hub_id: i64, bay: &dyn Bay, now: u64) -> bool {
    let Some(hub) = sov_hub::by_id(reg.deltas(), hub_id) else {
        return false;
    };
    let before = installed(reg.deltas(), hub_id);
    if before.is_empty() {
        return false;
    }
    let after = settle(reg, &hub, &stored_entries(&before), &[], bay, now, false);
    let changed = before
        .iter()
        .zip(&after)
        .any(|(a, b)| a.state != b.state || a.online != b.online);
    if changed {
        announce(reg, &hub, &after, now);
    }
    changed
}

/// Whether [`resimulate_with`] would move a state, decided under a read lock
/// without writing.
pub fn would_move(reg: &Registry, hub_id: i64, bay: &dyn Bay, now: u64) -> bool {
    let Some(hub) = sov_hub::by_id(reg.deltas(), hub_id) else {
        return false;
    };
    let before = installed(reg.deltas(), hub_id);
    let p = plan(reg, &hub, &stored_entries(&before), &[], bay, now);
    before
        .iter()
        .zip(&p.list)
        .any(|(a, b)| a.state != b.state || a.online != b.online)
}

/// [`resimulate_with`] on the production bay and the wall clock. Called when
/// reagents arrive or run dry and when the budget moves (`sov_hub_fuel`'s hook,
/// the skyhook switch).
pub fn resimulate(reg: &mut Registry, hub_id: i64) {
    resimulate_with(reg, hub_id, production_bay(), unix_now());
}

/// A corporation joined or left an alliance: the skyhooks it owns count (or
/// stop counting) towards the holder's budget, so every hub in a system where
/// it has a skyhook re-simulates and the resources window is told. **The call
/// belongs in `service/alliance.rs`** after `set_corp_alliance` (leave, and
/// the join path); that file is outside this lane (SOV-UPG-2 report).
pub fn corp_membership_changed(reg: &mut Registry, corp: i64) {
    corp_membership_changed_since(reg, corp, &crate::sov_workforce::Before::default());
}

/// The systems where `corp` owns a skyhook.
fn corp_systems(reg: &Registry, corp: i64) -> BTreeSet<i64> {
    crate::skyhook::all(reg.deltas())
        .iter()
        .filter(|s| s.corp == corp)
        .map(|s| s.system)
        .collect()
}

/// Snapshot the hubs a change of `corp`'s alliance can move, to be taken
/// ahead of the change (the hubs of [`corp_membership_changed`]'s systems and
/// their peers).
pub fn corp_membership_before(reg: &Registry, corp: i64) -> crate::sov_workforce::Before {
    let systems: Vec<i64> = corp_systems(reg, corp).into_iter().collect();
    crate::sov_workforce::before_systems(reg, &systems, false)
}

/// [`corp_membership_changed`], and the `StateChangedNotice`s of the hubs
/// whose state moved since `before` ([`corp_membership_before`]).
pub fn corp_membership_changed_since(
    reg: &mut Registry,
    corp: i64,
    before: &crate::sov_workforce::Before,
) {
    for system in corp_systems(reg, corp) {
        if let Some(h) = sov_hub::hub(reg.deltas(), system) {
            resimulate(reg, h.hub_id);
            crate::sov_resources::announce(reg, h.hub_id);
        }
        crate::sov_workforce::budgets_changed(reg, system);
    }
    crate::sov_workforce::tell_changed(reg, before);
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// `upgrade.Definition` (`upgrade_pb2.py:36-44` **[V]**).
fn definition_bytes(type_id: i64, row: Option<&Row>) -> Vec<u8> {
    let mut d = Vec::new();
    put_bytes(&mut d, 1, &id(type_id as u64));
    let Some(row) = row else {
        return d;
    };
    let def = &row.def;
    put_uint(&mut d, 2, def.power);
    put_uint(&mut d, 3, def.workforce);
    if let Some(ft) = def.fuel {
        put_bytes(&mut d, 4, &id(ft as u64));
    }
    put_uint(&mut d, 5, u64::from(def.per_hour));
    put_uint(&mut d, 6, u64::from(def.startup));
    put_bytes(&mut d, 7, row.group.as_bytes());
    put_uint(&mut d, 8, def.power_out);
    put_uint(&mut d, 9, def.workforce_out);
    d
}

/// `HubUpgrades` (`upgrade_pb2.py:48-54` **[V]**): the hub, one entry per
/// upgrade in order, the time.
fn hub_upgrades_bytes(content: &Content, hub_id: i64, list: &[Installed], stamp: u64) -> Vec<u8> {
    let t = table(content);
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &id(hub_id as u64));
    for i in list {
        let mut ident = Vec::new();
        put_bytes(&mut ident, 1, &id(hub_id as u64));
        // `installation_type` is deprecated in the proto; the client reads
        // `upgrade_type` (proto_parsers.py:21). Both carry the type id [A].
        put_bytes(&mut ident, 2, &id(i.type_id as u64));
        put_bytes(&mut ident, 3, &id(i.type_id as u64));
        let mut attrs = Vec::new();
        put_bytes(
            &mut attrs,
            1,
            &definition_bytes(i.type_id, t.get(i.type_id)),
        );
        put_uint(&mut attrs, 2, i.state as u64);
        let mut up = Vec::new();
        put_bytes(&mut up, 1, &ident);
        put_bytes(&mut up, 2, &attrs);
        put_bytes(&mut out, 2, &up);
    }
    put_bytes(&mut out, 3, &timestamp(stamp));
    out
}

/// The `last_updated` of a hub's answer: the bay's oldest stamp, else `now`.
/// **[A]** the client counts the burn from this one stamp (`data_types.py:166-171`)
/// and `docs/re/sov-gateway.md:143` does not say which of several row stamps it
/// expects; falsifier: a client run where the shown reagent time differs from
/// `amount_at` after a second deposit of another type.
fn stamp_of(d: &Deltas, hub_id: i64, now: u64) -> u64 {
    production_bay().oldest_stamp(d, hub_id).unwrap_or(now)
}

/// Queue `HubUpgradesSimulatedNotice` for the pilots in the hub's system, and
/// the resources notice (the allocated share moved). Under the world lock;
/// sends nothing.
fn announce(reg: &mut Registry, hub: &Hub, list: &[Installed], now: u64) {
    let mut payload = Vec::new();
    put_bytes(
        &mut payload,
        1,
        &hub_upgrades_bytes(
            reg.content(),
            hub.hub_id,
            list,
            stamp_of(reg.deltas(), hub.hub_id, now),
        ),
    );
    let members = crate::service::structures::system_audience(reg, hub.system_id);
    crate::gateway_out::queue(
        reg,
        NOTICE_SIMULATED,
        payload,
        Target::SolarSystem {
            id: hub.system_id,
            members,
        },
    );
    crate::sov_resources::announce(reg, hub.hub_id);
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

/// Whether `who`'s alliance holds the hub's system.
fn is_holder(reg: &Registry, hub: &Hub, who: i64) -> bool {
    let mine = reg.character(who).and_then(|c| c.alliance_id);
    mine.is_some() && mine == sov::holder(reg.deltas(), hub.system_id)
}

/// The window's own rule: the holder alliance and a station manager or
/// director (the menu's rule, `celestialCheckers.py:1076-1084` **[V]**). The
/// reason `who` may not manage the hub, `None` when they may; the two halves
/// are told apart for the 403 log line.
fn manage_refusal(reg: &Registry, hub: &Hub, who: i64) -> Option<&'static str> {
    if !is_holder(reg, hub, who) {
        Some("holder: not in the alliance that holds the system")
    } else if !crate::service::sov_hub::may_launch(reg, who) {
        Some("manage: lacks the corporation role")
    } else {
        None
    }
}

/// Whether `who`'s ship is within [`sov_hub::TRANSFER_RANGE`] of the hub's
/// surface (`const.maxCargoContainerTransferDistance`, `hubController.py:201-215`
/// **[V]**), measured as `/sovfuel` does.
fn in_range(reg: &mut Registry, who: i64, hub: &Hub) -> bool {
    let Some(c) = reg.character(who).cloned() else {
        return false;
    };
    reg.advance_character(who);
    let Some(here) = reg
        .roster()
        .get(who)
        .copied()
        .filter(|p| p.ship_id == c.ship_id && p.solar_system_id == hub.system_id)
    else {
        return false;
    };
    let hull = crate::destiny::hull::of_in(reg.content(), c.ship_type_id);
    let gap = crate::space_state::distance(here.motion.position, hub.position)
        - sov_hub::hub_radius(reg.content())
        - f64::from(hull.radius);
    gap <= sov_hub::TRANSFER_RANGE
}

/// The sov-held-for rule: `attributeDevIndexSovereignty` of the type against
/// the days the system has been held, through the strategic index table
/// (`hubController.py:229-237`, `devIndexUtil.GetTimeIndexLevelForDays`
/// **[V]**). A type without the attribute needs nothing **[I]**. The client
/// counts wall-clock days (`blue.os.GetWallclockTime`, `hubController.py:232`);
/// this counts the game clock's days, which is the same unless the clock is
/// scaled **[I]** (falsifier: a scaled-clock server where the client's window
/// and this check disagree about the same claim).
fn held_long_enough(reg: &Registry, hub: &Hub, type_id: i64) -> bool {
    let need = reg
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(type_id))
        .and_then(|row| row.attribute(crate::sov_index::ATTR_DEV_INDEX_SOVEREIGNTY))
        .map_or(0, |v| v as i64);
    if need <= 0 {
        return true;
    }
    let Some(claim_time) = sov::claim(reg.deltas(), hub.system_id).and_then(|c| c.claim_time)
    else {
        return false;
    };
    let secs = crate::sov_index::claimed_seconds(
        Some(claim_time),
        crate::pyutil::filetime_now(),
        crate::game_clock::of(reg),
    );
    crate::sov_index::strategic_level(secs) as i64 >= need
}

/// Log why the hub refused with 403 (the wire answer carries no reason) and
/// return the status. One `[sov_upgrade] 403 <reason>` line per refusal.
fn refuse_403(hub_id: i64, who: i64, reason: &str) -> u32 {
    crate::errln!(
        "{}",
        crate::sov_proto::refusal_line("sov_upgrade", hub_id, who, reason)
    );
    403
}

/// The checks on one dropped item, in the client's `GetDropError` order
/// (`hubController.py:170-190`: type, installed, exclusive group, range, sov
/// level, in the ship; the owner check first is ours), as the status the messenger branches on: 404 gone, 403 not
/// the pilot's or not in the ship or out of range or held too short, 400 not
/// an upgrade, 409 already installed or its group taken.
fn check_item(
    reg: &mut Registry,
    who: i64,
    hub: &Hub,
    item_id: i64,
    taken: &[i64],
) -> Result<i64, u32> {
    let ship = reg
        .character(who)
        .map(|c| c.ship_id)
        .ok_or_else(|| refuse_403(hub.hub_id, who, "no character"))?;
    let item = reg.store().get(item_id).cloned().ok_or(404u32)?;
    // Not the client's check, ours: another pilot's item is refused first.
    if item.owner_id != who {
        return Err(refuse_403(
            hub.hub_id,
            who,
            &format!("item {item_id} belongs to {}", item.owner_id),
        ));
    }
    if !UPGRADE_GROUPS.contains(&item.group_id) {
        return Err(400);
    }
    let row = table(reg.content())
        .get(item.type_id)
        .cloned()
        .ok_or(400u32)?;
    if taken.contains(&item.type_id) {
        return Err(409);
    }
    let clash = {
        let t = table(reg.content());
        taken
            .iter()
            .filter_map(|ty| t.get(*ty))
            .any(|r| r.group == row.group)
    };
    if clash {
        return Err(409);
    }
    if !in_range(reg, who, hub) {
        return Err(refuse_403(hub.hub_id, who, "range: out of transfer range"));
    }
    if !held_long_enough(reg, hub, item.type_id) {
        return Err(refuse_403(
            hub.hub_id,
            who,
            &format!("held: type {} needs a longer held claim", item.type_id),
        ));
    }
    if item.location_id != ship {
        return Err(refuse_403(
            hub.hub_id,
            who,
            &format!(
                "location: item {item_id} is in {}, not the ship {ship}",
                item.location_id
            ),
        ));
    }
    Ok(item.type_id)
}

// ---------------------------------------------------------------------------
// The arms
// ---------------------------------------------------------------------------

/// `ProcessConfigurationRequest` for `who`: the status to answer, or the
/// stored list in configured order. `new_items` are the dropped item ids,
/// `config` the whole window as `(type, online)`.
///
/// Nothing is stored or consumed on an `Err`. The checks, then the items
/// consumed, then the rule, the startup paid, the burns set and the notices.
pub fn configure(
    reg: &mut Registry,
    hub_id: i64,
    who: i64,
    new_items: &[i64],
    config: &[(i64, bool)],
    bay: &dyn Bay,
    now: u64,
) -> Result<Vec<Installed>, u32> {
    let hub = sov_hub::by_id(reg.deltas(), hub_id).ok_or(404u32)?;
    if let Some(why) = manage_refusal(reg, &hub, who) {
        return Err(refuse_403(hub_id, who, why));
    }
    let stored = installed(reg.deltas(), hub_id);
    let mut taken: Vec<i64> = stored.iter().map(|i| i.type_id).collect();
    let mut new_types = Vec::new();
    for item in new_items {
        let ty = check_item(reg, who, &hub, *item, &taken)?;
        taken.push(ty);
        new_types.push(ty);
    }
    // The configuration names exactly what is installed plus what is new,
    // each once.
    let mut named: Vec<i64> = config.iter().map(|(t, _)| *t).collect();
    named.sort_unstable();
    let mut want = taken.clone();
    want.sort_unstable();
    if named != want {
        return Err(400);
    }
    let edits: Vec<crate::item::Edit> = new_items
        .iter()
        .map(|item_id| crate::item::Edit::Consume {
            item_id: *item_id,
            quantity: 1,
        })
        .collect();
    if !edits.is_empty() {
        crate::sov_proto::apply_items_on_clock(reg, who, &edits).map_err(|e| {
            crate::errln!("[sov_upgrade] hub {hub_id}: consuming {new_items:?} failed: {e}");
            500u32
        })?;
    }
    let entries: Vec<Entry> = config
        .iter()
        .map(|(t, online)| Entry {
            type_id: *t,
            online: *online,
            prev: stored.iter().find(|i| i.type_id == *t).map(|i| i.state),
            new: new_types.contains(t),
        })
        .collect();
    let list = settle(reg, &hub, &entries, &[], bay, now, true);
    announce(reg, &hub, &list, now);
    Ok(list)
}

/// `UninstallRequest` for `who`: 404 when the type is not installed, 403
/// without the role. Removes it, re-runs the rule on the rest, re-burns,
/// announces. The upgrade item is gone for good **[A]**.
pub fn uninstall(
    reg: &mut Registry,
    hub_id: i64,
    who: i64,
    type_id: i64,
    bay: &dyn Bay,
    now: u64,
) -> Result<(), u32> {
    let hub = sov_hub::by_id(reg.deltas(), hub_id).ok_or(404u32)?;
    if let Some(why) = manage_refusal(reg, &hub, who) {
        return Err(refuse_403(hub_id, who, why));
    }
    let stored = installed(reg.deltas(), hub_id);
    if !stored.iter().any(|i| i.type_id == type_id) {
        return Err(404);
    }
    let gone_fuel: Vec<i64> = table(reg.content())
        .def(type_id)
        .and_then(|d| d.fuel)
        .into_iter()
        .collect();
    let rest: Vec<Installed> = stored
        .into_iter()
        .filter(|i| i.type_id != type_id)
        .collect();
    let list = settle(
        reg,
        &hub,
        &stored_entries(&rest),
        &gone_fuel,
        bay,
        now,
        true,
    );
    announce(reg, &hub, &list, now);
    Ok(())
}

fn id_of(body: &[u8], field_no: u64) -> Option<i64> {
    read_id(body, field_no)
        .filter(|h| *h > 0)
        .and_then(|h| i64::try_from(h).ok())
}

fn reply_for(e: u32) -> Reply {
    status(e)
}

/// `GetHubUpgradesRequest`: the stored list, re-run against the bay first. The
/// rule runs under the read lock; the write lock is taken only when a state
/// moved, so a poll that finds nothing new writes nothing and blocks nobody.
fn get_hub_upgrades(
    ty: &str,
    body: &[u8],
    character: i64,
    w: &std::sync::RwLock<Registry>,
    now: u64,
) -> Reply {
    let Some(hub_id) = id_of(body, 1) else {
        return status(404);
    };
    let reg = crate::service::world_read_timed(w, "sov_upgrade_get");
    let Some(hub) = sov_hub::by_id(reg.deltas(), hub_id) else {
        return status(404);
    };
    // 403 for a pilot outside the holding alliance is our policy [A]; the
    // client's 403 path is real (`external_messenger.py:46`).
    if !is_holder(&reg, &hub, character) {
        refuse_403(
            hub_id,
            character,
            "holder: not in the alliance that holds the system",
        );
        return status(403);
    }
    let moved = would_move(&reg, hub_id, production_bay(), now);
    let answer = |reg: &Registry| {
        let list = installed(reg.deltas(), hub_id);
        let mut out = Vec::new();
        put_bytes(
            &mut out,
            1,
            &hub_upgrades_bytes(
                reg.content(),
                hub_id,
                &list,
                stamp_of(reg.deltas(), hub_id, now),
            ),
        );
        typed(ty, out)
    };
    if !moved {
        return answer(&reg);
    }
    drop(reg);
    let mut reg = crate::service::world_write_timed(w, "sov_upgrade_get_write");
    resimulate_with(&mut reg, hub_id, production_bay(), now);
    answer(&reg)
}

/// The gateway arm (`gateway_ext::CHAIN`): `hub.upgrade.api`'s four requests
/// the client sends. `None` for everything else. Without a world every answer
/// is 404 (the item store and the content live there).
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    if !matches!(
        rest,
        "GetDefinitionsRequest"
            | "GetHubUpgradesRequest"
            | "ProcessConfigurationRequest"
            | "UninstallRequest"
    ) {
        return None;
    }
    let Ground::Server(w) = g else {
        return Some(status(404));
    };
    if rest == "GetDefinitionsRequest" {
        let reg = crate::service::world_read_timed(w, "sov_upgrade_definitions");
        let mut out = Vec::new();
        for (t, row) in table(reg.content()).rows() {
            put_bytes(&mut out, 1, &definition_bytes(t, Some(row)));
        }
        return Some(typed(ty, out));
    }
    let now = unix_now();
    if rest == "GetHubUpgradesRequest" {
        return Some(get_hub_upgrades(ty, body, character, w, now));
    }
    let mut reg = crate::service::world_write_timed(w, "sov_upgrade_gateway");
    Some(match rest {
        "ProcessConfigurationRequest" => {
            let Some(hub_id) = id_of(body, 3) else {
                return Some(status(404));
            };
            let items: Option<Vec<i64>> = field(body, 1)
                .iter()
                .map(|m| get_uint(m, 1).and_then(|v| i64::try_from(v).ok()))
                .collect();
            let config: Option<Vec<(i64, bool)>> = field(body, 2)
                .iter()
                .map(|m| {
                    let t = id_of(m, 1)?;
                    Some((t, get_uint(m, 2).unwrap_or(0) != 0))
                })
                .collect();
            let (Some(items), Some(config)) = (items, config) else {
                return Some(status(400));
            };
            match configure(
                &mut reg,
                hub_id,
                character,
                &items,
                &config,
                production_bay(),
                now,
            ) {
                Ok(list) => {
                    let mut out = Vec::new();
                    put_bytes(
                        &mut out,
                        1,
                        &hub_upgrades_bytes(
                            reg.content(),
                            hub_id,
                            &list,
                            stamp_of(reg.deltas(), hub_id, now),
                        ),
                    );
                    typed(ty, out)
                }
                Err(e) => reply_for(e),
            }
        }
        _ => {
            // `UninstallRequest { 1 upgrade { 1 hub, 3 upgrade_type } }`.
            let Some(up) = get_bytes(body, 1) else {
                return Some(status(404));
            };
            let (Some(hub_id), Some(type_id)) = (id_of(&up, 1), id_of(&up, 3)) else {
                return Some(status(404));
            };
            match uninstall(&mut reg, hub_id, character, type_id, production_bay(), now) {
                Ok(()) => typed(ty, Vec::new()),
                Err(e) => reply_for(e),
            }
        }
    })
}

// ---------------------------------------------------------------------------
// GM: /sov upgrade
// ---------------------------------------------------------------------------

/// The `/sov` words [`slash`] answers (`service/sov.rs` hands them over).
pub const SLASH_WORDS: &[&str] = &["upgrade"];

/// The usage text of this module's command (`service/sov.rs`'s `USAGE` carries the same words).
pub const USAGE: &str = "/sov upgrade <systemID|here> list | add <typeID> [offline] | clear";

fn slash_system(reg: &Registry, actor: Option<i64>, word: &str) -> Result<i64, String> {
    if word == "here" {
        return actor
            .and_then(|a| reg.session_system(a))
            .ok_or_else(|| "you are not in a system".to_owned());
    }
    word.parse::<i64>()
        .map_err(|_| format!("'{word}' is not a system id"))
}

fn state_name(p: Power) -> &'static str {
    match p {
        Power::Offline => "offline",
        Power::Online => "online",
        Power::Low => "low",
        Power::Pending => "pending",
    }
}

fn list_text(hub: i64, list: &[Installed]) -> String {
    if list.is_empty() {
        return format!("hub {hub}: no upgrades installed");
    }
    let rows: Vec<String> = list
        .iter()
        .map(|i| {
            format!(
                "{} {}{}",
                i.type_id,
                state_name(i.state),
                if i.online { "" } else { " (switched off)" }
            )
        })
        .collect();
    format!("hub {hub}: {}", rows.join(", "))
}

/// `/sov upgrade <here|system> list | add <typeID> [offline] | clear`: dev
/// seeding. `add` makes the item in the GM's cargo hold, then runs the same
/// [`configure`] path the hub window does (so the pilot must be in range,
/// hold the role, and the alliance must hold the system); `clear` runs
/// [`uninstall`] on every upgrade.
pub fn slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let go = |reg: &mut Registry| -> Result<String, String> {
        let ["upgrade", system, rest @ ..] = args else {
            return Err("usage".to_owned());
        };
        let system = slash_system(reg, actor, system)?;
        let hub = sov_hub::hub(reg.deltas(), system)
            .ok_or_else(|| format!("system {system} has no Sovereignty Hub"))?;
        let now = unix_now();
        let refused = |e: u32| format!("the hub refused it with status {e}");
        match rest {
            ["list"] => Ok(list_text(hub.hub_id, &installed(reg.deltas(), hub.hub_id))),
            ["add", ty, flag @ ..] if flag.is_empty() || flag == ["offline"] => {
                let who = actor.ok_or("this needs a pilot")?;
                let ty: i64 = ty.parse().map_err(|_| format!("'{ty}' is not a type id"))?;
                if table(reg.content()).get(ty).is_none() {
                    return Err(format!("type {ty} is not a hub upgrade"));
                }
                let ship = reg.character(who).map(|c| c.ship_id).ok_or("no ship")?;
                let changes = reg
                    .apply_items(&[crate::item::Edit::Create {
                        type_id: ty,
                        quantity: 1,
                        to: crate::item::Slot::new(who, ship, sov_hub::FLAG_CARGO),
                    }])
                    .map_err(|e| format!("could not make the item: {e:?}"))?;
                let item = changes
                    .iter()
                    .find_map(|c| match c {
                        crate::item::Change::Created { item_id, .. } => Some(*item_id),
                        _ => None,
                    })
                    .ok_or("the item was not made")?;
                let mut config: Vec<(i64, bool)> = installed(reg.deltas(), hub.hub_id)
                    .iter()
                    .map(|i| (i.type_id, i.online))
                    .collect();
                config.push((ty, flag.is_empty()));
                let list = match configure(
                    reg,
                    hub.hub_id,
                    who,
                    &[item],
                    &config,
                    production_bay(),
                    now,
                ) {
                    Ok(l) => l,
                    Err(e) => {
                        // A refused install leaves no stray item in the cargo.
                        if reg.store().get(item).is_some() {
                            let _ =
                                reg.apply_items(&[crate::item::Edit::Destroy { item_id: item }]);
                        }
                        return Err(refused(e));
                    }
                };
                Ok(list_text(hub.hub_id, &list))
            }
            ["clear"] => {
                let who = actor.ok_or("this needs a pilot")?;
                let types: Vec<i64> = installed(reg.deltas(), hub.hub_id)
                    .iter()
                    .map(|i| i.type_id)
                    .collect();
                for (n, t) in types.iter().enumerate() {
                    uninstall(reg, hub.hub_id, who, *t, production_bay(), now).map_err(|e| {
                        format!("{} ({n} of {} removed first)", refused(e), types.len())
                    })?;
                }
                Ok(format!(
                    "hub {}: {} upgrade(s) removed",
                    hub.hub_id,
                    types.len()
                ))
            }
            _ => Err("usage".to_owned()),
        }
    };
    match go(reg) {
        Ok(t) => t,
        Err(e) => format!("usage: {USAGE} -- {e}"),
    }
}

#[cfg(test)]
mod tests;
