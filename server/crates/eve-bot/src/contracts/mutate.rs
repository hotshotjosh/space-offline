//! **Bots roll the mutaplasmids they hold** (#223 MUTA-BOT).
//!
//! A submodule of [`crate::contracts`] (the sale of a roll is a contract, and
//! `lib.rs` is a hot file). Nothing from nowhere: a bot never mints or is
//! given a mutaplasmid. It rolls only those in its hangar, which came from
//! abyss caches (progress row "Abyss loot", #5) or from another pilot's
//! market ask or contract. Until one is held the step finds none and does
//! nothing.
//!
//! The decisions ([`plan`], [`score`], [`verdict`]) are pure; [`roll_held`]
//! is the thin wire: `dynamicItemService.CreateDynamicItem(mutator, source)`
//! then `GetDynamicItemInfo(new)` **[V]** (`docs/re/dynamic-items.md`), the
//! same two calls the client's crafting window makes.
//!
//! Every number here is **[A]** policy, named at its definition with the
//! observation that would falsify it; no test spells one as a literal.

use super::{Offer, create, open_count};
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use eve_server::mutator_data::{self, Mutator, Mutators};
use std::collections::{BTreeMap, BTreeSet};

/// Rolls attempted per dock visit **[A]**. Falsified by a soak where bots hold
/// mutaplasmids with a matching source for hours (too low), or burn their
/// whole source stock in one visit and sell a flood of rolls (too high).
pub const MAX_PER_DOCK: usize = 3;

/// The mean goodness ([`score`]) from which a roll is kept and flown in place
/// of its source **[A]**. Falsified by bots that fly rolls no better than the
/// module they replaced (too low), or that sell every roll (too high).
pub const KEEP_SCORE: f64 = 0.6;

/// The goodness at which a sale starts to ask more than the source's
/// reference price **[A]**: a coin-flip roll is worth what the source is.
pub const NEUTRAL_SCORE: f64 = 0.5;

/// How much of the source's reference price a perfect roll adds to its ask
/// **[A]**: `ask_factor = 1 + ASK_SPAN * (score - NEUTRAL_SCORE).max(0)`.
/// Falsified by contracts for rolls that sit unaccepted until they expire
/// (too high), or by traders buying rolls at a fraction of the ask (too low).
pub const ASK_SPAN: f64 = 4.0;

/// The ask over the source's reference price, for a roll of `score`.
pub fn ask_factor(score: f64) -> f64 {
    1.0 + ASK_SPAN * (score - NEUTRAL_SCORE).max(0.0)
}

/// A pair to roll: one mutaplasmid item and one source item, both loose in
/// the hangar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub mutator_item: i64,
    pub source_item: i64,
    pub result_type: i64,
}

/// A hangar row that can be used: loose in the hangar, not fitted.
fn loose(r: &ItemRow) -> bool {
    r.flag_id == crate::verbs::FLAG_HANGAR && !r.is_fitted()
}

