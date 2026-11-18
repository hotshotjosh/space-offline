//! The item traders of the FW Conquests (CQ-2, issue #291): four "Path to
//! Conquest" traders at the War HQ systems, where a pilot turns in what the
//! objectives collect and takes a token, and spends tokens on the trader's
//! own recipes. `specs/conquests.md` §2.2 and §8 (CQ-2).
//!
//! **The call.** `beyonce.CallComponentFromClient(itemID, ITEM_TRADER,
//! 'ProcessTrade', recipeID=, multiplier=)` with **keyword arguments**, and the
//! client unpacks `(success, items)` [V `itemTraderNew.py` `ProcessTrade`]. A
//! refusal is `(False, [])`, not a `Raise`: the client reads only `success` and
//! shows `ItemTradeFailed` itself [V `confirmWnd.py` `OnClickConfirm`]. The
//! router's own refusals (the pilot is not in space, the trader is not in the
//! pilot's system) stay [`crate::space_component::not_in_space`].
//!
//! **The data.** `conquest/traders.tsv` and `conquest/recipes.tsv` of the
//! bundle (CQ-1, `docs/content/schema.md`) are generic: they list every trader
//! of group 5254 and every recipe it names. [`Traders`] reads them whole; only
//! the traders whose faction has a War HQ in the authored
//! `conquest/season.tsv` are *placed* ([`placed`]), which today are the four PTC
//! traders. A bundle without the files loads as no traders.
//!
//! **Placement [A].** A trader stands at the faction's War HQ system, at the
//! Navy anchorage station's position plus the trader object's offset in its
//! dungeon room (`traders.tsv` `x y z`). Falsifier: a TQ bookmark distance
//! between the trader and the station. A trader type in two dungeons (Amarr's
//! 14047 and 14412) is placed once, at the **lowest dungeon id [A]**;
//! falsifier: a TQ bookmark distance to the other one. The Beacons beside the
//! traders (97927-97930) are not placed yet.
//! A War HQ station that is not a celestial of its HQ system is skipped and
//! logged once, never placed at the star.
//!
//! **The ball.** One static ball per placed trader, global in its system like
//! the skyhook (`balls_in`, one line in `destiny::scene`), with the trader's
//! type in its slim. Global **[V]**: the client's component data has
//! `isAlwaysGlobal = 1` for these traders. Its id
//! is `TRADER_ID_BASE + type_id` ([A], clear of every other band; a test lists
//! the neighbours).
//!
//! **Who may trade [A].** Militia only: a pilot trades at a trader whose
//! faction is the militia they serve (`facwar_mgr::my_faction`). This is the
//! recommended option of the open owner question Q19 (`season.tsv`
//! `trader_access`); `anyone` opens it. Falsifier: TQ traders serving anyone,
//! or the owner ruling otherwise.
//!
//! **The trade** ([`process_trade`]): locate the trader, check the pilot is
//! within its interaction range, the recipe is one of this trader's, the
//! multiplier is in `1..=cap` ([A] 1000, `season.tsv` `trader_multiplier_cap`),
//! then take `multiplier` times the inputs over the three cargo flags the
//! client's window reads (cargo, general mining hold, specialised ice hold [V
//! `itemTraderController.validCargoFlags`]) and the ISK, and create the outputs
//! in the cargo hold. All or nothing. The ISK leaves with a journal row of
//! [`REF_ITEM_TRADER_PAYMENT`] [V]. Last, [`crate::item_trader_hooks`]
//! hears the trade.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::item::{Edit, Slot};
use crate::service::{Registry, ServiceError};
use crate::space_component::CompCall;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The component name **[V]**: `componentConst.ITEM_TRADER` (`componentConst.py:38`,
/// the `COMPONENTS` of `spacecomponents/client/factory.py`).
pub const ITEM_TRADER: &str = "itemTrader";

/// The trader's ball id is `TRADER_ID_BASE + type_id` **[A]**: above the ESS
/// band (`ess::ESS_ID_BASE` plus a system id) and below the survey probes
/// (`scan::PROBE_ID_BASE`). Falsifier: another band using the same ids (the
/// test lists the bases).
pub const TRADER_ID_BASE: i64 = 3_200_000_000_000;

/// Type ids a trader id can name: the type table's range, with room.
const TYPE_ID_END: i64 = 1_000_000_000;

/// One past the last trader ball id.
pub const TRADER_ID_END: i64 = TRADER_ID_BASE + TYPE_ID_END;

const _: () = assert!(crate::ess::ESS_ID_BASE + crate::ess::SYSTEM_ID_END <= TRADER_ID_BASE);
const _: () = assert!(TRADER_ID_END <= crate::scan::PROBE_ID_BASE);

/// The cargo flags the trader window reads **[V]**
/// (`itemTraderController.validCargoFlags`): `flagCargo`,
/// `flagGeneralMiningHold`, `flagSpecializedIceHold`.
pub const CARGO_FLAGS: [i64; 3] = [
    crate::sov_hub::FLAG_CARGO,
    crate::mining_hold::FLAG_GENERAL_MINING_HOLD,
    FLAG_SPECIALIZED_ICE_HOLD,
];

/// `const.flagSpecializedIceHold` **[V]** (the ice hold of `item::HOLDS`,
/// flag 181; `compression::ORE_HOLD_FLAGS` names it the same).
pub const FLAG_SPECIALIZED_ICE_HOLD: i64 = 181;

/// The journal entry type of the ISK a trade costs **[V]**:
/// `appConst.refItemTraderPayment` = 142 (the client's `appConst`, read in
/// the review of CQ-2).
pub const REF_ITEM_TRADER_PAYMENT: i64 = 142;

/// The multiplier cap when `season.tsv` has no row **[A]** (spec A12).
pub const DEFAULT_CAP: i64 = 1000;

/// The ball's radius when the type table has none. **[A]**
pub const DEFAULT_RADIUS: f64 = 100.0;

/// The bundle files and their headers (`docs/content/schema.md`).
pub const TRADERS: (&str, &str) = (
    "conquest/traders.tsv",
    "#type_id\tfaction_id\tdungeon_id\tinteraction_range\tx\ty\tz",
);
pub const RECIPES: (&str, &str) = (
    "conquest/recipes.tsv",
    "#recipe_id\ttrader_type_id\tgroup\tside\ttype_id\tqty\tinput_isk",
);

/// The authored season table (`content/conquest/season.tsv`).
pub const SEASON_FILE: &str = "conquest/season.tsv";
pub const SEASON_HEADER: &str = "key\tfaction_id\tkind\tvalue\twhy";
const SEASON_SHIPPED: &str = include_str!("../../../../content/conquest/season.tsv");

/// Registration row for `content_tables::TABLES` (coordinator-only).
pub const TABLE: TableSpec = TableSpec {
    name: "conquest item traders",
    load: load_table,
    fixture: || Box::new(Traders::default()) as Boxed,
};

fn load_table(src: &Sources) -> Result<Boxed, String> {
    let season = {
        let file = src.authored(SEASON_FILE, SEASON_SHIPPED, SEASON_HEADER)?;
        let (rows, _) = file.parse_rows_logged(parse_season_row);
        Season::from_rows(rows)
    };
    let mut table = match src.bundle {
        Some(b) => Traders::from_bundle(b)?,
        None => Traders::default(),
    };
    table.season = season;
    Ok(Box::new(table))
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// One `traders.tsv` row: a trader type in one dungeon.
#[derive(Debug, Clone, PartialEq)]
pub struct TraderRow {
    pub type_id: i64,
    pub faction: i64,
    pub dungeon: i64,
    /// The component's `interactionRange`, metres.
    pub range: f64,
    /// The trader object's position in the dungeon room.
    pub offset: [f64; 3],
}

/// One recipe of one trader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub id: i64,
    pub trader_type: i64,
    pub group: String,
    /// `(type, qty)` taken.
    pub inputs: Vec<(i64, i64)>,
    /// `(type, qty)` given.
    pub outputs: Vec<(i64, i64)>,
    /// ISK taken.
    pub isk: i64,
}

/// Who may trade **[A]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Only a pilot who serves the trader's own militia.
    Militia,
    /// Anyone.
    Anyone,
}

