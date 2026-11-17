//! **The NES lines of the economy report** (lane ECON-LINES, issues #2 and
//! #23): what the day's PLEX, crate opens, SKINs and skill injectors did,
//! by bot career, printed at each rollover as `[econ] plex` and `[econ] nes`
//! and appended to the econ TSV.
//!
//! State: rows of the `econ` delta kind ([`ledger::KIND`]) whose first key
//! element is `"nes"`. [`ledger::prune`] only drops keys whose first element
//! parses as a day, so these survive it, and every other reader of the kind
//! matches its own key shape.
//!
//! - **running** `nes <what> <character> -> count units`: [`note`] adds one
//!   event at the success point of the action; [`fold`] reads them into a
//!   [`Day`] by `(what, career)` and deletes them.
//! - **snapshot** `nes snap plex <key> -> in out`: the PLEX ledger is
//!   cumulative, so the day's movement is the difference from the last fold's
//!   copy. `nes snap day` marks that a fold ran; before it the lines say
//!   `since=start` (the first day counts everything the counters ever saw).
//!
//! `what` is `open:<crate row name>`, `skin`, `inject` or `extract`. For an
//! injection `count` is the injectors used and `units` the points given; for
//! an extraction `units` is the points taken.

use super::TSV_HEADER;
use crate::econ_ledger as ledger;
use crate::service::Registry;
use std::collections::BTreeMap;

/// The first key element of every row of this module.
const TAG: &str = "nes";
/// The second element of the snapshot rows.
const SNAP: &str = "snap";
/// The career of a character the server has no bot row for (a human).
pub const HUMAN: &str = "human";
/// The career of a bot with no journal row in the window.
pub const IDLE: &str = "idle";

fn running_key(what: &str, character: i64) -> Vec<String> {
    vec![TAG.to_owned(), what.to_owned(), character.to_string()]
}

fn pair(v: &[String]) -> (i64, i64) {
    let n = |i: usize| v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
    (n(0), n(1))
}

fn row(a: i64, b: i64) -> Vec<String> {
    vec![a.to_string(), b.to_string()]
}

/// Count one event of `what` by `character`: `count` += `count`, `units` +=
/// `units`. Call it after the success point, before answering.
pub fn note(reg: &mut Registry, character: i64, what: &str, count: i64, units: i64) {
    let key = running_key(what, character);
    let (c0, u0) = reg
        .deltas()
        .of(ledger::KIND)
        .get(&key)
        .map_or((0, 0), |v| pair(v));
    reg.deltas_mut().put(
        ledger::KIND,
        key,
        row(c0.saturating_add(count), u0.saturating_add(units)),
    );
}

/// One day's folded numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Day {
    /// `(what, career) -> (count, units)`.
    pub by: BTreeMap<(String, String), (i64, i64)>,
    /// Users holding PLEX who have a bot: how many.
    pub held_bots: usize,
    /// Whole PLEX held, by the career of the user's first bot.
    pub held_by: BTreeMap<String, i64>,
    /// The folded average price of PLEX, when the market has one.
    pub plex_avg: Option<f64>,
    /// PLEX that came in since the last fold, by ledger key.
    pub plex_in: BTreeMap<String, i64>,
    /// PLEX that went out since the last fold, by ledger key.
    pub plex_out: BTreeMap<String, i64>,
    /// No fold ran before this one.
    pub since_start: bool,
}

fn snap_key(key: &str) -> Vec<String> {
    vec![
        TAG.to_owned(),
        SNAP.to_owned(),
        "plex".to_owned(),
        key.to_owned(),
    ]
}

fn day_key() -> Vec<String> {
    vec![TAG.to_owned(), SNAP.to_owned(), "day".to_owned()]
}

