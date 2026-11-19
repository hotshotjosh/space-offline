//! A system's power and workforce: what each planet and star gives, and what a
//! sov hub reads back (`hub.api.GetResources`). SOV-RES, #78, item L4 of the
//! sov parity spec (unpublished); the wire is `docs/re/sov-gateway.md`
//! §2.3 and §2.5, the budget rule §5.1 and the spec's §1.3 item 5.
//!
//! **Two authored tables** (`content/sov/planet_resources.tsv`,
//! `content/sov/star_power.tsv`, every number `[A]`: the server sends the
//! numbers, the client has none of its own, so they are ours). The client has
//! display bands for them (`orbitalSkyhook/resourceRichness.py` `[V]`); ours
//! are scaled to those bands `[I]`, so a planet reads as modest, average or
//! abundant rather than as zero. The planet table is keyed by planet *type*,
//! so a dozen rows cover the universe. A planet gives ONE kind: power,
//! workforce, or a colony reagent (RE §5.1 `[I]`; falsifier: a planet info
//! panel showing two kinds at once). A reagent is named in the table by its
//! item type id (an identifier, like the planet type ids; the client's
//! `typeColonyReagentIce` / `typeColonyReagentLava`, `inventorycommon/const.py`
//! `[V]`). Each id is checked once, on first use, against the bundle's
//! colony-reagent group ([`GROUP_COLONY_REAGENTS`]); a miss is logged and the
//! planets that name it read no reagent.
//!
//! **What the hub says.** `available` is the budget *before* upgrades (spec
//! §1.3 item 5): the window subtracts upgrade use itself. Until skyhooks
//! (L8) decide which planets are harvested, `available` is the maximum (the
//! owner's Q2 ruling, spec L4.3). [`allocated`] and [`harvested`]
//! are the two seams the later lanes replace: L6 fills `allocated`, L8
//! changes `harvested`.
//!
//! **The version** is not hand-bumped: it is `1.0.<FNV-1a of the loaded planet
//! table and its resolved reagent ids>` ([`PlanetTable::resolved`]), so an edit
//! to the planet file changes it at the next start and a client asks again. The
//! star table is not versioned on the wire (the client reads it once per
//! session): a star edit needs a client restart.
//!
//! **Registration (coordinator).** [`PLANETS`] and [`STARS`] in
//! `content_tables::TABLES`, and [`gateway`] in `gateway_ext::CHAIN`; until
//! they are added a registry has no table here and every yield reads zero
//! (nothing panics: the tables are read with `try_get`).
//!
//! **Clock:** nothing here reads a clock; yields are pure functions of the
//! catalogue and the tables.

use crate::celestial::Kind;
use crate::content::{Boxed, Content, Sources, TableSpec};
use crate::gateway::{Ground, Reply, Target, get_bytes, get_uint, put_bytes, put_varint};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The planet table, relative to `--authored`.
pub const PLANETS_FILE: &str = "sov/planet_resources.tsv";
/// Its header. `reagent_type_id` is an item type id (`-` or `0` for none).
pub const PLANETS_HEADER: &str = "planet_type_id\tpower\tworkforce\treagent_type_id\tamount_per_cycle\tcycle_s\tsecured_pct\tsecured_cap\tunsecured_cap\twhy";
/// The star table, relative to `--authored`.
pub const STARS_FILE: &str = "sov/star_power.tsv";
/// Its header. Star type `0` is the row for every star type not listed.
pub const STARS_HEADER: &str = "star_type_id\tpower\twhy";

const PLANETS_SHIPPED: &str = include_str!("../../../../content/sov/planet_resources.tsv");
const STARS_SHIPPED: &str = include_str!("../../../../content/sov/star_power.tsv");

/// `groupColonyReagents` **[V]**: read from the 3670638 bytecode
/// (`inventorycommon/const`) and already extracted as a typedata group by
/// `tools/typedata.py` (SOV-DATA, #216).
pub const GROUP_COLONY_REAGENTS: i64 = 4729;

/// The definitions' version, major and minor; the patch is derived from the
/// loaded table ([`PlanetTable::resolved`]). One version for the whole
/// universe keeps a skyhook's version from ever running ahead of the
/// definitions (RE §2.5 version hazard). The client keeps whichever version
/// the server last named (`planetary_resources_cache.py`), so a hash that is
/// not monotonic is fine `[V]`.
pub const RESOURCES_VERSION_BASE: (u32, u32) = (1, 0);

const PKG: &str = "eve_public.sovereignty.";
const PLANET_PKG: &str = "eve_public.sovereignty.resource.planet.api.";
const STAR_PKG: &str = "eve_public.sovereignty.resource.star.api.";
const HUB_PKG: &str = "eve_public.sovereignty.hub.api.";
const NOTICE_SIMULATED: &str = "eve_public.sovereignty.hub.api.ResourcesSimulatedNotice";

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// A planet type's reagent row, as authored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReagentRow {
    /// The reagent's item type id; checked against
    /// [`GROUP_COLONY_REAGENTS`] by [`PlanetTable::resolved`].
    pub type_id: i64,
    pub per_cycle: u32,
    pub cycle_s: u64,
    pub secured_pct: u32,
    pub secured_cap: u32,
    pub unsecured_cap: u32,
}

/// One planet type's yield, as authored: power, or workforce, or a reagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanetRow {
    pub power: u64,
    pub workforce: u64,
    pub reagent: Option<ReagentRow>,
}

/// `content/sov/planet_resources.tsv`, by planet type id.
#[derive(Debug, Default)]
pub struct PlanetTable {
    rows: BTreeMap<i64, PlanetRow>,
    /// Computed once, on first use, from the content's typedata.
    resolved: OnceLock<Resolved>,
}

impl Clone for PlanetTable {
    fn clone(&self) -> Self {
        PlanetTable {
            rows: self.rows.clone(),
            resolved: OnceLock::new(),
        }
    }
}

impl PartialEq for PlanetTable {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
    }
}