/// The authored numbers this module reads from `season.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Season {
    pub access: Access,
    pub cap: i64,
    /// `faction -> (War HQ system, anchorage station)`.
    pub hq: BTreeMap<i64, (i64, i64)>,
}

impl Default for Season {
    fn default() -> Self {
        Season {
            access: Access::Militia,
            cap: DEFAULT_CAP,
            hq: BTreeMap::new(),
        }
    }
}

/// One parsed `season.tsv` row this module cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeasonRow {
    Access(Access),
    Cap(i64),
    HqSystem(i64, i64),
    HqStation(i64, i64),
    Other,
}

/// A `season.tsv` row: `key faction_id kind value why`.
pub fn parse_season_row(r: &crate::authored::Row) -> Result<SeasonRow, String> {
    Ok(match r.field(0) {
        "trader_access" => SeasonRow::Access(match r.field(3) {
            "militia" => Access::Militia,
            "anyone" => Access::Anyone,
            other => return Err(format!("trader_access {other:?} is not militia or anyone")),
        }),
        "trader_multiplier_cap" => {
            let n = r.i64(3)?;
            if n < 1 {
                return Err("trader_multiplier_cap is below 1".to_owned());
            }
            SeasonRow::Cap(n)
        }
        "war_hq_system" => SeasonRow::HqSystem(r.i64(1)?, r.i64(3)?),
        "war_hq_station" => SeasonRow::HqStation(r.i64(1)?, r.i64(3)?),
        _ => SeasonRow::Other,
    })
}

