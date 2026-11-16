//! **A bot capital sieges and triages in a fight** (issue #34, CAP-SIEGE-BOT).
//!
//! A capital pilot ([`crate::capital`]) that flies a dreadnought or a force
//! auxiliary it owns carries a siege or a triage module. The server side is
//! built ([`eve_server::module_fuel`]): the module burns fuel each cycle
//! (`consumptionType` 713, `consumptionQuantity` 714 on the module type),
//! holds the ship still and refuses an early stop. This module is the bot's
//! half: the pure rule [`decide`] (start, stop or keep), the reader of the
//! fitted module ([`find`], from the type table at run time: no type id is
//! written here) and the executor [`tend`] that the pilot's turn calls.
//!
//! **The rule**: run the module only while the pilot is on the grid of a fight
//! ([`Scene::fight`]), holding still, with no warp-out due within one cycle
//! (a sieged ship cannot jump or dock until its cycle ends), and with a cycle's
//! fuel in the bay or hold. Stop it the moment the fight is over or a warp-out
//! comes due; a running module with no fuel left is simply kept (it ends by
//! itself at the next cycle; the server refuses an early stop anyway).
//!
//! **Nothing from nowhere**: the fuel is only what the pilot bought
//! ([`crate::capital::stock_bay`]: the hangar's own stacks first, then the
//! market, inside the wallet budget) or owns. Nothing here grants anything.
//!
//! **[A]** "on a fight grid" is: an NPC or a human outside the fleet is in the
//! park. Falsified by a pilot that sieges next to a passing hauler (tighten to
//! "someone is shooting a fleet ship") or one that never sieges in a real fight.
//! **[A]** a warp-out is due when the partner's cyno is lit (the pair's next
//! leg is the jump out). Falsified by a pilot that sieges and then cannot take
//! its own jump.

use crate::clock;
use crate::verbs::{ItemRow, LockState};
use crate::{Conn, Error, Result};
use eve_server::module_fuel::{EFFECT_SIEGE, EFFECT_TRIAGE};
use std::time::Duration;

/// The module type's fuel type and per-cycle quantity (`consumptionType`,
/// `consumptionQuantity`), as the server reads them.
pub const ATTR_FUEL_TYPE: i64 = eve_server::cyno::ATTR_FUEL_TYPE;
pub const ATTR_FUEL_QUANTITY: i64 = eve_server::cyno::ATTR_FUEL_QUANTITY;

/// `repeat` for a module that runs until stopped (as the cyno's).
pub const REPEAT: i64 = crate::capital::REPEAT;

/// The cycle assumed when the type table names no duration **[A]** (a siege
/// cycle is five minutes). Falsified by a module that cycles faster: the pilot
/// then keeps a wider berth than needed, never a narrower one.
pub const DEFAULT_CYCLE: Duration = Duration::from_secs(300);

/// Cycles of fuel a docked pilot stocks for the module **[A]**. Falsified by
/// fights that outlast it (raise) or fuel left over after each (lower).
pub const FUEL_CYCLES: i64 = 6;

/// How long a docked pilot waits after a stocking that moved nothing **[A]**.
/// Falsified by a market that restocks faster (lower) or a log full of empty asks
/// (raise).
pub const STOCK_BACKOFF: Duration = Duration::from_secs(600);

/// Whether a stocking did anything: the turn is spent only if it did.
pub fn moved_any(bought: i64, bayed: i64, held: i64) -> bool {
    bought + bayed + held > 0
}

/// What the pilot saw of the grid this turn (the brain's observation, kept in
/// [`crate::capital::Mind`] by the capital turn).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// NPCs on the grid.
    pub hostiles: usize,
    /// Humans outside the bot's fleet on the grid.
    pub humans: usize,
    /// Who is on the grid: `(ball, surface distance, may be shot)`. Empty in
    /// high sec and where the security is not known ([`pick_target`]).
    pub targets: Vec<(i64, f64, bool)>,
    /// The ship is warping (not holding still).
    pub warping: bool,
}

impl Scene {
    /// Whether this is the grid of a fight **[A]**.
    pub fn fight(&self) -> bool {
        self.hostiles + self.humans > 0
    }
}

/// A fitted siege or triage module, read off the type table.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub item_id: i64,
    pub type_id: i64,
    pub effect_id: i64,
    pub effect_name: String,
    /// The fuel type, if the module burns any.
    pub fuel: Option<i64>,
    /// Units per cycle (0 when it burns none).
    pub per_cycle: i64,
    pub cycle: Duration,
}

