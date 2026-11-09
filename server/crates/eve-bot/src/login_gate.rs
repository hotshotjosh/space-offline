//! **A process-wide login rate for the bots (#252 LOGIN-RATE).**
//!
//! The old human pace added 3-10 s per bot, cumulatively: 1,000 bots took
//! about 1.8 h to be in, and a society window opening at one hour let
//! everyone in at once. This is a token bucket (burst 1) shared by the whole
//! process: at most `R` logins a second, whoever asks and whatever the
//! reason (a first login, a window start, a bubble wake, a reconnect). Each
//! bot's *first* arrival at the gate is its own jitter of 0-10 s, drawn from
//! its name, so a rerun gives the same arrivals.
//!
//! The pure part is [`Bucket`]; the process part is [`install`] (once, by
//! `eve-bots --login-rate`) and [`reserve_now`] (the driver's
//! `Bot::queue_with`, once before a session; [`admit`] is the blocking form).
//! With no gate installed, the reservation is zero, so tests,
//! `Pace::Fast` and `--ramp` (the bench keeps its own schedule) are unchanged.
//!
//! **[A]** R = 2 a second is gentle for the server. Falsifier: a burst at 20 a
//! second (the bench default) showing no late ticks, in which case raise it.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The default rate, logins a second. **[A]**
pub const DEFAULT_RATE: f64 = 2.0;

/// The lowest rate `--login-rate` accepts (other than 0, which turns the gate
/// off): a gap of 1/R seconds must fit a `Duration`, and 1,000 s a login is
/// no rate anyone means.
pub const MIN_RATE: f64 = 0.001;

/// Parse a `--login-rate` value: 0 (gate off) or [`MIN_RATE`] and up.
pub fn parse_rate(v: &str) -> Result<f64, String> {
    v.parse::<f64>()
        .ok()
        .filter(|r| r.is_finite() && (*r == 0.0 || *r >= MIN_RATE))
        .ok_or_else(|| format!("--login-rate {v}: want logins a second, 0 or at least {MIN_RATE}"))
}

/// The longest first-arrival jitter, seconds.
pub const JITTER_MAX_S: f64 = 10.0;

/// A bot's first arrival at the gate: 0 to [`JITTER_MAX_S`] seconds, from its
/// name alone.
pub fn jitter(name: &str) -> Duration {
    let mut s = crate::brain::name_hash(name) | 1;
    // A few rounds so near names do not draw near.
    for _ in 0..3 {
        crate::brain::xorshift(&mut s);
    }
    let u = crate::brain::unit(crate::brain::xorshift(&mut s));
    Duration::from_secs_f64(JITTER_MAX_S * u)
}

/// A token bucket with a burst of one: the next free slot.
#[derive(Debug, Clone)]
pub struct Bucket {
    gap: Duration,
    next_free: Duration,
}

impl Bucket {
    /// `rate` logins a second; below [`MIN_RATE`] or not finite gives one a
    /// second.
    pub fn new(rate: f64) -> Bucket {
        let rate = if rate.is_finite() && rate >= MIN_RATE {
            rate
        } else {
            1.0
        };
        Bucket {
            gap: Duration::from_secs_f64(1.0 / rate),
            next_free: Duration::ZERO,
        }
    }

    /// Someone arrives `at` (time since the gate's start); how long they wait.
    pub fn reserve(&mut self, at: Duration) -> Duration {
        let slot = self.next_free.max(at);
        self.next_free = slot + self.gap;
        slot - at
    }
}

struct Gate {
    epoch: Instant,
    bucket: Mutex<Bucket>,
}

static GATE: OnceLock<Gate> = OnceLock::new();

/// Turn the process gate on at `rate` a second. The first call wins.
pub fn install(rate: f64) {
    let _ = GATE.set(Gate {
        epoch: Instant::now(),
        bucket: Mutex::new(Bucket::new(rate)),
    });
}

/// How long the caller waits for a slot now; zero with no gate.
pub fn reserve_now() -> Duration {
    let Some(g) = GATE.get() else {
        return Duration::ZERO;
    };
    let at = g.epoch.elapsed();
    g.bucket
        .lock()
        .map(|mut b| b.reserve(at))
        .unwrap_or_default()
}

