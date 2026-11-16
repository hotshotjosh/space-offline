//! **An online budget for the bots (#257 ONLINE-BUDGET).**
//!
//! `eve-bots --max-online N` caps how many bots are logged on at once, for
//! the whole process; `--online-profile FILE` scales the cap by the hour of
//! the day (`content/bots/online-budget.tsv`, 24 rows `hour<TAB>fraction`).
//! The limit at an hour is `floor(N x fraction(hour))`. With no `--max-online`
//! nothing is installed and every ask is granted, so the default is
//! unchanged.
//!
//! A bot asks before it connects ([`ask`]). If the budget is full it waits,
//! docked and offline: it has made no call, so its absence is the ordinary
//! offline settlement (the `econAgent.Park` of its last log-off) and the
//! budget grants nothing extra. The slots go in this order:
//!
//! 1. a bot the bubble pulled or leased for a human ([`Rank::Human`], set by
//!    `bubble.rs`);
//! 2. a wing or fleet member ([`Rank::Fleet`], set by the driver while the
//!    bot sits in a fleet), so a fleet is not left half there;
//! 3. everyone else, the longest waiting first.
//!
//! When the limit drops (an hour boundary) [`over`] says yes for the bots
//! that have been online longest, as many as the excess; the driver ends
//! their session through the ordinary `release_turn` (home, dock, park),
//! never one that is busy (a fight, a warp, transit) and never one leased to
//! a human (the bubble's shed guards). A priority never evicts a bot
//! that is already online; it only goes first in the queue.
//!
//! The pure part is [`Budget`] (time and hour are arguments, so the tests
//! need no clock); the process part is [`install`] and the free functions.
//!
//! A compressed `--day-secs` day is spread over the 24 profile hours from
//! the Unix epoch ([`hour_of`]), the society timetable's own anchor **[V]**
//! (`driver::Timetable::online` takes `now % period`; `weekly_with` puts
//! window k at `k * day + offset`). **[A]** The profile hour is UTC, not
//! local time, as `population.tsv`'s is. Falsifier: the owner reading the
//! hourly shape against local time. **[A]** A fleet rank is remembered from
//! the bot's last session, because a bot offline cannot be seen in its fleet. Falsifier: a
//! fleet found half there after a restart. **[A]** A fraction-by-hour cap
//! with no per-system cap is enough. Falsifier: `work_p95` following the
//! hot-system count, not the bots online.

use crate::clock;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

/// A waiter that has not asked again for this long has gone; it no longer
/// holds a place in the queue **[A]**. Falsified by a bot that waits longer
/// than this between asks (the driver asks every [`POLL_S`]).
pub const WAITER_TTL_MS: u64 = 120_000;

/// How often a waiting bot asks again, seconds **[A]**.
pub const POLL_S: u64 = 5;

/// Who gets a free slot first; lower is first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// Pulled or leased by the bubble for a human.
    Human,
    /// A wing or fleet member.
    Fleet,
    /// Anyone else.
    Other,
}

/// The hour-of-day fractions.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    frac: [f64; 24],
}

impl Profile {
    /// Parse 24 rows `hour<TAB>fraction`; `#` starts a comment, blank lines
    /// are skipped. Every hour 0-23 exactly once, each fraction in 0..=1.
    pub fn parse(text: &str) -> Result<Profile, String> {
        let mut frac = [f64::NAN; 24];
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let at = i + 1;
            let mut cols = line.split('\t');
            let (Some(h), Some(f), None) = (cols.next(), cols.next(), cols.next()) else {
                return Err(format!("line {at}: want hour<TAB>fraction"));
            };
            let h: usize = h
                .trim()
                .parse()
                .ok()
                .filter(|h| *h < 24)
                .ok_or_else(|| format!("line {at}: hour {h:?} is not 0-23"))?;
            let f: f64 = f
                .trim()
                .parse()
                .ok()
                .filter(|f: &f64| f.is_finite() && (0.0..=1.0).contains(f))
                .ok_or_else(|| format!("line {at}: fraction {f:?} is not 0 to 1"))?;
            if !frac[h].is_nan() {
                return Err(format!("line {at}: hour {h} twice"));
            }
            frac[h] = f;
        }
        if let Some(h) = frac.iter().position(|f| f.is_nan()) {
            return Err(format!("hour {h} is missing (want all 24)"));
        }
        Ok(Profile { frac })
    }

    /// The fraction at `hour` (0-23; larger wraps).
    pub fn at(&self, hour: u32) -> f64 {
        self.frac[(hour % 24) as usize]
    }
}