impl Eq for PlanetTable {}

/// What the table's reagent ids came to against the typedata, and the version
/// derived from the table and that outcome.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The reagent type ids that are in [`GROUP_COLONY_REAGENTS`].
    pub ok: BTreeSet<i64>,
    /// The ones that are not (absent, or in another group); logged once.
    pub unresolved: Vec<i64>,
    /// `(major, minor, patch)`, the patch an FNV-1a hash (u32).
    pub version: (u32, u32, u32),
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// `content/sov/star_power.tsv`, by star type id (`0` = any other type).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StarTable {
    rows: BTreeMap<i64, u64>,
}

fn dash(s: &str) -> bool {
    s.is_empty() || s == "-"
}

/// One planet row. A row must give exactly one kind; a reagent row needs an
/// amount, a `cycle_s` above 0 (the client divides by it, RE §5.4) and a
/// secured share within 0-100.
pub fn parse_planet_row(r: &crate::authored::Row) -> Result<(i64, PlanetRow), String> {
    let type_id = r.i64(0)?;
    let num = |i: usize| -> Result<u64, String> {
        let v = r.i64(i)?;
        u64::try_from(v).map_err(|_| format!("column {} is negative", i + 1))
    };
    let (power, workforce) = (num(1)?, num(2)?);
    let key = r.field(3);
    let reagent = if dash(key) || key == "0" {
        None
    } else {
        let type_id = key
            .parse::<i64>()
            .ok()
            .filter(|t| *t > 0)
            .ok_or("reagent_type_id is not a positive type id")?;
        let per_cycle = u32::try_from(num(4)?).map_err(|_| "amount_per_cycle too large")?;
        let cycle_s = num(5)?;
        let secured_pct = u32::try_from(num(6)?).map_err(|_| "secured_pct too large")?;
        if per_cycle == 0 {
            return Err("a reagent row needs amount_per_cycle above 0".into());
        }
        if cycle_s == 0 {
            return Err("a reagent row needs cycle_s above 0".into());
        }
        if secured_pct > 100 {
            return Err("secured_pct is above 100".into());
        }
        Some(ReagentRow {
            type_id,
            per_cycle,
            cycle_s,
            secured_pct,
            secured_cap: u32::try_from(num(7)?).map_err(|_| "secured_cap too large")?,
            unsecured_cap: u32::try_from(num(8)?).map_err(|_| "unsecured_cap too large")?,
        })
    };
    let kinds =
        usize::from(power > 0) + usize::from(workforce > 0) + usize::from(reagent.is_some());
    if kinds != 1 {
        return Err(format!(
            "a planet row gives exactly one of power, workforce, reagent (this one gives {kinds})"
        ));
    }
    Ok((
        type_id,
        PlanetRow {
            power,
            workforce,
            reagent,
        },
    ))
}

impl PlanetTable {
    /// A table from rows already parsed; the last row of a type wins
    /// (`BTreeMap::insert` replaces) and a repeat is reported.
    pub fn new(rows: Vec<(i64, PlanetRow)>) -> (PlanetTable, Vec<String>) {
        let mut map = BTreeMap::new();
        let mut dup = Vec::new();
        for (t, row) in rows {
            if map.insert(t, row).is_some() {
                dup.push(format!("planet type {t} appears twice; the last row wins"));
            }
        }
        (
            PlanetTable {
                rows: map,
                resolved: OnceLock::new(),
            },
            dup,
        )
    }

    /// The row for planet type `type_id`.
    pub fn get(&self, type_id: i64) -> Option<&PlanetRow> {
        self.rows.get(&type_id)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every distinct reagent type id the table names.
    pub fn reagent_ids(&self) -> BTreeSet<i64> {
        self.rows
            .values()
            .filter_map(|r| r.reagent.as_ref().map(|g| g.type_id))
            .collect()
    }

    /// Check the reagent ids against `content`'s typedata and derive the
    /// version; done once per table, logging each miss. An id the typedata
    /// does not hold in [`GROUP_COLONY_REAGENTS`] (or no typedata at all) is
    /// unresolved: its planets give no reagent.
    pub fn resolved(&self, content: &Content) -> &Resolved {
        self.resolved.get_or_init(|| {
            let ids = self.reagent_ids();
            let inst = content.try_get::<crate::typedata::Installed>();
            let (ok, unresolved) = resolve_ids(&ids, |t| {
                inst.and_then(|i| i.table().get(t))
                    .map(|row| row.group_id)
            });
            for t in &unresolved {
                crate::errln!(
                    "[sov_resources] {PLANETS_FILE}: reagent type {t} is not in group {GROUP_COLONY_REAGENTS} of the typedata; its planets give no reagent"
                );
            }
            let version = (
                RESOURCES_VERSION_BASE.0,
                RESOURCES_VERSION_BASE.1,
                fnv1a(self.canonical(&ok).as_bytes()),
            );
            Resolved {
                ok,
                unresolved,
                version,
            }
        })
    }

    /// The text the version hashes: every row in type order, then the
    /// reagent ids that resolved.
    fn canonical(&self, ok: &BTreeSet<i64>) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        for (t, r) in &self.rows {
            let _ = write!(s, "{t}|{}|{}|", r.power, r.workforce);
            match &r.reagent {
                Some(g) => {
                    let _ = write!(
                        s,
                        "{}:{}:{}:{}:{}:{}",
                        g.type_id,
                        g.per_cycle,
                        g.cycle_s,
                        g.secured_pct,
                        g.secured_cap,
                        g.unsecured_cap
                    );
                }
                None => s.push('-'),
            }
            s.push('\n');
        }
        let _ = write!(s, "ok:{ok:?}");
        s
    }
}

