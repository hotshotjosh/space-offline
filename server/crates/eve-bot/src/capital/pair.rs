//! **Society capital pairs** (issue #37, lane B): which corp member flies a capital and which
//! member is its cyno alt.
//!
//! A corp member reports each social turn, docked ([`CapReport`]). The hall runs
//! [`match_corp`] over a corp's reports at most once per [`pair_every`] and keeps the
//! result in [`crate::corpmind::HallState::cap_pairs`]. The driver sets
//! [`crate::brain::Policy::partner`] from it; the careers then open through
//! [`crate::capital::lift_blocks`] and the corp need ([`PAIR_NEED`]) lifts the pair's two
//! careers for the paired members only.
//!
//! **Nothing from nowhere** (owner rule): a pilot qualifies only with a hull it holds
//! (`CapReport::jumper`) or one it can buy from listed market asks
//! (`CapReport::buy_ready`: at its last career decision the cheapest ask on a jumper
//! type's book at its station was within [`crate::capital::HULL_WALLET_SHARE`] of the
//! wallet, [`crate::capital::jumper_ask_within`]; read only for a wallet over
//! [`crate::capital::SHOP_WALLET_FLOOR`]). With neither, nobody is paired and no alt is parked. The alt fits its own
//! generator from its hangar or the market ([`crate::capital_fight::fit_effect`]); nothing
//! here grants anything.
//!
//! Everything in the rules is **[A]**; each constant carries its falsifier.

use std::collections::BTreeMap;

/// How long a pair made on a buy-ready pilot (no hull yet) is kept, game days **[A]**.
/// Falsified by a soak where jumper asks sit listed for the grace and the paired pilot never
/// buys (raise it), or alts parked for days on pilots who never buy (lower it).
pub const BUY_GRACE_DAYS: u64 = 2;

/// How long a pilot whose buy-ready pair lapsed waits before it is paired again, game days
/// **[A]**. Falsified by a pilot that is re-paired and lapses again within the same soak
/// (raise it).
pub const COOL_DAYS: u64 = 1;

/// One pair per this many corp members, and at least one **[A]**. Falsified by a 10-bot corp
/// whose two hull holders both sit idle (lower it), or a corp whose builders are all spent as
/// alts (raise it).
pub const PAIR_PER_MEMBERS: usize = 10;

/// How old a member's report may be and still start a pair, game days **[A]**: a bot that
/// went offline keeps its last report in the hall, and an alt parked on it would idle.
/// Falsified by pairs that never form because members report less often than this (raise
/// it), or by a pair made on a bot that has been away for days (lower it).
pub const REPORT_FRESH_DAYS: u64 = 2;

/// How often a corp's pairs are matched, as a share of a game day: one game hour **[A]**
/// (a 24th). Falsified by a matching pass that shows up in a soak's profile (lower the
/// rate) or pairs that form hours after the hull arrives (raise it).
pub const PAIR_EVERY_PER_DAY: u64 = 24;

/// The factor the corp need of a paired member's own pair career is multiplied by **[A]**.
/// Falsified by a paired member that is not in its pair career after three decisions at
/// neutral traits (raise it); the corpmind test pins that it wins at neutral traits.
pub const PAIR_NEED: f64 = 25.0;

/// A game day's gap between matching passes, in the hall's seconds.
pub fn pair_every(day: u64) -> u64 {
    (day / PAIR_EVERY_PER_DAY).max(1)
}

/// What a corp member reports each social turn, docked (not persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapReport {
    pub corp: i64,
    /// A jumper hull it holds (hangar or active ship, [`crate::capital::owned_hull`] over
    /// [`crate::capital::JUMPER_GROUPS`]): its type.
    pub jumper: Option<i64>,
    /// No hull, but a listed jumper ask was within its wallet at its last career decision.
    pub buy_ready: bool,
    /// It owns an assembled non-capital ship (an alt needs a hull of its own).
    pub ship: bool,
    /// A cyno generator sits in its hangar (fitted modules are not in the observation).
    pub generator: bool,
    /// When it reported, in the hall's seconds.
    pub at: u64,
}