/// The answer to an ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Log on.
    Granted,
    /// Stay offline: `online` of `limit` slots are taken.
    Wait { online: usize, limit: usize },
}

#[derive(Debug, Clone)]
struct Waiter {
    rank: Rank,
    since: u64,
    last_ask: u64,
}

/// The budget's books.
#[derive(Debug, Clone)]
pub struct Budget {
    max: usize,
    profile: Option<Profile>,
    /// Bots online, and since when (ms).
    online: BTreeMap<String, u64>,
    waiting: BTreeMap<String, Waiter>,
    human: BTreeSet<String>,
    fleet: BTreeSet<String>,
}

impl Budget {
    pub fn new(max: usize, profile: Option<Profile>) -> Budget {
        Budget {
            max,
            profile,
            online: BTreeMap::new(),
            waiting: BTreeMap::new(),
            human: BTreeSet::new(),
            fleet: BTreeSet::new(),
        }
    }

    /// `min(N, N x fraction(hour))`.
    pub fn limit(&self, hour: u32) -> usize {
        match &self.profile {
            None => self.max,
            Some(p) => ((self.max as f64) * p.at(hour)).floor() as usize,
        }
        .min(self.max)
    }

    pub fn online(&self) -> usize {
        self.online.len()
    }

    pub fn rank_of(&self, bot: &str) -> Rank {
        if self.human.contains(bot) {
            Rank::Human
        } else if self.fleet.contains(bot) {
            Rank::Fleet
        } else {
            Rank::Other
        }
    }

    /// The bubble pulled (`true`) or returned (`false`) this bot.
    pub fn set_human(&mut self, bot: &str, on: bool) {
        if on {
            self.human.insert(bot.to_owned());
        } else {
            self.human.remove(bot);
        }
    }

    /// The bot is (`true`) or is not in a fleet.
    pub fn set_fleet(&mut self, bot: &str, on: bool) {
        if on {
            self.fleet.insert(bot.to_owned());
        } else {
            self.fleet.remove(bot);
        }
    }

    /// A bot wants to log on at `now` (ms, monotonic) in `hour`.
    pub fn ask(&mut self, bot: &str, now: u64, hour: u32) -> Ask {
        if self.online.contains_key(bot) {
            return Ask::Granted;
        }
        let limit = self.limit(hour);
        self.waiting
            .retain(|_, w| now.saturating_sub(w.last_ask) <= WAITER_TTL_MS);
        let rank = self.rank_of(bot);
        let me = self.waiting.entry(bot.to_owned()).or_insert(Waiter {
            rank,
            since: now,
            last_ask: now,
        });
        me.rank = rank;
        me.last_ask = now;
        let (my_rank, my_since) = (me.rank, me.since);
        let ahead = self
            .waiting
            .iter()
            .filter(|(n, w)| (w.rank, w.since, n.as_str()) < (my_rank, my_since, bot))
            .count();
        let free = limit.saturating_sub(self.online.len());
        if ahead < free {
            self.waiting.remove(bot);
            self.online.insert(bot.to_owned(), now);
            Ask::Granted
        } else {
            Ask::Wait {
                online: self.online.len(),
                limit,
            }
        }
    }

    /// The bot's session ended (or it gave up waiting): free its slot.
    pub fn release(&mut self, bot: &str) {
        self.online.remove(bot);
        self.waiting.remove(bot);
    }

    /// Should `bot` end its session now? Yes when more bots are online than
    /// the limit and it is among the longest online, as many as the excess.
    /// Never when `busy` (a fight, a warp or in transit: the bubble's own
    /// test, [`crate::bubble::is_busy`]): it is asked again next time. Never
    /// a bot the bubble leased to a human (the bubble's shed guard): such a
    /// bot is skipped, and the next longest online goes in its place. A busy
    /// bot's place is not passed on: online can stay above the limit until
    /// its fight or warp ends.
    pub fn over(&self, bot: &str, hour: u32, busy: bool) -> bool {
        let excess = self.online.len().saturating_sub(self.limit(hour));
        if busy || excess == 0 || !self.online.contains_key(bot) || self.human.contains(bot) {
            return false;
        }
        let mut order: Vec<(&u64, &String)> = self
            .online
            .iter()
            .filter(|(n, _)| !self.human.contains(n.as_str()))
            .map(|(n, t)| (t, n))
            .collect();
        order.sort();
        order.iter().take(excess).any(|(_, n)| n.as_str() == bot)
    }
}

