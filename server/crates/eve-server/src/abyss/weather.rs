//! **Weather on every ship in the pocket** (issue #6, AD-3).
//!
//! A filament key carries a `weatherID` **[V]** (`C:dogma/const.py:1337`),
//! and the client's weather is a **dbuff**, a continuous one **[V]**
//! (`C:dbuff/const.py:67-100`, `C:eve/common/script/mgt/buffBarConst.py:46-50,
//! 380-394`): per weather a penalty collection and a bonus collection, held on
//! every ship in the pocket until it leaves (`C:dbuff/common/registry.py:42-71`
//! `ApplyContinuousBuffsToItem`; the buff bar shows the slot with no timer,
//! `buffButtons.py:313-317`). The server holds them with
//! [`crate::dbuff::hold`], once per tick (idempotent), the source being the
//! room's site id so a ship that moves on swaps the old room's hold for the new.
//!
//! ## Where the numbers come from
//!
//! - **[V]** the five weathers, their key types and their collections
//!   ([`WEATHERS`]); the aggregate of a collection (the bundle's
//!   `dbuffs/collections.tsv`).
//! - **[I]** the pairing of a key's weather type to a buff-bar slot is by name
//!   (spec §0.3): Dark Matter Field to Darkness, Exotic Particle Storm to
//!   CausticToxin, Plasma Firestorm to Infernal, Gamma-Ray Afterglow to
//!   XenonGas, Electrical Storm to ElectricStorm.
//! - **[I]** the sign: a `Maximum` collection keeps the largest value so it
//!   takes `+pct`; a `Minimum` one keeps the most negative so it takes `-pct`
//!   ([`signed`]). Falsifier: a client tooltip showing the opposite direction.
//! - **[A]** the stage of a room (1 to 3) and the strength of a stage:
//!   `content/pve/abyss-weather.tsv`. The client keeps no strength in code
//!   (the stage is per encounter, chosen server-side). Falsifier: the extracted
//!   dogma of the weather types, or TQ's info window.
//! - **[A]** NPCs are not weathered: the NPC model has no dogma sheet.
//!   Falsifier: testers finding abyss NPCs too easy in a weather that should
//!   slow them.
//! - **[A]** the beacon and the stage's visual ball stand [`WEATHER_OFFSET`]
//!   above the room's entry. Falsifier: no weather visible in the room, or a
//!   client traceback on the ball.
//!
//! A run with weather 0 (the GM door, or a key without the attribute) has none.
//!
//! **A source rule for AD-4:** the area clouds must not use a run's room site id
//! as their dbuff source. `reconcile` releases every run-room site id a ship
//! holds when the ship is not in that room (and `close_pocket` releases it from
//! every ship), so a cloud held under that id would be dropped by a room
//! change. Use a source of the cloud's own (its ball's item id).
//!
//! ## The room's clouds (AD-4)
//!
//! Each area-of-effect locator of a PvE room (`AOE_10k`, `AOE_30k`, `AOE_80k`,
//! types 47460-47462 **[V-data]**, the sizes by the name **[I]**) may hold a
//! cloud, a group-1971 type whose name holds the size word and a family word
//! **[V-data]**. A cloud is a ball of its room's site; its **source** is that
//! ball's item id, never the room's site id, so a room change drops the old
//! room's clouds without touching the new room's.
//!
//! - **[A]** the family drawn per (run, room, locator): the `#cloud` rows of
//!   `content/pve/abyss-weather.tsv`. Falsifier: TQ rooms showing other mixes.
//! - **[I]** a cloud's buffs are its type's own `dbuffNID`/`dbuffValue` pairs
//!   (attributes 2468-2473 and 2536-2537), the value as given (the same pairs
//!   a burst module carries, `crate::dbuff::burst_of`). A cloud type without
//!   them is placed for its look and buffs nothing: no number is invented.
//! - **[V-data]** a cloud's reach is the type's attribute 54 (5, 15 and 40 km
//!   for the three sizes, half the locator's 10k/30k/80k name): the bundle's
//!   effects 7050, 7051 and 7058 (`aoe_beacon_*_cloud`, the three families) all
//!   carry `rangeAttributeID` 54 (`types/typedata.tsv`, `effect` rows). The
//!   bundle's `radius` column is 1 for every cloud, so it is no reach; a type
//!   with attribute 54 absent falls back to a `radius` over 1. Falsifier: the
//!   client drawing the cloud a different size than the reach.
//! - A group-1971 object with no buff pairs or no reach (every PvE room already
//!   holds some as scenery) is not a cloud here: [`buffing`] is the test.
//! - Pylons and suppressors (group 1981) are not clouds and not here.

