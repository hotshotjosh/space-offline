//! **HyperCore drops** (issue #203): an elite or heavy NPC's wreck may hold a
//! small stack of HyperCores, the token a HyperNet offer consumes. This is the
//! **only** supply: no blueprint makes them (`industry/blueprints.tsv` has no
//! product 52568, spec `parity-dailygoals-hypernet` §0.4), the store sells
//! none (owner question Q5) and no GM or bot kit grants them in closed play.
//! Like PLEX, injectors and filaments, a drop is an **item** in the wreck, so
//! it is looted, hauled and lost with the ship.
//!
//! The roll is a pure function of the world seed and the NPC's id, a bot's
//! chance is [`tunables::BOT_HYPERCORE_MULT`] times a human's, and past the
//! day's count every chance is [`tunables::OVER_BUDGET_MULT`] times itself.
//! The counter is the number of cores (not stacks) made on the day.
//!
//! The type is skipped quietly when the content bundle does not carry it.

use crate::KIND;
use crate::tunables::{self, HypercoreRow};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::FLAG_HANGAR;
use eve_server::module::{Deltas, KillEvent, Settled};
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;

/// The HyperCore (`typeRaffleToken`, `inventorycommon/const.py:2597`; the
/// raffles client's `TOKEN_TYPE_ID`) **[V]**.
pub const HYPERCORE: i64 = 52568;

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the NPC's stream so this module's draws are its own.
const SALT: u64 = 0x4859_5043;

fn day_key() -> Vec<String> {
    vec!["hday".to_owned()]
}