impl Season {
    /// The numbers from the parsed rows; a faction needs both an HQ system and
    /// a station to be placed.
    pub fn from_rows(rows: Vec<SeasonRow>) -> Season {
        let mut s = Season::default();
        let mut systems = BTreeMap::new();
        let mut stations = BTreeMap::new();
        for r in rows {
            match r {
                SeasonRow::Access(a) => s.access = a,
                SeasonRow::Cap(n) => s.cap = n,
                SeasonRow::HqSystem(f, v) => {
                    systems.insert(f, v);
                }
                SeasonRow::HqStation(f, v) => {
                    stations.insert(f, v);
                }
                SeasonRow::Other => {}
            }
        }
        for (f, sys) in systems {
            if let Some(st) = stations.get(&f) {
                s.hq.insert(f, (sys, *st));
            }
        }
        s
    }
}

/// The tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Traders {
    rows: Vec<TraderRow>,
    /// `(trader type, recipe id) -> recipe`.
    recipes: BTreeMap<(i64, i64), Recipe>,
    pub season: Season,
}

fn num<T: std::str::FromStr>(file: &str, n: usize, line: &str, s: &str) -> Result<T, String> {
    s.parse()
        .map_err(|_| format!("{file}: data line {n}: {s:?} is not a number: {line:?}"))
}