use super::{AbyssState, Pocket, Run};
use crate::damage::BlueTime;
use crate::dbuff_data::{Aggregate, Dbuffs};
use crate::service::Registry;
use crate::site;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The authored table **[A]**.
const SHIPPED: &str = include_str!("../../../../../content/pve/abyss-weather.tsv");

/// Where the beacon and the visual stand, from the room's entry, in metres
/// **[A]**.
pub const WEATHER_OFFSET: [f64; 3] = [0.0, 5_000.0, 0.0];

/// One weather: the key type that names it (and the beacon group 1983 type of
/// the same id), its two dbuff collections and the three stage visuals. The
/// visuals are types of group 1975 (stages 1 and 2) and group 1882
/// `groupMassiveEnvironments` (stage 3) **[V-data]** `types/market.tsv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weather {
    pub beacon: i64,
    pub penalty: i64,
    pub bonus: i64,
    pub visual: [i64; 3],
}

/// The five weathers **[V]** `C:abyss/common/constants.py` `WEATHER_TYPE_IDS`;
/// their buff-bar collections **[V]** `C:eve/common/script/mgt/buffBarConst.py:
/// 380-394`; the pairing by name **[I]**. Type ids from
/// `C:inventorycommon/const.py:2475-2494`, collections from
/// `C:dbuff/const.py:81-100`.
pub const WEATHERS: [Weather; 5] = [
    // Electrical
    Weather {
        beacon: 47865,
        penalty: 90,
        bonus: 92,
        visual: [47381, 47382, 47383],
    },
    // Exotic
    Weather {
        beacon: 47862,
        penalty: 99,
        bonus: 100,
        visual: [47384, 47385, 47386],
    },
    // Firestorm
    Weather {
        beacon: 47864,
        penalty: 95,
        bonus: 96,
        visual: [47390, 47391, 47392],
    },
    // Gamma
    Weather {
        beacon: 47866,
        penalty: 93,
        bonus: 94,
        visual: [47387, 47388, 47389],
    },
    // Dark
    Weather {
        beacon: 47863,
        penalty: 97,
        bonus: 98,
        visual: [47378, 47379, 47380],
    },
];

/// One tier's stage draw: `(stage, weight)`, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRow {
    pub tier: i64,
    pub weights: Vec<(u8, u32)>,
}

/// What a stage costs and gives, in percent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strength {
    pub stage: u8,
    pub penalty_pct: f64,
    pub bonus_pct: f64,
}

/// The parsed table: the tier rows and the strength of each stage.
pub type Stages = (Vec<StageRow>, BTreeMap<u8, (f64, f64)>);

#[derive(Clone, Copy, PartialEq)]
enum Section {
    None,
    Tier,
    Stage,
    /// The `#cloud` rows are read by [`parse_clouds`]; this parser skips them.
    Cloud,
}

