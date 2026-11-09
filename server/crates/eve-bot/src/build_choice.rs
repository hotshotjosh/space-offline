//! **FE-BUILD: a builder chooses what to build by margin** (R-FLUIDECON
//! 3.3 and the FE-BUILD packet; DOC-7's demand map).
//!
//! Until now a builder walked the ladder in rank order and `return`ed after
//! one blueprint, so the cheapest rung it held was built whatever the
//! market paid. Here it **appraises every blueprint it holds and can feed**:
//! the product at `V` (the value model, `pricing::Prices`), less the inputs
//! at `V` with the broker's fee, the listing's tax and an install fee, per
//! hour of the job's time ([`appraise`]). The jobs are ranked by that
//! ([`order_jobs`]), a job that loses money is skipped, and the builder
//! starts as many as it has free slots ([`jobs_to_start`]).
//!
//! **Demand tilts the choice.** The book's bids against its asks for the
//! product, and the units the leaders' doctrines want of a part
//! ([`crate::doctrine_parts::PartsBoard`]), make a pressure in -1..1
//! ([`pressure_of`]); the product's revenue is tilted by [`DEMAND_TILT`] of
//! it. A rung whose product is wanted climbs ahead of the ladder's order
//! ([`pull_demanded`]) once the pressure has held for [`FE_DEMAND_CYCLES`]
//! cycles or a doctrine names the part.
//!
//! Pure: no socket, no clock, no random. Every number is **[A]**; each
//! falsifier is on its const.

use crate::pricing::Rates;
use eve_server::industry::{self, Blueprint};
use std::collections::BTreeMap;

/// The bid pressure on a product (in -1..1) at which a builder buys the
/// blueprint ahead of the ladder's order **[A]** (R-FLUIDECON
/// `FE_DEMAND_BUY`). Falsified by builders that climb for a one-bid blip
/// (too low) or never pull a rung while hulls sell out (too high).
pub const FE_DEMAND_BUY: f64 = 0.3;
/// Cycles the pressure must hold before the pull **[A]**. Falsified by a
/// pull that follows a single human's bid and is gone next cycle.
pub const FE_DEMAND_CYCLES: u32 = 2;
/// Job-runs of inputs a builder keeps ahead for the job it will start next
/// **[A]**: it rests bids for this many jobs' worth while its slots are
/// full, so the next job starts the cycle a slot frees. Falsified by idle
/// slots waiting on inputs (too low) or wallets locked in bids (too high).
pub const FE_INPUT_RUNS: i64 = 2;
/// How much of a product's revenue the demand pressure moves **[A]**: at
/// full pressure the job is worth this much more (less at full glut).
/// Falsified by the tape's job-hours by basket not following a bid-heavy
/// shift (too low) or whipsawing on it (too high).
pub const DEMAND_TILT: f64 = 0.15;
/// The system cost index a bot assumes when it holds no industry table
/// **[A]** (the server's shipped default row, `content/econ/cost-index.tsv`
/// system 0, manufacturing). Falsified by an install whose ledger line
/// (refType 56) is far from `(this + SCC) x EIV`.
pub const FALLBACK_COST_INDEX: f64 = 0.04;
/// The station owner's tax on a job, as a share of the base cost: an NPC
/// station takes none ([`industry::NPC_STATION_TAX`], the server's own
/// constant; its value is **[A]** there, falsified by a captured `InstallJob`
/// preview at an NPC station showing a tax).
pub const NPC_STATION_TAX: f64 = industry::NPC_STATION_TAX;
/// Most picks that rest standing bids in one cycle **[A]**: the wallet
/// share bounds the sum, this bounds the spread. Falsified by stalls on
/// `NotEnoughMoney` (too high).
pub const BID_PICKS: usize = 2;

/// What one blueprint is worth to build, per hour of its slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appraisal {
    pub blueprint: i64,
    pub product: i64,
    /// ISK a run earns, net of the listing's tax and fee, less inputs at
    /// their value with the bid fee and the install fee.
    pub profit_per_run: f64,
    /// Game hours a run holds a slot.
    pub run_hours: f64,
    pub profit_per_hour: f64,
}