/// Split `ids` by whether `group_of` finds them in
/// [`GROUP_COLONY_REAGENTS`].
fn resolve_ids(
    ids: &BTreeSet<i64>,
    group_of: impl Fn(i64) -> Option<i64>,
) -> (BTreeSet<i64>, Vec<i64>) {
    let mut ok = BTreeSet::new();
    let mut bad = Vec::new();
    for t in ids {
        if group_of(*t) == Some(GROUP_COLONY_REAGENTS) {
            ok.insert(*t);
        } else {
            bad.push(*t);
        }
    }
    (ok, bad)
}

impl StarTable {
    pub fn new(rows: Vec<(i64, u64)>) -> StarTable {
        StarTable {
            rows: rows.into_iter().collect(),
        }
    }

    /// The power of a star of `type_id`: its own row, else the `0` row.
    pub fn power(&self, type_id: i64) -> u64 {
        self.rows
            .get(&type_id)
            .or_else(|| self.rows.get(&0))
            .copied()
            .unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn load_planets(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(PLANETS_FILE, PLANETS_SHIPPED, PLANETS_HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_planet_row);
    let (table, dups) = PlanetTable::new(rows);
    for d in dups {
        crate::outln!("[sov_resources] {PLANETS_FILE}: {d}");
    }
    Ok(Box::new(table))
}

fn load_stars(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(STARS_FILE, STARS_SHIPPED, STARS_HEADER)?;
    let rows = file.parse_rows(|r| {
        let p = u64::try_from(r.i64(1)?).map_err(|_| "power is negative".to_owned())?;
        Ok((r.i64(0)?, p))
    });
    Ok(Box::new(StarTable::new(rows)))
}

/// An invented reagent type id, never a real one.
pub const FIXTURE_REAGENT_TYPE: i64 = 990_000_777;

/// Invented rows: type ids 9_001..9_003 and star type 9_000, never the real
/// ones. Gas-like (power), land-like (workforce), ice-like (reagent).
pub fn fixture_planets() -> PlanetTable {
    PlanetTable::new(vec![
        (
            9_001,
            PlanetRow {
                power: 30,
                workforce: 0,
                reagent: None,
            },
        ),
        (
            9_002,
            PlanetRow {
                power: 0,
                workforce: 20,
                reagent: None,
            },
        ),
        (
            9_003,
            PlanetRow {
                power: 0,
                workforce: 0,
                reagent: Some(ReagentRow {
                    type_id: FIXTURE_REAGENT_TYPE,
                    per_cycle: 7,
                    cycle_s: 600,
                    secured_pct: 40,
                    secured_cap: 70,
                    unsecured_cap: 90,
                }),
            },
        ),
    ])
    .0
}

pub fn fixture_stars() -> StarTable {
    StarTable::new(vec![(9_000, 5)])
}

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const PLANETS: TableSpec = TableSpec {
    name: "sov planet resources",
    load: load_planets,
    fixture: || Box::new(fixture_planets()) as Boxed,
};

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const STARS: TableSpec = TableSpec {
    name: "sov star power",
    load: load_stars,
    fixture: || Box::new(fixture_stars()) as Boxed,
};

// ---------------------------------------------------------------------------
// Yields
// ---------------------------------------------------------------------------

/// A resolved reagent definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reagent {
    pub type_id: i64,
    pub per_cycle: u32,
    pub cycle_s: u64,
    pub secured_pct: u32,
    pub secured_cap: u32,
    pub unsecured_cap: u32,
}

/// What one planet gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanetYield {
    pub planet: i64,
    pub power: u64,
    pub workforce: u64,
    pub reagent: Option<Reagent>,
}

static NO_PLANETS: PlanetTable = PlanetTable {
    rows: BTreeMap::new(),
    resolved: OnceLock::new(),
};

/// The planet table in `content`; empty when none is registered.
fn planet_table(content: &Content) -> &PlanetTable {
    content.try_get::<PlanetTable>().unwrap_or(&NO_PLANETS)
}

/// The version `content` serves: derived from its planet table.
pub fn current_version(content: &Content) -> (u32, u32, u32) {
    planet_table(content).resolved(content).version
}

fn yield_of(row: &PlanetRow, planet: i64, res: &Resolved) -> PlanetYield {
    PlanetYield {
        planet,
        power: row.power,
        workforce: row.workforce,
        reagent: row
            .reagent
            .as_ref()
            .filter(|g| res.ok.contains(&g.type_id))
            .map(|g| Reagent {
                type_id: g.type_id,
                per_cycle: g.per_cycle,
                cycle_s: g.cycle_s,
                secured_pct: g.secured_pct,
                secured_cap: g.secured_cap,
                unsecured_cap: g.unsecured_cap,
            }),
    }
}

fn planet_type(reg: &Registry, planet: i64) -> Option<i64> {
    reg.celestials()
        .get(planet)
        .filter(|c| c.kind == Kind::Planet)
        .map(|c| c.type_id)
}

/// What `planet` gives, or `None` for a body that is not a catalogue planet
/// or whose type has no row.
pub fn planet_yield(reg: &Registry, planet: i64) -> Option<PlanetYield> {
    let table = planet_table(reg.content());
    let row = table.get(planet_type(reg, planet)?)?;
    Some(yield_of(row, planet, table.resolved(reg.content())))
}

/// The yields of `system`'s planets, in id order; a planet whose type has
/// no row is left out.
pub fn planets_in(reg: &Registry, system: i64) -> Vec<PlanetYield> {
    let table = planet_table(reg.content());
    planets_with(reg, system, table, table.resolved(reg.content()))
}

fn planets_with(
    reg: &Registry,
    system: i64,
    table: &PlanetTable,
    res: &Resolved,
) -> Vec<PlanetYield> {
    reg.celestials()
        .served_in(system)
        .filter(|c| c.kind == Kind::Planet)
        .filter_map(|c| table.get(c.type_id).map(|row| yield_of(row, c.id, res)))
        .collect()
}

/// The power of `system`'s star; 0 when it has none or no table is loaded.
pub fn star_power(reg: &Registry, system: i64) -> u64 {
    let Some(t) = reg.content().try_get::<StarTable>() else {
        return 0;
    };
    reg.celestials()
        .star_of(system)
        .map_or(0, |s| t.power(s.type_id))
}