/// Parse the table. `#` lines are comments except the two section headers
/// (`#tier` and `#stage`, first field). An error names its line.
pub fn parse(text: &str) -> Result<Stages, String> {
    let mut rows: Vec<(usize, StageRow)> = Vec::new();
    let mut strengths: BTreeMap<u8, (f64, f64)> = BTreeMap::new();
    let mut section = Section::None;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if line.starts_with('#') {
            match f[0] {
                "#tier" => section = Section::Tier,
                "#stage" => section = Section::Stage,
                "#cloud" => section = Section::Cloud,
                _ => {}
            }
            continue;
        }
        let bad = |why: &str| format!("abyss-weather line {n}: {why}");
        match section {
            Section::Cloud => {}
            Section::None => return Err(bad("a row before any #tier or #stage header")),
            Section::Tier => {
                let [tier, weights] = f[..] else {
                    return Err(bad("want 2 fields: tier and stage weights"));
                };
                let tier: i64 = tier.parse().map_err(|_| bad("the tier is not a number"))?;
                if rows.iter().any(|(_, r)| r.tier == tier) {
                    return Err(bad("the tier is listed twice"));
                }
                let mut ws = Vec::new();
                for pair in weights.split(',') {
                    let (s, w) = pair
                        .split_once(':')
                        .ok_or_else(|| bad("a weight is not stage:weight"))?;
                    let s: u8 = s.trim().parse().map_err(|_| bad("a stage is not 1-3"))?;
                    let w: u32 = w
                        .trim()
                        .parse()
                        .map_err(|_| bad("a weight is not a whole number"))?;
                    if !(1..=3).contains(&s) {
                        return Err(bad("a stage is not 1-3"));
                    }
                    if w == 0 {
                        return Err(bad("a weight is 0"));
                    }
                    ws.push((s, w));
                }
                rows.push((n, StageRow { tier, weights: ws }));
            }
            Section::Stage => {
                let [stage, penalty, bonus] = f[..] else {
                    return Err(bad("want 3 fields: stage, penalty_pct, bonus_pct"));
                };
                let stage: u8 = stage.parse().map_err(|_| bad("the stage is not 1-3"))?;
                let pct = |v: &str| {
                    v.parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                        .ok_or_else(|| bad("a percent is not a number of 0 or more"))
                };
                if strengths
                    .insert(stage, (pct(penalty)?, pct(bonus)?))
                    .is_some()
                {
                    return Err(bad("the stage is listed twice"));
                }
            }
        }
    }
    // A listed stage must have a strength.
    for (n, r) in &rows {
        if let Some((s, _)) = r.weights.iter().find(|(s, _)| !strengths.contains_key(s)) {
            return Err(format!(
                "abyss-weather line {n}: stage {s} has no #stage row"
            ));
        }
    }
    Ok((rows.into_iter().map(|(_, r)| r).collect(), strengths))
}

/// The shipped table, parsed once. A table that does not parse is reported
/// once and leaves a world with no weather.
fn shipped() -> &'static Stages {
    static ROWS: OnceLock<Stages> = OnceLock::new();
    ROWS.get_or_init(|| match parse(SHIPPED) {
        Ok(t) => t,
        Err(e) => {
            crate::errln!("[abyss] content/pve/abyss-weather.tsv: {e}");
            (Vec::new(), BTreeMap::new())
        }
    })
}

/// The weather whose key type is `beacon`.
pub fn of(weathers: &[Weather], beacon: i64) -> Option<Weather> {
    weathers.iter().copied().find(|w| w.beacon == beacon)
}

/// The stage a room draws at `tier` from `seed`: only ever one of the tier's
/// listed stages, the same for the same seed. `None` for a tier with no row.
pub fn stage_for(rows: &[StageRow], tier: i64, seed: u64) -> Option<u8> {
    let row = rows.iter().find(|r| r.tier == tier)?;
    let total: u64 = row.weights.iter().map(|(_, w)| u64::from(*w)).sum();
    if total == 0 {
        return None;
    }
    let mut pick = seed % total;
    for (stage, w) in &row.weights {
        let w = u64::from(*w);
        if pick < w {
            return Some(*stage);
        }
        pick -= w;
    }
    None
}

/// The dbuff value of a percent: `Maximum` keeps the largest so it is `+pct`,
/// `Minimum` keeps the most negative so it is `-pct` **[I]**.
pub fn signed(pct: f64, aggregate: Aggregate) -> f64 {
    match aggregate {
        Aggregate::Maximum => pct,
        Aggregate::Minimum => -pct,
    }
}

/// The room's `(collection, value)` pairs. A collection the table lacks is
/// left out (the bundle has no such buff).
pub fn buffs(w: Weather, s: Strength, dbuffs: &Dbuffs) -> Vec<(i64, f64)> {
    [(w.penalty, s.penalty_pct), (w.bonus, s.bonus_pct)]
        .into_iter()
        .filter_map(|(c, pct)| {
            let col = dbuffs.collection(c)?;
            Some((c, signed(pct, col.aggregate)))
        })
        .collect()
}