/// HyperCores dropped on `day`: the stored counter, or 0 when it is another
/// day's.
pub fn dropped_on(d: &Deltas, day: i64) -> i64 {
    let Some(v) = d.of(KIND).get(&day_key()) else {
        return 0;
    };
    match (v.first().and_then(|s| s.parse::<i64>().ok()), v.get(1)) {
        (Some(stored), Some(n)) if stored == day => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// The counter row after `qty` more cores on `day`.
fn counted(d: &Deltas, day: i64, qty: i64) -> Vec<String> {
    vec![day.to_string(), (dropped_on(d, day) + qty).to_string()]
}

fn add_dropped(d: &mut Deltas, day: i64, qty: i64) {
    let row = counted(d, day, qty);
    d.put(KIND, day_key(), row);
}

/// How many cores a roll of `row` yields, or `None`. Always draws the chance
/// first and the quantity second, so the stream's use does not depend on the
/// outcome.
pub fn decide(row: &HypercoreRow, s: &mut Stream, bot: bool, over_budget: bool) -> Option<i64> {
    let mut chance = row.chance;
    if bot {
        chance *= tunables::BOT_HYPERCORE_MULT;
    }
    if over_budget {
        chance *= tunables::OVER_BUDGET_MULT;
    }
    let hit = s.chance(chance);
    let qty = 1 + s.range(0, tunables::HYPERCORE_MAX_STACK) as i64;
    hit.then_some(qty)
}

/// True when `table` carries `type_id`: a drop of a type the bundle lacks
/// would be refused by the store, so the roll is skipped first.
fn in_bundle(table: &eve_server::typedata::Table, type_id: i64) -> bool {
    table.get(type_id).is_some()
}

/// The kill-hook row: an NPC killed by a character may leave HyperCores in
/// its wreck. Runs after the wreck is made (`kill_hooks::NPC`).
pub fn on_npc_kill(reg: &mut Registry, ev: &KillEvent) {
    roll(reg, ev, ev.victim_character, None);
}

/// The settle-hook row: an NPC killed in an offline settlement rolls the
/// same table, keyed by the kill's seed, and pays the killer's hangar. A
/// settled kill is a kill that happened.
pub fn on_settled(reg: &mut Registry, s: &Settled) {
    let Some(killer) = s.ev.killer else { return };
    let hangar = Slot::new(killer, s.station, FLAG_HANGAR);
    roll(reg, &s.ev, s.seed, Some(hangar));
}

/// One roll for `ev`, keyed by `key`: into the wreck of `ev`, or into
/// `hangar` when there is no wreck.
fn roll(reg: &mut Registry, ev: &KillEvent, key: i64, hangar: Option<Slot>) {
    let Some(npc) = ev.npc else { return };
    let Some(row) = tunables::hypercore_row_for(npc.bounty) else {
        return;
    };
    let Some(bot) = ev.killer.and_then(|k| reg.character(k)).map(|c| c.is_bot) else {
        return;
    };
    if !in_bundle(
        eve_server::typedata::installed_in(reg.content()).table(),
        HYPERCORE,
    ) {
        return;
    }
    let day = ev.when.div_euclid(DAY_TICKS);
    let over = dropped_on(reg.deltas(), day) >= tunables::DAILY_HYPERCORES;
    let mut s = seed::stream(reg.world().seed, Kind::Loot, &[key as u64, SALT]);
    let Some(qty) = decide(row, &mut s, bot, over) else {
        return;
    };
    let to = match hangar {
        Some(h) => h,
        None => {
            let Some(wreck) = reg
                .combat()
                .wrecks()
                .find(|w| w.ship_id == ev.victim_ship)
                .copied()
            else {
                return;
            };
            Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK)
        }
    };
    if leave_at(reg, to, qty, day) {
        eve_server::outln!("{}", drop_line(qty, ev.killer.unwrap_or(0), npc.bounty));
    }
}

/// The `[econ-mod] hypercores` line of one drop (`hypercores=N` for the soak
/// report to sum).
fn drop_line(qty: i64, killer: i64, bounty: f64) -> String {
    format!("[econ-mod] hypercores: hypercores={qty} for {killer} (bounty {bounty})")
}

/// Put `qty` HyperCores in `wreck` and, only if the store took them, count
/// them against `day`. `true` when it did. Quiet, as PLEX's.
pub fn leave_in(reg: &mut Registry, wreck: &Wreck, qty: i64, day: i64) -> bool {
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    leave_at(reg, to, qty, day)
}

/// As [`leave_in`], into any slot: a wreck, or a parked killer's hangar.
pub fn leave_at(reg: &mut Registry, to: Slot, qty: i64, day: i64) -> bool {
    leave_type(reg, to, HYPERCORE, qty, day)
}

/// As [`leave_at`] for any `type_id` (public so a test can use a type its
/// fixture knows; the module calls it with [`HYPERCORE`] only).
pub fn leave_type(reg: &mut Registry, to: Slot, type_id: i64, qty: i64, day: i64) -> bool {
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: qty,
        to,
    }]);
    if !matches!(made.as_deref(), Ok([Change::Created { .. }, ..])) {
        return false;
    }
    add_dropped(reg.deltas_mut(), day, qty);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::typedata::{FIXTURE, Table};

    const ROW: HypercoreRow = HypercoreRow {
        source: "t",
        min_bounty: 0.0,
        chance: 0.5,
    };

    fn rolls(bot: bool, over: bool) -> Vec<Option<i64>> {
        (0..6_000u64)
            .map(|i| decide(&ROW, &mut Stream::new(i), bot, over))
            .collect()
    }

    #[test]
    fn a_roll_is_deterministic_and_a_stack_is_in_range() {
        let all = rolls(false, false);
        assert_eq!(all, rolls(false, false));
        let max = tunables::HYPERCORE_MAX_STACK as i64;
        assert!(all.iter().flatten().all(|q| (1..=max).contains(q)));
        // Every size in 1..=max turns up.
        for q in 1..=max.min(5) {
            assert!(all.contains(&Some(q)), "no stack of {q}");
        }
    }

    #[test]
    fn a_bot_and_a_late_day_scale_the_chance() {
        let hits = |a: Vec<Option<i64>>| a.iter().flatten().count() as f64;
        let (human, bot, late) = (
            hits(rolls(false, false)),
            hits(rolls(true, false)),
            hits(rolls(false, true)),
        );
        assert!((bot / human - tunables::BOT_HYPERCORE_MULT).abs() < 0.12);
        assert!((late / human - tunables::OVER_BUDGET_MULT).abs() < 0.12);
    }

    #[test]
    fn the_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let hit = decide(&HypercoreRow { chance: 1.0, ..ROW }, &mut a, false, false);
        let miss = decide(&HypercoreRow { chance: 0.0, ..ROW }, &mut b, false, false);
        assert!(hit.is_some() && miss.is_none());
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rows_are_ordered_from_the_top_and_a_cheap_rat_has_none() {
        assert!(
            tunables::HYPERCORE_ROWS
                .windows(2)
                .all(|w| w[0].min_bounty > w[1].min_bounty)
        );
        assert!(tunables::hypercore_row_for(0.0).is_none());
        assert!(tunables::hypercore_row_for(f64::MAX).is_some());
        assert!(
            tunables::HYPERCORE_ROWS
                .iter()
                .all(|r| (0.0..=1.0).contains(&r.chance))
        );
    }

    #[test]
    fn a_type_the_bundle_lacks_is_skipped() {
        let table = Table::parse(FIXTURE).expect("the fixture parses");
        // 990000001 is the fixture's own frigate; 990999999 is in no table.
        assert!(in_bundle(&table, 990_000_001));
        assert!(!in_bundle(&table, 990_999_999));
    }

    #[test]
    fn the_day_counter_adds_cores_and_resets_on_a_new_day() {
        let mut d = Deltas::default();
        assert_eq!(dropped_on(&d, 7), 0);
        // `put` refuses a kind that delta_kinds (a core file) does not list.
        let row = counted(&d, 7, 3);
        d.put_verbatim(KIND, day_key(), row);
        let row = counted(&d, 7, 2);
        d.put_verbatim(KIND, day_key(), row);
        assert_eq!(dropped_on(&d, 7), 5);
        assert_eq!(dropped_on(&d, 8), 0);
        assert_eq!(
            drop_line(3, 5, 1200.0),
            "[econ-mod] hypercores: hypercores=3 for 5 (bounty 1200)"
        );
    }
}