/// The first fitted module whose type carries the siege or triage effect.
pub fn find(content: &eve_server::content::Content, fitted: &[ItemRow]) -> Option<Module> {
    let installed = eve_server::typedata::installed_in(content);
    let table = installed.table();
    fitted.iter().find_map(|r| {
        let ty = table.get(r.type_id)?;
        let effect_id = [EFFECT_SIEGE, EFFECT_TRIAGE]
            .into_iter()
            .find(|e| ty.has_effect(*e))?;
        let effect = table.effect(effect_id)?;
        let attr = |a| eve_server::typedata::attribute_in(content, r.type_id, a);
        let fuel = attr(ATTR_FUEL_TYPE)
            .map(|v| v.round() as i64)
            .filter(|t| *t > 0);
        let per_cycle = match fuel {
            Some(_) => attr(ATTR_FUEL_QUANTITY)
                .map(|v| v.round() as i64)
                .filter(|q| *q > 0)
                .unwrap_or(1),
            None => 0,
        };
        let cycle = (effect.duration_attribute > 0)
            .then(|| attr(effect.duration_attribute))
            .flatten()
            .filter(|ms| *ms > 0.0)
            .map_or(DEFAULT_CYCLE, |ms| Duration::from_secs_f64(ms / 1000.0));
        Some(Module {
            item_id: r.item_id,
            type_id: r.type_id,
            effect_id,
            effect_name: effect.name.clone(),
            fuel,
            per_cycle,
            cycle,
        })
    })
}

/// What the rule sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facts {
    pub fight: bool,
    /// Holding still: not warping.
    pub holding: bool,
    /// The module is running now.
    pub running: bool,
    /// How soon the pilot means to leave, if it does.
    pub warp_out_in: Option<Duration>,
    /// The module's cycle.
    pub cycle: Duration,
    /// Fuel held (bay and hold) and burnt per cycle.
    pub fuel: i64,
    pub per_cycle: i64,
}

/// What to do with the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Start,
    Stop,
    /// Leave it as it is: why.
    Keep(&'static str),
}

/// **The rule** (pure). See the module header.
pub fn decide(f: &Facts) -> Step {
    let leaving = f.warp_out_in.is_some_and(|d| d <= f.cycle);
    if f.running {
        return if !f.fight || leaving {
            Step::Stop
        } else {
            Step::Keep("running")
        };
    }
    if !f.fight {
        return Step::Keep("no fight here");
    }
    if !f.holding {
        return Step::Keep("not holding still");
    }
    if leaving {
        return Step::Keep("a warp-out is due within the cycle");
    }
    if f.fuel < f.per_cycle {
        return Step::Keep("no fuel for a cycle");
    }
    Step::Start
}

/// The fuel to buy so the bay holds [`FUEL_CYCLES`] cycles: what `have` lacks.
pub fn fuel_to_stock(m: &Module, have: i64) -> i64 {
    (m.per_cycle * FUEL_CYCLES - have).max(0)
}

/// **Tend the module** in space: read the facts, take [`decide`], do it.
/// Returns a note when it acted or has something to say, `None` when the ship
/// carries no siege or triage module. A refusal is a note; only a dead
/// connection is the error.
pub fn tend(
    c: &mut Conn,
    content: &eve_server::content::Content,
    ship: i64,
    scene: &Scene,
    warp_out_in: Option<Duration>,
    cache: &mut Option<Module>,
) -> Result<Option<String>> {
    // The module is read once and kept (L1); `stock` refreshes it docked.
    let m = match cache {
        Some(m) => m.clone(),
        None => match find(content, &c.fitted()?) {
            Some(m) => cache.insert(m).clone(),
            None => return Ok(None),
        },
    };
    let running = c.active_effects(m.item_id)?.contains(&m.effect_id);
    let fuel = match m.fuel {
        Some(t) => {
            crate::capital::held_at(c, ship, crate::capital::FUEL_BAY, t)?
                + crate::capital::held_at(c, ship, crate::capital::CARGO, t)?
        }
        None => 0,
    };
    let step = decide(&Facts {
        fight: scene.fight(),
        holding: !scene.warping,
        running,
        warp_out_in,
        cycle: m.cycle,
        fuel,
        per_cycle: m.per_cycle,
    });
    let outcome = match step {
        Step::Keep(_) => return Ok(None),
        Step::Start => c
            .activate(m.item_id, (&m.effect_name, m.effect_id), None, REPEAT)
            .map(|_| "on"),
        Step::Stop => c.deactivate(m.item_id, &m.effect_name).map(|_| "off"),
    };
    match outcome {
        Ok("on") if m.effect_id == EFFECT_SIEGE => {
            let after = match m.fuel {
                Some(t) => {
                    crate::capital::held_at(c, ship, crate::capital::FUEL_BAY, t)?
                        + crate::capital::held_at(c, ship, crate::capital::CARGO, t)?
                }
                None => 0,
            };
            Ok(Some(format!("capital: siege on, fuel {fuel} -> {after}")))
        }
        Ok(what) => Ok(Some(format!("capital: {} {what}", m.effect_name))),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Some(format!("capital: {} refused: {e}", m.effect_name))),
    }
}

/// **The nearest target** (pure): the nearest of `cands` that may be shot, the
/// lower ball id on a tie. `cands` is `(ball, distance, may be shot)`.
/// **[A]** "may be shot" is: an NPC on the grid, or a human the bot's own
/// hostile rules engage outside high sec ([`crate::hostile::Rules::prey`]'s
/// filter); the scene is empty in high sec. Falsified by a pilot that shoots a
/// corp mate, a fleet mate, or anything in a high-sec system.
pub fn pick_target(cands: &[(i64, f64, bool)]) -> Option<i64> {
    cands
        .iter()
        .filter(|c| c.2)
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|c| c.0)
}