/// Read the running rows into a [`Day`], take the PLEX snapshot difference
/// and the held PLEX, and clear the running rows. `careers` maps a bot to its
/// career; a bot not in it is [`IDLE`], a character that is no bot [`HUMAN`].
pub fn fold(reg: &mut Registry, careers: &BTreeMap<i64, String>) -> Day {
    let career_of = |reg: &Registry, id: i64| -> String {
        match reg.character(id) {
            Some(c) if c.is_bot => careers.get(&id).cloned().unwrap_or_else(|| IDLE.to_owned()),
            _ => HUMAN.to_owned(),
        }
    };
    let mut day = Day {
        since_start: !reg.deltas().of(ledger::KIND).contains_key(&day_key()),
        ..Day::default()
    };
    let running: Vec<(Vec<String>, (i64, i64))> = reg
        .deltas()
        .of(ledger::KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3 && k[0] == TAG && k[1] != SNAP)
        .map(|(k, v)| (k.clone(), pair(v)))
        .collect();
    for (k, (count, units)) in &running {
        let Ok(id) = k[2].parse::<i64>() else {
            continue;
        };
        let e = day
            .by
            .entry((k[1].clone(), career_of(reg, id)))
            .or_default();
        e.0 = e.0.saturating_add(*count);
        e.1 = e.1.saturating_add(*units);
    }
    // PLEX moved: the difference of the cumulative ledger from the snapshot.
    for (key, i, o) in crate::nes_vault::ledger(reg.deltas()) {
        let (i0, o0) = reg
            .deltas()
            .of(ledger::KIND)
            .get(&snap_key(&key))
            .map_or((0, 0), |v| pair(v));
        if i > i0 {
            day.plex_in.insert(key.clone(), i - i0);
        }
        if o > o0 {
            day.plex_out.insert(key.clone(), o - o0);
        }
        reg.deltas_mut()
            .put(ledger::KIND, snap_key(&key), row(i, o));
    }
    // PLEX held by bot accounts, by the first bot's career.
    let mut users: BTreeMap<i64, String> = BTreeMap::new();
    for c in reg.config().characters.iter().filter(|c| c.is_bot) {
        let user = c.owner_user_id.unwrap_or(reg.config().user_id);
        users
            .entry(user)
            .or_insert_with(|| career_of(reg, c.character_id));
    }
    for (user, career) in users {
        let plex = crate::nes_vault::balance(reg.deltas(), user);
        if plex > 0 {
            day.held_bots += 1;
            *day.held_by.entry(career).or_default() += plex;
        }
    }
    day.plex_avg = crate::econ_prices::average(reg, crate::econ_prices::PLEX_TYPE_ID);
    // Clear the running rows and mark the fold.
    let deltas = reg.deltas_mut();
    for (k, _) in running {
        deltas.del(ledger::KIND, &k);
    }
    deltas.put(ledger::KIND, day_key(), vec!["1".to_owned()]);
    day
}

fn list<K: std::fmt::Display, V: std::fmt::Display>(
    it: impl Iterator<Item = (K, V)>,
    sep: &str,
) -> String {
    let v: Vec<String> = it.map(|(k, v)| format!("{k}{sep}{v}")).collect();
    if v.is_empty() {
        "none".to_owned()
    } else {
        v.join(",")
    }
}

/// The `(career, count, units)` of every row of `what` (a prefix when it
/// ends with `:`).
fn of_what<'a>(d: &'a Day, what: &'a str) -> impl Iterator<Item = (&'a str, i64, i64)> {
    d.by.iter()
        .filter(move |((w, _), _)| w == what)
        .map(|((_, c), (n, u))| (c.as_str(), *n, *u))
}

fn crates_of(d: &Day) -> BTreeMap<String, (i64, i64)> {
    let mut m: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for ((w, c), (n, u)) in &d.by {
        if let Some(name) = w.strip_prefix("open:")
            && name != "geodes"
        {
            m.insert(format!("{c}:{name}"), (*n, *u));
        }
    }
    m
}

/// The `[econ] plex` line.
pub fn plex_line(day: u32, d: &Day) -> String {
    let avg = d.plex_avg.map_or("none".to_owned(), |p| format!("{p:.0}"));
    format!(
        "[econ] plex day {day} held_bots={} held_by={} plex_avg={avg} in={} out={}{}",
        d.held_bots,
        list(d.held_by.iter(), ":"),
        list(d.plex_in.iter(), ":"),
        list(d.plex_out.iter(), ":"),
        if d.since_start { " since=start" } else { "" },
    )
}