/// The weather whose key type is `beacon`, from the table in force.
fn weather_of(reg: &Registry, beacon: i64) -> Option<Weather> {
    let list = reg.ext::<AbyssState>().weathers.as_deref();
    of(list.unwrap_or(&WEATHERS), beacon)
}

/// The table in force, borrowed.
fn table(reg: &Registry) -> &Stages {
    reg.ext::<AbyssState>()
        .stages
        .as_ref()
        .unwrap_or_else(|| shipped())
}

/// The stage a new room draws, 0 for none: no weather, an unknown weather, or
/// a tier without a row. `seed` is the room's.
pub(super) fn draw(reg: &Registry, tier: i64, weather: i64, seed: u64) -> u8 {
    if weather == 0 || weather_of(reg, weather).is_none() {
        return 0;
    }
    stage_for(&table(reg).0, super::play_tier(tier), seed).unwrap_or(0)
}

/// The strength of a pocket's stage, if it has one the table knows.
fn strength(reg: &Registry, stage: u8) -> Option<Strength> {
    let (penalty_pct, bonus_pct) = *table(reg).1.get(&stage)?;
    Some(Strength {
        stage,
        penalty_pct,
        bonus_pct,
    })
}

/// **The two balls of a freshly made room**: the weather's beacon and the
/// stage's visual, at the entry offset by [`WEATHER_OFFSET`]. Called once per
/// instance made, so a room made again carries one of each (the old instance
/// took its balls with it).
pub(crate) fn place(reg: &mut Registry, run: &Run, p: &Pocket) {
    place_weather(reg, run, p);
    place_clouds(reg, run, p, shipped_clouds());
}

fn place_weather(reg: &mut Registry, run: &Run, p: &Pocket) {
    if p.stage == 0 {
        return;
    }
    let Some(w) = weather_of(reg, run.weather) else {
        return;
    };
    let at = [
        p.entry[0] + WEATHER_OFFSET[0],
        p.entry[1] + WEATHER_OFFSET[1],
        p.entry[2] + WEATHER_OFFSET[2],
    ];
    site::add_object(reg, p.site_id, w.beacon, at);
    if let Some(v) = w.visual.get(usize::from(p.stage) - 1) {
        site::add_object(reg, p.site_id, *v, at);
    }
}

/// **Hold and release**, every tick, allocating only when something changes:
/// a pilot in an open weathered room holds the room's collections (source: the
/// room's site id); a ship that holds a room it is not in, or a room that is
/// closed, lets go. Everyone who was ever in the run is looked at, so a pilot
/// who left it (and is now somewhere else) is released too. A run with no
/// weather returns at once.
pub(crate) fn reconcile(reg: &mut Registry, run: &Run, now: BlueTime) {
    reconcile_weather(reg, run, now);
    reconcile_clouds(reg, run, now);
}

fn reconcile_weather(reg: &mut Registry, run: &Run, now: BlueTime) {
    if run.weather == 0 || run.pockets.iter().all(|p| p.stage == 0) {
        return;
    }
    let Some(w) = weather_of(reg, run.weather) else {
        return;
    };
    for &who in &run.ever {
        let at = reg
            .roster()
            .get(who)
            .map(|p| (p.ship_id, p.solar_system_id));
        let Some(ship) = at
            .map(|(s, _)| s)
            .or_else(|| reg.character(who).map(|c| c.ship_id))
        else {
            continue;
        };
        let pilot = run.pilot(who).is_some();
        for p in run.pockets.iter().filter(|p| p.stage > 0) {
            let want = pilot && !p.closed && at.is_some_and(|(_, s)| s == p.system);
            let holds = crate::dbuff::holds(reg, ship, p.site_id);
            if want && !holds {
                let held = strength(reg, p.stage)
                    .zip(reg.content().try_get::<Dbuffs>())
                    .map(|(s, d)| buffs(w, s, d))
                    .unwrap_or_default();
                crate::dbuff::hold(reg, ship, p.site_id, &held, now);
            } else if !want && holds {
                crate::dbuff::release(reg, ship, p.site_id, now);
            }
        }
    }
}