/// **The pairs to roll.** For each mutaplasmid stack (lowest item id first),
/// the source to use: a hangar item the mutaplasmid applies to, of a
/// published type, preferring (1) a type fitted on the active ship, then
/// (2) the cheapest by `prices` (no price sorts last), then the lowest id.
/// Each item is used at most once; a drone source is skipped (owner question
/// 2: no mutated drones from bots), and so is anything the loot sale keeps
/// ([`crate::escort::is_loot`], `kept_guns`). At most [`MAX_PER_DOCK`] pairs.
pub fn plan(
    hangar: &[ItemRow],
    fitted: &[ItemRow],
    t: &Mutators,
    published: &dyn Fn(i64) -> bool,
    prices: &BTreeMap<i64, f64>,
    kept_guns: &BTreeSet<i64>,
) -> Vec<Pair> {
    let fitted_types: BTreeSet<i64> = fitted.iter().map(|r| r.type_id).collect();
    let mut mutators: Vec<&ItemRow> = hangar
        .iter()
        .filter(|r| loose(r) && t.is_mutator(r.type_id))
        .collect();
    mutators.sort_by_key(|r| r.item_id);
    let mut used: BTreeSet<i64> = BTreeSet::new();
    let mut out = Vec::new();
    for m in mutators {
        if out.len() >= MAX_PER_DOCK {
            break;
        }
        let best = hangar
            .iter()
            .filter(|s| {
                loose(s)
                    && s.category_id != eve_server::drones::CATEGORY_DRONE
                    && !used.contains(&s.item_id)
                    && published(s.type_id)
                    // Only stock the loot sale would sell anyway: never a
                    // module the bot keeps on purpose (pvp kit, remote
                    // repairer, camp kit, the guns of its fit plan).
                    && crate::escort::is_loot(s)
                    && !kept_guns.contains(&s.type_id)
            })
            .filter_map(|s| Some((s, t.result_for(m.type_id, s.type_id)?)))
            .min_by(|(a, _), (b, _)| {
                let price = |r: &ItemRow| prices.get(&r.type_id).copied().unwrap_or(f64::INFINITY);
                (!fitted_types.contains(&a.type_id))
                    .cmp(&!fitted_types.contains(&b.type_id))
                    .then(price(a).total_cmp(&price(b)))
                    .then(a.item_id.cmp(&b.item_id))
            });
        if let Some((s, result_type)) = best {
            used.insert(m.item_id);
            used.insert(s.item_id);
            out.push(Pair {
                mutator_item: m.item_id,
                source_item: s.item_id,
                result_type,
            });
        }
    }
    out
}

/// **How good a roll is**, in `[0, 1]`: the mean over its modifiers of `m`
/// where a higher modifier is better (`mutator_data::modifier_is_good_up`),
/// else `1 - m`. A modifier whose range is unknown or has no `high_is_good`
/// is left out; with none left the score is 0 (nothing known to be good).
pub fn score(modifiers: &[(i64, f64)], m: &Mutator) -> f64 {
    let goods: Vec<f64> = modifiers
        .iter()
        .filter_map(|&(a, v)| {
            let up = mutator_data::modifier_is_good_up(m.range(a)?)?;
            let v = v.clamp(0.0, 1.0);
            Some(if up { v } else { 1.0 - v })
        })
        .collect();
    if goods.is_empty() {
        0.0
    } else {
        goods.iter().sum::<f64>() / goods.len() as f64
    }
}

/// What to do with a roll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Fly it: unfit `replaces` (the fitted module of the source's type).
    Fly { replaces: i64 },
    /// Sell it by contract at `price`.
    Sell { price: f64 },
}

/// **Keep or sell.** Fly a roll of at least [`KEEP_SCORE`] whose source type
/// is fitted (it replaces the lowest-id fitted module of that type); sell the
/// rest at `source_ref_price * ask_factor(score)`, rounded to whole ISK.
pub fn verdict(score: f64, source_type: i64, fitted: &[ItemRow], source_ref_price: f64) -> Verdict {
    if score >= KEEP_SCORE
        && let Some(old) = fitted
            .iter()
            .filter(|r| r.type_id == source_type)
            .map(|r| r.item_id)
            .min()
    {
        return Verdict::Fly { replaces: old };
    }
    Verdict::Sell {
        price: (source_ref_price * ask_factor(score)).round(),
    }
}

/// The modifiers of a `GetDynamicItemInfo` answer: `(attributeID, modifier)`
/// from its `modifiers` dict (entries are `(value, key)` on the wire).
pub fn modifiers_of(info: &PyValue) -> Vec<(i64, f64)> {
    let Some(PyValue::Dict(d)) = crate::dict_get(crate::attributes(info), "modifiers") else {
        return Vec::new();
    };
    d.iter()
        .filter_map(|(v, k)| match (k, v) {
            (PyValue::Int(a), PyValue::Real(m)) => Some((*a, *m)),
            _ => None,
        })
        .collect()
}