/// What the pilot's guns remember: read once, and what they are shooting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guns {
    /// `(item, type)` of each fitted gun or launcher, once read.
    pub guns: Option<Vec<(i64, i64)>>,
    /// The target they were last put on.
    pub target: Option<i64>,
}

/// What this turn does about the target (pure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    /// Nothing may be shot: stop the guns.
    Idle,
    /// The same target as before.
    Keep(i64),
    /// A new target; `old` is the lock to drop.
    Switch { old: Option<i64>, new: i64 },
}

/// **Keep the target** while it may still be shot (no flap between two
/// near-equal distances); else the nearest ([`pick_target`]).
pub fn aim(cands: &[(i64, f64, bool)], current: Option<i64>) -> Aim {
    if let Some(cur) = current.filter(|&t| cands.iter().any(|c| c.0 == t && c.2)) {
        return Aim::Keep(cur);
    }
    match pick_target(cands) {
        Some(new) => Aim::Switch { old: current, new },
        None => Aim::Idle,
    }
}

/// Whether a lock lets the guns fire now: a lock still pending does not.
pub fn lock_ready(state: &LockState) -> bool {
    matches!(state, LockState::Locked)
}

/// Whether a module's effect is among its running ones.
pub fn is_running(active: &[i64], effect: i64) -> bool {
    active.contains(&effect)
}

/// Stop every gun that is running ([`Conn::deactivate`]).
fn cease(c: &mut Conn, content: &eve_server::content::Content, guns: &[(i64, i64)]) -> Result<()> {
    for &(item, ty) in guns {
        let Some((name, effect)) = crate::verbs::weapon_effect(Some(content), ty) else {
            continue;
        };
        if is_running(&c.active_effects(item)?, effect) {
            match c.deactivate(item, name) {
                Ok(_) => {}
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(_) => {}
            }
        }
    }
    Ok(())
}

/// **Shoot the nearest target** with every fitted gun, only on a fight grid.
/// Keeps its target while it may still be shot, else locks the nearest
/// ([`Conn::lock_target`]; the old lock is dropped) and fires each turret or
/// launcher that is not already running ([`Conn::fire`]); with nothing to shoot
/// it stops its guns. A lock still pending fires nothing yet. A refusal is not
/// an error; only a dead connection is. Returns the note
/// `capital: firing {n} guns at {target}` when it fired.
pub fn shoot(
    c: &mut Conn,
    content: &eve_server::content::Content,
    scene: &Scene,
    cache: &mut Guns,
) -> Result<Option<String>> {
    let cands: &[(i64, f64, bool)] = if scene.fight() { &scene.targets } else { &[] };
    let target = match aim(cands, cache.target) {
        Aim::Idle => {
            cache.target = None;
            if let Some(guns) = cache.guns.clone() {
                cease(c, content, &guns)?;
            }
            return Ok(None);
        }
        Aim::Keep(t) => t,
        Aim::Switch { old, new } => {
            if let Some(guns) = cache.guns.clone() {
                cease(c, content, &guns)?;
            }
            if let Some(old) = old
                && let Err(e @ (Error::Io(_) | Error::Protocol(_))) = c.unlock_target(old)
            {
                return Err(e);
            }
            cache.target = None;
            new
        }
    };
    if cache.guns.is_none() {
        let guns: Vec<(i64, i64)> = c
            .fitted()?
            .iter()
            .filter(|r| crate::verbs::weapon_effect(Some(content), r.type_id).is_some())
            .map(|r| (r.item_id, r.type_id))
            .collect();
        cache.guns = Some(guns);
    }
    let guns = cache.guns.clone().unwrap_or_default();
    if guns.is_empty() {
        return Ok(None);
    }
    match c.lock_target(target) {
        Ok(state) => {
            cache.target = Some(target);
            if !lock_ready(&state) {
                return Ok(None);
            }
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => return Ok(None),
    }
    let mut fired = 0usize;
    for (item, ty) in guns {
        let Some((_, effect)) = crate::verbs::weapon_effect(Some(content), ty) else {
            continue;
        };
        if is_running(&c.active_effects(item)?, effect) {
            continue;
        }
        match c.fire(item, ty, Some(content), target) {
            Ok(_) => fired += 1,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => {}
        }
    }
    Ok((fired > 0).then(|| format!("capital: firing {fired} guns at {target}")))
}

/// **Stock the module's fuel** at the docked station (the pilot's own money,
/// the hangar's own stacks first: [`crate::capital::stock_bay`]). A note only
/// when it bought or moved something: a fuel nobody sells must never end the
/// turn, or the isotopes behind it would never be stocked (review M1). After
/// an empty try it waits [`STOCK_BACKOFF`] before it asks the market again.
pub fn stock(
    c: &mut Conn,
    content: &eve_server::content::Content,
    ship: i64,
    cache: &mut Option<Module>,
    retry: &mut Option<std::time::Instant>,
) -> Result<Option<String>> {
    let now = clock::instant();
    if retry.is_some_and(|t| now < t) {
        return Ok(None);
    }
    *cache = find(content, &c.fitted()?);
    let Some(m) = cache.clone() else {
        return Ok(None);
    };
    let Some(t) = m.fuel else {
        return Ok(None);
    };
    let have = crate::capital::held_at(c, ship, crate::capital::FUEL_BAY, t)?
        + crate::capital::held_at(c, ship, crate::capital::CARGO, t)?;
    let want = fuel_to_stock(&m, have);
    if want == 0 {
        return Ok(None);
    }
    let (bought, bayed, held) = crate::capital::stock_bay(c, ship, t, want)?;
    if !moved_any(bought, bayed, held) {
        *retry = Some(now + STOCK_BACKOFF);
        return Ok(None);
    }
    *retry = None;
    Ok(Some(format!(
        "capital: stocked {want} {} fuel: bought {bought}, bay {bayed}, hold {held}",
        m.effect_name
    )))
}

// ---- fitting the module (issue #140, CAP-FIT-SIEGE) ----------------------------------------------

/// Dreadnought 485 and Lancer Dreadnought 4594 (the groups of
/// [`crate::capital::JUMPER_GROUPS`] that siege) **[V]** for 485; **[A]** for
/// 4594 (a Lancer takes the siege effect). Falsified by a Lancer pilot that
/// fits a siege module and is refused: take 4594 out.
pub const SIEGE_GROUPS: [i64; 2] = [485, 4594];
/// Force Auxiliary 1538 **[V]**: triage.
pub const TRIAGE_GROUPS: [i64; 1] = [1538];
/// `categoryModule` **[V]**.
pub const CATEGORY_MODULE: i64 = 7;

/// The share of the wallet a pilot spends on its siege or triage module
/// **[A]**. Falsified by a pilot that cannot afford the fuel after buying it
/// (lower) or one that never fits (raise).
pub const FIT_WALLET_SHARE: f64 = 0.25;

/// The effect a hull of this group fights with: siege for a dreadnought,
/// triage for a force auxiliary, nothing for the rest (a carrier).
pub fn effect_for_hull(group_id: i64) -> Option<i64> {
    if SIEGE_GROUPS.contains(&group_id) {
        Some(EFFECT_SIEGE)
    } else if TRIAGE_GROUPS.contains(&group_id) {
        Some(EFFECT_TRIAGE)
    } else {
        None
    }
}

/// The module types of the type table that carry `effect` (found by effect at
/// run time; no id is written here).
pub fn module_types(content: &eve_server::content::Content, effect: i64) -> Vec<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.category_id == CATEGORY_MODULE && t.has_effect(effect))
        .map(|t| t.type_id)
        .collect()
}