impl Traders {
    /// The files from a checked bundle; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Traders, String> {
        let mut t = Traders::parse(
            &b.data_lines(TRADERS.0, TRADERS.1)?,
            &b.data_lines(RECIPES.0, RECIPES.1)?,
        )?;
        t.season = Season::default();
        Ok(t)
    }

    /// From each file's data lines. A short row, a bad number or side, or a
    /// non-positive quantity is an error naming the line.
    pub fn parse<S: AsRef<str>>(traders: &[S], recipes: &[S]) -> Result<Traders, String> {
        let mut t = Traders::default();
        for (i, line) in traders.iter().enumerate() {
            let (n, line) = (i + 1, line.as_ref());
            let f: Vec<&str> = line.split('\t').collect();
            let [ty, fac, dun, range, x, y, z] = f[..] else {
                return Err(format!(
                    "{}: data line {n}: want 7 columns: {line:?}",
                    TRADERS.0
                ));
            };
            t.rows.push(TraderRow {
                type_id: num(TRADERS.0, n, line, ty)?,
                faction: num(TRADERS.0, n, line, fac)?,
                dungeon: num(TRADERS.0, n, line, dun)?,
                range: num(TRADERS.0, n, line, range)?,
                offset: [
                    num(TRADERS.0, n, line, x)?,
                    num(TRADERS.0, n, line, y)?,
                    num(TRADERS.0, n, line, z)?,
                ],
            });
        }
        for (i, line) in recipes.iter().enumerate() {
            let (n, line) = (i + 1, line.as_ref());
            let f: Vec<&str> = line.split('\t').collect();
            let [rid, trader, group, side, ty, qty, isk] = f[..] else {
                return Err(format!(
                    "{}: data line {n}: want 7 columns: {line:?}",
                    RECIPES.0
                ));
            };
            let (rid, trader): (i64, i64) = (
                num(RECIPES.0, n, line, rid)?,
                num(RECIPES.0, n, line, trader)?,
            );
            let (ty, qty): (i64, i64) =
                (num(RECIPES.0, n, line, ty)?, num(RECIPES.0, n, line, qty)?);
            if qty < 1 {
                return Err(format!(
                    "{}: data line {n}: quantity below 1: {line:?}",
                    RECIPES.0
                ));
            }
            let r = t.recipes.entry((trader, rid)).or_insert_with(|| Recipe {
                id: rid,
                trader_type: trader,
                group: group.to_owned(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                isk: 0,
            });
            r.isk = num(RECIPES.0, n, line, isk)?;
            match side {
                "in" => r.inputs.push((ty, qty)),
                "out" => r.outputs.push((ty, qty)),
                other => {
                    return Err(format!(
                        "{}: data line {n}: side {other:?} is not in or out: {line:?}",
                        RECIPES.0
                    ));
                }
            }
        }
        Ok(t)
    }

    /// Every trader row, in file order.
    pub fn rows(&self) -> &[TraderRow] {
        &self.rows
    }

    /// `trader_type`'s recipe `id`.
    pub fn recipe(&self, trader_type: i64, id: i64) -> Option<&Recipe> {
        self.recipes.get(&(trader_type, id))
    }

    /// How many recipes there are.
    pub fn recipe_count(&self) -> usize {
        self.recipes.len()
    }
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// A trader in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The ball id ([`trader_id`]).
    pub id: i64,
    pub type_id: i64,
    pub faction: i64,
    pub system: i64,
    pub station: i64,
    pub range: f64,
    pub offset: [f64; 3],
}

/// The ball id of the trader of `type_id`.
pub fn trader_id(type_id: i64) -> i64 {
    TRADER_ID_BASE + type_id
}

fn type_of_id(id: i64) -> Option<i64> {
    let t = id.checked_sub(TRADER_ID_BASE)?;
    (1..TYPE_ID_END).contains(&t).then_some(t)
}

/// The placed traders: for each trader type, its lowest-numbered dungeon's
/// row, when the row's faction has a War HQ in the season table.
pub fn placed(t: &Traders) -> Vec<Placed> {
    let mut by_type: BTreeMap<i64, &TraderRow> = BTreeMap::new();
    for r in &t.rows {
        if !t.season.hq.contains_key(&r.faction) || !(1..TYPE_ID_END).contains(&r.type_id) {
            continue;
        }
        let e = by_type.entry(r.type_id).or_insert(r);
        if r.dungeon < e.dungeon {
            *e = r;
        }
    }
    by_type
        .into_values()
        .filter_map(|r| {
            let (system, station) = *t.season.hq.get(&r.faction)?;
            Some(Placed {
                id: trader_id(r.type_id),
                type_id: r.type_id,
                faction: r.faction,
                system,
                station,
                range: r.range,
                offset: r.offset,
            })
        })
        .collect()
}

fn table(w: &Registry) -> Option<&Traders> {
    w.content().try_get::<Traders>()
}

/// [`placed`], less the traders whose anchorage station is not a celestial of
/// their HQ system (logged once): a station the catalogue lacks would put the
/// trader at the origin, which is the star.
fn placed_in(w: &Registry) -> Vec<Placed> {
    let Some(t) = table(w) else {
        return Vec::new();
    };
    placed(t)
        .into_iter()
        .filter(|p| {
            let ok = w.celestials().system_of(p.station) == Some(p.system);
            if !ok && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::outln!(
                    "[item_trader] trader {} skipped: war_hq_station {} is not a celestial of system {}",
                    p.type_id,
                    p.station,
                    p.system
                );
            }
            ok
        })
        .collect()
}