/// The result type of a `GetDynamicItemInfo` answer.
pub fn type_of(info: &PyValue) -> Option<i64> {
    match crate::dict_get(crate::attributes(info), "typeID")? {
        PyValue::Int(t) => Some(*t),
        _ => None,
    }
}

/// Unfit `module` into the hangar: the hangar's `Add(moduleID, shipID,
/// flag=4)`, the client's unfit **[V]** (`fitting.rs`, `invControllers.py`).
/// Evidence: the module is no longer fitted.
fn unfit(conn: &mut Conn, module: i64) -> Result<()> {
    let ship = conn
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("unfit: the session names no ship".to_owned()))?;
    let hangar = conn.open_hangar()?;
    let out = conn.call_on(
        &hangar,
        "Add",
        vec![PyValue::Int(module), PyValue::Int(ship)],
        vec![
            (PyValue::None, eve_server::pyutil::bstr("qty")),
            (
                PyValue::Int(crate::verbs::FLAG_HANGAR),
                eve_server::pyutil::bstr("flag"),
            ),
        ],
    )?;
    if conn.fitted()?.iter().any(|r| r.item_id == module) {
        return Err(Error::Refused(format!(
            "unfit {module}: still fitted ({})",
            out.refusal().unwrap_or_else(|| "no answer".to_owned())
        )));
    }
    Ok(())
}