/// The pressure of `bids` against `asks`, in -1..1 (0 with neither).
pub fn pressure_of(bids: i64, asks: i64) -> f64 {
    let (b, a) = (bids.max(0) as f64, asks.max(0) as f64);
    if b + a <= 0.0 {
        0.0
    } else {
        ((b - a) / (b + a)).clamp(-1.0, 1.0)
    }
}

/// **The job fee as a share of the estimated item value** (EIV), mirroring
/// the server (`industry::cost_lines`): the system cost index times the base,
/// plus the SCC surcharge ([`industry::SCC_SURCHARGE_RATE`]) and the
/// station's tax ([`NPC_STATION_TAX`]), both on the unmodified base. `index`
/// is the system's manufacturing index; `None` (a bot with no industry
/// table) reads [`FALLBACK_COST_INDEX`]; a value outside `(0, 1]` is the
/// table's row the server would skip, so it reads as the fallback too.
pub fn install_fee_share(index: Option<f64>) -> f64 {
    let index = index
        .filter(|i| i.is_finite() && *i > 0.0 && *i <= 1.0)
        .unwrap_or(FALLBACK_COST_INDEX);
    index + industry::SCC_SURCHARGE_RATE + NPC_STATION_TAX
}

/// The manufacturing cost index of `recipes`: the default row (system 0),
/// which the server applies to every system without its own row; 1.0 with
/// no row at all, as the server charges (`CostIndex::get`). The bot does
/// not read the station's system row **[A]**, falsified by an authored
/// per-system row that differs from the default.
pub fn manufacturing_index(recipes: &industry::Table) -> f64 {
    recipes.cost_index.get(0, industry::MANUFACTURING)
}

/// **One run's profit** (the one place [`appraise`] and the career signal
/// price a job): the product sold from a resting ask, net of tax and fee and
/// tilted by `pressure`, less the inputs bought with resting bids
/// (`input_value`, at their prices and the run's ME), less the job fee
/// `fee_share x eiv` (`eiv`: the recipe's base materials at the same
/// prices, **[A]** for the server's adjusted-price EIV; falsified by the
/// refType 56 lines differing from this).
pub fn run_profit(
    product_value: f64,
    input_value: f64,
    eiv: f64,
    rates: Rates,
    fee_share: f64,
    pressure: f64,
) -> f64 {
    let tilt = 1.0 + DEMAND_TILT * pressure.clamp(-1.0, 1.0);
    rates.net_of_listing(product_value) * tilt - rates.cost_of_bid(input_value) - eiv * fee_share
}

/// **The appraisal** of `bp` at ME `me`: the product at `mkt[product]`, each
/// input at `mkt[input]` (the `V` of [`crate::pricing::Prices::mkt_of`]),
/// `pressure` the demand for the product, `fee_share` the job fee
/// ([`install_fee_share`]). `None` when a price is missing or the recipe
/// yields nothing.
pub fn appraise(
    bp: &Blueprint,
    me: i64,
    runs: i64,
    mkt: &BTreeMap<i64, f64>,
    pressure: f64,
    rates: Rates,
    fee_share: f64,
) -> Option<Appraisal> {
    let runs = runs.max(1);
    let price = |t: i64| mkt.get(&t).copied().filter(|p| *p > 0.0);
    let product = price(bp.product_type_id)?;
    let units = bp.product_quantity.max(0) as f64;
    if units <= 0.0 || bp.time <= 0 {
        return None;
    }
    let mut inputs = 0.0;
    for (t, q) in crate::builder::materials_for(bp, runs, me) {
        inputs += q as f64 * price(t)?;
    }
    let mut eiv = 0.0;
    for &(t, q) in &bp.materials {
        eiv += q.max(0) as f64 * price(t)?;
    }
    let profit = run_profit(
        product * units,
        inputs / runs as f64,
        eiv,
        rates,
        fee_share,
        pressure,
    );
    let hours = bp.time as f64 / 3_600.0;
    Some(Appraisal {
        blueprint: bp.blueprint_type_id,
        product: bp.product_type_id,
        profit_per_run: profit,
        run_hours: hours,
        profit_per_hour: profit / hours,
    })
}