static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn find(w: &Registry, id: i64) -> Option<Placed> {
    let ty = type_of_id(id)?;
    placed_in(w).into_iter().find(|p| p.type_id == ty)
}

/// Where the trader stands **[A]**: its anchorage station plus the object's
/// offset in the dungeon room.
pub fn position(w: &Registry, p: &Placed) -> [f64; 3] {
    let at = w.station_position(p.station);
    [
        at[0] + p.offset[0],
        at[1] + p.offset[1],
        at[2] + p.offset[2],
    ]
}

fn radius(w: &Registry, type_id: i64) -> f64 {
    crate::typedata::installed_in(w.content())
        .table()
        .get(type_id)
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(DEFAULT_RADIUS)
}

/// The system trader `item` is in, for the router's locate (`CALLS`).
pub fn locate(w: &Registry, item: i64) -> Option<i64> {
    find(w, item).map(|p| p.system)
}

/// `system`'s trader balls and slims, for `destiny::scene` (coordinator: one
/// loop there). Global: in no `grid::local_balls`.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    placed_in(w)
        .into_iter()
        .filter(|p| p.system == system)
        .map(|p| {
            let ball = Ball {
                ball_id: p.id,
                mode: Mode::Stop,
                radius: radius(w, p.type_id) as f32,
                position: position(w, &p),
                flags: FLAGS_STATIC_BALL,
                mass: 0.0,
                cloak: 0,
                harmonic: -1,
                corporation_id: i32::try_from(p.faction).unwrap_or(-1),
                alliance_id: -1,
                physics: None,
                formation_id: NO_FORMATION,
            };
            let slim = Slim {
                item_id: p.id,
                type_id: p.type_id,
                owner_id: p.faction,
                char_id: None,
                corp_id: Some(p.faction),
                response_thresholds: None,
            };
            (ball, slim)
        })
        .collect()
}

/// Where trader `ball_id` is, for `Registry::ball_target`: approach, orbit,
/// warp to. **[A]** no warp-in point (a small object, not a 90 km body).
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let p = find(w, ball_id)?;
    Some(crate::model::Target {
        position: position(w, &p),
        radius: radius(w, p.type_id),
        warp_in: None,
    })
}

// ---------------------------------------------------------------------------
// The trade
// ---------------------------------------------------------------------------

/// `(False, [])`: a refused trade.
fn refused() -> PyValue {
    PyValue::Tuple(vec![PyValue::Bool(false), PyValue::List(Vec::new())])
}

/// Why a trade did not happen. Only a test reads it; the client is told
/// `(False, [])` whatever the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum No {
    NoSuchTrader,
    NotInSpace,
    NotYourMilitia,
    OutOfRange,
    NoSuchRecipe,
    BadMultiplier,
    Short,
    NoIsk,
    NoRoom,
    Failed,
}

/// What a good trade gave: `(type, qty)` per output, in recipe order.
pub type Gave = Vec<(i64, i64)>;

/// `ProcessTrade(recipeID=, multiplier=)` through `CallComponentFromClient`:
/// `(success, items)`.
pub fn process_trade(reg: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
    let (Some(recipe), Some(multiplier)) = (c.args.int_kw("recipeID"), c.args.int_kw("multiplier"))
    else {
        return Ok(refused());
    };
    Ok(match trade(reg, c.actor, c.item_id, recipe, multiplier) {
        Ok(gave) => PyValue::Tuple(vec![
            PyValue::Bool(true),
            PyValue::List(
                gave.into_iter()
                    .map(|(t, q)| PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(q)]))
                    .collect(),
            ),
        ]),
        Err(_) => refused(),
    })
}