/// **Roll what the hangar holds.** For each [`plan`] pair, `CreateDynamicItem`
/// then `GetDynamicItemInfo`, then the [`verdict`]: `Fly` unfits the old
/// module and fits the new one (a refused fit refits the old one and sells
/// instead); `Sell` is one item-exchange contract for the one item. A refusal
/// or error is a note, never a stop, except a dead connection. Empty when
/// nothing is held to roll.
pub fn roll_held(
    conn: &mut Conn,
    content: &eve_server::content::Content,
    current: crate::hull_choice::Weapon,
    seed: u64,
) -> Result<String> {
    let Some(t) = content.try_get::<Mutators>().filter(|t| !t.is_empty()) else {
        return Ok(String::new());
    };
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Ok(String::new());
    };
    let hangar = conn.hangar_items()?;
    if !hangar.iter().any(|r| loose(r) && t.is_mutator(r.type_id)) {
        return Ok(String::new());
    }
    let types = eve_server::typedata::installed_in(content).table();
    let published = |id: i64| types.get(id).is_some_and(|r| r.published);
    let prices: BTreeMap<i64, f64> = conn.adjusted_prices()?.into_iter().collect();
    let mut fitted = conn.fitted()?;
    let kept_guns: BTreeSet<i64> = crate::escort::plan_weapons_of(
        current,
        crate::progression::hull_to_fly(None, &hangar),
        seed,
    )
    .into_iter()
    .flat_map(|w| crate::fit_plan::keep_guns(w).map(|(ty, _)| ty))
    .collect();
    let pairs = plan(&hangar, &fitted, t, &published, &prices, &kept_guns);
    if pairs.is_empty() {
        return Ok(String::new());
    }
    let mut notes = Vec::new();
    let mut room = super::MAX_OPEN.saturating_sub(open_count(conn, me)?);
    for p in &pairs {
        let source_type = hangar
            .iter()
            .find(|r| r.item_id == p.source_item)
            .map_or(0, |r| r.type_id);
        let mutator_type = hangar
            .iter()
            .find(|r| r.item_id == p.mutator_item)
            .map_or(0, |r| r.type_id);
        let made = conn.call(
            "dynamicItemService",
            "CreateDynamicItem",
            vec![PyValue::Int(p.mutator_item), PyValue::Int(p.source_item)],
        )?;
        let new = match (made.refusal(), made.value()) {
            (Some(why), _) => {
                notes.push(format!("roll {mutator_type}+{source_type} refused: {why}"));
                continue;
            }
            (None, Some(PyValue::Int(id))) => id,
            (None, other) => {
                notes.push(format!(
                    "roll {mutator_type}+{source_type} answered {other:?}"
                ));
                continue;
            }
        };
        let info = conn.call(
            "dynamicItemService",
            "GetDynamicItemInfo",
            vec![PyValue::Int(new)],
        )?;
        let mods = info.value().map(|v| modifiers_of(&v)).unwrap_or_default();
        let result_type = info
            .value()
            .and_then(|v| type_of(&v))
            .unwrap_or(p.result_type);
        let s = t.get(mutator_type).map_or(0.0, |m| score(&mods, m));
        let reference = prices.get(&source_type).copied().unwrap_or(0.0);
        let mut v = verdict(s, source_type, &fitted, reference);
        if let Verdict::Fly { replaces } = v {
            let flown = match unfit(conn, replaces) {
                // Any refusal of the fit (not only a skill wait) leaves the old
                // module out: it goes back below. Only a dead connection stops.
                Ok(()) => match crate::acquire::fit_auto_gated(conn, &[new]) {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(wait)) => Err(wait),
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => Err(e.to_string()),
                },
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                Err(e) => Err(e.to_string()),
            };
            match flown {
                Ok(()) => {
                    notes.push(format!(
                        "rolled {result_type} #{new} (score {s:.2}) flown in place of #{replaces}"
                    ));
                    fitted = conn.fitted()?;
                    continue;
                }
                Err(why) => {
                    // The old module goes back; the roll is sold instead.
                    if !conn.fitted()?.iter().any(|r| r.item_id == replaces) {
                        match crate::acquire::fit_auto_gated(conn, &[replaces]) {
                            Ok(Ok(_)) => {}
                            Ok(Err(wait)) => {
                                notes.push(format!("refit of #{replaces} waits: {wait}"));
                            }
                            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                            Err(e) => notes.push(format!("refit of #{replaces} refused: {e}")),
                        }
                    }
                    fitted = conn.fitted()?;
                    notes.push(format!("fit of #{new} refused ({why})"));
                    v = Verdict::Sell {
                        price: (reference * ask_factor(s)).round(),
                    };
                }
            }
        }
        if let Verdict::Sell { price } = v {
            if price < 1.0 || room == 0 {
                notes.push(format!(
                    "rolled {result_type} #{new} (score {s:.2}) kept: {}",
                    if room == 0 {
                        "contracts full"
                    } else {
                        "no reference price"
                    }
                ));
                continue;
            }
            let offer = Offer {
                item_id: new,
                type_id: result_type,
                quantity: 1,
                price,
            };
            match create(conn, station, &offer)? {
                Ok(id) => {
                    room -= 1;
                    notes.push(format!(
                        "rolled {result_type} #{new} (score {s:.2}) sold by contract #{id} for {price:.0}"
                    ));
                }
                Err(why) => notes.push(format!(
                    "rolled {result_type} #{new} (score {s:.2}) not sold: {why}"
                )),
            }
        }
    }
    Ok(format!("mutate: {}", notes.join("; ")))
}

#[cfg(test)]
mod tests {
    //! Invented ids and rows; the named consts stand for the [A] values.
    use super::*;

    const MUT_A: i64 = 981_000_001;
    const MUT_B: i64 = 981_000_002;
    const SRC_1: i64 = 981_100_001;
    const SRC_2: i64 = 981_100_002;
    const SRC_DRONE: i64 = 981_100_003;
    const SRC_OFF: i64 = 981_100_004;
    const RES_X: i64 = 981_200_001;
    const RES_D: i64 = 981_200_002;
    const ATTR_UP: i64 = 981_300_001;
    const ATTR_DOWN: i64 = 981_300_002;
    const ATTR_NONE: i64 = 981_300_003;
    const SHIP: i64 = 981_400_001;