static BUDGET: OnceLock<Mutex<Budget>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
static DAY_SECS: OnceLock<u64> = OnceLock::new();

/// Turn the process budget on. The first call wins. `day_secs` is the
/// length of a day in seconds (`--day-secs`, else 86400): the profile's 24
/// hours are spread over it.
pub fn install(max: usize, profile: Option<Profile>, day_secs: u64) {
    EPOCH.get_or_init(clock::instant);
    let _ = DAY_SECS.set(day_secs.max(1));
    let _ = BUDGET.set(Mutex::new(Budget::new(max, profile)));
}

fn now_ms() -> u64 {
    EPOCH.get().map_or(0, |e| {
        clock::since(*e).as_millis().min(u128::from(u64::MAX)) as u64
    })
}

/// The profile hour (0-23) at `unix_s` when a day lasts `day_secs`: the day
/// starts at the Unix epoch, as the society timetable's does [V], so a
/// compressed `--day-secs` day runs through all 24 hours. Pure.
pub fn hour_of(unix_s: u64, day_secs: u64) -> u32 {
    let day = u128::from(day_secs.max(1));
    ((u128::from(unix_s) % day) * 24 / day) as u32
}

/// The profile hour now: UTC for a 86400 s day, else scaled by `--day-secs`.
pub fn hour_now() -> u32 {
    let s = clock::unix_secs();
    hour_of(s, DAY_SECS.get().copied().unwrap_or(86_400))
}

/// Run `f` on the books. A lock poisoned by a panicking bot thread is
/// taken anyway: a panic elsewhere never turns the cap off. `default` only
/// when no budget is installed.
fn with<R>(default: R, f: impl FnOnce(&mut Budget) -> R) -> R {
    match BUDGET.get() {
        Some(m) => f(&mut locked(m)),
        None => default,
    }
}

/// Take the lock even when a bot thread panicked holding it.
fn locked(m: &Mutex<Budget>) -> MutexGuard<'_, Budget> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A budget slot held across a session: dropping it frees the slot, also
/// when the session panics (the unwinding drop runs).
#[derive(Debug)]
pub struct Slot {
    bot: String,
    free: fn(&str),
}

impl Slot {
    pub fn new(bot: &str) -> Slot {
        Slot::with_release(bot, release)
    }