/// A room leaving space: every ship that still holds it lets go.
pub(crate) fn release_pocket(reg: &mut Registry, p: &Pocket) {
    let now = crate::pyutil::filetime_now();
    let balls: Vec<i64> = cloud_balls(reg, p.site_id)
        .into_iter()
        .map(|(ball, _, _)| ball)
        .collect();
    for ship in reg.ext::<crate::dbuff::Boosts>().boosted() {
        crate::dbuff::release(reg, ship, p.site_id, now);
        for ball in &balls {
            crate::dbuff::release(reg, ship, *ball, now);
        }
    }
}

// ---------------------------------------------------------------------------
// The room's clouds (AD-4)
// ---------------------------------------------------------------------------

/// The group of the cloud types **[V-data]** (`GROUP_WEATHER` of the type
/// tables; the abyssal environment clouds).
pub const GROUP_CLOUD: i64 = 1971;

/// The attribute pairs of a type's buffs, `(dbuffNID, dbuffValue)` **[V]**
/// (`C:dogma/const.py`, the same pairs a burst module carries).
pub const BUFF_PAIRS: [(i64, i64); 4] = [(2468, 2469), (2470, 2471), (2472, 2473), (2536, 2537)];

/// The attribute that holds a cloud's reach in metres **[V-data]** (the range
/// attribute of the cloud effects 7050, 7051 and 7058; the size is **[I]**): 5, 15 and 40 km
/// on the three sizes, the radii of the locators' 10k, 30k and 80k diameters.
pub const ATTR_REACH: i64 = 54;

/// The stream of the cloud draw, apart from the room and the stage draws.
const CLOUD_STREAM: u64 = 0x636c_6f75;

/// The locators' names **[V-data]** (spec §0, the bundle's `AOE_*` locators);
/// the bundle's type rows carry no names for them.
pub const LOCATOR_NAMES: [(i64, &str); 3] = [
    (47460, "AOE_10k_locator"),
    (47461, "AOE_30k_locator"),
    (47462, "AOE_80k_locator"),
];

/// A cloud's size, by its locator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    /// The word a cloud type's name holds.
    pub fn word(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
}

/// The size a locator's name asks for **[I]**: `AOE_10k` small, `AOE_30k`
/// medium, `AOE_80k` large; `None` for any other name.
pub fn size_of_name(name: &str) -> Option<Size> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("aoe_")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if !rest[digits.len()..].starts_with('k') {
        return None;
    }
    match digits.as_str() {
        "10" => Some(Size::Small),
        "30" => Some(Size::Medium),
        "80" => Some(Size::Large),
        _ => None,
    }
}

/// The size of the locator type `type_id`, if it is one of the three.
pub fn locator_size(type_id: i64) -> Option<Size> {
    LOCATOR_NAMES
        .iter()
        .find(|(id, _)| *id == type_id)
        .and_then(|(_, name)| size_of_name(name))
}

/// A cloud's family, the word its type's name holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Bioluminescence,
    Caustic,
    Filament,
}

impl Family {
    pub fn word(self) -> &'static str {
        match self {
            Family::Bioluminescence => "bioluminescence",
            Family::Caustic => "caustic",
            Family::Filament => "filament",
        }
    }

    fn named(word: &str) -> Option<Family> {
        [Family::Bioluminescence, Family::Caustic, Family::Filament]
            .into_iter()
            .find(|f| f.word() == word)
    }
}

/// The `#cloud` rows: a family and its chance in percent per locator; what is
/// left of 100 is no cloud.
pub type CloudRows = Vec<(Family, u32)>;