    fn table() -> Mutators {
        let ranges = [
            // Up is good: max > min with high_is_good.
            format!("{MUT_A}\t{ATTR_UP}\t0.5\t2.0\t1"),
            // Reversed range with high_is_good 0: a higher modifier is good.
            format!("{MUT_A}\t{ATTR_DOWN}\t1.5\t0.5\t0"),
            format!("{MUT_A}\t{ATTR_NONE}\t0.5\t1.5\t"),
            format!("{MUT_B}\t{ATTR_UP}\t0.5\t2.0\t1"),
        ];
        let maps = [
            format!("{MUT_A}\t{RES_X}\t{SRC_1}"),
            format!("{MUT_A}\t{RES_X}\t{SRC_2}"),
            format!("{MUT_A}\t{RES_X}\t{SRC_OFF}"),
            format!("{MUT_A}\t{RES_D}\t{SRC_DRONE}"),
            format!("{MUT_B}\t{RES_X}\t{SRC_1}"),
        ];
        Mutators::parse(&ranges, &maps).expect("fixture parses")
    }

    fn row(item_id: i64, type_id: i64, category_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: crate::verbs::FLAG_HANGAR,
            quantity: 1,
            group_id: 0,
            category_id,
            // A packaged stack: loot, as the sale sees it.
            singleton: false,
        }
    }

    fn module(item_id: i64, type_id: i64) -> ItemRow {
        row(item_id, type_id, eve_server::typedata::CATEGORY_MODULE)
    }

    fn fitted_row(item_id: i64, type_id: i64) -> ItemRow {
        ItemRow {
            flag_id: 11,
            location_id: SHIP,
            ..module(item_id, type_id)
        }
    }

    fn all(_: i64) -> bool {
        true
    }

    #[test]
    fn plan_prefers_a_fitted_type_then_the_cheapest() {
        let t = table();
        let hangar = [module(1, MUT_A), module(2, SRC_1), module(3, SRC_2)];
        let prices = BTreeMap::from([(SRC_1, 10.0), (SRC_2, 20.0)]);
        let cheap = plan(&hangar, &[], &t, &all, &prices, &BTreeSet::new());
        assert_eq!(cheap.len(), 1);
        assert_eq!((cheap[0].mutator_item, cheap[0].source_item), (1, 2));
        let fitted = [fitted_row(9, SRC_2)];
        let p = plan(&hangar, &fitted, &t, &all, &prices, &BTreeSet::new());
        assert_eq!(p[0].source_item, 3, "the fitted type wins over the cheaper");
        assert_eq!(p[0].result_type, RES_X);
    }

    #[test]
    fn plan_uses_no_item_twice_and_caps() {
        let t = table();
        let mut hangar = vec![module(1, MUT_A), module(2, MUT_B), module(3, SRC_1)];
        let one = plan(&hangar, &[], &t, &all, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(one.len(), 1, "one source serves one mutaplasmid");
        for i in 0..(MAX_PER_DOCK as i64 + 3) {
            hangar.push(module(100 + i, MUT_A));
            hangar.push(module(200 + i, SRC_2));
        }
        let many = plan(&hangar, &[], &t, &all, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(many.len(), MAX_PER_DOCK);
        let mut items: Vec<i64> = many
            .iter()
            .flat_map(|p| [p.mutator_item, p.source_item])
            .collect();
        items.sort();
        items.dedup();
        assert_eq!(items.len(), many.len() * 2);
    }

    #[test]
    fn plan_skips_unpublished_drones_fitted_and_inapplicable_sources() {
        let t = table();
        let hangar = [
            module(1, MUT_A),
            module(2, SRC_OFF),
            row(3, SRC_DRONE, eve_server::drones::CATEGORY_DRONE),
            fitted_row(4, SRC_1),
            module(5, SRC_DRONE + 1000),
        ];
        let unpublished = |t: i64| t != SRC_OFF;
        assert!(
            plan(
                &hangar,
                &[],
                &t,
                &unpublished,
                &BTreeMap::new(),
                &BTreeSet::new()
            )
            .is_empty()
        );
        assert!(
            plan(
                &[module(2, SRC_1)],
                &[],
                &t,
                &all,
                &BTreeMap::new(),
                &BTreeSet::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn plan_never_takes_a_module_the_bot_keeps() {
        let t = table();
        let none = BTreeSet::new();
        let prices = BTreeMap::new();
        // An assembled (singleton) module is not loot: the sale would not sell it.
        let kept = ItemRow {
            singleton: true,
            ..module(2, SRC_1)
        };
        assert!(plan(&[module(1, MUT_A), kept], &[], &t, &all, &prices, &none).is_empty());
        // A type the fit plan keeps as guns is not a source.
        let guns = BTreeSet::from([SRC_1]);
        let hangar = [module(1, MUT_A), module(2, SRC_1)];
        assert!(plan(&hangar, &[], &t, &all, &prices, &guns).is_empty());
        assert_eq!(plan(&hangar, &[], &t, &all, &prices, &none).len(), 1);
        // Neither of loot's categories: kept stock, not loot.
        let odd = ItemRow {
            category_id: 0,
            ..module(2, SRC_1)
        };
        assert!(plan(&[module(1, MUT_A), odd], &[], &t, &all, &prices, &none).is_empty());
    }

    #[test]
    fn score_runs_from_worst_to_best_and_ignores_unknown_ranges() {
        let t = table();
        let m = t.get(MUT_A).expect("mutator");
        // ATTR_UP: high modifier is good. ATTR_DOWN: min > max, high_is_good
        // 0, so (max > min) == high_is_good: a high modifier is good too.
        let best = score(&[(ATTR_UP, 1.0), (ATTR_DOWN, 1.0)], m);
        let worst = score(&[(ATTR_UP, 0.0), (ATTR_DOWN, 0.0)], m);
        assert_eq!(best, 1.0);
        assert_eq!(worst, 0.0);
        assert_eq!(score(&[(ATTR_UP, 1.0), (ATTR_NONE, 0.0)], m), 1.0);
        assert_eq!(score(&[(ATTR_NONE, 0.7), (981_399_999, 0.7)], m), 0.0);
        assert_eq!(score(&[], m), 0.0);
    }

    #[test]
    fn verdict_flies_only_a_good_roll_of_a_fitted_type() {
        let fitted = [fitted_row(9, SRC_1), fitted_row(8, SRC_1)];
        assert_eq!(
            verdict(KEEP_SCORE, SRC_1, &fitted, 100.0),
            Verdict::Fly { replaces: 8 }
        );
        assert_eq!(
            verdict(1.0, SRC_1, &fitted, 100.0),
            Verdict::Fly { replaces: 8 }
        );
        assert!(matches!(
            verdict(KEEP_SCORE, SRC_2, &fitted, 100.0),
            Verdict::Sell { .. }
        ));
        let below = KEEP_SCORE - 0.01;
        assert!(matches!(
            verdict(below, SRC_1, &fitted, 100.0),
            Verdict::Sell { .. }
        ));
    }

    #[test]
    fn a_sale_asks_at_least_the_reference_price() {
        for s in [0.0, 0.3, NEUTRAL_SCORE, 0.8, 1.0] {
            let Verdict::Sell { price } = verdict(s, SRC_1, &[], 1_000.0) else {
                panic!("nothing fitted: always a sale");
            };
            assert!(price >= 1_000.0, "score {s}: {price}");
        }
        assert!(ask_factor(1.0) > ask_factor(KEEP_SCORE));
    }

    #[test]
    fn the_info_answer_is_read() {
        let info = eve_server::pyutil::keyval(vec![
            ("typeID", PyValue::Int(RES_X)),
            (
                "modifiers",
                PyValue::Dict(vec![
                    (PyValue::Real(0.25), PyValue::Int(ATTR_UP)),
                    (PyValue::Real(0.75), PyValue::Int(ATTR_DOWN)),
                ]),
            ),
        ]);
        assert_eq!(type_of(&info), Some(RES_X));
        let mut m = modifiers_of(&info);
        m.sort_by_key(|&(a, _)| a);
        assert_eq!(m, vec![(ATTR_UP, 0.25), (ATTR_DOWN, 0.75)]);
        assert!(modifiers_of(&PyValue::None).is_empty());
    }
}