/// The trade itself: all or nothing.
pub fn trade(
    reg: &mut Registry,
    actor: i64,
    trader: i64,
    recipe_id: i64,
    multiplier: i64,
) -> Result<Gave, No> {
    let p = find(reg, trader).ok_or(No::NoSuchTrader)?;
    let t = table(reg).ok_or(No::NoSuchTrader)?;
    let (access, cap) = (t.season.access, t.season.cap);
    // 2: one of this trader's recipes.
    let recipe = t
        .recipe(p.type_id, recipe_id)
        .cloned()
        .ok_or(No::NoSuchRecipe)?;
    // 3: the multiplier.
    if !(1..=cap).contains(&multiplier) {
        return Err(No::BadMultiplier);
    }
    if access == Access::Militia
        && crate::service::facwar_mgr::my_faction(reg, actor, crate::pyutil::filetime_now())
            != Some(p.faction)
    {
        return Err(No::NotYourMilitia);
    }
    let ship = reg
        .character(actor)
        .map(|ch| ch.ship_id)
        .ok_or(No::NotInSpace)?;
    // 1: in range of the trader's surface.
    reg.advance_character(actor);
    let here = reg.roster().get(actor).copied().ok_or(No::NotInSpace)?;
    if here.solar_system_id != p.system {
        return Err(No::NotInSpace);
    }
    let gap = crate::space_state::distance(here.motion.position, position(reg, &p))
        - radius(reg, p.type_id);
    if gap > p.range {
        return Err(No::OutOfRange);
    }
    // 4: the inputs over the three flags, lowest item id first.
    let mut edits = Vec::new();
    let mut freed_cargo = 0.0;
    for &(type_id, each) in &recipe.inputs {
        let mut need = each.checked_mul(multiplier).ok_or(No::Short)?;
        let mut stacks: Vec<(i64, i64, i64)> = reg
            .store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == type_id
                    && !i.singleton
                    && i.owner_id == actor
                    && i.location_id == ship
                    && CARGO_FLAGS.contains(&i.flag_id)
            })
            .map(|i| (i.item_id, i.quantity, i.flag_id))
            .collect();
        stacks.sort_unstable();
        let unit = crate::item::find_in(reg.content(), type_id).map_or(0.0, |t| t.volume);
        for (item_id, have, flag) in stacks {
            if need == 0 {
                break;
            }
            let take = have.min(need);
            edits.push(if take == have {
                Edit::Destroy { item_id }
            } else {
                Edit::Consume {
                    item_id,
                    quantity: take,
                }
            });
            if flag == crate::sov_hub::FLAG_CARGO {
                freed_cargo += unit * take as f64;
            }
            need -= take;
        }
        if need > 0 {
            return Err(No::Short);
        }
    }
    // 5: the ISK, then room for the outputs.
    let isk = recipe.isk.checked_mul(multiplier).ok_or(No::NoIsk)?;
    if isk > 0
        && reg
            .character(actor)
            .is_none_or(|ch| ch.balance < isk as f64)
    {
        return Err(No::NoIsk);
    }
    let mut gave = Gave::new();
    let mut out_volume = 0.0;
    for &(type_id, each) in &recipe.outputs {
        let qty = each.checked_mul(multiplier).ok_or(No::Failed)?;
        out_volume +=
            crate::item::find_in(reg.content(), type_id).map_or(0.0, |t| t.volume) * qty as f64;
        edits.push(Edit::Create {
            type_id,
            quantity: qty,
            to: Slot::new(actor, ship, crate::sov_hub::FLAG_CARGO),
        });
        gave.push((type_id, qty));
    }
    if out_volume > crate::fitting::cargo_free_in(reg, ship) + freed_cargo + 1e-9 {
        return Err(No::NoRoom);
    }
    if !edits.is_empty() && reg.apply_items(&edits).is_err() {
        return Err(No::Failed);
    }
    if isk > 0 {
        reg.charge(
            actor,
            isk as f64,
            REF_ITEM_TRADER_PAYMENT,
            p.faction,
            recipe_id,
        );
    }
    crate::item_trader_hooks::on_trade(reg, actor, trader, recipe_id, multiplier);
    Ok(gave)
}

#[cfg(test)]
mod tests;