/// The `[econ] nes` line.
pub fn nes_line(day: u32, d: &Day) -> String {
    let n = |what: &str| list(of_what(d, what).map(|(c, n, _)| (c, n)), ":");
    let nu = |what: &str| {
        list(
            of_what(d, what).map(|(c, n, u)| (c, format!("{n}/{u}"))),
            ":",
        )
    };
    let crates = crates_of(d);
    format!(
        "[econ] nes day {day} geodes={} skins={} injectors={} extractions={} crates={}{}",
        n("open:geodes"),
        n("skin"),
        nu("inject"),
        nu("extract"),
        list(crates.iter().map(|(k, (n, _))| (k, n)), ":"),
        if d.since_start { " since=start" } else { "" },
    )
}

/// The TSV rows (no header) of the day, in the econ report's columns.
pub fn tsv_rows(day: u32, d: &Day) -> Vec<String> {
    let mut v = Vec::new();
    for (kind, what) in [
        ("nes_geode", "open:geodes"),
        ("nes_skin", "skin"),
        ("nes_inject", "inject"),
        ("nes_extract", "extract"),
    ] {
        for (career, n, u) in of_what(d, what) {
            v.push(format!("{day}\tall\t{kind}\t{career}\t{n}\t0\t{u}\t0\t0"));
        }
    }
    for (key, (n, u)) in crates_of(d) {
        v.push(format!("{day}\tall\tnes_crate\t{key}\t{n}\t0\t{u}\t0\t0"));
    }
    for (career, plex) in &d.held_by {
        v.push(format!(
            "{day}\tall\tplex\theld:{career}\t{plex}\t0\t0\t0\t0"
        ));
    }
    if let Some(p) = d.plex_avg {
        v.push(format!("{day}\tall\tplex\tavg\t0\t0\t{p:.0}\t0\t0"));
    }
    for (src, n) in &d.plex_in {
        v.push(format!("{day}\tall\tplex\tin:{src}\t{n}\t0\t0\t0\t0"));
    }
    for (key, n) in &d.plex_out {
        v.push(format!("{day}\tall\tplex\tout:{key}\t{n}\t0\t0\t0\t0"));
    }
    debug_assert!(
        v.iter()
            .all(|l| l.split('\t').count() == TSV_HEADER.split('\t').count())
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::model::{Character, SandboxConfig};

    const BOT: i64 = 9_620_001;
    const HUMAN_ID: i64 = 9_620_002;
    const USER: i64 = 9_620_100;

    fn world() -> Registry {
        let mut w = Registry::new(SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w.seed_character(Character {
            character_id: BOT,
            ship_id: 9_620_011,
            is_bot: true,
            owner_user_id: Some(USER),
            ..Character::default()
        });
        w.seed_character(Character {
            character_id: HUMAN_ID,
            ship_id: 9_620_012,
            ..Character::default()
        });
        w
    }

    fn careers() -> BTreeMap<i64, String> {
        BTreeMap::from([(BOT, "miner".to_owned())])
    }

    #[test]
    fn a_geode_open_counts_once_under_the_openers_career() {
        let mut w = world();
        note(&mut w, BOT, "open:geodes", 1, 1);
        note(&mut w, HUMAN_ID, "open:geodes", 1, 1);
        let d = fold(&mut w, &careers());
        assert_eq!(
            d.by[&("open:geodes".to_owned(), "miner".to_owned())],
            (1, 1)
        );
        assert_eq!(d.by[&("open:geodes".to_owned(), HUMAN.to_owned())], (1, 1));
        let line = nes_line(3, &d);
        assert!(line.contains("geodes=human:1,miner:1"), "{line}");
    }

    #[test]
    fn a_skin_clear_counts_nothing() {
        // The clear path of `skins::apply` has no `note` call; a fold of an
        // empty world is empty.
        let mut w = world();
        let d = fold(&mut w, &careers());
        assert!(d.by.is_empty());
        assert!(nes_line(1, &d).contains("skins=none"));
    }

    #[test]
    fn the_plex_line_differences_two_snapshots() {
        let mut w = world();
        crate::nes_vault::ledger_in(w.deltas_mut(), "invented:src", 10);
        let first = fold(&mut w, &careers());
        assert_eq!(first.plex_in["invented:src"], 10);
        crate::nes_vault::ledger_in(w.deltas_mut(), "invented:src", 4);
        crate::nes_vault::ledger_out(w.deltas_mut(), "invented:sink", 3);
        let second = fold(&mut w, &careers());
        assert_eq!(second.plex_in["invented:src"], 4);
        assert_eq!(second.plex_out["invented:sink"], 3);
        let line = plex_line(5, &second);
        assert!(line.contains("in=invented:src:4"), "{line}");
        assert!(line.contains("out=invented:sink:3"), "{line}");
    }

    #[test]
    fn the_first_fold_says_since_start() {
        let mut w = world();
        let first = fold(&mut w, &careers());
        assert!(first.since_start);
        assert!(plex_line(1, &first).ends_with(" since=start"));
        let second = fold(&mut w, &careers());
        assert!(!second.since_start);
        assert!(!nes_line(2, &second).contains("since=start"));
    }

    #[test]
    fn the_fold_clears_the_running_rows() {
        let mut w = world();
        note(&mut w, BOT, "skin", 1, 1);
        note(&mut w, BOT, "skin", 1, 1);
        assert_eq!(
            fold(&mut w, &careers()).by[&("skin".to_owned(), "miner".to_owned())],
            (2, 2)
        );
        assert!(fold(&mut w, &careers()).by.is_empty());
    }

    #[test]
    fn plex_held_is_summed_by_the_first_bots_career() {
        let mut w = world();
        assert_eq!(fold(&mut w, &careers()).held_bots, 0, "an empty vault");
        crate::nes_vault::apply(w.deltas_mut(), USER, 12, "invented:src", 1).expect("credit");
        let d = fold(&mut w, &careers());
        assert_eq!(d.held_bots, 1);
        assert_eq!(d.held_by["miner"], 12);
        assert!(plex_line(2, &d).contains("held_by=miner:12"));
    }

    #[test]
    fn prune_keeps_nes_rows() {
        let mut w = world();
        crate::nes_vault::ledger_in(w.deltas_mut(), "invented:src", 7);
        note(&mut w, BOT, "inject", 2, 500);
        let _ = fold(&mut w, &careers());
        note(&mut w, BOT, "inject", 1, 100);
        let day = crate::rollover::Day {
            day: 100_000,
            days: 1,
            now: World::FIXTURE.birth,
        };
        ledger::on_rollover(&mut w, &day);
        let d = fold(&mut w, &careers());
        assert!(!d.since_start, "the fold marker survived");
        assert!(d.plex_in.is_empty(), "the snapshot survived");
        assert_eq!(d.by[&("inject".to_owned(), "miner".to_owned())], (1, 100));
    }

    #[test]
    fn the_tsv_rows_have_the_econ_columns() {
        let mut w = world();
        note(&mut w, BOT, "open:geodes", 2, 2);
        note(&mut w, BOT, "extract", 1, 500_000);
        crate::nes_vault::ledger_in(w.deltas_mut(), "invented:src", 3);
        let d = fold(&mut w, &careers());
        let rows = tsv_rows(9, &d);
        assert!(
            rows.contains(&"9\tall\tnes_geode\tminer\t2\t0\t2\t0\t0".to_owned()),
            "{rows:?}"
        );
        assert!(rows.contains(&"9\tall\tnes_extract\tminer\t1\t0\t500000\t0\t0".to_owned()));
        assert!(rows.contains(&"9\tall\tplex\tin:invented:src\t3\t0\t0\t0\t0".to_owned()));
        for l in &rows {
            assert_eq!(l.split('\t').count(), 9, "{l}");
        }
    }
}