/// Read the `#cloud` section of the table. A row is `family<TAB>chance_pct`;
/// `none` is allowed (it states the rest) and takes no share. An error names
/// its line.
pub fn parse_clouds(text: &str) -> Result<CloudRows, String> {
    let mut rows: CloudRows = Vec::new();
    let mut inside = false;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if line.starts_with('#') {
            // Only a section header switches section; any other `#` line is a comment.
            if matches!(f[0], "#tier" | "#stage" | "#cloud") {
                inside = f[0] == "#cloud";
            }
            continue;
        }
        if !inside {
            continue;
        }
        let bad = |why: &str| format!("abyss-weather line {n}: {why}");
        let [family, pct] = f[..] else {
            return Err(bad("want 2 fields: family and chance_pct"));
        };
        let pct: u32 = pct
            .parse()
            .ok()
            .filter(|p| *p <= 100)
            .ok_or_else(|| bad("the chance is not a whole number of 0 to 100"))?;
        if family == "none" {
            continue;
        }
        let family = Family::named(family).ok_or_else(|| bad("the family is unknown"))?;
        if rows.iter().any(|(f, _)| *f == family) {
            return Err(bad("the family is listed twice"));
        }
        rows.push((family, pct));
    }
    if rows.iter().map(|(_, p)| *p).sum::<u32>() > 100 {
        return Err("abyss-weather: the cloud chances add up to over 100".to_owned());
    }
    Ok(rows)
}

/// The shipped `#cloud` rows, parsed once; a table that does not parse leaves
/// a world with no clouds (and [`shipped`] has already said why).
fn shipped_clouds() -> &'static [(Family, u32)] {
    static ROWS: OnceLock<CloudRows> = OnceLock::new();
    ROWS.get_or_init(|| parse_clouds(SHIPPED).unwrap_or_default())
}

/// The family a locator draws from `seed`: the same for the same seed, `None`
/// for no cloud.
pub fn draw_family(rows: &[(Family, u32)], seed: u64) -> Option<Family> {
    let mut pick = seed % 100;
    for (family, pct) in rows {
        let pct = u64::from(*pct);
        if pick < pct {
            return Some(*family);
        }
        pick -= pct;
    }
    None
}

/// The group-1971 type whose name holds the size word and the family word
/// **[V-data]**: the lowest id when several do.
pub fn cloud_type(reg: &Registry, size: Size, family: Family) -> Option<i64> {
    reg.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .types()
        .filter(|r| r.group_id == GROUP_CLOUD)
        .filter(|r| {
            let name = r.name.to_ascii_lowercase();
            name.starts_with(size.word()) && name.contains(family.word())
        })
        .map(|r| r.type_id)
        .min()
}

/// A cloud type's buffs: `(collection, value)` from its own attribute pairs,
/// the value as the type gives it **[I]**. Empty when it carries none.
pub fn cloud_buffs(row: &crate::typedata::TypeRow) -> Vec<(i64, f64)> {
    BUFF_PAIRS
        .iter()
        .filter_map(|(nid, val)| Some((row.attribute(*nid)? as i64, row.attribute(*val)?)))
        .collect()
}

/// A cloud type's reach in metres **[I]**: [`ATTR_REACH`], else a `radius`
/// over 1 (the bundle's 1 is a placeholder); `None` for neither.
pub fn cloud_reach(row: &crate::typedata::TypeRow) -> Option<f64> {
    row.attribute(ATTR_REACH)
        .or_else(|| (row.radius > 1.0).then_some(row.radius))
        .filter(|r| r.is_finite() && *r > 0.0)
}

/// Whether a type is a cloud that buffs: group 1971, with buff pairs and a
/// reach. Every abyssal room already holds group-1971 objects that are neither
/// (the bundle's "Unstable Abyssal Depths", Tachyon clouds), and those are
/// scenery, not clouds of this module.
fn buffing(row: &crate::typedata::TypeRow) -> bool {
    row.group_id == GROUP_CLOUD && !cloud_buffs(row).is_empty() && cloud_reach(row).is_some()
}

/// Every buffing cloud ball of a site still in space: `(ball id, type,
/// position)`.
fn cloud_balls(reg: &Registry, site_id: i64) -> Vec<(i64, i64, [f64; 3])> {
    let (Some(inst), Some(t)) = (
        reg.ext::<site::Sites>().get(site_id),
        reg.content().try_get::<crate::typedata::Installed>(),
    ) else {
        return Vec::new();
    };
    inst.objects
        .iter()
        .filter(|o| t.table().get(o.type_id).is_some_and(buffing))
        .filter(|o| site::object(reg, o.item_id).is_some())
        .map(|o| (o.item_id, o.type_id, o.position))
        .collect()
}

