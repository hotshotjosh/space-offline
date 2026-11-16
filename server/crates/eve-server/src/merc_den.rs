//! Mercenary dens (MERC-1, issue #83): deploy over a planet's skyhook, own,
//! see, evolve and make infomorphs. Wire contract: `docs/re/sov-gateway.md`
//! §2.7, §3.2, §3.3, §7; the client's own rules are cited at each function.
//!
//! **What a den is.** An item of group [`GROUP_MERCENARY_DEN`] **[V]**
//! (`inventorycommon/const.py:1111`) in space at its solar system
//! ([`FLAG_IN_SPACE`]), owned by the launching **character**
//! (`OfferAccessMercenaryDen` needs `IsOwnedByMe`, RE §3.4), plus one row of
//! kind [`KIND`] (`delta_kinds::KINDS`):
//!
//! ```text
//! mercden <denID> -> owner skyhookID planetID systemID typeID enabled paused
//!     dev_base dev_at anarchy_base anarchy_at infomorphs last_gen_tick seen
//! ```
//!
//! The spec's row has thirteen values; `seen` trails them (as the skyhook's
//! `state` does) so a reader of the thirteen still parses: it is the pair of
//! stages the owner was last told about, `development * 10 + anarchy`, `0`
//! before the first look. Every time is **unix seconds on the wall clock**:
//! the client extrapolates a den's levels from the record with
//! `GetSecondsSinceWallclockTime` (`repository.py:301-306`), so every rule
//! here is a pure function of the stored row and `now`.
//!
//! **Deploy.** `ship.Drop` ([`drop_call`], "Launch for Self",
//! `menusvc.py:349-353`) for an item of the group. The checks are the
//! client's own refusal reasons **[V]** (`sovereignty/mercenaryden/common/
//! errors.py:20-27`, names in `findings/user-errors.tsv`): the pilot's
//! den-management skill allows another den (`SkillLimits`,
//! `MaximumDensReached`); a skyhook within [`LAUNCH_RANGE`] of the pilot
//! (`RequiresNearbySkyhook`), switched on and held (`RequiresOnlineSkyhook`),
//! with no den yet (`SkyhookAlreadyOccupied`), over a planet that gives
//! workforce (`MustBeTemperatePlanet`; the temperate type is a workforce
//! planet in `content/sov/planet_resources.tsv` **[I]**). Whose skyhook it is
//! does not matter **[I]** (the shapes carry `skyhook_owner` apart from the
//! den's owner). The refusal goes back in the `Drop` answer under the
//! client's own label, which `LaunchFromShip` raises as a `UserError`
//! (`eveMisc.py:46-56`); a success pushes
//! `OnMercenaryDenRegistrationSuccess` (`repository.py:98-106`) **[V]**.
//! There is no `...Invalid` push: the server checks **before** it launches, so
//! the dialog comes once, from the answer (a push would repeat it).
//!
//! **Evolution.** [`level`] is the client's formula. **Infomorphs**:
//! [`generate`]. **Extraction**: [`extract_at`] (`CallComponentFromClient`).
//! **Removal**: [`tick_at`] notices an item that is gone.
//!
//! **A ball near its skyhook**, global in its system like the skyhook **[A]**
//! (falsifier: the client never drawing it off-grid).

use crate::delta::Deltas;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::gateway::{Ground, Reply, Target, put_bytes};
use crate::item::{Edit, Slot};
use crate::notify::Push;
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::skyhook;
use crate::sov_proto::{id, put_bool, put_uint, read_id, status, timestamp, typed};
use crate::space_component::CompCall;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

pub mod activity;
mod slash_cmd;
#[cfg(test)]
mod tests;

pub use slash_cmd::slash;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "mercden";

/// `invConst.groupMercenaryDen` **[V]** (`inventorycommon/const.py:1111`).
pub const GROUP_MERCENARY_DEN: i64 = 4810;

/// `invConst.typeMercenaryDenManagementSkill` **[V]**
/// (`inventorycommon/const.py:1872`).
pub const SKILL_DEN_MANAGEMENT: i64 = 85_233;

/// The `componentConst.MERCENARY_DEN` name **[V]** (`componentConst.py:91`).
pub const MERCENARY_DEN: &str = "mercenaryDen";

/// Where an item in space sits.
pub const FLAG_IN_SPACE: i64 = crate::combat::FLAG_IN_SPACE;

/// How far from a skyhook's spot a den may be launched. **[A]** 100 km (spec
/// L10.2), the skyhook's own launch range; falsified by a client-side refusal
/// naming another distance.
pub const LAUNCH_RANGE: f64 = skyhook::LAUNCH_RANGE;

/// How far from the den's surface a pilot may take infomorphs:
/// `maxCargoContainerTransferDistance` **[V]** (`checkers.py:7-14`).
pub const TRANSFER_RANGE: f64 = crate::sov_hub::TRANSFER_RANGE;

/// How far the den stands from its skyhook's spot, along the planet's +z
/// **[A]** (falsifier: the two overlapping on a planet the client draws).
pub const DEN_OFFSET: f64 = 4_000.0;

/// The ball's radius when the type table has none. **[A]**
pub const DEFAULT_RADIUS: f64 = 1_000.0;

/// The skill's top level; the absolute maximum of dens is this times the
/// table's dens per level.
pub const SKILL_TOP: u32 = 5;

/// Generation ticks one settle works through at most; the rest of a very long
/// idle spell is skipped (the count could only be at its cap by then
/// anyway, unless the band's lower bound is 0). **[A]**
pub const MAX_TICKS_PER_SETTLE: u64 = 100_000;

/// The client's own message key for a launch that worked **[V]**
/// (`findings/user-errors.tsv`, `MercenaryDen_DeploymentSuccessful`).
pub const MSG_SUCCESS: &str = "MercenaryDen_DeploymentSuccessful";

/// `OnMyMercenaryDenRemoved`'s first argument. The client reads only
/// `notifyArgs` (`repository.py:93-96`) **[V]**, so the key is a label.
pub const MSG_REMOVED: &str = "MercenaryDen_Removed";

const API_PKG: &str = "eve_public.sovereignty.mercenaryden.api.";

/// The den table, relative to `--authored`.
pub const FILE: &str = "sov/merc_dens.tsv";
/// Its header.
pub const HEADER: &str = "den_type_id\tdens_per_skill_level\tdev_unit_s\tanarchy_unit_s\tdev_bands\tanarchy_bands\tanarchy_workforce\tgen_tick_s\tgen_bands\tcargo_cap\tinfomorph_type\twhy";
const SHIPPED: &str = include_str!("../../../../content/sov/merc_dens.tsv");

/// Wall-clock unix seconds now.
pub fn now() -> u64 {
    crate::sov_proto::unix_now()
}

// ---------------------------------------------------------------------------
// The table (authored; content/sov/merc_dens.tsv)
// ---------------------------------------------------------------------------

/// What one den type evolves by and makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenDef {
    pub dens_per_level: u32,
    pub dev_unit_s: u64,
    pub anarchy_unit_s: u64,
    /// `(lower, upper)` of stages 1..=5.
    pub dev_bands: [(u32, u32); 5],
    pub anarchy_bands: [(u32, u32); 5],
    /// Workforce consumed in anarchy stages 1..=5.
    pub anarchy_workforce: [u64; 5],
    pub gen_tick_s: u64,
    /// Infomorphs per tick in development stages 1..=5, `(lower, upper)`.
    pub gen_bands: [(u32, u32); 5],
    pub cargo_cap: u32,
    pub infomorph_type: i64,
}