/// Sum of the planets' power and the star's power (RE §5.1).
pub fn max_power(reg: &Registry, system: i64) -> u64 {
    let table = planet_table(reg.content());
    let planets: u64 = reg
        .celestials()
        .served_in(system)
        .filter(|c| c.kind == Kind::Planet)
        .filter_map(|c| table.get(c.type_id))
        .map(|r| r.power)
        .sum();
    planets + star_power(reg, system)
}

/// Sum of the planets' workforce.
pub fn max_workforce(reg: &Registry, system: i64) -> u64 {
    let table = planet_table(reg.content());
    reg.celestials()
        .served_in(system)
        .filter(|c| c.kind == Kind::Planet)
        .filter_map(|c| table.get(c.type_id))
        .map(|r| r.workforce)
        .sum()
}

/// One of the two resources, as `GetResources` and its notice carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Triple {
    pub available: u64,
    pub allocated: u64,
    pub local_harvest: u64,
}

/// What `GetResources` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubResources {
    pub power: Triple,
    pub workforce: Triple,
}

/// `(power, workforce)` the hub's installed upgrades use: the sum over its
/// **Online** slots (the window shows it as display; spec L6.6). A slot whose
/// type has no row uses nothing. The workforce also includes what the
/// mercenary dens in the hub's system use (`merc_den::workforce_use`).
pub fn allocated(reg: &Registry, hub_id: i64) -> (u64, u64) {
    let t = crate::sov_upgrade::table(reg.content());
    let (p, w) = crate::sov_upgrade::installed(reg.deltas(), hub_id)
        .iter()
        .filter(|i| i.state == crate::sov_upgrade::Power::Online)
        .filter_map(|i| t.def(i.type_id))
        .fold((0, 0), |(p, w), d| (p + d.power, w + d.workforce));
    let dens = crate::sov_hub::by_id(reg.deltas(), hub_id).map_or(0, |h| {
        crate::merc_den::workforce_use(reg, h.system_id, crate::merc_den::now())
    });
    (p, w.saturating_add(dens))
}

/// `(power, workforce)` harvested in `system` (the owner's Q2 ruling): the
/// star's power plus the yields of the planets with an **active** skyhook of
/// a corporation in the holder alliance (`skyhook::counted`, SKY-1). A
/// reagent planet gives neither. The workforce is then moved between hubs by
/// the transfer configuration (SOV-WF): plus what the system's sources hand
/// it, minus what its connected export takes.
pub fn harvested(reg: &Registry, system: i64) -> (u64, u64) {
    let (p, w) = crate::skyhook::counted(reg, system);
    (
        star_power(reg, system) + p,
        crate::sov_workforce::net_workforce(reg, system, w),
    )
}

/// What `hub_id` reads: `None` for an id no hub has. `available` is the
/// budget before upgrades (spec §1.3 item 5), here the harvest.
pub fn hub_resources(reg: &Registry, hub_id: i64) -> Option<HubResources> {
    let hub = crate::sov_hub::by_id(reg.deltas(), hub_id)?;
    let (hp, hw) = harvested(reg, hub.system_id);
    let (ap, aw) = allocated(reg, hub_id);
    Some(HubResources {
        power: Triple {
            available: hp,
            allocated: ap,
            local_harvest: hp,
        },
        workforce: Triple {
            available: hw,
            allocated: aw,
            // The gross harvest, not the net after transfers (`data_types.py:13-24`).
            local_harvest: crate::skyhook::counted(reg, hub.system_id).1,
        },
    })
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

fn uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// A one-field identifier message: `{1: sequential}`.
fn ident(id: i64) -> Vec<u8> {
    let mut m = Vec::new();
    uint(&mut m, 1, id as u64);
    m
}

fn version_bytes(v: (u32, u32, u32)) -> Vec<u8> {
    let mut out = Vec::new();
    uint(&mut out, 1, u64::from(v.0));
    uint(&mut out, 2, u64::from(v.1));
    uint(&mut out, 3, u64::from(v.2));
    out
}

fn is_ours(spec: &[u8], ours: (u32, u32, u32)) -> bool {
    let f = |n| get_uint(spec, n).unwrap_or(0);
    (f(1), f(2), f(3)) == (u64::from(ours.0), u64::from(ours.1), u64::from(ours.2))
}

/// One `reagent.Definition` inside a planet definition. The reagent's item
/// type goes at field **3** (`reagent_type`), which the client reads; the
/// generic field 1 is left unset (RE §2.5, the field-3 trap).
fn reagent_bytes(g: &Reagent) -> Vec<u8> {
    let mut d = Vec::new();
    uint(&mut d, 1, u64::from(g.per_cycle));
    let mut dur = Vec::new();
    uint(&mut dur, 1, g.cycle_s);
    put_bytes(&mut d, 2, &dur);
    uint(&mut d, 3, u64::from(g.secured_pct));
    uint(&mut d, 4, u64::from(g.secured_cap));
    uint(&mut d, 5, u64::from(g.unsecured_cap));
    let mut r = Vec::new();
    put_bytes(&mut r, 2, &d);
    put_bytes(&mut r, 3, &ident(g.type_id));
    r
}

fn definition_bytes(y: &PlanetYield) -> Vec<u8> {
    let mut d = Vec::new();
    put_bytes(&mut d, 1, &ident(y.planet));
    if y.power > 0 {
        uint(&mut d, 2, y.power);
    }
    if y.workforce > 0 {
        uint(&mut d, 3, y.workforce);
    }
    if let Some(g) = &y.reagent {
        put_bytes(&mut d, 4, &reagent_bytes(g));
    }
    d
}

/// Every planet of every claimable system that has a yield (a planet with no
/// yield has no definition: the client reads that as "gives nothing"). `[A]`
/// claimable systems only (falsifier: a planet info panel outside sov null
/// that wants a yield).
fn all_definitions(reg: &Registry) -> Vec<u8> {
    let table = planet_table(reg.content());
    let res = table.resolved(reg.content());
    let mut out = Vec::new();
    if table.is_empty() {
        return out;
    }
    let Some(u) = reg.content().try_get::<crate::celestial::Universe>() else {
        return out;
    };
    for s in u.systems().filter(|s| crate::sov::claimable(u, s.id)) {
        for y in planets_with(reg, s.id, table, res) {
            put_bytes(&mut out, 1, &definition_bytes(&y));
        }
    }
    out
}

fn reply(status: u32, payload: Option<(String, Vec<u8>)>) -> Reply {
    Reply { status, payload }
}

fn typed(ty: &str, body: Vec<u8>) -> Reply {
    let base = ty.strip_suffix("Request").unwrap_or(ty);
    reply(200, Some((format!("{base}Response"), body)))
}

fn triple_bytes(t: &Triple) -> Vec<u8> {
    let mut m = Vec::new();
    uint(&mut m, 1, t.available);
    uint(&mut m, 2, t.allocated);
    uint(&mut m, 3, t.local_harvest);
    m
}

fn resources_bytes(r: &HubResources) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &triple_bytes(&r.power));
    put_bytes(&mut out, 2, &triple_bytes(&r.workforce));
    out
}

