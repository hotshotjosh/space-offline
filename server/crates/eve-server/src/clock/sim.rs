//! The pure lockstep simulator (#148 FF-2): virtual time that advances only
//! when every actor waits and no byte is in flight. Spec: `specs/ff-now.md`
//! §3 L4a and `specs/fast-forward.md` §2.4.
//!
//! **Inert.** Nothing in the running server constructs a [`Sim`] yet; the
//! scheduler, the bot reader thread and the byte counters in `push.rs` and
//! `conn.rs` arrive with the hot batch. This file reads no virtual or wall
//! clock to decide anything (it *is* the clock of a lockstep run). The one
//! real-time wait is the watchdog in [`Sim::wait_quiescent`] and
//! [`Sim::wait_and_advance`], a `Condvar::wait_timeout_while` bound; the
//! clock lint does not scan `clock/`, so it is allowed here.
//!
//! **The scheduler.** The scheduler thread must not be an actor (it would
//! wait for itself). It loops on [`Sim::wait_and_advance`], which checks
//! quiescence and moves `now` under one lock, so no actor can wake between
//! the check and the move. Threads that are not actors (a keepalive writing
//! `s2c_written`, the host registering a bot, a `conn.rs` or wire thread)
//! may still act at any time; their bytes are counted, so they hold time
//! back, but a thread that has not counted its byte yet is invisible.
//! The `external_next_due` snapshot (outbox, `Registry::scheduled`) is taken
//! by the caller before the call and can be stale by the time the lock is
//! held; closing that race belongs to the hot PR that owns those queues.
//!
//! **Wakeups.** Every counter update and state change calls `notify_all`.
//! `[I]` At ~1,000 bots that wakes every waiter per update (O(N) per event);
//! two condvars (one for the scheduler, one for actors) would cut it.
//! Deferred until a bench shows it matters. Falsifier: tick health stays
//! flat at 1,000 bots with the single condvar.
//!
//! **The model.** One `Mutex` and one `Condvar` guard:
//! - the virtual `now`;
//! - the *actors*, threads that wait on game time. An actor is `Running`, or
//!   `Waiting { until, mailbox }`. A waiter is *runnable* when `until <= now`
//!   or its mailbox holds a payload, and a runnable waiter is not at rest;
//! - the *links*, one per bot connection, each with four monotone byte
//!   counters (`c2s_sent`, `c2s_done`, `s2c_written`, `s2c_read`) and a
//!   mailbox of raw payloads. Each counter is bumped before the act or after
//!   it is complete, so equal pairs mean nothing is in flight. A retired
//!   link is forgotten, so bytes lost on a dead socket cannot wedge time, and
//!   a mailbox waiter on it wakes with [`Mail::Gone`].
//!
//! **Quiescent** (spec §2.4): every actor `Waiting` and not runnable; every
//! open link has `sent == done` and `written == read`; no external entry (the
//! outbox, `Registry::scheduled`) is due at or before `now`. Those external
//! entries enter only through the `external_next_due` argument, so this file
//! needs nothing from `push.rs` or `service.rs`.
//!
//! `[A]` The byte-counting contract with `conn.rs` and `push.rs` (what each
//! counter counts, and when) is the spec's; the first lane that wires it may
//! need more here. Falsifier: a lockstep run that stalls with every counter
//! equal. [`Sim::stall_report`] exists to make that visible.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// What a mailbox wait returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mail {
    /// A raw payload from the link's mailbox.
    Payload(Vec<u8>),
    /// Virtual time reached the deadline with nothing to read.
    Timeout,
    /// The link is retired or was never opened: nothing will ever arrive.
    Gone,
}

/// A link's key: the bot's local socket address (spec §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LinkId(SocketAddr);