/// Whether a site holds any buffing cloud ball: the tick's cheap test, no
/// allocation.
fn has_cloud(reg: &Registry, site_id: i64) -> bool {
    let (Some(inst), Some(t)) = (
        reg.ext::<site::Sites>().get(site_id),
        reg.content().try_get::<crate::typedata::Installed>(),
    ) else {
        return false;
    };
    inst.objects.iter().any(|o| {
        t.table().get(o.type_id).is_some_and(buffing) && site::object(reg, o.item_id).is_some()
    })
}

/// **Draw and place the room's clouds**: one per AOE locator of the room's
/// dungeon whose draw from `rows` names a family and whose size has a type.
/// The seed is the run, the room and the locator's own id, so a room made again
/// draws the same clouds. A cloud lands at the locator's place.
pub(crate) fn place_clouds(reg: &mut Registry, run: &Run, p: &Pocket, rows: &[(Family, u32)]) {
    if rows.is_empty() {
        return;
    }
    let spots: Vec<(i64, i64, [f64; 3])> = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&p.dungeon))
        .map(|d| {
            d.objects
                .iter()
                .filter(|o| super::AOE_LOCATORS.contains(&o.type_id))
                .map(|o| (o.id, o.type_id, o.pos))
                .collect()
        })
        .unwrap_or_default();
    let seed = reg.world().seed;
    for (id, locator, at) in spots {
        let Some(size) = locator_size(locator) else {
            continue;
        };
        let draw = crate::seed::mix(seed, &[run.id, p.room as u64, id as u64, CLOUD_STREAM]);
        let Some(family) = draw_family(rows, draw) else {
            continue;
        };
        if let Some(ty) = cloud_type(reg, size, family) {
            site::add_object(reg, p.site_id, ty, at);
        }
    }
}

/// What holds a cloud on a ship: the ship and where it is, for a pilot of the
/// run (or not).
fn ship_of(reg: &Registry, who: i64) -> Option<(i64, Option<i64>)> {
    let at = reg
        .roster()
        .get(who)
        .map(|p| (p.ship_id, p.solar_system_id));
    at.map(|(s, sys)| (s, Some(sys)))
        .or_else(|| reg.character(who).map(|c| (c.ship_id, None)))
}

/// A buffing cloud of a room: its ball (the dbuff source), its reach, where it
/// stands and what it gives.
struct Live {
    ball: i64,
    reach: f64,
    at: [f64; 3],
    buffs: Vec<(i64, f64)>,
}

/// **Hold and release the clouds**, every tick: a pilot's ship within reach of
/// a buffing cloud of its room holds the type's buffs with the cloud's ball id
/// as the source; a ship outside it, or out of the room, lets go. A room with
/// no cloud costs a scan of its objects and allocates nothing.
pub(crate) fn reconcile_clouds(reg: &mut Registry, run: &Run, now: BlueTime) {
    for p in run.pockets.iter().filter(|p| !p.closed) {
        if !has_cloud(reg, p.site_id) {
            continue;
        }
        let clouds: Vec<Live> = {
            let balls = cloud_balls(reg, p.site_id);
            let Some(t) = reg.content().try_get::<crate::typedata::Installed>() else {
                continue;
            };
            balls
                .into_iter()
                .filter_map(|(ball, ty, at)| {
                    let row = t.table().get(ty)?;
                    let buffs = cloud_buffs(row);
                    let reach = cloud_reach(row)?;
                    (!buffs.is_empty()).then_some(Live {
                        ball,
                        reach,
                        at,
                        buffs,
                    })
                })
                .collect()
        };
        if clouds.is_empty() {
            continue;
        }
        for &who in &run.ever {
            let Some((ship, system)) = ship_of(reg, who) else {
                continue;
            };
            let here = run.pilot(who).is_some() && system == Some(p.system);
            let pos = if here {
                reg.advance_character(who);
                reg.roster().get(who).map(|r| r.motion.position)
            } else {
                None
            };
            for Live {
                ball,
                reach,
                at,
                buffs,
            } in &clouds
            {
                let want = pos.is_some_and(|q| crate::space_state::distance(q, *at) <= *reach);
                let holds = crate::dbuff::holds(reg, ship, *ball);
                if want && !holds {
                    crate::dbuff::hold(reg, ship, *ball, buffs, now);
                } else if !want && holds {
                    crate::dbuff::release(reg, ship, *ball, now);
                }
            }
        }
    }
}