/// `GetResources` for `character`: 404 for a hub that does not exist, 403
/// for a pilot whose alliance does not hold the system. The 403 for a
/// non-holder is our policy, not TQ's; the client's 403 path is `[V]`
/// (`sovereignty/resource/client/messenger.py:93`).
fn resources_reply(reg: &Registry, hub_id: i64, character: i64) -> Reply {
    let Some(res) = hub_resources(reg, hub_id) else {
        return reply(404, None);
    };
    let holder = crate::sov_hub::by_id(reg.deltas(), hub_id)
        .and_then(|h| crate::sov::holder(reg.deltas(), h.system_id));
    let mine = reg.character(character).and_then(|c| c.alliance_id);
    if holder.is_none() || holder != mine {
        return reply(403, None);
    }
    typed(
        &format!("{HUB_PKG}GetResourcesRequest"),
        resources_bytes(&res),
    )
}

/// The gateway arm (`gateway_ext::CHAIN`): the planet definitions, the star
/// configurations and `hub.api.GetResources`. `None` for everything else.
/// On a [`Ground::Local`] there is no world: 404.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    ty.strip_prefix(PKG)?;
    let kind = if let Some(r) = ty.strip_prefix(PLANET_PKG) {
        match r {
            "GetAllDefinitionsRequest" | "GetDefinitionsVersionRequest" => r,
            _ => return None,
        }
    } else if let Some(r) = ty.strip_prefix(STAR_PKG) {
        if r != "GetAllConfigurationsRequest" {
            return None;
        }
        "stars"
    } else if ty == format!("{HUB_PKG}GetResourcesRequest") {
        "hub"
    } else {
        return None;
    };
    // Every answer needs the world: the version is derived from the loaded
    // table, so even the 304 reads it.
    let Ground::Server(w) = g else {
        return Some(reply(404, None));
    };
    let reg = crate::service::world_read_timed(w, "sov_resources_gateway");
    let ours = current_version(reg.content());
    // Field 1 of both definition requests is the version the client holds.
    let held = get_bytes(body, 1);
    Some(match kind {
        "GetAllDefinitionsRequest" => {
            // Field 1 `known_version`; field 2 `no_known_version`.
            if held.is_some_and(|v| is_ours(&v, ours)) {
                return Some(reply(304, None));
            }
            let mut out = all_definitions(&reg);
            put_bytes(&mut out, 2, &version_bytes(ours));
            typed(ty, out)
        }
        "GetDefinitionsVersionRequest" => {
            if !held.is_some_and(|v| is_ours(&v, ours)) {
                return Some(reply(404, None));
            }
            typed(ty, all_definitions(&reg))
        }
        "stars" => {
            let mut out = Vec::new();
            if let (Some(t), Some(u)) = (
                reg.content().try_get::<StarTable>(),
                reg.content().try_get::<crate::celestial::Universe>(),
            ) {
                for s in u.systems().filter(|s| crate::sov::claimable(u, s.id)) {
                    let Some(star) = reg.celestials().star_of(s.id) else {
                        continue;
                    };
                    let mut c = Vec::new();
                    put_bytes(&mut c, 1, &ident(star.id));
                    uint(&mut c, 2, t.power(star.type_id));
                    put_bytes(&mut out, 1, &c);
                }
            }
            typed(ty, out)
        }
        _ => {
            let hub = get_bytes(body, 1)
                .and_then(|h| get_uint(&h, 1))
                .filter(|h| *h > 0);
            match hub {
                Some(h) => resources_reply(&reg, h as i64, character),
                None => reply(404, None),
            }
        }
    })
}