/// The lowest share of a dropped job's bid prices **[A]**: a bid cannot be
/// cancelled (no verb), so it is brought down toward break-even and no lower
/// than this. Falsified by dropped jobs' bids still filling at a loss (too
/// high) or modification fees on prices nobody could ever meet (too low).
pub const DROPPED_BID_FLOOR: f64 = 0.05;

/// **The scale that brings a losing job's input prices to break-even**
/// (pure): `profit_per_run` is the run's profit at the prices the inputs
/// were appraised at, `input_cost_per_run` what those inputs cost a run
/// (with the bid fee). A job that pays scales by 1; one that loses scales
/// the inputs down by the loss, never below [`DROPPED_BID_FLOOR`].
pub fn break_even_scale(profit_per_run: f64, input_cost_per_run: f64) -> f64 {
    let losing = profit_per_run.is_finite() && profit_per_run < 0.0;
    if !losing || !input_cost_per_run.is_finite() || input_cost_per_run <= 0.0 {
        return 1.0;
    }
    ((input_cost_per_run + profit_per_run) / input_cost_per_run).clamp(DROPPED_BID_FLOOR, 1.0)
}

/// **The jobs worth doing, best first**: a job that does not pay
/// (`profit_per_hour <= 0`) is dropped; the rest by profit per hour, then
/// by `rank` (the ladder's, higher first), then by blueprint id. `T` is
/// whatever the caller carries along (a blueprint row, its bids).
pub fn order_jobs<T>(jobs: Vec<(T, Appraisal, i64)>) -> Vec<(T, Appraisal)> {
    let mut v: Vec<(T, Appraisal, i64)> = jobs
        .into_iter()
        .filter(|(_, a, _)| a.profit_per_hour.is_finite() && a.profit_per_hour > 0.0)
        .collect();
    v.sort_by(|a, b| {
        b.1.profit_per_hour
            .total_cmp(&a.1.profit_per_hour)
            .then(b.2.cmp(&a.2))
            .then(a.1.blueprint.cmp(&b.1.blueprint))
    });
    v.into_iter().map(|(t, a, _)| (t, a)).collect()
}

/// The first `free` of the ranked jobs, each product once (two originals of
/// one blueprint are one choice).
pub fn jobs_to_start<T: Clone>(ranked: &[(T, Appraisal)], free: usize) -> Vec<(T, Appraisal)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (t, a) in ranked {
        if out.len() >= free {
            break;
        }
        if seen.contains(&a.product) {
            continue;
        }
        seen.push(a.product);
        out.push((t.clone(), *a));
    }
    out
}

/// The runs of inputs to rest bids for while the slots are full: the job's
/// `planned` runs times [`FE_INPUT_RUNS`].
pub fn buffered_runs(planned: i64) -> i64 {
    planned.max(0).saturating_mul(FE_INPUT_RUNS)
}

/// Does the demand for a product call for its blueprint ahead of the
/// ladder? A doctrine wants it (`wanted_units > 0`), or the pressure has
/// been at least [`FE_DEMAND_BUY`] for [`FE_DEMAND_CYCLES`] cycles.
pub fn pulls(wanted_units: i64, streak: u32) -> bool {
    wanted_units > 0 || streak >= FE_DEMAND_CYCLES
}