/// Where the module comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// A hangar item (its item id) of the right type.
    Hangar(i64),
    /// Buy one of this type at this price.
    Buy { type_id: i64, price: f64 },
    /// Neither; why.
    Nothing(String),
}

/// **Hangar stock first, then the cheapest ask inside `budget`.** `hangar` is
/// `(item_id, type_id)` of what sits in the hangar, `asks` is `(type_id,
/// price)` of the cheapest ask per type at this station. No budget and no
/// stock is [`Pick::Nothing`], never a grant.
pub fn pick(hangar: &[(i64, i64)], types: &[i64], asks: &[(i64, f64)], budget: f64) -> Pick {
    if let Some((item, _)) = hangar.iter().find(|(_, t)| types.contains(t)) {
        return Pick::Hangar(*item);
    }
    if budget <= 0.0 {
        return Pick::Nothing("no budget for the module".to_owned());
    }
    asks.iter()
        .filter(|(t, p)| types.contains(t) && *p > 0.0 && *p <= budget)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or_else(
            || Pick::Nothing(format!("none on sale here inside {budget:.0} ISK")),
            |&(type_id, price)| Pick::Buy { type_id, price },
        )
}

/// **Fit the siege or triage module** to the docked active ship, if it has
/// none: the pilot's own, by the effect its hull fights with
/// ([`effect_for_hull`]), then [`fit_effect`]. `None` when nothing needs doing
/// (the hull has no such module, one is fitted) or the try moved nothing (waits
/// [`STOCK_BACKOFF`]); a note when it fitted or was refused. Only a dead
/// connection is the error.
pub fn fit(
    c: &mut Conn,
    content: &eve_server::content::Content,
    cache: &mut Option<Module>,
    retry: &mut Option<std::time::Instant>,
) -> Result<Option<String>> {
    let now = clock::instant();
    if retry.is_some_and(|t| now < t) {
        return Ok(None);
    }
    let Some(ship) = c.session().ship_id() else {
        return Ok(None);
    };
    if c.session().station_id().is_none() {
        return Ok(None);
    }
    let rows = c.hangar_items()?;
    // A hull with nothing to fit is settled for a while: back off so the next
    // turns do not read the hangar again (L2).
    let Some(effect) = rows
        .iter()
        .find(|r| r.item_id == ship)
        .and_then(|r| effect_for_hull(r.group_id))
    else {
        *retry = Some(now + STOCK_BACKOFF);
        return Ok(None);
    };
    fit_effect(c, content, effect, cache, retry)
}