/// Queue `ResourcesSimulatedNotice` for the pilots in `hub_id`'s system: the
/// window re-reads when the budget changes. Under the world lock; sends
/// nothing (`gateway_out`).
pub fn announce(reg: &mut Registry, hub_id: i64) {
    let Some(hub) = crate::sov_hub::by_id(reg.deltas(), hub_id) else {
        return;
    };
    let Some(res) = hub_resources(reg, hub_id) else {
        return;
    };
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &ident(hub_id));
    put_bytes(&mut p, 2, &ident(hub.system_id));
    put_bytes(&mut p, 3, &triple_bytes(&res.power));
    put_bytes(&mut p, 4, &triple_bytes(&res.workforce));
    let members = crate::service::structures::system_audience(reg, hub.system_id);
    crate::gateway_out::queue(
        reg,
        NOTICE_SIMULATED,
        p,
        Target::SolarSystem {
            id: hub.system_id,
            members,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::field;
    use std::sync::{Arc, RwLock};

    // Invented ids. The system is in the fixture universe as a claimable
    // null-sec system (`Universe::fixture`).
    const SYSTEM: i64 = 39_000_003;
    const PLANET_POWER: i64 = 40_900_001;
    const PLANET_LAND: i64 = 40_900_002;
    const PLANET_REAGENT: i64 = 40_900_003;
    const STAR: i64 = 40_900_000;
    const HUB: i64 = 40_900_100;
    const ALLIANCE: i64 = 91_000_001;
    const OTHER: i64 = 91_000_002;
    const HOLDER: i64 = 1_001;
    const OUTSIDER: i64 = 1_002;
    const REAGENT_TYPE: i64 = FIXTURE_REAGENT_TYPE;
    /// Invented: a type that exists but is not a colony reagent.
    const WRONG_GROUP_TYPE: i64 = 990_000_778;
    /// Invented: a type the typedata does not hold.
    const MISSING_TYPE: i64 = 990_000_779;

    fn catalogue() -> crate::celestial::Catalogue {
        crate::celestial::Catalogue::parse(&format!(
            "star\t{STAR}\t9000\t1000\t0\t0\t0\t{SYSTEM}\t0\t0\n\
             planet\t{PLANET_POWER}\t9001\t100\t1\t0\t0\t{SYSTEM}\t0\t0\n\
             planet\t{PLANET_LAND}\t9002\t100\t2\t0\t0\t{SYSTEM}\t0\t0\n\
             planet\t{PLANET_REAGENT}\t9003\t100\t3\t0\t0\t{SYSTEM}\t0\t0\n"
        ))
    }

    /// A content with the invented reagent type in the colony-reagent group.
    fn content() -> Content {
        let mut c = Content::fixture();
        c.insert("sov planets", fixture_planets());
        c.insert("sov stars", fixture_stars());
        let text = format!(
            "{}type\t{REAGENT_TYPE}\t{GROUP_COLONY_REAGENTS}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Reagent\n\
             type\t{WRONG_GROUP_TYPE}\t{}\t43\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Other\n",
            crate::typedata::FIXTURE,
            GROUP_COLONY_REAGENTS + 1
        );
        let table = crate::typedata::Table::parse(&text).unwrap();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn reg() -> Registry {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            celestials: catalogue(),
            characters: vec![
                crate::model::Character {
                    character_id: HOLDER,
                    ship_id: 770_001,
                    alliance_id: Some(ALLIANCE),
                    ..Default::default()
                },
                crate::model::Character {
                    character_id: OUTSIDER,
                    ship_id: 770_002,
                    alliance_id: Some(OTHER),
                    ..Default::default()
                },
            ],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(Arc::new(content()));
        let hub = crate::sov_hub::Hub {
            system_id: SYSTEM,
            hub_id: HUB,
            position: [0.0; 3],
            launcher_id: None,
            launch_time: 0,
            offline_days: 0,
        };
        crate::sov_hub::put(reg.deltas_mut(), &hub);
        crate::sov::set_claim(reg.deltas_mut(), SYSTEM, Some((ALLIANCE, 98_000_001)), 1);
        reg
    }

    fn ground(reg: Registry) -> Ground {
        Ground::Server(Arc::new(RwLock::new(reg)))
    }

    fn req(pkg: &str, name: &str) -> String {
        format!("{pkg}{name}")
    }

    fn version_msg(a: u64, b: u64, c: u64) -> Vec<u8> {
        let mut v = Vec::new();
        uint(&mut v, 1, a);
        uint(&mut v, 2, b);
        uint(&mut v, 3, c);
        v
    }

    fn body_of(r: &Reply) -> &[u8] {
        &r.payload.as_ref().expect("a payload").1
    }

    #[test]
    fn max_power_is_the_planets_plus_the_star_and_workforce_the_planets() {
        let reg = reg();
        // Read the invented rows back rather than restating the numbers.
        let p = fixture_planets();
        let s = fixture_stars();
        let want_p = p.get(9_001).unwrap().power + s.power(9_000);
        assert!(want_p > 0);
        assert_eq!(max_power(&reg, SYSTEM), want_p);
        assert_eq!(max_workforce(&reg, SYSTEM), p.get(9_002).unwrap().workforce);
        // A system without bodies has nothing.
        assert_eq!(max_power(&reg, 39_000_001), 0);
    }

    #[test]
    fn each_planet_gives_one_kind_and_the_reagent_resolves_to_its_type() {
        let reg = reg();
        let ys = planets_in(&reg, SYSTEM);
        assert_eq!(ys.len(), 3);
        for y in &ys {
            let kinds = usize::from(y.power > 0)
                + usize::from(y.workforce > 0)
                + usize::from(y.reagent.is_some());
            assert_eq!(kinds, 1, "{y:?}");
        }
        // The fixture reagent names the invented type, which is in the group.
        let g = planet_yield(&reg, PLANET_REAGENT).unwrap().reagent.unwrap();
        assert_eq!(g.type_id, REAGENT_TYPE);
        assert!(planet_yield(&reg, STAR).is_none(), "a star is not a planet");
    }

    #[test]
    fn a_reagent_key_with_no_type_leaves_the_planet_without_a_reagent() {
        let mut reg = reg();
        // The plain fixture typedata has no colony-reagent type.
        let mut plain = Content::fixture();
        plain.insert("sov planets", fixture_planets());
        reg = reg.with_content(Arc::new(plain));
        assert!(
            planet_yield(&reg, PLANET_REAGENT)
                .unwrap()
                .reagent
                .is_none()
        );
    }

    fn reagent_row(type_id: i64) -> PlanetRow {
        PlanetRow {
            power: 0,
            workforce: 0,
            reagent: Some(ReagentRow {
                type_id,
                per_cycle: 3,
                cycle_s: 60,
                secured_pct: 10,
                secured_cap: 20,
                unsecured_cap: 30,
            }),
        }
    }

    #[test]
    fn a_reagent_id_outside_the_group_or_the_typedata_is_reported_and_gives_nothing() {
        let (table, _) = PlanetTable::new(vec![
            (9_003, reagent_row(REAGENT_TYPE)),
            (9_004, reagent_row(WRONG_GROUP_TYPE)),
            (9_005, reagent_row(MISSING_TYPE)),
        ]);
        let c = content();
        let res = table.resolved(&c);
        assert_eq!(res.ok.iter().copied().collect::<Vec<_>>(), [REAGENT_TYPE]);
        assert_eq!(res.unresolved, [WRONG_GROUP_TYPE, MISSING_TYPE]);
        let ok = yield_of(table.get(9_003).unwrap(), 1, res);
        assert_eq!(ok.reagent.map(|g| g.type_id), Some(REAGENT_TYPE));
        for t in [9_004, 9_005] {
            assert!(yield_of(table.get(t).unwrap(), 1, res).reagent.is_none());
        }
    }

    #[test]
    fn the_version_follows_the_table_and_the_resolved_ids() {
        let c = content();
        let base = fixture_planets();
        let v0 = base.resolved(&c).version;
        assert_eq!((v0.0, v0.1), RESOURCES_VERSION_BASE);
        // The same rows give the same version.
        assert_eq!(fixture_planets().resolved(&c).version, v0);
        // A changed number gives another.
        let mut rows: Vec<(i64, PlanetRow)> = vec![
            (9_001, base.get(9_001).unwrap().clone()),
            (9_002, base.get(9_002).unwrap().clone()),
            (9_003, base.get(9_003).unwrap().clone()),
        ];
        rows[0].1.power += 1;
        let v1 = PlanetTable::new(rows).0.resolved(&c).version;
        assert_ne!(v1, v0);
        // The same rows against a typedata that lacks the reagent give another.
        let plain = Content::fixture();
        assert_ne!(fixture_planets().resolved(&plain).version, v0);
    }

    #[test]
    fn a_client_holding_the_old_version_is_answered_in_full_after_an_edit() {
        let c = Arc::new(content());
        let old = fixture_planets().resolved(&c).version;
        let mut edited = reg();
        let mut rows = vec![(
            9_001,
            PlanetRow {
                power: 31,
                workforce: 0,
                reagent: None,
            },
        )];
        let p = fixture_planets();
        rows.push((9_002, p.get(9_002).unwrap().clone()));
        rows.push((9_003, p.get(9_003).unwrap().clone()));
        let mut c2 = content();
        c2.insert("sov planets", PlanetTable::new(rows).0);
        edited = edited.with_content(Arc::new(c2));
        let g = ground(edited);
        let mut held = Vec::new();
        put_bytes(
            &mut held,
            1,
            &version_msg(u64::from(old.0), u64::from(old.1), u64::from(old.2)),
        );
        let r = gateway(
            &req(PLANET_PKG, "GetAllDefinitionsRequest"),
            &held,
            HOLDER,
            &g,
        )
        .unwrap();
        assert_eq!(r.status, 200, "the old version is not the edited one");
    }

    #[test]
    fn the_loader_drops_a_zero_cycle_and_a_row_with_two_kinds() {
        let text = format!(
            "{PLANETS_HEADER}\n\
             1\t10\t0\t-\t0\t0\t0\t0\t0\tok power\n\
             2\t0\t0\t{REAGENT_TYPE}\t5\t0\t50\t10\t10\tzero cycle\n\
             3\t10\t10\t-\t0\t0\t0\t0\t0\ttwo kinds\n\
             4\t0\t0\t{REAGENT_TYPE}\t5\t60\t101\t10\t10\tsecured over 100\n\
             5\t0\t0\t{REAGENT_TYPE}\t5\t60\t50\t10\t10\tok reagent\n\
             6\t0\t0\tice\t5\t60\t50\t10\t10\ta key, not an id\n"
        );
        let file = crate::authored::Dir::compiled_only()
            .load("x.tsv", &text, PLANETS_HEADER)
            .unwrap();
        let (rows, skipped) = file.parse_rows_logged(parse_planet_row);
        let kept: Vec<i64> = rows.iter().map(|(t, _)| *t).collect();
        assert_eq!(kept, [1, 5]);
        assert_eq!(skipped.len(), 4, "{skipped:?}");
    }

    #[test]
    fn the_shipped_files_are_well_formed() {
        let dir = crate::authored::Dir::compiled_only();
        let f = dir
            .load(PLANETS_FILE, PLANETS_SHIPPED, PLANETS_HEADER)
            .unwrap();
        let (rows, skipped) = f.parse_rows_logged(parse_planet_row);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(!rows.is_empty());
        let f = dir.load(STARS_FILE, STARS_SHIPPED, STARS_HEADER).unwrap();
        assert!(f.parse_rows(|r| r.i64(1)).len() == f.rows.len());
    }

    #[test]
    fn the_known_version_gets_304_and_an_unknown_one_the_definitions() {
        let reg = reg();
        let (a, b, c) = current_version(reg.content());
        let g = ground(reg);
        let ty = req(PLANET_PKG, "GetAllDefinitionsRequest");
        let mut known = Vec::new();
        put_bytes(
            &mut known,
            1,
            &version_msg(u64::from(a), u64::from(b), u64::from(c)),
        );
        let r = gateway(&ty, &known, HOLDER, &g).unwrap();
        assert_eq!((r.status, r.payload), (304, None));

        let mut none = Vec::new();
        uint(&mut none, 2, 1);
        let r = gateway(&ty, &none, HOLDER, &g).unwrap();
        assert_eq!(r.status, 200);
        let body = body_of(&r);
        let defs = field(body, 1);
        assert_eq!(
            defs.len(),
            3,
            "the three planets of the one claimable system"
        );
        assert_eq!(
            get_bytes(body, 2).map(|v| get_uint(&v, 1)),
            Some(Some(u64::from(a)))
        );

        // An older version is also answered in full.
        let mut old = Vec::new();
        put_bytes(&mut old, 1, &version_msg(0, 0, 1));
        assert_eq!(gateway(&ty, &old, HOLDER, &g).unwrap().status, 200);
    }

    #[test]
    fn a_reagent_definition_carries_its_type_at_field_3_and_not_field_1() {
        let g = ground(reg());
        let ty = req(PLANET_PKG, "GetAllDefinitionsRequest");
        let r = gateway(&ty, &[], HOLDER, &g).unwrap();
        let defs = field(body_of(&r), 1);
        let mut seen = 0;
        for d in &defs {
            for rd in field(d, 4) {
                seen += 1;
                let t = get_bytes(&rd, 3).and_then(|m| get_uint(&m, 1));
                assert_eq!(t, Some(REAGENT_TYPE as u64));
                assert!(get_bytes(&rd, 1).is_none(), "field 1 stays unset");
                let def = get_bytes(&rd, 2).unwrap();
                let secs = get_bytes(&def, 2).and_then(|m| get_uint(&m, 1));
                assert!(secs.is_some_and(|s| s > 0), "cycle period above 0");
            }
        }
        assert_eq!(seen, 1);
    }

    #[test]
    fn definitions_by_version_answer_ours_and_404_another() {
        let reg = reg();
        let (a, b, c) = current_version(reg.content());
        let g = ground(reg);
        let ty = req(PLANET_PKG, "GetDefinitionsVersionRequest");
        let mut ours = Vec::new();
        put_bytes(
            &mut ours,
            1,
            &version_msg(u64::from(a), u64::from(b), u64::from(c)),
        );
        let r = gateway(&ty, &ours, HOLDER, &g).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(field(body_of(&r), 1).len(), 3);
        let mut other = Vec::new();
        put_bytes(&mut other, 1, &version_msg(u64::from(a) + 1, 0, 0));
        assert_eq!(gateway(&ty, &other, HOLDER, &g).unwrap().status, 404);
    }

    #[test]
    fn the_star_configurations_name_each_star_and_its_power() {
        let g = ground(reg());
        let ty = req(STAR_PKG, "GetAllConfigurationsRequest");
        let r = gateway(&ty, &[], HOLDER, &g).unwrap();
        assert_eq!(r.status, 200);
        let cfgs = field(body_of(&r), 1);
        assert_eq!(cfgs.len(), 1);
        let star = get_bytes(&cfgs[0], 1).and_then(|m| get_uint(&m, 1));
        assert_eq!(star, Some(STAR as u64));
        assert_eq!(get_uint(&cfgs[0], 2), Some(fixture_stars().power(9_000)));
    }

    #[test]
    fn get_resources_404_for_an_unknown_hub_403_for_a_stranger_and_the_budget_for_the_holder() {
        let g = ground(reg());
        let ty = req(HUB_PKG, "GetResourcesRequest");
        let ask = |hub: i64, who: i64| {
            let mut b = Vec::new();
            put_bytes(&mut b, 1, &ident(hub));
            gateway(&ty, &b, who, &g).unwrap()
        };
        assert_eq!(ask(HUB + 1, HOLDER).status, 404);
        // The 403 for a non-holder is ours, not TQ's; the client's 403 path
        // is [V] (messenger.py:93).
        assert_eq!(ask(HUB, OUTSIDER).status, 403);
        assert_eq!(ask(HUB, 999).status, 403, "a pilot the world does not know");
        let r = ask(HUB, HOLDER);
        assert_eq!(r.status, 200);
        let body = body_of(&r);
        let p = get_bytes(body, 1).unwrap();
        let w = get_bytes(body, 2).unwrap();
        // available is what is harvested (SKY-1): the star alone while no
        // skyhook is active, and allocated is 0.
        assert_eq!(get_uint(&p, 1), Some(fixture_stars().power(9_000)));
        assert_eq!(get_uint(&p, 2), Some(0));
        assert_eq!(get_uint(&p, 3), Some(fixture_stars().power(9_000)));
        assert_eq!(get_uint(&w, 1), Some(0));
    }

    #[test]
    fn a_neutral_hub_opens_for_nobody() {
        let mut reg = reg();
        crate::sov::set_claim(reg.deltas_mut(), SYSTEM, None, 2);
        let g = ground(reg);
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &ident(HUB));
        let r = gateway(&req(HUB_PKG, "GetResourcesRequest"), &b, HOLDER, &g).unwrap();
        assert_eq!(r.status, 403);
    }

    #[test]
    fn other_requests_pass_and_a_local_ground_has_no_world() {
        let local = Ground::local();
        assert!(gateway("eve_public.invented.thing.GetRequest", &[], 1, &local).is_none());
        assert!(
            gateway(
                &req(PLANET_PKG, "GetAllConfigurationsRequest"),
                &[],
                1,
                &local
            )
            .is_none()
        );
        let r = gateway(
            &req(STAR_PKG, "GetAllConfigurationsRequest"),
            &[],
            1,
            &local,
        )
        .unwrap();
        assert_eq!(r.status, 404);
    }

    #[test]
    fn the_notice_names_hub_system_and_both_triples() {
        let mut reg = reg();
        announce(&mut reg, HUB);
        let q = crate::gateway_out::take(&mut reg);
        assert_eq!(q.len(), 1);
        let (ty, payload, _) = &q[0];
        assert_eq!(ty, NOTICE_SIMULATED);
        assert_eq!(
            get_bytes(payload, 1).and_then(|m| get_uint(&m, 1)),
            Some(HUB as u64)
        );
        assert_eq!(
            get_bytes(payload, 2).and_then(|m| get_uint(&m, 1)),
            Some(SYSTEM as u64)
        );
        let p = get_bytes(payload, 3).unwrap();
        assert_eq!(get_uint(&p, 1), Some(fixture_stars().power(9_000)));
        assert!(get_bytes(payload, 4).is_some());
        // An unknown hub queues nothing.
        announce(&mut reg, HUB + 5);
        assert!(crate::gateway_out::take(&mut reg).is_empty());
    }
}