/// The table: den type -> definition; type `0` is every type without a row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenTable {
    rows: BTreeMap<i64, DenDef>,
}

impl DenTable {
    pub fn new(rows: Vec<(i64, DenDef)>) -> DenTable {
        DenTable {
            rows: rows.into_iter().collect(),
        }
    }

    /// The definition for den type `type_id`: its own row, else the `0` row.
    pub fn get(&self, type_id: i64) -> Option<&DenDef> {
        self.rows.get(&type_id).or_else(|| self.rows.get(&0))
    }

    /// The definition the pilot-wide maximum is read from: the `0` row, else
    /// the first.
    pub fn default_row(&self) -> Option<&DenDef> {
        self.rows.get(&0).or_else(|| self.rows.values().next())
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn bands(r: &crate::authored::Row, i: usize) -> Result<[(u32, u32); 5], String> {
    let v = r.list_i64(i)?;
    if v.len() != 10 {
        return Err(format!(
            "column {} needs 10 numbers (lower,upper for 5 stages), has {}",
            i + 1,
            v.len()
        ));
    }
    let mut out = [(0u32, 0u32); 5];
    for (s, pair) in v.chunks(2).enumerate() {
        let lo = u32::try_from(pair[0]).map_err(|_| format!("column {} is negative", i + 1))?;
        let hi = u32::try_from(pair[1]).map_err(|_| format!("column {} is negative", i + 1))?;
        if lo > hi {
            return Err(format!(
                "column {} stage {}: lower above upper",
                i + 1,
                s + 1
            ));
        }
        out[s] = (lo, hi);
    }
    Ok(out)
}

/// One row of the table. A row needs a generation tick and a cargo cap above
/// 0 and a positive infomorph type.
pub fn parse_def_row(r: &crate::authored::Row) -> Result<(i64, DenDef), String> {
    let num = |i: usize| -> Result<u64, String> {
        u64::try_from(r.i64(i)?).map_err(|_| format!("column {} is negative", i + 1))
    };
    let small = |i: usize| -> Result<u32, String> {
        u32::try_from(num(i)?).map_err(|_| format!("column {} is too large", i + 1))
    };
    let wf = r.list_i64(6)?;
    if wf.len() != 5 || wf.iter().any(|w| *w < 0) {
        return Err("anarchy_workforce needs 5 non-negative numbers".into());
    }
    let mut anarchy_workforce = [0u64; 5];
    for (slot, w) in anarchy_workforce.iter_mut().zip(&wf) {
        *slot = *w as u64;
    }
    let def = DenDef {
        dens_per_level: small(1)?,
        dev_unit_s: num(2)?,
        anarchy_unit_s: num(3)?,
        dev_bands: bands(r, 4)?,
        anarchy_bands: bands(r, 5)?,
        anarchy_workforce,
        gen_tick_s: num(7)?,
        gen_bands: bands(r, 8)?,
        cargo_cap: small(9)?,
        infomorph_type: r.i64(10)?,
    };
    if def.gen_tick_s == 0 {
        return Err("gen_tick_s must be above 0".into());
    }
    if def.cargo_cap == 0 {
        return Err("cargo_cap must be above 0".into());
    }
    if def.infomorph_type <= 0 {
        return Err("infomorph_type is not a positive type id".into());
    }
    Ok((r.i64(0)?, def))
}

fn load_table(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = file.parse_rows_logged(parse_def_row);
    Ok(Box::new(DenTable::new(rows)))
}

/// Invented rows for tests, never the shipped numbers: a den type `0` that
/// rises a level every 100 s (anarchy: 200 s), stages of ten levels,
/// generation every 60 s of 1 to 3 infomorphs (stage 1) up to 5 to 9
/// (stage 5), capped at 40 of the invented type [`FIXTURE_INFOMORPH`].
pub fn fixture_table() -> DenTable {
    DenTable::new(vec![(
        0,
        DenDef {
            dens_per_level: 2,
            dev_unit_s: 100,
            anarchy_unit_s: 200,
            dev_bands: [(0, 9), (10, 19), (20, 29), (30, 39), (40, 99)],
            anarchy_bands: [(0, 9), (10, 19), (20, 29), (30, 39), (40, 99)],
            anarchy_workforce: [0, 10, 20, 30, 40],
            gen_tick_s: 60,
            gen_bands: [(1, 3), (2, 4), (3, 5), (4, 7), (5, 9)],
            cargo_cap: 40,
            infomorph_type: FIXTURE_INFOMORPH,
        },
    )])
}

/// An invented infomorph type id, never a real one.
pub const FIXTURE_INFOMORPH: i64 = 990_830_001;

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "sov mercenary dens",
    load: load_table,
    fixture: || Box::new(fixture_table()) as crate::content::Boxed,
};

/// The definition for a den type in `content`.
pub fn def_of(content: &crate::content::Content, type_id: i64) -> Option<&DenDef> {
    content.try_get::<DenTable>()?.get(type_id)
}

// ---------------------------------------------------------------------------
// The rule (pure; the client's own)
// ---------------------------------------------------------------------------

/// `_calculate_level_based_on_natural_progression` **[V]**
/// (`repository.py:301-306`): `base` when paused or the unit is 0, else
/// `base + elapsed / unit` in integer division (Python 2). A clock behind
/// `at` adds nothing (`max(0, ..)`).
pub fn level(base: u64, at: u64, unit: u64, paused: bool, now: u64) -> u64 {
    if paused || unit == 0 {
        base
    } else {
        base.saturating_add(now.saturating_sub(at) / unit)
    }
}

/// The stage (1..=5) of `level`: the highest stage whose lower bound the
/// level has reached; stage 1 below the first bound.
pub fn stage_of(bands: &[(u32, u32); 5], level: u64) -> u32 {
    (1..=5u32)
        .rev()
        .find(|s| level >= u64::from(bands[*s as usize - 1].0))
        .unwrap_or(1)
}

fn mix(den: i64, tick: u64) -> u64 {
    let mut x = (den as u64) ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The infomorphs the tick that ends at `at` makes: a count in the band of
/// the development stage then, fixed by `(den, tick number)` **[A]**
/// (falsifier: TQ's generation being random per tick, or banded another way;
/// the client only shows the band).
pub fn tick_yield(den: i64, def: &DenDef, stage: u32, at: u64) -> u32 {
    let (lo, hi) = def.gen_bands[(stage.clamp(1, 5) - 1) as usize];
    let span = u64::from(hi - lo) + 1;
    lo + (mix(den, at / def.gen_tick_s) % span) as u32
}

// ---------------------------------------------------------------------------
// The row
// ---------------------------------------------------------------------------

/// One den.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Den {
    pub id: i64,
    pub owner: i64,
    pub skyhook: i64,
    pub planet: i64,
    pub system: i64,
    pub type_id: i64,
    pub enabled: bool,
    /// The evolution clock is frozen: `held || !enabled`. What the levels
    /// are computed with.
    pub paused: bool,
    pub dev_base: u64,
    pub dev_at: u64,
    pub anarchy_base: u64,
    pub anarchy_at: u64,
    pub infomorphs: u32,
    pub last_gen: u64,
    /// `development * 10 + anarchy`, the stages the owner was last told.
    pub seen: u32,
    /// A GM pause (`/mercden pause`), kept apart from `enabled` so the
    /// skyhook going off and on does not undo it.
    pub held: bool,
}

impl Den {
    pub fn dev_level(&self, def: &DenDef, now: u64) -> u64 {
        level(self.dev_base, self.dev_at, def.dev_unit_s, self.paused, now)
    }