/// **Fit a module that carries `effect`** to the docked active ship, if none is
/// fitted: from the hangar, else bought inside [`FIT_WALLET_SHARE`] of the
/// wallet, then `MultiAdd` with the auto-fit flag ([`Conn::fit_auto`]). The pilot
/// calls it with its siege or triage effect ([`fit`]); the cyno alt with the
/// cynosural effect ([`crate::capital::CYNO_EFFECT_ID`]), so the alt fits its own
/// generator (nothing from nowhere). `cache` takes the siege or triage module
/// once fitted (stays `None` for a generator); `retry` is the backoff.
pub fn fit_effect(
    c: &mut Conn,
    content: &eve_server::content::Content,
    effect: i64,
    cache: &mut Option<Module>,
    retry: &mut Option<std::time::Instant>,
) -> Result<Option<String>> {
    let now = clock::instant();
    if retry.is_some_and(|t| now < t) {
        return Ok(None);
    }
    let Some(ship) = c.session().ship_id() else {
        return Ok(None);
    };
    let Some(station) = c.session().station_id() else {
        return Ok(None);
    };
    let rows = c.hangar_items()?;
    let fitted = c.fitted()?;
    let table = eve_server::typedata::installed_in(content).table();
    if fitted
        .iter()
        .any(|r| table.get(r.type_id).is_some_and(|t| t.has_effect(effect)))
    {
        *cache = find(content, &fitted);
        *retry = Some(now + STOCK_BACKOFF);
        return Ok(None);
    }
    let types = module_types(content, effect);
    if types.is_empty() {
        *retry = Some(now + STOCK_BACKOFF);
        return Ok(None);
    }
    let stock: Vec<(i64, i64)> = rows
        .iter()
        .filter(|r| r.item_id != ship && types.contains(&r.type_id))
        .map(|r| (r.item_id, r.type_id))
        .collect();
    let mut asks = Vec::new();
    let mut budget = 0.0;
    if stock.is_empty() {
        // H1: never buy a module the pilot's skills cannot fit; queue its
        // skills (ahead of the rest) and wait.
        let levels = crate::acquire::gate_levels(c)?;
        let (ok, lack) = skilled_types(&types, |t| {
            levels
                .as_ref()
                .map_or_else(Vec::new, |l| crate::acquire::lacking_of(l, t))
        });
        if ok.is_empty() {
            *retry = Some(now + STOCK_BACKOFF);
            let missing = crate::acquire::learn_urgent_soft(c, &lack)?;
            return Ok(Some(format!(
                "capital: the module waits on skills {lack:?}{}",
                crate::acquire::missing_note(&missing)
            )));
        }
        // #189: the pledged ISK is not spent, the bot's own ask not bought.
        budget = crate::acquire::spendable(c)? * FIT_WALLET_SHARE;
        let own = crate::capital::own_ids_soft(c)?;
        for t in &ok {
            let book = c.market_orders(*t)?;
            if let Some(a) = crate::capital::cheapest_ask(&book, station, &own) {
                asks.push((*t, a.price));
            }
        }
    }
    let (item, how) = match pick(&stock, &types, &asks, budget) {
        Pick::Hangar(item) => (item, "from the hangar".to_owned()),
        Pick::Buy { type_id, price } => {
            match c.buy(station, type_id, price, 1, 0, crate::pricing::BROKER_FEE) {
                Ok(_) => {}
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(e) => {
                    *retry = Some(now + STOCK_BACKOFF);
                    return Ok(Some(format!("capital: buying the module {type_id}: {e}")));
                }
            }
            match c.hangar_items()?.into_iter().find(|r| r.type_id == type_id) {
                Some(r) => (r.item_id, format!("bought for {price:.0} ISK")),
                None => {
                    *retry = Some(now + STOCK_BACKOFF);
                    return Ok(None);
                }
            }
        }
        Pick::Nothing(why) => {
            // L3: a wait is not silent; once per backoff.
            *retry = Some(now + STOCK_BACKOFF);
            return Ok(Some(format!("capital: no module to fit: {why}")));
        }
    };
    // H1: the gated fit queues the skills a refusal names (SKILLGATE).
    match crate::acquire::fit_auto_gated(c, &[item]) {
        Ok(Ok(_)) => {
            *retry = None;
            *cache = find(content, &c.fitted()?);
            Ok(Some(format!("capital: fitted the module ({how})")))
        }
        Ok(Err(note)) => {
            *retry = Some(now + STOCK_BACKOFF);
            Ok(Some(format!("capital: {note}")))
        }
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            *retry = Some(now + STOCK_BACKOFF);
            Ok(Some(format!("capital: fitting the module refused: {e}")))
        }
    }
}