impl LinkId {
    /// The link for a bot connection with this local address.
    pub fn new(local: SocketAddr) -> LinkId {
        LinkId(local)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActorState {
    Running,
    Waiting {
        until: Duration,
        mailbox: Option<LinkId>,
    },
}

#[derive(Debug)]
struct Actor {
    name: String,
    state: ActorState,
}

#[derive(Debug, Default)]
struct Link {
    c2s_sent: u64,
    c2s_done: u64,
    s2c_written: u64,
    s2c_read: u64,
    mailbox: VecDeque<Vec<u8>>,
}

impl Link {
    fn settled(&self) -> bool {
        self.c2s_sent == self.c2s_done && self.s2c_written == self.s2c_read
    }
}

#[derive(Debug, Default)]
struct State {
    now: Duration,
    next_actor: u64,
    actors: BTreeMap<u64, Actor>,
    links: HashMap<LinkId, Link>,
}

impl State {
    fn has_mail(&self, l: LinkId) -> bool {
        self.links.get(&l).is_some_and(|k| !k.mailbox.is_empty())
    }

    /// A waiter whose deadline has come or whose mailbox has a payload.
    fn runnable(&self, s: &ActorState) -> bool {
        match s {
            ActorState::Running => true,
            // A waiter on a link that is gone is runnable too: it must wake
            // and see `Mail::Gone`.
            ActorState::Waiting { until, mailbox } => {
                *until <= self.now
                    || mailbox.is_some_and(|l| self.has_mail(l) || !self.links.contains_key(&l))
            }
        }
    }

    fn set(&mut self, id: u64, state: ActorState) {
        if let Some(a) = self.actors.get_mut(&id) {
            a.state = state;
        }
    }

    fn quiescent(&self, external_next_due: Option<Duration>) -> bool {
        self.actors.values().all(|a| !self.runnable(&a.state))
            && self.links.values().all(Link::settled)
            && external_next_due.is_none_or(|d| d > self.now)
    }

    fn next_advance(&self, next_tick: Duration, external_next_due: Option<Duration>) -> Duration {
        let waiting = self.actors.values().filter_map(|a| match a.state {
            ActorState::Waiting { until, .. } => Some(until),
            ActorState::Running => None,
        });
        waiting
            .chain(external_next_due)
            .fold(next_tick, Duration::min)
            .max(self.now)
    }
}

#[derive(Debug, Default)]
struct Inner {
    state: Mutex<State>,
    cv: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking actor must not take the whole simulator down with it:
        // the counters are plain integers, so the state stays usable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The simulator. Cloning shares it, so every actor thread holds a clone.
#[derive(Debug, Clone, Default)]
pub struct Sim {
    inner: Arc<Inner>,
}

/// An actor's registration. Dropping it deregisters the actor, so a thread
/// that ends or panics cannot hold time back.
#[derive(Debug)]
pub struct ActorGuard {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for ActorGuard {
    fn drop(&mut self) {
        self.inner.lock().actors.remove(&self.id);
        self.inner.cv.notify_all();
    }
}

impl Sim {
    /// A simulator at virtual time zero, with no actors and no links.
    pub fn new() -> Sim {
        Sim::default()
    }

    /// The virtual time since the simulator's start.
    pub fn now(&self) -> Duration {
        self.inner.lock().now
    }

    /// Register an actor, `Running`. Do it before the thread is spawned.
    pub fn register_actor(&self, name: &str) -> ActorGuard {
        let mut st = self.inner.lock();
        let id = st.next_actor;
        st.next_actor += 1;
        st.actors.insert(
            id,
            Actor {
                name: name.to_string(),
                state: ActorState::Running,
            },
        );
        drop(st);
        ActorGuard {
            inner: self.inner.clone(),
            id,
        }
    }

    /// How many actors are registered.
    pub fn actor_count(&self) -> usize {
        self.inner.lock().actors.len()
    }

    /// How many actors are `Waiting`, runnable or not.
    pub fn waiting_count(&self) -> usize {
        let st = self.inner.lock();
        st.actors
            .values()
            .filter(|a| matches!(a.state, ActorState::Waiting { .. }))
            .count()
    }

    /// Block until virtual time reaches `until` (the lockstep `clock::sleep`).
    /// Returns at once if it already has.
    pub fn wait_until(&self, a: &ActorGuard, until: Duration) {
        debug_assert!(Arc::ptr_eq(&self.inner, &a.inner), "guard from another Sim");
        let mut st = self.inner.lock();
        if st.now >= until {
            return;
        }
        st.set(
            a.id,
            ActorState::Waiting {
                until,
                mailbox: None,
            },
        );
        self.inner.cv.notify_all();
        st = self
            .inner
            .cv
            .wait_while(st, |s| s.now < until)
            .unwrap_or_else(|e| e.into_inner());
        st.set(a.id, ActorState::Running);
    }

    /// Block until link `l` has a payload or virtual time reaches `until`
    /// (the lockstep `Conn::recv_within`). A payload already waiting is
    /// returned even if `until` has passed. A link that is retired, or was
    /// never opened, gives [`Mail::Gone`] at once, and a wait in progress
    /// wakes with it when the link is retired. After `Gone` the actor must
    /// drop its guard or rest with [`Sim::wait_until`]: a repeat
    /// `wait_mailbox` on a gone link returns at once and never rests, so
    /// time would hold.
    pub fn wait_mailbox(&self, a: &ActorGuard, l: LinkId, until: Duration) -> Mail {
        debug_assert!(Arc::ptr_eq(&self.inner, &a.inner), "guard from another Sim");
        let mut st = self.inner.lock();
        if st.links.contains_key(&l) && !st.has_mail(l) && st.now < until {
            st.set(
                a.id,
                ActorState::Waiting {
                    until,
                    mailbox: Some(l),
                },
            );
            self.inner.cv.notify_all();
            st = self
                .inner
                .cv
                .wait_while(st, |s| {
                    s.now < until && !s.has_mail(l) && s.links.contains_key(&l)
                })
                .unwrap_or_else(|e| e.into_inner());
            st.set(a.id, ActorState::Running);
        }
        match st.links.get_mut(&l) {
            None => Mail::Gone,
            Some(k) => k.mailbox.pop_front().map_or(Mail::Timeout, Mail::Payload),
        }
    }

    /// Register a link, before the connection's first byte. Opening an open
    /// link keeps its counters.
    pub fn open_link(&self, l: LinkId) {
        self.inner.lock().links.entry(l).or_default();
        self.inner.cv.notify_all();
    }

    /// Forget a link (its socket is closed): its counters stop counting, its
    /// unread payloads are dropped, and a mailbox waiter on it wakes. `[I]`
    /// a link reopened under the same [`LinkId`] before that waiter takes the
    /// lock back hides the `Gone` from it (it waits on the new connection);
    /// falsified by an L4b log where one reader thread spans two connections.
    pub fn retire_link(&self, l: LinkId) {
        self.inner.lock().links.remove(&l);
        self.inner.cv.notify_all();
    }

    fn bump(&self, l: LinkId, f: impl FnOnce(&mut Link)) {
        if let Some(k) = self.inner.lock().links.get_mut(&l) {
            f(k);
        }
        self.inner.cv.notify_all();
    }

    /// The bot is about to write `n` bytes toward the server. Ignored on a
    /// retired link, as are the three below.
    pub fn c2s_sent(&self, l: LinkId, n: u64) {
        self.bump(l, |k| k.c2s_sent = k.c2s_sent.saturating_add(n));
    }

    /// The server finished `n` bytes: dispatched and its reply written.
    pub fn c2s_done(&self, l: LinkId, n: u64) {
        self.bump(l, |k| k.c2s_done = k.c2s_done.saturating_add(n));
    }

    /// The server is about to write `n` bytes toward the bot.
    pub fn s2c_written(&self, l: LinkId, n: u64) {
        self.bump(l, |k| k.s2c_written = k.s2c_written.saturating_add(n));
    }

    /// The bot's reader thread split `n` bytes into whole frames.
    pub fn s2c_read(&self, l: LinkId, n: u64) {
        self.bump(l, |k| k.s2c_read = k.s2c_read.saturating_add(n));
    }

    /// Deliver a raw payload to the link's mailbox. Dropped if the link is
    /// retired.
    pub fn push_mailbox(&self, l: LinkId, payload: Vec<u8>) {
        self.bump(l, |k| k.mailbox.push_back(payload));
    }

    /// Whether virtual time may advance (see the module docs).
    /// `external_next_due` is the earliest virtual due time among the entries
    /// the sim cannot see (the outbox, `Registry::scheduled`).
    pub fn quiescent(&self, external_next_due: Option<Duration>) -> bool {
        self.inner.lock().quiescent(external_next_due)
    }

    /// Block on the condvar until [`Sim::quiescent`], for at most `real_limit`
    /// of wall time (a watchdog, not a clock read). Returns whether it is
    /// quiescent. `external_next_due` is read once, so a caller whose external
    /// entries change polls in slices. Checking and then advancing as two
    /// calls is racy; the scheduler uses [`Sim::wait_and_advance`].
    pub fn wait_quiescent(
        &self,
        external_next_due: Option<Duration>,
        real_limit: Duration,
    ) -> bool {
        let st = self.inner.lock();
        let (st, _) = self
            .inner
            .cv
            .wait_timeout_while(st, real_limit, |s| !s.quiescent(external_next_due))
            .unwrap_or_else(|e| e.into_inner());
        st.quiescent(external_next_due)
    }

    /// If quiescent now, move `now` to [`Sim::next_advance`] and wake every
    /// waiter, all under one lock. Returns the new time, or `None` if time
    /// must hold. Never moves time back. The caller passes a `next_tick`
    /// later than `now`: with no waiters a stale one returns `Some(now)`
    /// without moving, and a loop on it would spin.
    pub fn advance_if_quiescent(
        &self,
        next_tick: Duration,
        external_next_due: Option<Duration>,
    ) -> Option<Duration> {
        let st = self.inner.lock();
        self.advance_locked(st, next_tick, external_next_due)
    }

    /// The scheduler's step: wait (at most `real_limit` of wall time) for
    /// quiescence, then advance as [`Sim::advance_if_quiescent`] does, with
    /// no gap between the check and the move. `None` is a timeout with time
    /// held. The calling thread must not be an actor.
    pub fn wait_and_advance(
        &self,
        next_tick: Duration,
        external_next_due: Option<Duration>,
        real_limit: Duration,
    ) -> Option<Duration> {
        let st = self.inner.lock();
        let (st, _) = self
            .inner
            .cv
            .wait_timeout_while(st, real_limit, |s| !s.quiescent(external_next_due))
            .unwrap_or_else(|e| e.into_inner());
        self.advance_locked(st, next_tick, external_next_due)
    }

    fn advance_locked(
        &self,
        mut st: MutexGuard<'_, State>,
        next_tick: Duration,
        external_next_due: Option<Duration>,
    ) -> Option<Duration> {
        if !st.quiescent(external_next_due) {
            return None;
        }
        let t = st.next_advance(next_tick, external_next_due);
        st.now = st.now.max(t);
        let now = st.now;
        drop(st);
        self.inner.cv.notify_all();
        Some(now)
    }

    /// The time to advance to: the least of `next_tick`, the earliest waiting
    /// actor's `until` and `external_next_due`, never before `now`.
    pub fn next_advance(
        &self,
        next_tick: Duration,
        external_next_due: Option<Duration>,
    ) -> Duration {
        self.inner.lock().next_advance(next_tick, external_next_due)
    }

    /// Move virtual time forward to `t` and wake every waiter. Time never
    /// goes back: an earlier `t` is ignored.
    pub fn advance_to(&self, t: Duration) {
        let mut st = self.inner.lock();
        st.now = st.now.max(t);
        drop(st);
        self.inner.cv.notify_all();
    }

    /// The body of an `[ff] stall` line (spec §2) after its `real_s` field,
    /// which the caller owns because this file reads no clock:
    /// `virt_s=3601.25 running=bot-ore-3,bot-sov-1,bot-due-2 links=127.0.0.1:40122(c2s 812/790)`.
    /// `running` lists the actors that are not at rest (running, or waiting
    /// but runnable), in registration order, or `-`. `links` lists each open
    /// link whose counters differ, as `(c2s sent/done s2c written/read)`
    /// with only the unequal pair shown, in address order, or `-`.
    pub fn stall_report(&self) -> String {
        let st = self.inner.lock();
        let running: Vec<&str> = st
            .actors
            .values()
            .filter(|a| st.runnable(&a.state))
            .map(|a| a.name.as_str())
            .collect();
        let mut unequal: Vec<(&LinkId, &Link)> =
            st.links.iter().filter(|(_, k)| !k.settled()).collect();
        unequal.sort_by_key(|(l, _)| **l);
        let links: Vec<String> = unequal
            .into_iter()
            .map(|(l, k)| {
                let mut parts = Vec::new();
                if k.c2s_sent != k.c2s_done {
                    parts.push(format!("c2s {}/{}", k.c2s_sent, k.c2s_done));
                }
                if k.s2c_written != k.s2c_read {
                    parts.push(format!("s2c {}/{}", k.s2c_written, k.s2c_read));
                }
                format!("{}({})", l.0, parts.join(" "))
            })
            .collect();
        let or_dash = |v: Vec<String>| {
            if v.is_empty() {
                "-".to_string()
            } else {
                v.join(",")
            }
        };
        format!(
            "virt_s={:.2} running={} links={}",
            st.now.as_secs_f64(),
            or_dash(running.iter().map(|s| s.to_string()).collect()),
            or_dash(links),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn link(port: u16) -> LinkId {
        LinkId::new(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    /// Spin (yielding, no clock) until `n` actors are waiting. Bounded, so a
    /// bug fails the test instead of hanging it.
    fn until_waiting(sim: &Sim, n: usize) {
        for _ in 0..50_000_000u64 {
            if sim.waiting_count() == n {
                return;
            }
            thread::yield_now();
        }
        panic!("{} actors waiting, wanted {n}", sim.waiting_count());
    }

    /// Put an actor into a state without a thread, so a test can inspect the
    /// instant between "time moved" and "the waiter woke".
    fn force(sim: &Sim, g: &ActorGuard, s: ActorState) {
        sim.inner.lock().set(g.id, s);
    }

    fn waiting(until: Duration) -> ActorState {
        ActorState::Waiting {
            until,
            mailbox: None,
        }
    }

    /// Run `f` on a thread whose result arrives through a channel, so a lost
    /// wakeup fails the test (see [`within`]) instead of hanging the suite.
    fn spawn_bounded<T: Send + 'static>(
        f: impl FnOnce() -> T + Send + 'static,
    ) -> std::sync::mpsc::Receiver<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx
    }

    /// The thread's result within 10 s of wall time (a watchdog: the correct
    /// code answers in microseconds). On a miss the blocked thread is left
    /// behind; the test process still ends.
    fn within<T>(rx: &std::sync::mpsc::Receiver<T>, what: &str) -> T {
        rx.recv_timeout(secs(10))
            .unwrap_or_else(|e| panic!("{what}: {e:?}"))
    }

    #[test]
    fn advances_only_when_every_actor_waits() {
        let sim = Sim::new();
        let a = sim.register_actor("a");
        let b = sim.register_actor("b");
        let ta = {
            let s = sim.clone();
            thread::spawn(move || {
                s.wait_until(&a, secs(10));
                a
            })
        };
        until_waiting(&sim, 1);
        // `b` is still running: time must hold.
        assert!(!sim.quiescent(None));
        let tb = {
            let s = sim.clone();
            thread::spawn(move || {
                s.wait_until(&b, secs(5));
                b
            })
        };
        until_waiting(&sim, 2);
        assert!(sim.quiescent(None));
        assert!(sim.wait_quiescent(None, secs(5)));
        sim.advance_to(secs(5));
        let b = tb.join().unwrap();
        // `b` is Running again after its wake.
        assert!(!sim.quiescent(None));
        drop(b);
        until_waiting(&sim, 1);
        assert!(sim.quiescent(None));
        sim.advance_to(secs(10));
        drop(ta.join().unwrap());
        assert_eq!(sim.actor_count(), 0);
    }

    #[test]
    fn an_unequal_link_holds_time() {
        let sim = Sim::new();
        let l = link(40001);
        sim.open_link(l);
        assert!(sim.quiescent(None));
        sim.c2s_sent(l, 8);
        assert!(!sim.quiescent(None), "sent but not done");
        sim.c2s_done(l, 7);
        assert!(!sim.quiescent(None), "done short by one");
        sim.c2s_done(l, 1);
        assert!(sim.quiescent(None));
        sim.s2c_written(l, 3);
        assert!(!sim.quiescent(None), "written but not read");
        sim.s2c_read(l, 3);
        assert!(sim.quiescent(None));
        // The same holds with an actor at rest.
        let g = sim.register_actor("w");
        force(&sim, &g, waiting(secs(9)));
        assert!(sim.quiescent(None));
        sim.s2c_written(l, 1);
        assert!(!sim.quiescent(None));
    }

    #[test]
    fn advance_goes_to_the_earliest_deadline() {
        let sim = Sim::new();
        let mut hs = Vec::new();
        for (name, t) in [("x", 7), ("y", 3), ("z", 9)] {
            let g = sim.register_actor(name);
            let s = sim.clone();
            hs.push(thread::spawn(move || {
                s.wait_until(&g, secs(t));
                g
            }));
        }
        until_waiting(&sim, 3);
        assert_eq!(sim.next_advance(secs(100), None), secs(3));
        assert_eq!(sim.next_advance(secs(2), None), secs(2), "a nearer tick");
        assert_eq!(
            sim.next_advance(secs(100), Some(secs(1))),
            secs(1),
            "a nearer external entry"
        );
        assert_eq!(sim.next_advance(secs(100), Some(secs(50))), secs(3));
        sim.advance_to(secs(3));
        assert_eq!(sim.now(), secs(3));
        // Time never goes back, and next_advance never returns the past.
        sim.advance_to(secs(1));
        assert_eq!(sim.now(), secs(3));
        assert_eq!(sim.next_advance(secs(0), None), secs(3));
        let y = hs.remove(1).join().unwrap();
        drop(y);
        until_waiting(&sim, 2);
        assert_eq!(sim.next_advance(secs(100), None), secs(7));
        sim.advance_to(secs(9));
        for h in hs {
            drop(h.join().unwrap());
        }
    }

    #[test]
    fn a_closed_link_is_retired() {
        let sim = Sim::new();
        let l = link(40002);
        sim.open_link(l);
        sim.c2s_sent(l, 10);
        sim.s2c_written(l, 4);
        assert!(!sim.quiescent(None));
        sim.retire_link(l);
        assert!(sim.quiescent(None), "a retired link no longer counts");
        // Late traffic for a retired link neither panics nor revives it.
        sim.c2s_sent(l, 1);
        sim.push_mailbox(l, vec![1]);
        assert!(sim.quiescent(None));
        assert!(sim.stall_report().ends_with("links=-"));
        // A mailbox wait on a retired link is Gone at once.
        let g = sim.register_actor("late");
        assert_eq!(sim.wait_mailbox(&g, l, secs(2)), Mail::Gone);
    }

    #[test]
    fn retiring_a_link_wakes_its_mailbox_waiter() {
        let sim = Sim::new();
        let l = link(40010);
        sim.open_link(l);
        let g = sim.register_actor("rx");
        let s = sim.clone();
        let t = spawn_bounded(move || {
            let got = s.wait_mailbox(&g, l, secs(500));
            (got, g)
        });
        until_waiting(&sim, 1);
        assert!(sim.quiescent(None));
        sim.retire_link(l);
        let (got, g) = within(&t, "retire_link did not wake the waiter");
        assert_eq!(got, Mail::Gone);
        drop(g);
        // A deadline and a payload still give their own answers.
        sim.open_link(l);
        let g = sim.register_actor("rx2");
        let s = sim.clone();
        let t = spawn_bounded(move || {
            let got = s.wait_mailbox(&g, l, secs(4));
            (got, g)
        });
        until_waiting(&sim, 1);
        sim.advance_to(secs(4));
        let (got, g) = within(&t, "advance_to did not wake the waiter");
        assert_eq!(got, Mail::Timeout);
        sim.push_mailbox(l, b"x".to_vec());
        assert_eq!(
            sim.wait_mailbox(&g, l, secs(1)),
            Mail::Payload(b"x".to_vec())
        );
    }

    #[test]
    fn a_runnable_waiter_is_not_quiescent() {
        let sim = Sim::new();
        let g = sim.register_actor("w");
        // Waiting until 5 s: at rest at 0, runnable once time reaches 5.
        force(&sim, &g, waiting(secs(5)));
        assert!(sim.quiescent(None));
        sim.advance_to(secs(5));
        assert!(
            !sim.quiescent(None),
            "deadline reached, waiter not yet woken"
        );
        // A waiter on a mailbox is runnable when a payload arrives.
        let l = link(40003);
        sim.open_link(l);
        force(
            &sim,
            &g,
            ActorState::Waiting {
                until: secs(500),
                mailbox: Some(l),
            },
        );
        assert!(sim.quiescent(None));
        sim.push_mailbox(l, b"frame".to_vec());
        assert!(!sim.quiescent(None), "payload waiting");
        // End to end: a real mailbox waiter takes the payload and returns.
        let s = sim.clone();
        sim.inner.lock().links.get_mut(&l).unwrap().mailbox.clear();
        drop(g);
        let g2 = sim.register_actor("w2");
        let t = thread::spawn(move || {
            let got = s.wait_mailbox(&g2, l, secs(500));
            (got, g2)
        });
        until_waiting(&sim, 1);
        assert!(sim.quiescent(None));
        sim.push_mailbox(l, b"hello".to_vec());
        let (got, g2) = t.join().unwrap();
        assert_eq!(got, Mail::Payload(b"hello".to_vec()));
        assert!(!sim.quiescent(None), "the actor is running again");
        drop(g2);
    }

    #[test]
    fn a_due_external_entry_is_not_quiescent() {
        let sim = Sim::new();
        let g = sim.register_actor("w");
        force(&sim, &g, waiting(secs(100)));
        sim.advance_to(secs(10));
        assert!(sim.quiescent(None));
        assert!(sim.quiescent(Some(secs(11))), "due later");
        assert!(!sim.quiescent(Some(secs(10))), "due now");
        assert!(!sim.quiescent(Some(secs(3))), "overdue");
        assert!(!sim.wait_quiescent(Some(secs(10)), Duration::from_millis(1)));
    }

    #[test]
    fn the_stall_report_names_running_actors_and_unequal_links() {
        let sim = Sim::new();
        let _ore = sim.register_actor("bot-ore-3");
        let sov = sim.register_actor("bot-sov-1");
        let rest = sim.register_actor("bot-idle-9");
        force(&sim, &rest, waiting(secs(9_999)));
        // Waiting, but its deadline is before the stall time: not at rest.
        let due = sim.register_actor("bot-due-2");
        force(&sim, &due, waiting(secs(1)));
        sim.advance_to(Duration::from_millis(3_601_250));
        let bad = link(40122);
        let good = link(40123);
        sim.open_link(bad);
        sim.open_link(good);
        sim.c2s_sent(bad, 812);
        sim.c2s_done(bad, 790);
        sim.c2s_sent(good, 5);
        sim.c2s_done(good, 5);
        assert_eq!(
            sim.stall_report(),
            "virt_s=3601.25 running=bot-ore-3,bot-sov-1,bot-due-2 links=127.0.0.1:40122(c2s 812/790)"
        );
        // Both directions unequal show both pairs; nothing amiss shows dashes.
        sim.s2c_written(bad, 2);
        assert!(
            sim.stall_report()
                .ends_with("links=127.0.0.1:40122(c2s 812/790 s2c 2/0)")
        );
        drop(_ore);
        drop(sov);
        drop(due);
        sim.c2s_done(bad, 22);
        sim.s2c_read(bad, 2);
        assert_eq!(sim.stall_report(), "virt_s=3601.25 running=- links=-");
    }

    #[test]
    fn advance_if_quiescent_moves_time_only_at_rest() {
        let sim = Sim::new();
        let g = sim.register_actor("w");
        // Running: time holds.
        assert_eq!(sim.advance_if_quiescent(secs(100), None), None);
        assert_eq!(sim.now(), secs(0));
        force(&sim, &g, waiting(secs(6)));
        assert_eq!(sim.advance_if_quiescent(secs(100), None), Some(secs(6)));
        assert_eq!(sim.now(), secs(6));
        // The waiter is now runnable (deadline reached): hold again.
        assert_eq!(sim.advance_if_quiescent(secs(100), None), None);
        force(&sim, &g, waiting(secs(50)));
        // A due external entry holds time; a later one bounds the step.
        assert_eq!(sim.advance_if_quiescent(secs(100), Some(secs(6))), None);
        assert_eq!(
            sim.advance_if_quiescent(secs(100), Some(secs(9))),
            Some(secs(9))
        );
        // A nearer tick wins, and an old tick never moves time back.
        assert_eq!(sim.advance_if_quiescent(secs(12), None), Some(secs(12)));
        assert_eq!(sim.advance_if_quiescent(secs(1), None), Some(secs(12)));
        // An unequal link holds it.
        let l = link(40020);
        sim.open_link(l);
        sim.c2s_sent(l, 1);
        assert_eq!(sim.advance_if_quiescent(secs(100), None), None);
    }

    #[test]
    fn wait_and_advance_waits_then_advances_in_one_step() {
        let sim = Sim::new();
        // Nothing to wait for with no actors: a tick step.
        assert_eq!(
            sim.wait_and_advance(secs(2), None, Duration::from_millis(1)),
            Some(secs(2))
        );
        let g = sim.register_actor("w");
        // Running past the limit: held.
        assert_eq!(
            sim.wait_and_advance(secs(9), None, Duration::from_millis(1)),
            None
        );
        assert_eq!(sim.now(), secs(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let sch = sim.clone();
        let t = thread::spawn(move || {
            tx.send(()).unwrap();
            sch.wait_and_advance(secs(100), None, secs(30))
        });
        rx.recv().unwrap();
        let s = sim.clone();
        let w = thread::spawn(move || {
            s.wait_until(&g, secs(7));
            g
        });
        // The scheduler sees the actor rest and advances to its deadline.
        assert_eq!(t.join().unwrap(), Some(secs(7)));
        drop(w.join().unwrap());
        assert_eq!(sim.now(), secs(7));
    }

    #[test]
    fn wait_quiescent_wakes_when_the_last_actor_rests() {
        let sim = Sim::new();
        let g = sim.register_actor("w");
        let (tx, rx) = std::sync::mpsc::channel();
        let s = sim.clone();
        let t = thread::spawn(move || {
            tx.send(()).unwrap();
            let t0 = std::time::Instant::now();
            let ok = s.wait_quiescent(None, secs(30));
            (ok, t0.elapsed())
        });
        rx.recv().unwrap();
        // Let the waiter reach the condvar (a wrong guess only weakens the
        // test, it cannot fail it).
        thread::sleep(Duration::from_millis(200));
        let s = sim.clone();
        let w = thread::spawn(move || {
            s.wait_until(&g, secs(5));
            g
        });
        let (ok, took) = t.join().unwrap();
        assert!(ok);
        assert!(
            took < secs(10),
            "woken by the state change, not the limit: {took:?}"
        );
        sim.advance_to(secs(5));
        drop(w.join().unwrap());
    }

    #[test]
    fn a_panicking_actor_is_deregistered() {
        let sim = Sim::new();
        let g = sim.register_actor("doomed");
        let (tx, rx) = std::sync::mpsc::channel();
        let s = sim.clone();
        let t = thread::spawn(move || {
            tx.send(()).unwrap();
            let t0 = std::time::Instant::now();
            let ok = s.wait_quiescent(None, secs(30));
            (ok, t0.elapsed())
        });
        rx.recv().unwrap();
        thread::sleep(Duration::from_millis(200));
        let p = thread::spawn(move || {
            let _held = g;
            panic!("invented actor failure");
        });
        assert!(p.join().is_err());
        let (ok, took) = t.join().unwrap();
        assert!(ok);
        assert!(
            took < secs(10),
            "woken by the drop, not the limit: {took:?}"
        );
        assert_eq!(sim.actor_count(), 0);
        assert!(sim.quiescent(None));
        // The sim is still usable after the panic.
        let again = sim.register_actor("next");
        assert_eq!(sim.actor_count(), 1);
        drop(again);
    }
}