    pub fn anarchy_level(&self, def: &DenDef, now: u64) -> u64 {
        level(
            self.anarchy_base,
            self.anarchy_at,
            def.anarchy_unit_s,
            self.paused,
            now,
        )
    }

    pub fn dev_stage(&self, def: &DenDef, now: u64) -> u32 {
        stage_of(&def.dev_bands, self.dev_level(def, now))
    }

    pub fn anarchy_stage(&self, def: &DenDef, now: u64) -> u32 {
        stage_of(&def.anarchy_bands, self.anarchy_level(def, now))
    }

    /// The pair of stages as [`Den::seen`] keeps them.
    pub fn stages_code(&self, def: &DenDef, now: u64) -> u32 {
        self.dev_stage(def, now) * 10 + self.anarchy_stage(def, now)
    }
}

fn parse_row(key: &[String], v: &[String]) -> Option<Den> {
    let n = |i: usize| v.get(i)?.parse::<u64>().ok();
    let i = |i: usize| v.get(i)?.parse::<i64>().ok();
    Some(Den {
        id: key.first()?.parse().ok()?,
        owner: i(0)?,
        skyhook: i(1)?,
        planet: i(2)?,
        system: i(3)?,
        type_id: i(4)?,
        enabled: n(5)? != 0,
        paused: n(6)? != 0,
        dev_base: n(7)?,
        dev_at: n(8)?,
        anarchy_base: n(9)?,
        anarchy_at: n(10)?,
        infomorphs: u32::try_from(n(11)?).ok()?,
        last_gen: n(12)?,
        seen: n(13).and_then(|s| u32::try_from(s).ok()).unwrap_or(0),
        held: n(14).is_some_and(|h| h != 0),
    })
}

/// Every den, in id order. A row that does not parse is logged and left out.
pub fn all(d: &Deltas) -> Vec<Den> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let row = parse_row(k, v);
            if row.is_none() {
                crate::errln!("[merc_den] skipping row {k:?}: {v:?}");
            }
            row
        })
        .collect()
}

/// The den `id`.
pub fn by_id(d: &Deltas, id: i64) -> Option<Den> {
    let key = vec![id.to_string()];
    parse_row(&key, d.of(KIND).get(&key)?)
}

/// The dens in `system`.
pub fn in_system(d: &Deltas, system: i64) -> Vec<Den> {
    all(d).into_iter().filter(|s| s.system == system).collect()
}

/// The workforce the enabled dens in `system` use at `now`: each den's
/// anarchy-stage consumption, the figure the den window shows
/// ([`evolution_definition_bytes`]). The hub's allocated workforce adds it
/// (`sov_resources::allocated`). **[A]** a disabled (skyhook off) den uses
/// none; falsifier: the hub window showing a draw while the skyhook is off.
pub fn workforce_use(reg: &Registry, system: i64, now: u64) -> u64 {
    in_system(reg.deltas(), system)
        .iter()
        .filter(|d| d.enabled)
        .filter_map(|d| {
            let def = def_for(reg, d)?;
            def.anarchy_workforce
                .get(d.anarchy_stage(&def, now) as usize - 1)
                .copied()
        })
        .fold(0, u64::saturating_add)
}

/// A den's draw on its system's hub changed (launched, removed, switched, a
/// stage change): re-run the hub's upgrades against the new budget and tell
/// its window. Nothing when the system has no hub.
fn tell_hub(reg: &mut Registry, system: i64) {
    if let Some(hub) = crate::sov_hub::hub(reg.deltas(), system) {
        crate::sov_upgrade::resimulate(reg, hub.hub_id);
        crate::sov_resources::announce(reg, hub.hub_id);
    }
}

/// The dens `owner` has.
pub fn owned_by(d: &Deltas, owner: i64) -> Vec<Den> {
    all(d).into_iter().filter(|s| s.owner == owner).collect()
}

/// Write one row.
pub fn put(d: &mut Deltas, s: &Den) {
    d.put(
        KIND,
        vec![s.id.to_string()],
        vec![
            s.owner.to_string(),
            s.skyhook.to_string(),
            s.planet.to_string(),
            s.system.to_string(),
            s.type_id.to_string(),
            u8::from(s.enabled).to_string(),
            u8::from(s.paused).to_string(),
            s.dev_base.to_string(),
            s.dev_at.to_string(),
            s.anarchy_base.to_string(),
            s.anarchy_at.to_string(),
            s.infomorphs.to_string(),
            s.last_gen.to_string(),
            s.seen.to_string(),
            u8::from(s.held).to_string(),
        ],
    );
}

fn def_for(reg: &Registry, den: &Den) -> Option<DenDef> {
    def_of(reg.content(), den.type_id).cloned()
}

// ---------------------------------------------------------------------------
// Infomorph generation
// ---------------------------------------------------------------------------

/// The count and the last generation tick after the whole ticks between
/// `den.last_gen` and `now`. Each tick makes [`tick_yield`] in the band of
/// the development stage **at that tick**; the count never passes
/// `cargo_cap`; `last_gen` advances by whole ticks only. A den that is not
/// enabled makes nothing and its tick clock does not move (enabling it sets
/// the clock, see [`set_enabled`]).
pub fn generate(den: &Den, def: &DenDef, now: u64) -> (u32, u64) {
    if !den.enabled || now <= den.last_gen {
        return (den.infomorphs, den.last_gen);
    }
    let ticks = (now - den.last_gen) / def.gen_tick_s;
    if ticks == 0 {
        return (den.infomorphs, den.last_gen);
    }
    let mut count = den.infomorphs.min(def.cargo_cap);
    for n in 1..=ticks.min(MAX_TICKS_PER_SETTLE) {
        if count >= def.cargo_cap {
            break;
        }
        let at = den.last_gen + n * def.gen_tick_s;
        let stage = stage_of(
            &def.dev_bands,
            level(den.dev_base, den.dev_at, def.dev_unit_s, den.paused, at),
        );
        count = count
            .saturating_add(tick_yield(den.id, def, stage, at))
            .min(def.cargo_cap);
    }
    (count, den.last_gen + ticks * def.gen_tick_s)
}

/// What the den holds at `now`, without writing.
pub fn projected(den: &Den, def: &DenDef, now: u64) -> u32 {
    generate(den, def, now).0
}

/// Bring the stored count up to `now` and write the row. The den as stored
/// afterwards.
pub fn settle(reg: &mut Registry, id: i64, now: u64) -> Option<Den> {
    let mut den = by_id(reg.deltas(), id)?;
    let def = def_for(reg, &den)?;
    let (count, last) = generate(&den, &def, now);
    if (count, last) != (den.infomorphs, den.last_gen) {
        den.infomorphs = count;
        den.last_gen = last;
        put(reg.deltas_mut(), &den);
    }
    Some(den)
}