/// Wait for a login slot: `sleep` is the caller's stoppable wait.
pub fn admit(sleep: impl FnOnce(Duration)) {
    let wait = reserve_now();
    if !wait.is_zero() {
        sleep(wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The gate is process-wide and a test binary shares one process: no test
    // may call `install`. The rates below are invented (not DEFAULT_RATE).

    /// Invented names; the starts of `n` of them at `rate`, sorted.
    fn starts(n: usize, rate: f64) -> Vec<f64> {
        let mut arrivals: Vec<Duration> = (0..n).map(|i| jitter(&format!("bot-{i}"))).collect();
        arrivals.sort();
        let mut b = Bucket::new(rate);
        let mut out: Vec<f64> = arrivals
            .iter()
            .map(|&a| (a + b.reserve(a)).as_secs_f64())
            .collect();
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out
    }

    #[test]
    fn no_second_holds_more_than_rate_plus_one() {
        let r = 3.0;
        let s = starts(1000, r);
        let mut worst = 0;
        let mut j = 0;
        for i in 0..s.len() {
            while s[j] < s[i] - 1.0 {
                j += 1;
            }
            // starts in (s[i]-1, s[i]]
            worst = worst.max(i - j + 1);
        }
        assert!(worst <= r as usize + 1, "worst window {worst}");
    }

    #[test]
    fn last_start_is_n_over_r_plus_jitter() {
        let (n, r) = (1000, 4.0);
        let last = *starts(n, r).last().unwrap();
        assert!(last <= n as f64 / r + JITTER_MAX_S, "last {last}");
        // and the rate is the limit: it cannot be faster than N/R - 1/R.
        assert!(last >= (n - 1) as f64 / r - 1e-6, "last {last}");
    }

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        assert_eq!(jitter("alpha"), jitter("alpha"));
        assert_ne!(jitter("alpha"), jitter("alphb"));
        for i in 0..500 {
            assert!(jitter(&format!("n{i}")).as_secs_f64() < JITTER_MAX_S);
        }
        assert_eq!(starts(50, 3.0), starts(50, 3.0));
    }

    #[test]
    fn jitter_spreads_over_the_range() {
        let v: Vec<f64> = (0..1000)
            .map(|i| jitter(&format!("bot-{i}")).as_secs_f64())
            .collect();
        assert!(v.iter().any(|&x| x < 1.0));
        assert!(v.iter().any(|&x| x > 9.0));
    }

    #[test]
    fn a_quiet_arrival_waits_nothing_and_a_crowd_queues() {
        let mut b = Bucket::new(4.0);
        assert_eq!(b.reserve(Duration::from_secs(5)), Duration::ZERO);
        // five arrive together at 100 s: waits 0, .25, .5, .75, 1
        let waits: Vec<f64> = (0..5)
            .map(|_| b.reserve(Duration::from_secs(100)).as_secs_f64())
            .collect();
        assert_eq!(waits, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn a_bad_rate_is_one_a_second() {
        let mut b = Bucket::new(0.0);
        b.reserve(Duration::ZERO);
        assert_eq!(b.reserve(Duration::ZERO), Duration::from_secs(1));
    }

    #[test]
    fn a_tiny_rate_is_refused_and_cannot_panic_the_bucket() {
        assert_eq!(parse_rate("0"), Ok(0.0));
        assert_eq!(parse_rate("0.001"), Ok(0.001));
        assert!(parse_rate("0.0005").is_err());
        assert!(parse_rate("1e-300").is_err());
        assert!(parse_rate("-1").is_err());
        assert!(parse_rate("nan").is_err());
        assert!(parse_rate("inf").is_err());
        // A rate that reaches the bucket anyway is one a second, not a panic.
        let mut b = Bucket::new(1e-300);
        b.reserve(Duration::ZERO);
        assert_eq!(b.reserve(Duration::ZERO), Duration::from_secs(1));
    }

    #[test]
    fn no_gate_installed_waits_nothing() {
        // The gate is process-wide and never installed in this test binary.
        let mut slept = false;
        admit(|_| slept = true);
        assert!(!slept);
    }
}