/// The cloud type `/abyss cloud` puts down: the first cloud already in the
/// room, else the lowest cloud type that carries buffs, else the lowest
/// group-1971 type named a cloud.
fn gm_cloud_type(reg: &Registry, site_id: i64) -> Option<i64> {
    if let Some((_, ty, _)) = cloud_balls(reg, site_id).first() {
        return Some(*ty);
    }
    let t = reg.content().try_get::<crate::typedata::Installed>()?;
    let clouds = || {
        t.table()
            .types()
            .filter(|r| r.group_id == GROUP_CLOUD && r.name.to_ascii_lowercase().ends_with("cloud"))
    };
    clouds()
        .filter(|r| buffing(r))
        .map(|r| r.type_id)
        .min()
        .or_else(|| clouds().map(|r| r.type_id).min())
}

/// The most buffing clouds a room holds before `/abyss cloud` refuses, so the
/// instance's 1,000-id band cannot fill.
pub const GM_CLOUD_CAP: usize = 12;

/// **`/abyss cloud clear`** (GM): take every buffing cloud of the pilot's room
/// out of space (`RemoveBalls`), and let go of what they held. Answers how many.
pub(crate) fn gm_clear(reg: &mut Registry, actor: i64) -> String {
    let Some(p) = pilot_pocket(reg, actor) else {
        return "you are not in a pocket".to_owned();
    };
    let balls = cloud_balls(reg, p.site_id);
    let now = crate::pyutil::filetime_now();
    for ship in reg.ext::<crate::dbuff::Boosts>().boosted() {
        for (ball, _, _) in &balls {
            crate::dbuff::release(reg, ship, *ball, now);
        }
    }
    let ids: Vec<i64> = balls.iter().map(|(b, _, _)| *b).collect();
    for id in &ids {
        site::destroy_object(reg, *id);
    }
    let to = crate::dungeon::audience(reg, p.system);
    let stamp = reg.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, ids.clone()) {
        reg.schedule(std::time::Duration::ZERO, gone);
    }
    format!("{} cloud(s) removed", ids.len())
}

/// The open pocket the pilot is in.
fn pilot_pocket(reg: &Registry, actor: i64) -> Option<Pocket> {
    let run = super::run_of(reg, actor)?;
    run.pilot(actor)
        .and_then(|pl| run.pocket_of(pl))
        .filter(|p| !p.closed)
        .cloned()
}

/// **`/abyss cloud`** (GM): put a cloud at the pilot's position in the pilot's
/// room. Answers its ball id, its type, how many buffs it carries and its
/// reach, or why not.
pub(crate) fn gm_cloud(reg: &mut Registry, actor: i64) -> String {
    let Some(p) = pilot_pocket(reg, actor) else {
        return "you are not in a pocket".to_owned();
    };
    let site_id = p.site_id;
    if cloud_balls(reg, site_id).len() >= GM_CLOUD_CAP {
        return format!("the room holds {GM_CLOUD_CAP} clouds already; /abyss cloud clear");
    }
    let Some(ty) = gm_cloud_type(reg, site_id) else {
        return "the bundle has no cloud type (group 1971)".to_owned();
    };
    reg.advance_character(actor);
    let Some(at) = reg.roster().get(actor).map(|r| r.motion.position) else {
        return "you are not in space".to_owned();
    };
    let (name, buffs, reach) = reg
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(ty))
        .map_or((String::new(), 0, None), |r| {
            (r.name.clone(), cloud_buffs(r).len(), cloud_reach(r))
        });
    match site::add_object(reg, site_id, ty, at) {
        Some(b) => format!(
            "cloud {} type {ty} ({name}) buffs: {buffs} reach: {} m",
            b.item_id,
            reach.map_or_else(|| "none".to_owned(), |r| format!("{r:.0}"))
        ),
        None => "the room is full".to_owned(),
    }
}

#[cfg(test)]
mod tests;