// ---------------------------------------------------------------------------
// Placement, the ball, the slim
// ---------------------------------------------------------------------------

/// Where the den stands: its skyhook's spot, [`DEN_OFFSET`] off along +z
/// **[A]**.
pub fn position(reg: &Registry, den: &Den) -> Option<[f64; 3]> {
    let at = skyhook::position_of(reg.celestials().get(den.planet)?);
    Some([at[0], at[1], at[2] + DEN_OFFSET])
}

fn type_of(reg: &Registry, id: i64) -> Option<i64> {
    reg.store().get(id).map(|i| i.type_id)
}

fn radius(reg: &Registry, id: i64) -> f64 {
    let content = reg.content();
    content
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(type_of(reg, id).unwrap_or(0)))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(DEFAULT_RADIUS)
}

/// The den's ball: static, global in its system **[A]**.
pub fn ball(reg: &Registry, den: &Den) -> Option<Ball> {
    type_of(reg, den.id)?;
    Some(Ball {
        ball_id: den.id,
        mode: Mode::Stop,
        radius: radius(reg, den.id) as f32,
        position: position(reg, den)?,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: -1,
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    })
}

/// The five basic slim attributes; `ownerID` is the pilot (RE §3.4).
pub fn slim(reg: &Registry, den: &Den) -> Option<Slim> {
    Some(Slim {
        item_id: den.id,
        type_id: type_of(reg, den.id)?,
        owner_id: den.owner,
        char_id: None,
        corp_id: None,
        response_thresholds: None,
    })
}

/// `component_activate = (done, time)` (`mercenaryDen.py:60-75`) **[V]**: a
/// den is built the moment it is launched **[A]** (falsifier: the client
/// showing a build bar the type's activate component names), so `done` is
/// true and the model switches its production state on.
pub fn component_activate(den: &Den) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Bool(true),
        PyValue::Int(skyhook::filetime(den.dev_at)),
    ])
}

/// The extra slim keys: `component_activate`.
pub fn extras_of(den: &Den) -> Vec<(&'static str, PyValue)> {
    vec![("component_activate", component_activate(den))]
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|d| ids.contains(&d.id))
        .map(|d| (d.id, extras_of(d)))
        .collect()
}

/// `system`'s den balls and slims, for `destiny::scene` (coordinator: one
/// loop there). Global: in no `grid::local_balls`.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    in_system(w.deltas(), system)
        .iter()
        .filter_map(|d| Some((ball(w, d)?, slim(w, d)?)))
        .collect()
}

/// Where the den `ball_id` is, for `Registry::ball_target` (a hot line in
/// `service.rs`, specified in the report): approach, orbit, warp to.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let den = by_id(w.deltas(), ball_id)?;
    Some(crate::model::Target {
        position: position(w, &den)?,
        radius: radius(w, ball_id),
        warp_in: None,
    })
}

fn in_space(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// A macho notify to the owner: `(notifyMsg, {'itemID': id, 'typeID': type})`
/// (`repository.py:93-106`) **[V]**. The message text (728099) names the den
/// by `{[item]typeID.name}`, so the args carry the den's type **[V]**
/// (`out/research/_w/loc.tsv`); without it the client's localizer raises.
#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    pub to: i64,
    pub item: i64,
    pub type_id: i64,
}