/// **The module types the pilot's skills fit** (pure over `lacking`, a type's
/// missing `(skill, level)` list), with, when none does, what the first type
/// lacks (what to queue). A module bought without its skills sits in the
/// hangar for good (review H1).
pub fn skilled_types(
    types: &[i64],
    lacking: impl Fn(i64) -> Vec<(i64, i64)>,
) -> (Vec<i64>, Vec<(i64, i64)>) {
    let ok: Vec<i64> = types
        .iter()
        .copied()
        .filter(|t| lacking(*t).is_empty())
        .collect();
    let lack = if ok.is_empty() {
        types.first().map_or_else(Vec::new, |t| lacking(*t))
    } else {
        Vec::new()
    };
    (ok, lack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::content::Content;

    const SIEGE_TYPE: i64 = 990_734_101;
    const FUEL_TYPE: i64 = 990_734_102;
    const PLAIN_TYPE: i64 = 990_734_103;
    const FX_NAME: &str = "fixtureSiegeEffect";
    const DUR: i64 = 990_734_201;

    fn facts() -> Facts {
        Facts {
            fight: true,
            holding: true,
            running: false,
            warp_out_in: None,
            cycle: Duration::from_secs(300),
            fuel: 10,
            per_cycle: 4,
        }
    }

    #[test]
    fn sieges_on_a_fight_grid_holding_with_fuel() {
        assert_eq!(decide(&facts()), Step::Start);
    }

    #[test]
    fn no_fight_no_siege_and_a_running_one_is_stopped() {
        let f = Facts {
            fight: false,
            ..facts()
        };
        assert_eq!(decide(&f), Step::Keep("no fight here"));
        assert_eq!(decide(&Facts { running: true, ..f }), Step::Stop);
    }

    #[test]
    fn no_fuel_no_siege_but_a_running_module_is_left_to_finish() {
        let f = Facts { fuel: 3, ..facts() };
        assert_eq!(decide(&f), Step::Keep("no fuel for a cycle"));
        assert_eq!(decide(&Facts { running: true, ..f }), Step::Keep("running"));
        assert_eq!(
            decide(&Facts {
                per_cycle: 0,
                fuel: 0,
                ..facts()
            }),
            Step::Start,
            "a module that burns nothing needs no fuel"
        );
    }

    #[test]
    fn a_warp_out_due_within_the_cycle_blocks_and_ends_it() {
        let due = Facts {
            warp_out_in: Some(Duration::from_secs(120)),
            ..facts()
        };
        assert_eq!(
            decide(&due),
            Step::Keep("a warp-out is due within the cycle")
        );
        assert_eq!(
            decide(&Facts {
                running: true,
                ..due
            }),
            Step::Stop
        );
        let later = Facts {
            warp_out_in: Some(Duration::from_secs(900)),
            ..facts()
        };
        assert_eq!(decide(&later), Step::Start, "a leave beyond the cycle");
    }

    #[test]
    fn a_stocking_that_moved_nothing_does_not_end_the_turn() {
        assert!(!moved_any(0, 0, 0));
        assert!(moved_any(0, 3, 0));
        assert!(moved_any(2, 0, 0));
        assert!(moved_any(0, 0, 1));
    }

    #[test]
    fn a_warp_out_due_now_stops_a_running_module_and_blocks_a_start() {
        let now = Some(Duration::ZERO);
        assert_eq!(
            decide(&Facts {
                warp_out_in: now,
                ..facts()
            }),
            Step::Keep("a warp-out is due within the cycle")
        );
        assert_eq!(
            decide(&Facts {
                warp_out_in: now,
                running: true,
                ..facts()
            }),
            Step::Stop
        );
    }

    #[test]
    fn a_warping_ship_does_not_siege() {
        let f = Facts {
            holding: false,
            ..facts()
        };
        assert_eq!(decide(&f), Step::Keep("not holding still"));
    }

    #[test]
    fn a_fight_is_an_npc_or_a_foreign_human_on_the_grid() {
        assert!(!Scene::default().fight());
        assert!(
            Scene {
                hostiles: 1,
                ..Scene::default()
            }
            .fight()
        );
        assert!(
            Scene {
                humans: 2,
                ..Scene::default()
            }
            .fight()
        );
    }

    #[test]
    fn the_nearest_hostile_is_the_target() {
        let c = [(9_600_001, 40_000.0, true), (9_600_002, 12_000.0, true)];
        assert_eq!(pick_target(&c), Some(9_600_002));
        let tie = [(9_600_004, 5.0, true), (9_600_003, 5.0, true)];
        assert_eq!(pick_target(&tie), Some(9_600_003));
    }

    #[test]
    fn no_hostile_no_target() {
        assert_eq!(pick_target(&[]), None);
        assert_eq!(pick_target(&[(9_600_005, 1.0, false)]), None);
    }

    #[test]
    fn a_corp_mate_is_never_the_target() {
        // The corp mate is nearer, and not hostile: the far NPC is shot.
        let c = [(9_600_006, 1_000.0, false), (9_600_007, 90_000.0, true)];
        assert_eq!(pick_target(&c), Some(9_600_007));
    }

    #[test]
    fn the_target_is_kept_while_it_may_be_shot() {
        let c = [(9_600_011, 50_000.0, true), (9_600_012, 10_000.0, true)];
        assert_eq!(aim(&c, Some(9_600_011)), Aim::Keep(9_600_011));
        assert_eq!(
            aim(&c, None),
            Aim::Switch {
                old: None,
                new: 9_600_012
            }
        );
    }

    #[test]
    fn a_target_that_may_not_be_shot_or_is_gone_is_replaced_and_its_lock_dropped() {
        let barred = [(9_600_011, 1.0, false), (9_600_012, 10_000.0, true)];
        assert_eq!(
            aim(&barred, Some(9_600_011)),
            Aim::Switch {
                old: Some(9_600_011),
                new: 9_600_012
            }
        );
        let gone = [(9_600_012, 10_000.0, true)];
        assert_eq!(
            aim(&gone, Some(9_600_013)),
            Aim::Switch {
                old: Some(9_600_013),
                new: 9_600_012
            }
        );
    }

    #[test]
    fn nothing_to_shoot_is_idle_so_the_guns_stop() {
        assert_eq!(aim(&[], Some(9_600_011)), Aim::Idle);
        assert_eq!(aim(&[(9_600_011, 1.0, false)], Some(9_600_011)), Aim::Idle);
    }

    #[test]
    fn a_pending_lock_does_not_fire() {
        assert!(lock_ready(&LockState::Locked));
        assert!(!lock_ready(&LockState::Pending));
    }

    #[test]
    fn a_gun_is_running_when_its_effect_is_active() {
        assert!(is_running(&[9_600_021, 9_600_022], 9_600_022));
        assert!(!is_running(&[9_600_021], 9_600_022));
        assert!(!is_running(&[], 9_600_022));
    }

    fn content() -> Content {
        let rows = format!(
            "{}\
             type\t{SIEGE_TYPE}\t515\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t{EFFECT_SIEGE}\t{ATTR_FUEL_TYPE}={FUEL_TYPE};{ATTR_FUEL_QUANTITY}=5;{DUR}=240000\tFixture Siege\n\
             type\t{PLAIN_TYPE}\t53\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Plain\n\
             effect\t{EFFECT_SIEGE}\t1\t0\t{DUR}\t0\t0\t0\t0\t{FX_NAME}\t\n",
            eve_server::typedata::FIXTURE,
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        c
    }

    fn row(item: i64, ty: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id: ty,
            owner_id: 1,
            location_id: 2,
            flag_id: 27,
            quantity: 1,
            group_id: 0,
            category_id: 7,
            singleton: true,
        }
    }

    #[test]
    fn the_module_and_its_fuel_come_from_the_type_table() {
        let c = content();
        let m = find(&c, &[row(1, PLAIN_TYPE), row(2, SIEGE_TYPE)]).expect("fitted siege");
        assert_eq!(m.item_id, 2);
        assert_eq!(m.fuel, Some(FUEL_TYPE));
        assert_eq!(m.per_cycle, 5);
        assert_eq!(m.cycle, Duration::from_secs(240));
        assert_eq!(m.effect_name, FX_NAME);
        assert!(find(&c, &[row(1, PLAIN_TYPE)]).is_none());
    }

    #[test]
    fn it_stocks_what_the_bay_lacks_of_a_few_cycles() {
        let c = content();
        let m = find(&c, &[row(2, SIEGE_TYPE)]).expect("fitted siege");
        assert_eq!(fuel_to_stock(&m, 0), 5 * FUEL_CYCLES);
        assert_eq!(fuel_to_stock(&m, 5 * FUEL_CYCLES + 9), 0);
    }

    #[test]
    fn a_dreadnought_sieges_a_force_auxiliary_triages_a_carrier_neither() {
        assert_eq!(effect_for_hull(SIEGE_GROUPS[0]), Some(EFFECT_SIEGE));
        assert_eq!(effect_for_hull(TRIAGE_GROUPS[0]), Some(EFFECT_TRIAGE));
        assert_eq!(effect_for_hull(990_000_547), None); // an invented group
        assert_eq!(effect_for_hull(0), None);
    }

    #[test]
    fn a_module_the_skills_do_not_fit_is_not_bought() {
        // Invented ids: type 1 needs skill 90 at 3, type 2 needs nothing.
        let lacking = |t: i64| if t == 1 { vec![(90, 3)] } else { Vec::new() };
        // Only the unskilled type: nothing to buy, the skill to queue.
        assert_eq!(skilled_types(&[1], lacking), (vec![], vec![(90, 3)]));
        // A fittable type among them is the only one bought.
        assert_eq!(skilled_types(&[1, 2], lacking), (vec![2], vec![]));
        // No types, no skills to queue.
        assert_eq!(skilled_types(&[], lacking), (vec![], vec![]));
    }

    #[test]
    fn a_skill_refusal_is_the_fit_gate_and_nothing_else() {
        let skill = Error::Refused("FittingHasSkillPrerequisites".to_owned());
        assert!(crate::acquire::is_skill_refusal(&skill));
        assert!(!crate::acquire::is_skill_refusal(&Error::Refused(
            "NotEnoughCpu".to_owned()
        )));
    }

    #[test]
    fn the_module_types_are_found_by_effect() {
        let c = content();
        assert_eq!(module_types(&c, EFFECT_SIEGE), vec![SIEGE_TYPE]);
        assert!(module_types(&c, EFFECT_TRIAGE).is_empty());
    }

    #[test]
    fn hangar_stock_first_then_the_cheapest_ask_in_budget() {
        let types = [10, 11];
        assert_eq!(
            pick(&[(5, 99), (6, 11)], &types, &[(10, 1.0)], 100.0),
            Pick::Hangar(6)
        );
        assert_eq!(
            pick(&[], &types, &[(10, 50.0), (11, 20.0), (12, 1.0)], 100.0),
            Pick::Buy {
                type_id: 11,
                price: 20.0
            }
        );
        assert!(matches!(
            pick(&[], &types, &[(10, 500.0)], 100.0),
            Pick::Nothing(_)
        ));
    }

    /// B4: the alt's generator is picked by the cynosural effect over the same
    /// stock-then-market rule as the pilot's module (invented ids).
    #[test]
    fn fit_effect_takes_hangar_stock_before_the_market() {
        const GEN_A: i64 = 990_734_301;
        const GEN_B: i64 = 990_734_302;
        let rows = format!(
            "{}\
             type\t{GEN_A}\t515\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t{}\t\tFixture Generator A\n\
             type\t{GEN_B}\t515\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t{}\t\tFixture Generator B\n",
            eve_server::typedata::FIXTURE,
            crate::capital::CYNO_EFFECT_ID,
            crate::capital::CYNO_EFFECT_ID,
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        let mut types = module_types(&c, crate::capital::CYNO_EFFECT_ID);
        types.sort_unstable();
        assert_eq!(types, vec![GEN_A, GEN_B]);
        assert!(module_types(&c, EFFECT_SIEGE).is_empty());
        assert_eq!(
            pick(&[(5, GEN_B)], &types, &[(GEN_A, 1.0)], 100.0),
            Pick::Hangar(5)
        );
        assert_eq!(
            pick(&[], &types, &[(GEN_A, 60.0), (GEN_B, 40.0)], 100.0),
            Pick::Buy {
                type_id: GEN_B,
                price: 40.0
            }
        );
        assert!(matches!(pick(&[], &types, &[], 0.0), Pick::Nothing(_)));
    }

    /// A dev roster's cyno alt: the dev kit leaves the generator in the hangar and the
    /// wallet may be empty. The alt's fit step ([`crate::capital`] `alt`, not gated on
    /// society) takes hangar stock with no budget and no ask; with none it grants nothing.
    #[test]
    fn a_dev_alt_fits_the_generator_its_kit_left_in_the_hangar_with_no_budget() {
        const GEN: i64 = 990_734_311;
        const OTHER: i64 = 990_734_312;
        assert_eq!(
            pick(&[(41, OTHER), (42, GEN)], &[GEN], &[], 0.0),
            Pick::Hangar(42)
        );
        assert!(matches!(
            pick(&[(41, OTHER)], &[GEN], &[], 0.0),
            Pick::Nothing(_)
        ));
    }

    #[test]
    fn no_budget_no_stock_no_fit_and_nothing_granted() {
        assert!(matches!(
            pick(&[], &[10], &[(10, 1.0)], 0.0),
            Pick::Nothing(_)
        ));
        assert!(matches!(pick(&[], &[], &[], 10.0), Pick::Nothing(_)));
        assert!(matches!(
            pick(&[(1, 5)], &[10], &[], 10.0),
            Pick::Nothing(_)
        ));
    }

    #[test]
    fn a_module_picked_by_effect_is_one_find_reads_once_fitted() {
        let c = content();
        let t = module_types(&c, EFFECT_SIEGE);
        let fitted = [row(7, t[0])];
        assert!(find(&c, &fitted).is_some());
    }

    /// #189: a module is never bought from the bot's own ask, and a wallet at
    /// the founder's reserve buys none.
    #[test]
    fn the_module_comes_from_no_own_ask_and_no_reserve() {
        const STATION: i64 = 990_893_001;
        const OWN: i64 = 990_893_101;
        const OTHER: i64 = 990_893_102;
        let ask = |id, price| crate::verbs::BookOrder {
            order_id: id,
            type_id: SIEGE_TYPE,
            station_id: STATION,
            price,
            vol_remaining: 1,
            bid: false,
        };
        let book = crate::verbs::Book {
            asks: vec![ask(OWN, 10.0), ask(OTHER, 20.0)],
            bids: vec![],
        };
        let price_of =
            |own: &[i64]| crate::capital::cheapest_ask(&book, STATION, own).map(|a| a.price);
        assert_eq!(price_of(&[]), Some(10.0));
        assert_eq!(
            price_of(&[OWN]),
            None,
            "nothing is strictly cheaper than the bot's own floor"
        );
        let asks = [(SIEGE_TYPE, 20.0)];
        let spendable = crate::acquire::budget(1_000.0, 1_000.0) * FIT_WALLET_SHARE;
        assert!(matches!(
            pick(&[], &[SIEGE_TYPE], &asks, spendable),
            Pick::Nothing(_)
        ));
        assert!(matches!(
            pick(&[], &[SIEGE_TYPE], &asks, 1_000.0 * FIT_WALLET_SHARE),
            Pick::Buy { .. }
        ));
    }
}