/// A capital pilot and its cyno alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub pilot: i64,
    pub alt: i64,
    pub corp: i64,
    /// When the pair formed, in the hall's seconds.
    pub since: u64,
    /// The pilot held a hull when the pair formed or since (false: made on `buy_ready`).
    pub held: bool,
}

/// Why a pair ended, for the hall's act line.
pub fn why_ended(p: &Pair, careers: &BTreeMap<i64, String>, cooled: &[i64]) -> &'static str {
    if !careers.contains_key(&p.pilot) || !careers.contains_key(&p.alt) {
        "a member left the corp"
    } else if cooled.contains(&p.pilot) {
        "no hull within the grace"
    } else {
        "the pilot lost its hull"
    }
}

/// **The corp's pairs after one pass** (pure; same inputs, same pairs).
///
/// - `corp`: the corp; `careers`: every member's career (this is the member list);
///   `reports`: the members' reports (a report of another corp is ignored);
///   `ceo`: the corp's CEO; `had`: the pairs the corp had; `cool`: pilot to the second its
///   cooldown ends; `seen`: member to when it was last heard from, used for a kept pair's
///   half with no report (a restart); `now`; `day`: a game day's length in the same unit.
/// - Returns the corp's pairs, ascending by pilot, and the pilots whose buy-ready pair
///   lapsed this pass (the caller starts their cooldown, [`COOL_DAYS`]).
///
/// 1. **Keep** a pair while both are members and the pilot holds a jumper (`held` turns
///    true). A pair made on `buy_ready` is kept [`BUY_GRACE_DAYS`]; then, still no hull, it
///    ends and the pilot cools. A pair ends at once (no cooldown) when either half was last
///    heard from more than [`REPORT_FRESH_DAYS`] ago (its report, else `seen`; neither: it
///    stands) or when the alt no longer qualifies (no ship, holds a jumper, or is the CEO).
///    A pilot that has not reported (a restart) keeps its pair.
/// 2. **Pilots**: unpaired members with a fresh report holding a jumper first, then
///    `buy_ready` members not cooling; lowest id first. At most `max(1, members /
///    PAIR_PER_MEMBERS)` pairs in all. With no held hull and no buy-ready member nobody is
///    paired.
/// 3. **Alts**: unpaired, not the CEO, no jumper, a fresh report with `ship`; the member
///    with a generator first, then career `cyno_alt`, then the lowest id.
#[allow(clippy::too_many_arguments)]
pub fn match_corp(
    corp: i64,
    reports: &BTreeMap<i64, CapReport>,
    ceo: Option<i64>,
    careers: &BTreeMap<i64, String>,
    had: &[Pair],
    cool: &BTreeMap<i64, u64>,
    seen: &BTreeMap<i64, u64>,
    now: u64,
    day: u64,
) -> (Vec<Pair>, Vec<i64>) {
    let day = day.max(1);
    let mut kept: Vec<Pair> = Vec::new();
    let mut cooled: Vec<i64> = Vec::new();
    for p in had.iter().filter(|p| p.corp == corp) {
        if !careers.contains_key(&p.pilot) || !careers.contains_key(&p.alt) {
            continue;
        }
        let mut p = *p;
        let quiet = |id: i64| {
            reports
                .get(&id)
                .map(|r| r.at)
                .or_else(|| seen.get(&id).copied())
                .is_some_and(|at| now.saturating_sub(at) > REPORT_FRESH_DAYS * day)
        };
        if quiet(p.pilot) || quiet(p.alt) {
            continue;
        }
        let alt_ok = Some(p.alt) != ceo
            && reports
                .get(&p.alt)
                .filter(|r| r.corp == corp)
                .is_none_or(|r| r.ship && r.jumper.is_none());
        if !alt_ok {
            continue;
        }
        match reports.get(&p.pilot).filter(|r| r.corp == corp) {
            // Not heard from since a restart: the pair stands.
            None => {}
            Some(r) if r.jumper.is_some() => p.held = true,
            Some(_) if p.held => continue,
            Some(_) => {
                if now.saturating_sub(p.since) >= BUY_GRACE_DAYS * day {
                    cooled.push(p.pilot);
                    continue;
                }
            }
        }
        kept.push(p);
    }
    let cap = (careers.len() / PAIR_PER_MEMBERS).max(1);
    let taken: Vec<i64> = kept.iter().flat_map(|p| [p.pilot, p.alt]).collect();
    let fresh = |id: &i64| {
        reports
            .get(id)
            .filter(|r| r.corp == corp && careers.contains_key(id))
            .filter(|r| now.saturating_sub(r.at) <= REPORT_FRESH_DAYS * day)
    };
    let mut pilots: Vec<(i64, bool)> = Vec::new();
    // Held hulls first (lowest id first), then the buy-ready ones.
    for held in [true, false] {
        for id in careers.keys().filter(|id| !taken.contains(id)) {
            let Some(r) = fresh(id) else { continue };
            let ok = if held {
                r.jumper.is_some()
            } else {
                r.jumper.is_none() && r.buy_ready && cool.get(id).is_none_or(|until| *until <= now)
            };
            if ok && !cooled.contains(id) {
                pilots.push((*id, held));
            }
        }
    }
    // Generator first, then the cyno_alt career, then the lowest id (`false` sorts first).
    let mut ranked: Vec<(bool, bool, i64)> = careers
        .iter()
        .filter(|(id, _)| !taken.contains(id) && Some(**id) != ceo)
        .filter_map(|(id, career)| {
            let r = fresh(id)?;
            (r.jumper.is_none() && r.ship).then_some((!r.generator, career != "cyno_alt", *id))
        })
        .collect();
    ranked.sort_unstable();
    let mut alts: Vec<i64> = ranked.into_iter().map(|(_, _, id)| id).collect();
    let mut out = kept;
    let mut used: Vec<i64> = Vec::new();
    for (pilot, held) in pilots {
        if out.len() >= cap {
            break;
        }
        if used.contains(&pilot) {
            continue;
        }
        alts.retain(|a| *a != pilot);
        if alts.is_empty() {
            break;
        }
        let alt = alts.remove(0);
        used.extend([pilot, alt]);
        out.push(Pair {
            pilot,
            alt,
            corp,
            since: now,
            held,
        });
    }
    out.sort_by_key(|p| p.pilot);
    (out, cooled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORP: i64 = 9_310_001;
    const DAY: u64 = 86_400;
    const NOW: u64 = 5 * DAY;
    const P1: i64 = 9_300_001;
    const A1: i64 = 9_300_002;
    const HULL: i64 = 9_400_001;

    fn rep(f: impl FnOnce(&mut CapReport)) -> CapReport {
        let mut r = CapReport {
            corp: CORP,
            jumper: None,
            buy_ready: false,
            ship: false,
            generator: false,
            at: NOW,
        };
        f(&mut r);
        r
    }

    fn careers(ids: &[i64]) -> BTreeMap<i64, String> {
        ids.iter().map(|i| (*i, "miner".to_owned())).collect()
    }

    fn run(
        reports: &BTreeMap<i64, CapReport>,
        ceo: Option<i64>,
        careers: &BTreeMap<i64, String>,
        had: &[Pair],
        cool: &BTreeMap<i64, u64>,
    ) -> (Vec<Pair>, Vec<i64>) {
        match_corp(
            CORP,
            reports,
            ceo,
            careers,
            had,
            cool,
            &BTreeMap::new(),
            NOW,
            DAY,
        )
    }

    fn pair(pilot: i64, alt: i64, since: u64, held: bool) -> Pair {
        Pair {
            pilot,
            alt,
            corp: CORP,
            since,
            held,
        }
    }

    #[test]
    fn a_corp_pairs_its_hull_holder_with_a_member_who_owns_a_ship() {
        let reports = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (A1, rep(|r| r.ship = true)),
        ]);
        let (pairs, cooled) = run(&reports, None, &careers(&[P1, A1]), &[], &BTreeMap::new());
        assert_eq!(pairs, vec![pair(P1, A1, NOW, true)]);
        assert!(cooled.is_empty());
    }

    #[test]
    fn nobody_is_paired_without_a_held_hull_or_a_buyable_one() {
        let reports = BTreeMap::from([
            (P1, rep(|r| r.ship = true)),
            (A1, rep(|r| r.ship = true)),
            (9_300_003, rep(|r| r.ship = true)),
        ]);
        let c = careers(&[P1, A1, 9_300_003]);
        let (pairs, _) = run(&reports, None, &c, &[], &BTreeMap::new());
        assert!(pairs.is_empty());
        // A buy-ready pilot pairs (a listed ask it can pay), a cooling one does not.
        let mut reports = reports;
        reports.insert(P1, rep(|r| (r.buy_ready, r.ship) = (true, true)));
        let (pairs, _) = run(&reports, None, &c, &[], &BTreeMap::new());
        assert_eq!(pairs.len(), 1);
        assert!(!pairs[0].held);
        let cool = BTreeMap::from([(P1, NOW + 10)]);
        let (pairs, _) = run(&reports, None, &c, &[], &cool);
        assert!(pairs.is_empty());
    }

    #[test]
    fn the_ceo_and_another_hull_holder_are_never_the_alt() {
        let ceo = 9_300_003;
        let holder2 = 9_300_004;
        let reports = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (holder2, rep(|r| (r.jumper, r.ship) = (Some(HULL), true))),
            (ceo, rep(|r| r.ship = true)),
        ]);
        let c = careers(&[P1, holder2, ceo]);
        let (pairs, _) = run(&reports, Some(ceo), &c, &[], &BTreeMap::new());
        assert!(pairs.is_empty(), "{pairs:?}");
        // Without the CEO rule the CEO would be the alt.
        let (pairs, _) = run(&reports, None, &c, &[], &BTreeMap::new());
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].alt, ceo);
    }

    #[test]
    fn a_member_with_a_generator_is_preferred_then_a_cyno_alt_then_the_lowest_id() {
        let (a, b, c3, d) = (9_300_011, 9_300_012, 9_300_013, 9_300_014);
        let reports = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (a, rep(|r| r.ship = true)),
            (b, rep(|r| r.ship = true)),
            (c3, rep(|r| (r.ship, r.generator) = (true, true))),
            (d, rep(|r| (r.ship, r.generator) = (true, true))),
        ]);
        let mut cs = careers(&[P1, a, b, c3, d]);
        let alt_of =
            |cs: &BTreeMap<i64, String>| run(&reports, None, cs, &[], &BTreeMap::new()).0[0].alt;
        assert_eq!(alt_of(&cs), c3, "the generator, lowest id");
        cs.insert(d, "cyno_alt".to_owned());
        assert_eq!(alt_of(&cs), d, "generator then cyno_alt career");
        let reports2: BTreeMap<i64, CapReport> = reports
            .iter()
            .map(|(k, v)| (*k, rep(|r| (r.jumper, r.ship) = (v.jumper, v.ship))))
            .collect();
        cs.insert(b, "cyno_alt".to_owned());
        let (pairs, _) = run(&reports2, None, &cs, &[], &BTreeMap::new());
        assert_eq!(pairs[0].alt, b, "no generator: the cyno_alt career");
        cs.insert(b, "miner".to_owned());
        cs.insert(d, "miner".to_owned());
        let (pairs, _) = run(&reports2, None, &cs, &[], &BTreeMap::new());
        assert_eq!(pairs[0].alt, a, "nothing else: the lowest id");
    }

    #[test]
    fn pairs_are_capped_at_one_per_ten_members() {
        let n = 2 * PAIR_PER_MEMBERS + 5;
        let ids: Vec<i64> = (0..n as i64).map(|i| 9_301_000 + i).collect();
        let mut reports = BTreeMap::new();
        for id in &ids[..6] {
            reports.insert(*id, rep(|r| r.jumper = Some(HULL)));
        }
        for id in &ids[6..] {
            reports.insert(*id, rep(|r| r.ship = true));
        }
        let (pairs, _) = run(&reports, None, &careers(&ids), &[], &BTreeMap::new());
        assert_eq!(pairs.len(), 2, "two tens and a half: two pairs");
        let small = &ids[..PAIR_PER_MEMBERS - 1];
        let (pairs, _) = run(&reports, None, &careers(small), &[], &BTreeMap::new());
        assert_eq!(pairs.len(), 1, "under ten members, one pair at least");
        // Kept pairs count against the cap.
        let had = [pair(ids[0], ids[6], NOW - 10, true)];
        let (pairs, _) = run(&reports, None, &careers(small), &had, &BTreeMap::new());
        assert_eq!(pairs, had);
    }

    #[test]
    fn a_pair_ends_when_either_leaves_the_corp_or_the_pilot_loses_the_hull() {
        let reports = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (A1, rep(|r| r.ship = true)),
        ]);
        let had = [pair(P1, A1, NOW - 10, true)];
        let both = careers(&[P1, A1]);
        assert_eq!(run(&reports, None, &both, &had, &BTreeMap::new()).0, had);
        for gone in [P1, A1] {
            let mut cs = both.clone();
            cs.remove(&gone);
            let (pairs, _) = run(&reports, None, &cs, &had, &BTreeMap::new());
            assert!(pairs.is_empty(), "{gone} left");
            assert_eq!(why_ended(&had[0], &cs, &[]), "a member left the corp");
        }
        // The pilot sold its hull and cannot buy: the held pair ends, no cooldown.
        let lost = BTreeMap::from([(P1, rep(|_| {})), (A1, rep(|r| r.ship = true))]);
        let (pairs, cooled) = run(&lost, None, &both, &had, &BTreeMap::new());
        assert!(pairs.is_empty() && cooled.is_empty());
        assert_eq!(why_ended(&had[0], &both, &[]), "the pilot lost its hull");
        // Not heard from since a restart: the pair stands.
        let (pairs, _) = run(&BTreeMap::new(), None, &both, &had, &BTreeMap::new());
        assert_eq!(pairs, had);
    }

    #[test]
    fn a_buy_ready_pair_ends_after_the_grace_and_the_pilot_cools_down() {
        let reports = BTreeMap::from([
            (P1, rep(|r| r.buy_ready = true)),
            (A1, rep(|r| r.ship = true)),
        ]);
        let both = careers(&[P1, A1]);
        let young = [pair(P1, A1, NOW - (BUY_GRACE_DAYS * DAY - 1), false)];
        let (pairs, cooled) = run(&reports, None, &both, &young, &BTreeMap::new());
        assert_eq!(pairs, young, "inside the grace");
        assert!(cooled.is_empty());
        let old = [pair(P1, A1, NOW - BUY_GRACE_DAYS * DAY, false)];
        let (pairs, cooled) = run(&reports, None, &both, &old, &BTreeMap::new());
        assert!(
            pairs.is_empty(),
            "the lapsed pilot is not re-paired in the same pass"
        );
        assert_eq!(cooled, vec![P1]);
        assert_eq!(
            why_ended(&old[0], &both, &cooled),
            "no hull within the grace"
        );
        // A hull bought inside the grace makes the pair a held one for good.
        let bought = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (A1, rep(|r| r.ship = true)),
        ]);
        let (pairs, cooled) = run(&bought, None, &both, &old, &BTreeMap::new());
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].held && cooled.is_empty());
    }

    #[test]
    fn the_same_reports_give_the_same_pairs() {
        let ids: Vec<i64> = (0..30).map(|i| 9_302_000 + i).collect();
        let mut reports = BTreeMap::new();
        for (i, id) in ids.iter().enumerate() {
            reports.insert(
                *id,
                rep(|r| {
                    r.jumper = (i % 7 == 0).then_some(HULL);
                    r.buy_ready = i % 5 == 0;
                    r.ship = i % 2 == 0;
                    r.generator = i % 3 == 0;
                }),
            );
        }
        let c = careers(&ids);
        let a = run(&reports, Some(ids[1]), &c, &[], &BTreeMap::new());
        let b = run(&reports, Some(ids[1]), &c, &[], &BTreeMap::new());
        assert_eq!(a, b);
        assert!(!a.0.is_empty());
    }

    #[test]
    fn a_stale_alt_or_a_stale_pilot_ends_a_kept_pair() {
        let had = [pair(P1, A1, NOW - 10, true)];
        let both = careers(&[P1, A1]);
        let old = NOW - (REPORT_FRESH_DAYS + 1) * DAY;
        let alt_stale = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (A1, rep(|r| (r.ship, r.at) = (true, old))),
        ]);
        let (pairs, cooled) = run(&alt_stale, None, &both, &had, &BTreeMap::new());
        assert!(pairs.is_empty() && cooled.is_empty(), "stale alt");
        let pilot_stale = BTreeMap::from([
            (P1, rep(|r| (r.jumper, r.at) = (Some(HULL), old))),
            (A1, rep(|r| r.ship = true)),
        ]);
        let (pairs, _) = run(&pilot_stale, None, &both, &had, &BTreeMap::new());
        assert!(pairs.is_empty(), "stale pilot");
        // No report: the `seen` time stands in for it.
        let seen = BTreeMap::from([(P1, NOW), (A1, old)]);
        let none = BTreeMap::new();
        let nocool = BTreeMap::new();
        let (pairs, _) = match_corp(CORP, &none, None, &both, &had, &nocool, &seen, NOW, DAY);
        assert!(pairs.is_empty(), "stale by seen");
        let seen = BTreeMap::from([(P1, NOW), (A1, NOW)]);
        let (pairs, _) = match_corp(CORP, &none, None, &both, &had, &nocool, &seen, NOW, DAY);
        assert_eq!(pairs, had);
    }

    #[test]
    fn a_kept_pair_ends_when_the_alt_no_longer_qualifies() {
        let had = [pair(P1, A1, NOW - 10, true)];
        let both = careers(&[P1, A1]);
        let pilot = rep(|r| r.jumper = Some(HULL));
        for alt in [
            rep(|_| {}),
            rep(|r| (r.ship, r.jumper) = (true, Some(HULL))),
        ] {
            let reports = BTreeMap::from([(P1, pilot), (A1, alt)]);
            let (pairs, _) = run(&reports, None, &both, &had, &BTreeMap::new());
            assert!(pairs.is_empty(), "{alt:?}");
        }
        let ok = BTreeMap::from([(P1, pilot), (A1, rep(|r| r.ship = true))]);
        assert_eq!(run(&ok, None, &both, &had, &BTreeMap::new()).0, had);
        let (pairs, _) = run(&ok, Some(A1), &both, &had, &BTreeMap::new());
        assert!(pairs.is_empty(), "the CEO");
    }

    #[test]
    fn a_stale_report_starts_no_pair() {
        let reports = BTreeMap::from([
            (P1, rep(|r| r.jumper = Some(HULL))),
            (
                A1,
                rep(|r| (r.ship, r.at) = (true, NOW - (REPORT_FRESH_DAYS + 1) * DAY)),
            ),
        ]);
        let (pairs, _) = run(&reports, None, &careers(&[P1, A1]), &[], &BTreeMap::new());
        assert!(pairs.is_empty());
    }
}