impl Push for Registered {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMercenaryDenRegistrationSuccess",
            vec![
                crate::pyutil::bstr(MSG_SUCCESS),
                PyValue::Dict(crate::pyutil::pypairs(vec![
                    ("itemID", PyValue::Int(self.item)),
                    ("typeID", PyValue::Int(self.type_id)),
                ])),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnMyMercenaryDenRemoved(msg, {'itemID': id})` **[V]**
/// (`repository.py:93-96`).
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    pub to: i64,
    pub item: i64,
}

impl Push for Removed {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMyMercenaryDenRemoved",
            vec![
                crate::pyutil::bstr(MSG_REMOVED),
                PyValue::Dict(crate::pyutil::pypairs(vec![(
                    "itemID",
                    PyValue::Int(self.item),
                )])),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `AddBalls2` of a den to every park in its system.
fn announce_added(reg: &mut Registry, den: &Den) {
    let to = in_space(reg, den.system);
    let (Some(b), Some(s)) = (ball(reg, den), slim(reg, den)) else {
        return;
    };
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![b],
        },
        vec![(s, None)],
    )
    .map(|a| a.with_extras(slim_extras(reg, &[den.id])));
    if let Some(a) = added {
        reg.push(crate::sov_hub::HubAdded(a));
    }
}

fn announce_removed(reg: &mut Registry, system: i64, id: i64) {
    let to = in_space(reg, system);
    let stamp = reg.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, vec![id]) {
        reg.push(gone);
    }
}

// ---------------------------------------------------------------------------
// The wire: one encoder per message
// ---------------------------------------------------------------------------

fn attributes_bytes(den: &Den) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &id(den.owner as u64));
    put_bytes(&mut m, 2, &id(den.skyhook as u64));
    put_bytes(&mut m, 3, &id(den.system as u64));
    put_bytes(&mut m, 4, &id(den.planet as u64));
    put_bytes(&mut m, 5, &id(den.type_id as u64));
    m
}

/// `EvolutionSimulation` (`gw-protos.txt` evolution.proto): `paused` or
/// `started`, then for each axis the **stored base** level, its stage **now**,
/// and `paused_at` or `simulated_at` -- the client extrapolates the level
/// from the record (`repository.py:288-312`) **[V]**, so the base and the
/// time it was taken are what must be on the wire.
pub fn simulation_bytes(den: &Den, def: &DenDef, now: u64) -> Vec<u8> {
    let mut m = Vec::new();
    put_bool(&mut m, if den.paused { 1 } else { 2 }, true);
    let axes = [
        (3, den.dev_base, den.dev_at, den.dev_stage(def, now)),
        (
            4,
            den.anarchy_base,
            den.anarchy_at,
            den.anarchy_stage(def, now),
        ),
    ];
    for (field, base, at, stage) in axes {
        let mut s = Vec::new();
        put_uint(&mut s, 1, base);
        put_uint(&mut s, 2, u64::from(stage));
        put_bytes(&mut s, if den.paused { 3 } else { 4 }, &timestamp(at));
        put_bytes(&mut m, field, &s);
    }
    m
}

fn stage_bytes(stage: usize, lo: u32, hi: u32, workforce: Option<u64>) -> Vec<u8> {
    let mut s = Vec::new();
    put_uint(&mut s, 1, stage as u64);
    put_uint(&mut s, 2, u64::from(lo));
    put_uint(&mut s, 3, u64::from(hi));
    if let Some(w) = workforce {
        put_uint(&mut s, 4, w);
    }
    s
}

/// `EvolutionDefinition`: unit time and the bands per stage, plus the anarchy
/// workforce consumption.
pub fn evolution_definition_bytes(def: &DenDef) -> Vec<u8> {
    let mut dev = Vec::new();
    put_uint(&mut dev, 1, def.dev_unit_s);
    for (i, (lo, hi)) in def.dev_bands.iter().enumerate() {
        put_bytes(&mut dev, 2, &stage_bytes(i + 1, *lo, *hi, None));
    }
    let mut an = Vec::new();
    put_uint(&mut an, 1, def.anarchy_unit_s);
    for (i, (lo, hi)) in def.anarchy_bands.iter().enumerate() {
        put_bytes(
            &mut an,
            2,
            &stage_bytes(i + 1, *lo, *hi, Some(def.anarchy_workforce[i])),
        );
    }
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &dev);
    put_bytes(&mut m, 2, &an);
    m
}

/// `InfomorphsGenerationDefinition`.
pub fn infomorph_definition_bytes(def: &DenDef) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &id(def.infomorph_type as u64));
    put_uint(&mut m, 2, def.gen_tick_s);
    for (i, (lo, hi)) in def.gen_bands.iter().enumerate() {
        let mut b = Vec::new();
        put_uint(&mut b, 1, i as u64 + 1);
        put_uint(&mut b, 2, u64::from(*lo));
        put_uint(&mut b, 3, u64::from(*hi));
        put_bytes(&mut m, 3, &b);
    }
    put_uint(&mut m, 4, u64::from(def.cargo_cap));
    m
}

/// The `GetAsOwnerResponse` body for `den` at `now`: attributes, enabled,
/// evolution, infomorphs, extraction enabled, the skyhook's owner corp.
pub fn response(reg: &Registry, den: &Den, def: &DenDef, now: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &attributes_bytes(den));
    put_bool(&mut out, 2, den.enabled);
    let mut evo = Vec::new();
    put_bytes(&mut evo, 1, &evolution_definition_bytes(def));
    put_bytes(&mut evo, 2, &simulation_bytes(den, def, now));
    put_bytes(&mut out, 3, &evo);
    let mut inf = Vec::new();
    put_bytes(&mut inf, 1, &infomorph_definition_bytes(def));
    let mut contents = Vec::new();
    put_uint(&mut contents, 1, u64::from(projected(den, def, now)));
    // The last generation tick the projected count includes, not the stored one.
    put_bytes(&mut contents, 2, &timestamp(generate(den, def, now).1));
    put_bytes(&mut inf, 2, &contents);
    put_bytes(&mut out, 4, &inf);
    put_bool(&mut out, 5, den.enabled);
    let corp = skyhook::by_id(reg.deltas(), den.skyhook).map_or(0, |s| s.corp);
    put_bytes(&mut out, 6, &id(corp as u64));
    out
}

/// `EvolutionStateChangedNotice` to the owner: the simulation as it now is.
fn announce_evolution(reg: &mut Registry, den: &Den, now: u64) {
    let Some(def) = def_for(reg, den) else { return };
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(den.id as u64));
    put_bytes(&mut p, 2, &simulation_bytes(den, &def, now));
    crate::gateway_out::queue(
        reg,
        "eve_public.sovereignty.mercenaryden.api.EvolutionStateChangedNotice",
        p,
        Target::Character(den.owner),
    );
}

// ---------------------------------------------------------------------------
// Skill and maximum
// ---------------------------------------------------------------------------

/// Dens per skill level: the default row's, for the launch check and the
/// window alike.
fn per_level(reg: &Registry) -> u32 {
    reg.content()
        .try_get::<DenTable>()
        .and_then(DenTable::default_row)
        .map_or(0, |d| d.dens_per_level)
}

/// The pilot's trained level of the den-management skill, at most [`SKILL_TOP`].
fn trained_level(reg: &Registry, who: i64) -> u32 {
    u32::try_from(crate::drone_skills::level_of(
        reg,
        who,
        SKILL_DEN_MANAGEMENT,
    ))
    .unwrap_or(0)
    .min(SKILL_TOP)
}

/// `(current, absolute)` dens a pilot may have: the trained level of the
/// den-management skill times the table's dens per level, and the top level
/// times it **[A]** (falsifier: the "My dens" window showing other maxima
/// for a level on TQ; the client only reads them,
/// `request_messenger.py:179-189`).
pub fn maximum_for(reg: &Registry, who: i64) -> (u32, u32) {
    let per = per_level(reg);
    let lvl = u32::try_from(crate::drone_skills::level_of(
        reg,
        who,
        SKILL_DEN_MANAGEMENT,
    ))
    .unwrap_or(0)
    .min(SKILL_TOP);
    (lvl.saturating_mul(per), SKILL_TOP.saturating_mul(per))
}

// ---------------------------------------------------------------------------
// Deploy (`ship.Drop`)
// ---------------------------------------------------------------------------

/// Why a launch was refused. The den rules carry the client's own reason
/// key **[V]** (`sovereignty/mercenaryden/common/errors.py:20-27`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotInSpace,
    NotInHold,
    NotADen,
    Warping,
    /// The table has no row for the type (a bundle without the file).
    NoDefinition,
    /// No den-management skill trained.
    NoSkill,
    /// As many dens as the skill allows.
    AtMaximum {
        max: u32,
        /// The pilot's trained level of the den-management skill.
        level: u32,
    },
    /// No skyhook in the system.
    NoSkyhook,
    /// The nearest skyhook is too far.
    TooFar {
        metres: i64,
    },
    /// The skyhook is not switched on or its owner no longer holds the system.
    SkyhookOffline,
    /// The planet does not give workforce.
    NotWorkforce,
    /// The skyhook already has a den.
    Occupied,
    Store(String),
}

impl Refused {
    /// The client's reason key for a den rule, `None` for the plain launch
    /// problems (which answer as a `CustomNotify`).
    pub fn key(&self) -> Option<&'static str> {
        Some(match self {
            Refused::NoSkill => "MercenaryDen_DeploymentFailed_SkillLimits",
            // Below the top level the next skill level raises the cap, so the
            // skill-limits text applies **[I]** (from the texts 728097 and
            // 728102; the client builds no args, `errors.py:20-31`);
            // falsifier: a TQ pilot below level 5 at the cap seeing the
            // maximum-reached dialog.
            Refused::AtMaximum { level, .. } if *level < SKILL_TOP => {
                "MercenaryDen_DeploymentFailed_SkillLimits"
            }
            Refused::AtMaximum { .. } => "MercenaryDen_DeploymentFailed_MaximumDensReached",
            Refused::NoSkyhook | Refused::TooFar { .. } => {
                "MercenaryDen_DeploymentFailed_RequiresNearbySkyhook"
            }
            Refused::SkyhookOffline => "MercenaryDen_DeploymentFailed_RequiresOnlineSkyhook",
            Refused::NotWorkforce => "MercenaryDen_DeploymentFailed_MustBeTemperatePlanet",
            Refused::Occupied => "MercenaryDen_DeploymentFailed_SkyhookAlreadyOccupied",
            Refused::NoDefinition => "MercenaryDen_DeploymentFailed",
            _ => return None,
        })
    }

    /// The args the reason's message text reads **[V]**
    /// (`out/research/_w/loc.tsv`, ids 728096-728102 and 870990): every
    /// keyed text names the den by `{[item]typeID.name}`, so `typeID` is
    /// always there; the client's localizer raises without it. Skill limits
    /// (728097) also read `skillTypeID` and `level`; `level` is the pilot's
    /// trained level `trained` plus one **[I]** (the text says "increase ...
    /// to level {level}" and the server's rule decides the next level). The
    /// nearby-skyhook text (728100) reads `minDistance` in metres **[V]**:
    /// `[numeric]x.distance` goes through `numericPropertyHandler.py:25-26`
    /// to `FmtDist(value)`, `format.py:193-209` (metres under 10 000, else
    /// kilometres).
    pub fn args(&self, den_type: i64, trained: u32) -> Vec<(&'static str, PyValue)> {
        if self.key().is_none() {
            return Vec::new();
        }
        let mut a = vec![("typeID", PyValue::Int(den_type))];
        match self {
            Refused::NoSkill | Refused::AtMaximum { .. } => {
                if self.key() == Some("MercenaryDen_DeploymentFailed_SkillLimits") {
                    let have = match self {
                        Refused::AtMaximum { level, .. } => *level,
                        _ => trained,
                    };
                    a.push(("skillTypeID", PyValue::Int(SKILL_DEN_MANAGEMENT)));
                    a.push(("level", PyValue::Int(i64::from(have) + 1)));
                }
            }
            Refused::NoSkyhook | Refused::TooFar { .. } => {
                a.push(("minDistance", PyValue::Real(LAUNCH_RANGE)));
            }
            _ => {}
        }
        a
    }

    pub fn text(&self) -> String {
        match self {
            Refused::NotInSpace => "You can only launch that in space, in your ship.".to_owned(),
            Refused::NotInHold => {
                "That is not in your ship's cargo hold, fleet hangar or infrastructure hangar."
                    .to_owned()
            }
            Refused::NotADen => "That is not a mercenary den.".to_owned(),
            Refused::Warping => "You cannot launch anything while in warp.".to_owned(),
            Refused::NoDefinition => "This server has no mercenary den rules.".to_owned(),
            Refused::NoSkill => {
                "You need the Mercenary Den Management skill to launch a den.".to_owned()
            }
            Refused::AtMaximum { max, .. } => {
                format!("Your skill allows {max} mercenary den(s); you have them all out.")
            }
            Refused::NoSkyhook => {
                "There is no skyhook in this system to launch a den by.".to_owned()
            }
            Refused::TooFar { metres } => format!(
                "A den is launched by a skyhook; fly within {} km of it (you are {} km away).",
                (LAUNCH_RANGE / 1000.0) as i64,
                metres / 1000
            ),
            Refused::SkyhookOffline => {
                "The skyhook is not online; a den needs one that is.".to_owned()
            }
            Refused::NotWorkforce => "A den can only stand over a temperate planet.".to_owned(),
            Refused::Occupied => "This skyhook already has a mercenary den.".to_owned(),
            Refused::Store(e) => format!("The launch failed: {e}"),
        }
    }
}

/// The checks of one launch, in the order a pilot meets them (spec L10.2).
/// `Ok` with the item, the skyhook it goes by and the definition.
fn check_launch(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
) -> Result<(crate::item::Item, skyhook::Skyhook), Refused> {
    let c = reg.character(actor).cloned().ok_or(Refused::NotInSpace)?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or(Refused::NotInSpace)?;
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor
                && i.location_id == c.ship_id
                && crate::sov_hub::LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .ok_or(Refused::NotInHold)?;
    if item.group_id != GROUP_MERCENARY_DEN {
        return Err(Refused::NotADen);
    }
    if here.motion.warp_active() {
        return Err(Refused::Warping);
    }
    // The same per-level figure the window's maximum shows (`maximum_for`).
    def_of(reg.content(), item.type_id).ok_or(Refused::NoDefinition)?;
    let per = per_level(reg);
    let level = trained_level(reg, actor);
    if level == 0 {
        return Err(Refused::NoSkill);
    }
    let max = level.saturating_mul(per);
    if owned_by(reg.deltas(), actor).len() as u64 >= u64::from(max) {
        return Err(Refused::AtMaximum { max, level });
    }
    // The nearest skyhook of the system, any owner.
    let system = here.solar_system_id;
    let mut best: Option<(skyhook::Skyhook, f64)> = None;
    for sk in skyhook::in_system(reg.deltas(), system) {
        let Some(at) = skyhook::position(reg, &sk) else {
            continue;
        };
        let away = crate::space_state::distance(here.motion.position, at);
        if best.as_ref().is_none_or(|b| away < b.1) {
            best = Some((sk, away));
        }
    }
    let (sk, away) = best.ok_or(Refused::NoSkyhook)?;
    if away > LAUNCH_RANGE {
        return Err(Refused::TooFar {
            metres: away as i64,
        });
    }
    if !skyhook::running(reg.deltas(), &sk) {
        return Err(Refused::SkyhookOffline);
    }
    let works = crate::sov_resources::planet_yield(reg, sk.planet)
        .is_some_and(|y| y.workforce > 0 && y.reagent.is_none());
    if !works {
        return Err(Refused::NotWorkforce);
    }
    if all(reg.deltas())
        .iter()
        .any(|d| d.skyhook == sk.id || d.planet == sk.planet)
    {
        return Err(Refused::Occupied);
    }
    Ok((item, sk))
}

fn created(changes: &[crate::item::Change], fallback: i64) -> i64 {
    changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// A new row for an item already in space, by `sk`, owned by `owner`.
fn place(reg: &mut Registry, id: i64, owner: i64, sk: &skyhook::Skyhook, now: u64) -> Option<Den> {
    let type_id = type_of(reg, id)?;
    let def = def_of(reg.content(), type_id)?.clone();
    let mut den = Den {
        id,
        owner,
        skyhook: sk.id,
        planet: sk.planet,
        system: sk.system,
        type_id,
        enabled: true,
        paused: false,
        dev_base: 0,
        dev_at: now,
        anarchy_base: 0,
        anarchy_at: now,
        infomorphs: 0,
        last_gen: now,
        seen: 0,
        held: false,
    };
    den.seen = den.stages_code(&def, now);
    put(reg.deltas_mut(), &den);
    announce_added(reg, &den);
    Some(den)
}

/// Launch one den from `actor`'s hold at `now`: the checks, the item into
/// space owned by the pilot, the row, the ball, the success notify. The new
/// den's id.
pub fn launch_one_at(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
    now: u64,
) -> Result<i64, Refused> {
    let (item, sk) = check_launch(reg, actor, item_id)?;
    let to = Slot::new(actor, sk.system, FLAG_IN_SPACE);
    let edit = if item.quantity > 1 {
        Edit::Split {
            item_id,
            quantity: 1,
            to,
        }
    } else {
        Edit::Move { item_id, to }
    };
    let changes = reg
        .apply_items(&[edit])
        .map_err(|e| Refused::Store(e.to_string()))?;
    let new_id = created(&changes, item_id);
    let Some(den) = place(reg, new_id, actor, &sk, now) else {
        return Err(Refused::Store("the den has no type row".to_owned()));
    };
    reg.push(Registered {
        to: actor,
        item: den.id,
        type_id: den.type_id,
    });
    crate::errln!(
        "[merc_den] {} launched by {} at skyhook {} over {}",
        den.id,
        actor,
        sk.id,
        sk.planet
    );
    tell_hub(reg, den.system);
    Ok(den.id)
}

/// [`launch_one_at`] on the wall clock.
pub fn launch_one(reg: &mut Registry, actor: i64, item_id: i64) -> Result<i64, Refused> {
    launch_one_at(reg, actor, item_id, now())
}

/// One refusal in a launch answer: `(label, {args})`, which
/// `eveMisc.LaunchFromShip` raises as `UserError(label, args)` **[V]**
/// (`eveMisc.py:46-56`). A den rule answers under the client's own reason key
/// **[I]** (falsifier: a "message not found" dialog); the rest as a
/// `CustomNotify`.
fn refusal(r: &Refused, den_type: i64, trained: u32) -> PyValue {
    match r.key() {
        Some(key) => PyValue::Tuple(vec![
            crate::pyutil::bstr(key),
            PyValue::Dict(crate::pyutil::pypairs(r.args(den_type, trained))),
        ]),
        None => crate::mtu::refusal(&r.text()),
    }
}

/// `ship.Drop([(itemID, qty)], whoseBehalfID, ignoreWarning)`'s link for a
/// den (`service/inventory.rs`, after the skyhook's): `Some` only when one of
/// the items is of [`GROUP_MERCENARY_DEN`]. The answer is the hub's:
/// `{itemID: [newID]}` or `{itemID: [(label, args)]}` **[V]**. Both launch
/// entries send this call (`menusvc.py:341-353`).
pub(crate) fn drop_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let list = Args::new(req).list(0)?;
    let items: Vec<i64> = list
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => match p.first() {
                Some(PyValue::Int(id)) => Some(*id),
                _ => None,
            },
            PyValue::Int(id) => Some(*id),
            _ => None,
        })
        .collect();
    if !items.iter().any(|id| {
        reg.store()
            .get(*id)
            .is_some_and(|i| i.group_id == GROUP_MERCENARY_DEN)
    }) {
        return None;
    }
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    for item in items {
        let den_type = reg.store().get(item).map_or(0, |i| i.type_id);
        let trained = req.actor.map_or(0, |a| trained_level(reg, a));
        let entry = match req.actor {
            None => refusal(&Refused::NotInSpace, den_type, trained),
            Some(a) => match launch_one(reg, a, item) {
                Ok(id) => PyValue::Int(id),
                Err(r) => refusal(&r, den_type, trained),
            },
        };
        answer.push((PyValue::List(vec![entry]), PyValue::Int(item)));
    }
    Some(Ok(PyValue::Dict(answer)))
}

// ---------------------------------------------------------------------------
// Pause, resume, enable
// ---------------------------------------------------------------------------

/// Take the levels to `now` as new bases (so the unit counts from here).
fn rebase(den: &mut Den, def: &DenDef, now: u64) {
    den.dev_base = den.dev_level(def, now);
    den.anarchy_base = den.anarchy_level(def, now);
    den.dev_at = now;
    den.anarchy_at = now;
}

/// Pause or resume the evolution of `id` at `now`. A pause freezes the levels
/// where they are and `paused_at` is `now`; a resume counts the unit from
/// `now`. The owner is told. `None` for an unknown den; no change when it
/// already is so.
pub fn set_paused(reg: &mut Registry, id: i64, paused: bool, now: u64) -> Option<Den> {
    let mut den = by_id(reg.deltas(), id)?;
    let def = def_for(reg, &den)?;
    if den.held == paused {
        return Some(den);
    }
    rebase(&mut den, &def, now);
    den.held = paused;
    den.paused = den.held || !den.enabled;
    den.seen = den.stages_code(&def, now);
    put(reg.deltas_mut(), &den);
    announce_evolution(reg, &den, now);
    Some(den)
}

/// Switch a den on or off (its skyhook went away or came back). Off pauses
/// the evolution; on resumes it and starts the generation clock at `now`.
fn set_enabled(reg: &mut Registry, den: &Den, enabled: bool, now: u64) {
    let Some(def) = def_for(reg, den) else { return };
    let mut den = *den;
    if enabled {
        // Settle nothing: a disabled den made nothing. Start the clock.
        den.last_gen = now;
    } else {
        // Count what it made up to the moment it stopped.
        let (count, last) = generate(&den, &def, now);
        den.infomorphs = count;
        den.last_gen = last;
    }
    rebase(&mut den, &def, now);
    den.enabled = enabled;
    den.paused = den.held || !enabled;
    den.seen = den.stages_code(&def, now);
    put(reg.deltas_mut(), &den);
    announce_evolution(reg, &den, now);
    tell_hub(reg, den.system);
}

// ---------------------------------------------------------------------------
// The clock stage
// ---------------------------------------------------------------------------

/// Take a den out of the world: the item if it is still there, the row, the
/// ball, and tell the owner.
pub fn remove(reg: &mut Registry, den: &Den) {
    // A den scooped into a hold is kept; one still standing is destroyed.
    if standing(reg, den)
        && let Err(e) = reg.apply_items_quiet(&[Edit::Destroy { item_id: den.id }])
    {
        crate::errln!("[merc_den] {} not destroyed: {e}", den.id);
    }
    reg.deltas_mut().del(KIND, &[den.id.to_string()]);
    activity::den_removed(reg, den);
    announce_removed(reg, den.system, den.id);
    reg.push(Removed {
        to: den.owner,
        item: den.id,
    });
    tell_hub(reg, den.system);
}

/// Whether the den's item is still where the row says: in space in its
/// system.
fn standing(reg: &Registry, den: &Den) -> bool {
    reg.store()
        .get(den.id)
        .is_some_and(|i| i.location_id == den.system && i.flag_id == FLAG_IN_SPACE)
}

/// Whether the den's skyhook is working, so the den is.
fn skyhook_works(reg: &Registry, den: &Den) -> bool {
    skyhook::by_id(reg.deltas(), den.skyhook).is_some_and(|s| skyhook::running(reg.deltas(), &s))
}

/// The clock stage `merc_den` (every 20 ticks, phase 2; `CADENCE_OK`): a den
/// whose item is gone is removed (destroyed or scooped, `OnMyMercenaryDenRemoved`,
/// RE §7.6); a den follows its skyhook on and off; infomorphs are settled; a
/// change of stage is told to the owner. Keyed on stored times, so a late run
/// does what an early one would have.
/// Scheduled, not pushed: `pending` drains only after a client's call, as structure_space's clock pushes do (#44).
pub fn tick(reg: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    crate::sov_hub::on_the_clock(reg, |reg| tick_at(reg, now()));
}

/// [`tick`] at `now`.
pub fn tick_at(reg: &mut Registry, now: u64) {
    if reg.deltas().of(KIND).is_empty() {
        return;
    }
    for mut den in all(reg.deltas()) {
        if !standing(reg, &den) {
            remove(reg, &den);
            continue;
        }
        // Its skyhook row is gone (destroyed): a skyhook rebuilt over the
        // same planet takes the den over, so the planet is not blocked for
        // good. Until then the den is off, as `skyhook_works` says.
        if skyhook::by_id(reg.deltas(), den.skyhook).is_none()
            && let Some(sk) = skyhook::at_planet(reg.deltas(), den.planet)
        {
            den.skyhook = sk.id;
            put(reg.deltas_mut(), &den);
        }
        let works = skyhook_works(reg, &den);
        if works != den.enabled {
            set_enabled(reg, &den, works, now);
        }
        let Some(den) = settle(reg, den.id, now) else {
            continue;
        };
        let Some(def) = def_for(reg, &den) else {
            continue;
        };
        let code = den.stages_code(&def, now);
        if den.seen != code {
            let mut den = den;
            let told = den.seen != 0;
            // The anarchy digit is what the hub's budget follows.
            let stage_moved = !told || code % 10 != den.seen % 10;
            den.seen = code;
            put(reg.deltas_mut(), &den);
            if told {
                announce_evolution(reg, &den, now);
            }
            if stage_moved {
                tell_hub(reg, den.system);
            }
        }
    }
    activity::tick_at(reg, now);
}

// ---------------------------------------------------------------------------
// The gateway arm
// ---------------------------------------------------------------------------

fn read_reply(reg: &Registry, rest: &str, ty: &str, body: &[u8], who: i64, now: u64) -> Reply {
    match rest {
        "GetAsOwnerRequest" => {
            // 404 for an unknown den and for another pilot's: the client
            // cannot tell them apart (`request_messenger.py:42-45`).
            let den = read_id(body, 1)
                .and_then(|i| by_id(reg.deltas(), i as i64))
                .filter(|d| d.owner == who);
            match den.and_then(|d| def_for(reg, &d).map(|def| response(reg, &d, &def, now))) {
                Some(bytes) => typed(ty, bytes),
                None => status(404),
            }
        }
        "GetAllOwnedRequest" => {
            let mut out = Vec::new();
            for d in owned_by(reg.deltas(), who) {
                put_bytes(&mut out, 1, &id(d.id as u64));
            }
            typed(ty, out)
        }
        _ => {
            let (cur, abs) = maximum_for(reg, who);
            let mut out = Vec::new();
            put_uint(&mut out, 1, u64::from(cur));
            put_uint(&mut out, 2, u64::from(abs));
            typed(ty, out)
        }
    }
}

/// The gateway arm (`gateway_ext::CHAIN`, above `sov_gw_stub`): `GetAsOwner`,
/// `GetAllOwned` and `GetMaximumForCharacter` of `mercenaryden.api`, then the
/// activity messages ([`activity::gateway`]); every other type is `None`. On a
/// [`Ground::Local`] there is no world: `GetAsOwner` answers 404 and
/// `GetAllOwned` an empty list.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    // The activity messages (MERC-2) are `activity.api`, a package of their own.
    if let Some(r) = activity::gateway(ty, body, character, g) {
        return Some(r);
    }
    let rest = ty.strip_prefix(API_PKG)?;
    if !matches!(
        rest,
        "GetAsOwnerRequest" | "GetAllOwnedRequest" | "GetMaximumForCharacterRequest"
    ) {
        return None;
    }
    let Ground::Server(w) = g else {
        return match rest {
            "GetAsOwnerRequest" => Some(status(404)),
            "GetAllOwnedRequest" => Some(typed(ty, Vec::new())),
            _ => None,
        };
    };
    let reg = crate::service::world_read_timed(w, "merc_den_gateway");
    Some(read_reply(&reg, rest, ty, body, character, now()))
}

// ---------------------------------------------------------------------------
// Component call (`beyonce.CallComponentFromClient`)
// ---------------------------------------------------------------------------

/// `space_component::Locate` for the den: the system of a den that is a live
/// item.
pub fn locate(reg: &Registry, item: i64) -> Option<i64> {
    let den = by_id(reg.deltas(), item)?;
    reg.store().get(item).map(|_| den.system)
}

fn failed(text: &str) -> ServiceError {
    // The client wraps any non-UserError into `MercenaryDenExtractionFailed`
    // (`repository.py:349-352`) **[V]**.
    crate::mtu::notify(text)
}

/// `ExtractInfomorphsToShip(flagID, qty)` -> the quantity moved. The owner,
/// within [`TRANSFER_RANGE`] of the den's surface, extraction enabled;
/// settles the generation; `moved = min(qty, held, room in the hold)`; makes
/// the infomorph items in the flag of the pilot's ship. The client already
/// clipped `qty` to the free volume (`repository.py:335-353`) **[V]**.
pub fn extract(reg: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
    extract_at(reg, c, now())
}

/// [`extract`] at `now`.
pub fn extract_at(reg: &mut Registry, c: &CompCall<'_>, now: u64) -> Result<PyValue, ServiceError> {
    let flag = c.args.int(0).ok_or_else(|| failed("No hold."))?;
    let qty = c.args.int(1).unwrap_or(0);
    if !matches!(
        flag,
        crate::sov_hub::FLAG_CARGO | crate::sov_hub::FLAG_INFRASTRUCTURE_HANGAR
    ) || qty <= 0
    {
        return Err(failed("That cannot be taken there."));
    }
    let den = by_id(reg.deltas(), c.item_id).ok_or_else(|| failed("No such mercenary den."))?;
    if den.owner != c.actor {
        return Err(failed("This is not your mercenary den."));
    }
    if !den.enabled {
        return Err(failed(
            "This mercenary den is not producing; it cannot be emptied.",
        ));
    }
    let def = def_for(reg, &den).ok_or_else(|| failed("This den has no rules."))?;
    let ship = reg
        .character(c.actor)
        .map(|ch| ch.ship_id)
        .ok_or_else(|| failed("You are not in space."))?;
    reg.advance_character(c.actor);
    let here = reg
        .roster()
        .get(c.actor)
        .copied()
        .ok_or_else(|| failed("You are not in space."))?;
    let at = position(reg, &den).ok_or_else(|| failed("The den has no place."))?;
    if crate::space_state::distance(here.motion.position, at) - radius(reg, den.id) > TRANSFER_RANGE
    {
        return Err(failed("You are too far from the mercenary den."));
    }
    let den = settle(reg, den.id, now).ok_or_else(|| failed("No such mercenary den."))?;
    let ship_type = reg.store().get(ship).map_or(0, |s| s.type_id);
    let capacity = if flag == crate::sov_hub::FLAG_CARGO {
        crate::fitting::cargo_capacity_in(reg, ship)
    } else {
        crate::item::hold_capacity_in(reg.content(), ship_type, flag)
    };
    let free = (capacity - reg.store().used_volume_in(reg.content(), ship, flag)).max(0.0);
    let unit = crate::item::find_in(reg.content(), def.infomorph_type).map_or(0.0, |t| t.volume);
    let room = crate::fitting::units_that_fit(free, unit);
    let moved = qty.min(i64::from(den.infomorphs)).min(room);
    if moved == 0 {
        return Err(failed(if room == 0 {
            "There is no room in that hold."
        } else {
            "The mercenary den holds nothing to take."
        }));
    }
    reg.apply_items(&[Edit::Create {
        type_id: def.infomorph_type,
        quantity: moved,
        to: Slot::new(c.actor, ship, flag),
    }])
    .map_err(|e| failed(&format!("The infomorphs could not be moved: {e}")))?;
    let mut den = den;
    den.infomorphs -= moved as u32;
    put(reg.deltas_mut(), &den);
    Ok(PyValue::Int(moved))
}