/// **Demand first**: `rungs` with those `demanded` accepts moved to the
/// front, the order kept within each half (the ladder's, and the demand
/// order among the demanded).
pub fn pull_demanded(rungs: Vec<Vec<i64>>, demanded: &dyn Fn(&[i64]) -> bool) -> Vec<Vec<i64>> {
    let (mut front, back): (Vec<_>, Vec<_>) = rungs.into_iter().partition(|r| demanded(r));
    front.extend(back);
    front
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCT_A: i64 = 991_100_001;
    const PRODUCT_B: i64 = 991_100_002;
    const INPUT: i64 = 991_100_101;
    const BP_A: i64 = 991_100_201;
    const BP_B: i64 = 991_100_202;

    fn bp(id: i64, product: i64, qty: i64, input_qty: i64, time: i64) -> Blueprint {
        Blueprint {
            blueprint_type_id: id,
            product_type_id: product,
            product_quantity: qty,
            time,
            max_production_limit: 100,
            materials: vec![(INPUT, input_qty)],
            skills: vec![],
        }
    }

    fn mkt(a: f64, b: f64, input: f64) -> BTreeMap<i64, f64> {
        BTreeMap::from([(PRODUCT_A, a), (PRODUCT_B, b), (INPUT, input)])
    }

    fn rated(bp: &Blueprint, m: &BTreeMap<i64, f64>, pressure: f64) -> Option<Appraisal> {
        appraise(bp, 0, 1, m, pressure, Rates::UNSKILLED, fee())
    }

    /// An invented system index for the tests, not a reading of any table.
    const TEST_INDEX: f64 = 0.05;

    fn fee() -> f64 {
        install_fee_share(Some(TEST_INDEX))
    }

    #[test]
    fn chooses_the_job_with_the_best_profit_per_hour() {
        // Same inputs; B pays more per hour although A pays more per run.
        let a = bp(BP_A, PRODUCT_A, 1, 10, 7_200);
        let b = bp(BP_B, PRODUCT_B, 1, 10, 1_800);
        let m = mkt(1_000.0, 800.0, 10.0);
        let (ra, rb) = (rated(&a, &m, 0.0).unwrap(), rated(&b, &m, 0.0).unwrap());
        assert!(ra.profit_per_run > rb.profit_per_run);
        let ranked = order_jobs(vec![("a", ra, 5), ("b", rb, 0)]);
        assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), ["b", "a"]);
    }

    #[test]
    fn negative_margin_job_is_skipped() {
        let a = bp(BP_A, PRODUCT_A, 1, 100, 3_600);
        let m = mkt(50.0, 50.0, 10.0);
        let r = rated(&a, &m, 0.0).unwrap();
        assert!(r.profit_per_hour < 0.0);
        assert!(order_jobs(vec![((), r, 0)]).is_empty());
    }

    #[test]
    fn a_missing_price_is_no_appraisal() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let mut m = mkt(100.0, 100.0, 1.0);
        m.remove(&INPUT);
        assert!(rated(&a, &m, 0.0).is_none());
    }

    #[test]
    fn two_free_slots_two_jobs() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let b = bp(BP_B, PRODUCT_B, 1, 10, 3_600);
        let m = mkt(500.0, 400.0, 10.0);
        let ranked = order_jobs(vec![
            (1, rated(&a, &m, 0.0).unwrap(), 0),
            (2, rated(&b, &m, 0.0).unwrap(), 0),
            // A second original of A's blueprint is the same choice.
            (3, rated(&a, &m, 0.0).unwrap(), 0),
        ]);
        assert_eq!(jobs_to_start(&ranked, 2).len(), 2);
        assert_eq!(jobs_to_start(&ranked, 1).len(), 1);
        assert_eq!(jobs_to_start(&ranked, 5).len(), 2, "each product once");
        assert!(jobs_to_start(&ranked, 0).is_empty());
    }

    #[test]
    fn demand_pressure_tilts_the_choice_within_its_bound() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        let m = mkt(500.0, 500.0, 10.0);
        let cold = rated(&a, &m, -1.0).unwrap();
        let flat = rated(&a, &m, 0.0).unwrap();
        let hot = rated(&a, &m, 1.0).unwrap();
        assert!(cold.profit_per_hour < flat.profit_per_hour);
        assert!(flat.profit_per_hour < hot.profit_per_hour);
        let net = Rates::UNSKILLED.net_of_listing(500.0);
        assert!((hot.profit_per_run - flat.profit_per_run - DEMAND_TILT * net).abs() < 1e-6);
        assert_eq!(pressure_of(10, 0), 1.0);
        assert_eq!(pressure_of(0, 0), 0.0);
        assert!(pressure_of(1, 3) < 0.0);
    }

    #[test]
    fn input_buffer_keeps_two_runs() {
        assert_eq!(buffered_runs(5), 5 * FE_INPUT_RUNS);
        assert_eq!(buffered_runs(0), 0);
        assert_eq!(buffered_runs(-3), 0);
    }

    #[test]
    fn demanded_product_pulls_its_bpo_ahead_of_the_ladder() {
        let ladder = vec![vec![1, 2], vec![3], vec![4, 5], vec![6]];
        let hot = |r: &[i64]| r.contains(&4) || r.contains(&6);
        assert_eq!(
            pull_demanded(ladder.clone(), &hot),
            vec![vec![4, 5], vec![6], vec![1, 2], vec![3]]
        );
        assert_eq!(pull_demanded(ladder.clone(), &|_| false), ladder);
        // A doctrine naming the part pulls at once; a bid streak needs its cycles.
        assert!(pulls(3, 0));
        assert!(!pulls(0, FE_DEMAND_CYCLES - 1));
        assert!(pulls(0, FE_DEMAND_CYCLES));
    }

    /// The fee mirrors the server's lines: the index on the base, plus the
    /// SCC surcharge on the base, plus the NPC station's nothing.
    #[test]
    fn the_install_fee_is_the_servers_index_plus_scc_on_the_base() {
        // Against the server's own lines: an NPC station, EIV 10,000.
        let eiv = 10_000.0;
        let facility =
            industry::Facility::npc_station(69_000_001, 69_000_002, 69_000_003, 69_000_004);
        let server = industry::cost_lines(eiv, industry::MANUFACTURING, &facility, TEST_INDEX);
        let mine = eiv * install_fee_share(Some(TEST_INDEX));
        assert!(
            (server.total() as f64 - mine).abs() <= 2.0,
            "server {} vs bot {mine}",
            server.total()
        );
        assert_eq!(
            install_fee_share(Some(TEST_INDEX)),
            TEST_INDEX + industry::SCC_SURCHARGE_RATE + NPC_STATION_TAX
        );
        // EIV 1,000: the index line, the SCC line, no station tax.
        let eiv = 1_000.0;
        let lines = eiv * TEST_INDEX + eiv * industry::SCC_SURCHARGE_RATE;
        assert!((eiv * install_fee_share(Some(TEST_INDEX)) - lines).abs() < 1e-9);
        // No table, or a row the server would skip: the documented fallback.
        for bad in [None, Some(0.0), Some(2.0), Some(f64::NAN)] {
            assert_eq!(
                install_fee_share(bad),
                FALLBACK_COST_INDEX + industry::SCC_SURCHARGE_RATE
            );
        }
    }

    /// A losing job's inputs scale down by the loss, to break-even.
    #[test]
    fn a_losing_jobs_inputs_come_down_to_break_even() {
        assert_eq!(break_even_scale(10.0, 100.0), 1.0, "a job that pays");
        assert_eq!(break_even_scale(-25.0, 100.0), 0.75);
        assert_eq!(break_even_scale(-500.0, 100.0), DROPPED_BID_FLOOR);
        assert_eq!(break_even_scale(-5.0, 0.0), 1.0, "no inputs, no scale");
        assert_eq!(break_even_scale(f64::NAN, 100.0), 1.0);
    }

    /// A recipe that pays before the job fee and loses after it: the fee is
    /// in the appraisal, and the choice drops the job.
    #[test]
    fn the_job_fee_can_turn_a_thin_margin_negative() {
        let a = bp(BP_A, PRODUCT_A, 1, 10, 3_600);
        // Inputs 100 at 10; the product sells for 200 gross.
        let m = mkt(200.0, 200.0, 10.0);
        let free = appraise(&a, 0, 1, &m, 0.0, Rates::UNSKILLED, 0.0).unwrap();
        let paid = rated(&a, &m, 0.0).unwrap();
        assert!((free.profit_per_run - paid.profit_per_run - 100.0 * fee()).abs() < 1e-9);
        let pricey = appraise(&a, 0, 1, &m, 0.0, Rates::UNSKILLED, 0.9).unwrap();
        assert!(pricey.profit_per_hour < 0.0);
        assert!(order_jobs(vec![((), pricey, 0)]).is_empty());
    }
}