    /// A slot that frees itself with `free` (the tests' own books).
    fn with_release(bot: &str, free: fn(&str)) -> Slot {
        Slot {
            bot: bot.to_owned(),
            free,
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        (self.free)(&self.bot);
    }
}

/// Ask for a slot. With no budget installed: always granted.
pub fn ask(bot: &str) -> Ask {
    with(Ask::Granted, |b| b.ask(bot, now_ms(), hour_now()))
}

/// Free the bot's slot (every session end).
pub fn release(bot: &str) {
    with((), |b| b.release(bot));
}

/// Should this online bot end its session for the budget? `false` with none.
pub fn over(bot: &str, busy: bool) -> bool {
    with(false, |b| b.over(bot, hour_now(), busy))
}

/// The bubble pulled (`true`) or returned (`false`) the bot.
pub fn set_human(bot: &str, on: bool) {
    with((), |b| b.set_human(bot, on));
}

/// The bot is, or is not, in a fleet.
pub fn set_fleet(bot: &str, on: bool) {
    with((), |b| b.set_fleet(bot, on));
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u32 = 12;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("pilot{i:04}")).collect()
    }

    fn granted(a: Ask) -> bool {
        a == Ask::Granted
    }

    #[test]
    fn nothing_installed_grants_everyone() {
        // The process functions with no budget: the default is unlimited.
        assert!(granted(ask("nobody-installed")));
        assert!(!over("nobody-installed", false));
        release("nobody-installed");
    }

    #[test]
    fn the_cap_is_honoured_for_a_thousand_personas() {
        let mut b = Budget::new(300, None);
        let all = names(1000);
        let mut t = 0;
        for round in 0..3 {
            for n in &all {
                t += 1;
                b.ask(n, t, H);
                assert!(b.online() <= 300, "round {round}");
            }
        }
        assert_eq!(b.online(), 300);
        // The ones that wait get a place as slots free.
        let first = all[0].clone();
        assert!(granted(b.ask(&first, t + 1, H)));
        b.release(&first);
        assert_eq!(b.online(), 299);
    }

    #[test]
    fn priority_is_human_then_fleet_then_longest_waiting() {
        let mut b = Budget::new(1, None);
        assert!(granted(b.ask("holder", 0, H)));
        b.set_human("pulled", true);
        b.set_fleet("wingman", true);
        // Asks arrive in the opposite order of their priority.
        assert!(!granted(b.ask("early", 10, H)));
        assert!(!granted(b.ask("late", 20, H)));
        assert!(!granted(b.ask("wingman", 30, H)));
        assert!(!granted(b.ask("pulled", 40, H)));
        let mut got = Vec::new();
        let mut holder = "holder";
        let mut t = 50;
        for _ in 0..4 {
            b.release(holder);
            for n in ["early", "late", "wingman", "pulled"] {
                if got.contains(&n) {
                    continue;
                }
                t += 1;
                if granted(b.ask(n, t, H)) {
                    got.push(n);
                    holder = n;
                }
            }
        }
        assert_eq!(got, ["pulled", "wingman", "early", "late"]);
    }

    #[test]
    fn a_release_frees_a_slot_and_a_stale_waiter_stops_blocking() {
        let mut b = Budget::new(1, None);
        assert!(granted(b.ask("a", 0, H)));
        assert_eq!(
            b.ask("b", 1, H),
            Ask::Wait {
                online: 1,
                limit: 1
            }
        );
        b.release("a");
        // "b" waited first but never asks again; "c" must not be held up.
        let later = WAITER_TTL_MS + 10;
        assert!(granted(b.ask("c", later, H)));
        assert_eq!(b.online(), 1);
        b.release("c");
        assert_eq!(b.online(), 0);
    }

    #[test]
    fn a_dropping_limit_releases_the_longest_online_never_a_busy_one() {
        let p = Profile::parse(&profile_text(|h| if h < 12 { 1.0 } else { 0.5 })).unwrap();
        let mut b = Budget::new(4, Some(p));
        for (i, n) in ["w", "x", "y", "z"].iter().enumerate() {
            assert!(granted(b.ask(n, i as u64, 11)));
        }
        assert!(!b.over("w", 11, false), "limit 4: nobody over");
        // Hour 12: limit 2, excess 2: the two longest online, w and x.
        assert!(b.over("w", 12, false));
        assert!(b.over("x", 12, false));
        assert!(!b.over("y", 12, false));
        assert!(!b.over("w", 12, true), "busy: never");
        assert!(!b.over("stranger", 12, false));
    }

    #[test]
    fn the_profile_hour_lookup_scales_the_cap() {
        let p = Profile::parse(&profile_text(|h| f64::from(h) / 23.0)).unwrap();
        assert_eq!(p.at(0), 0.0);
        assert_eq!(p.at(23), 1.0);
        let b = Budget::new(100, Some(p));
        assert_eq!(b.limit(0), 0);
        assert_eq!(b.limit(23), 100);
        assert_eq!(b.limit(11), 47); // floor(100 * 11/23)
        assert_eq!(b.limit(24 + 23), 100, "an hour past 23 wraps");
        // Without a profile the cap is flat; a fraction never exceeds it.
        assert_eq!(Budget::new(7, None).limit(3), 7);
    }

    #[test]
    fn a_profile_that_is_wrong_is_refused() {
        let ok = profile_text(|_| 0.5);
        assert!(Profile::parse(&ok).is_ok());
        assert!(Profile::parse("").is_err(), "no rows");
        assert!(
            Profile::parse(&format!("{ok}3\t0.2\n")).is_err(),
            "hour twice"
        );
        assert!(
            Profile::parse(&ok.replace("0.5", "1.5")).is_err(),
            "above 1"
        );
        assert!(Profile::parse("24\t0.5\n").is_err(), "hour 24");
        assert!(Profile::parse("0 0.5\n").is_err(), "no tab");
        let short: String = ok.lines().skip(1).map(|l| format!("{l}\n")).collect();
        assert!(Profile::parse(&short).is_err(), "hour 0 missing");
        // Comments and blank lines are fine.
        assert!(Profile::parse(&format!("# note\n\n{ok}")).is_ok());
    }

    #[test]
    fn a_human_leased_bot_is_never_over() {
        let mut b = Budget::new(
            3,
            Some(Profile::parse(&profile_text(|h| if h < 12 { 1.0 } else { 0.0 })).unwrap()),
        );
        for (i, n) in ["w", "x", "y"].iter().enumerate() {
            assert!(granted(b.ask(n, i as u64, 11)));
        }
        b.set_human("w", true);
        // Limit 0, excess 3: the human one stays, the others go.
        assert!(!b.over("w", 12, false), "leased: never");
        assert!(b.over("x", 12, false));
        assert!(b.over("y", 12, false));
        // Excess 1 (limit 2 of 3): the oldest is human, so the next goes.
        let mut c = Budget::new(
            3,
            Some(Profile::parse(&profile_text(|h| if h < 12 { 1.0 } else { 0.67 })).unwrap()),
        );
        for (i, n) in ["w", "x", "y"].iter().enumerate() {
            assert!(granted(c.ask(n, i as u64, 11)));
        }
        c.set_human("w", true);
        assert!(!c.over("w", 12, false));
        assert!(c.over("x", 12, false));
        assert!(!c.over("y", 12, false));
        c.set_human("w", false);
        assert!(c.over("w", 12, false), "returned: evictable again");
    }

    #[test]
    fn the_hour_follows_the_day_length() {
        assert_eq!(hour_of(0, 86_400), 0);
        assert_eq!(hour_of(3_600 * 5 + 59, 86_400), 5);
        assert_eq!(hour_of(86_400 * 7 + 3_600 * 23, 86_400), 23);
        // An invented 240 s day: 10 s per profile hour.
        assert_eq!(hour_of(0, 240), 0);
        assert_eq!(hour_of(9, 240), 0);
        assert_eq!(hour_of(10, 240), 1);
        assert_eq!(hour_of(239, 240), 23);
        assert_eq!(hour_of(240 * 1000 + 125, 240), 12);
        assert_eq!(
            hour_of(5, 0),
            hour_of(5, 1),
            "a zero day is a one-second day"
        );
        assert_eq!(hour_of(u64::MAX - 1, u64::MAX), 23, "no overflow");
    }

    #[test]
    fn a_poisoned_lock_does_not_turn_the_cap_off() {
        let m = Mutex::new(Budget::new(1, None));
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _g = m.lock().unwrap();
                panic!("a bot thread dies holding the books");
            })
            .join()
        });
        assert!(m.is_poisoned());
        let mut b = locked(&m);
        assert!(granted(b.ask("a", 0, H)));
        assert!(!granted(b.ask("b", 1, H)), "still capped");
    }

    #[test]
    fn a_slot_frees_on_return_and_on_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static FREED: AtomicUsize = AtomicUsize::new(0);
        fn free(bot: &str) {
            assert_eq!(bot, "pilot0001");
            FREED.fetch_add(1, Ordering::SeqCst);
        }
        {
            let _s = Slot::with_release("pilot0001", free);
        }
        assert_eq!(FREED.load(Ordering::SeqCst), 1, "normal end");
        let r = std::panic::catch_unwind(|| {
            let _s = Slot::with_release("pilot0001", free);
            panic!("the session panics");
        });
        assert!(r.is_err());
        assert_eq!(FREED.load(Ordering::SeqCst), 2, "panic");
    }

    #[test]
    fn a_turnover_day_with_a_dropping_limit_stays_within_the_limit() {
        // 40 personas, cap 20, hours 0-11 full, 12-17 a quarter, then half.
        let p = Profile::parse(&profile_text(|h| match h {
            0..=11 => 1.0,
            12..=17 => 0.25,
            _ => 0.5,
        }))
        .unwrap();
        let mut b = Budget::new(20, Some(p));
        let all = names(40);
        let mut t: u64 = 0;
        let mut cursor = 0;
        for hour in 0..24u32 {
            for _step in 0..6 {
                t += 10_000;
                // Sessions end: every bot online longest-first for a while
                // leaves of its own accord (turnover), the budget evicts
                // the rest when over.
                for n in &all {
                    if b.over(n, hour, false) {
                        b.release(n);
                    }
                }
                // A few leave on their own; a few waiting bots ask.
                for _ in 0..2 {
                    let who = all[cursor % all.len()].clone();
                    cursor += 7;
                    b.release(&who);
                }
                for n in &all {
                    if !b.online.contains_key(n) {
                        b.ask(n, t, hour);
                    }
                }
                // The books hold the limit once the excess has settled.
                let limit = b.limit(hour);
                assert!(b.online() <= limit, "hour {hour}: {} > {limit}", b.online());
            }
            // The limit is used, not just respected, once asks catch up.
            assert_eq!(b.online(), b.limit(hour), "hour {hour}");
        }
    }

    fn profile_text(f: impl Fn(u32) -> f64) -> String {
        (0..24).map(|h| format!("{h}\t{}\n", f(h))).collect()
    }
}
