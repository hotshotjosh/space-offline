//! **Daily goals** (#202, lane DG-2; `specs/parity-dailygoals-hypernet.md` §2,
//! with the owner's ruling of 2026-11-07 in §0.4).
//!
//! The Opportunities window's daily-goal section talks to the **public
//! gateway** (package `eve_public.dailygoal.api`, `dailygoals/client/
//! goalMessenger.py`), not to machoNet. This module is that server:
//!
//! - **The day's goals are a pure function** of `(world seed, character,
//!   day)` ([`todays`]): [`N_DAILY`] distinct `daily` rows of the content
//!   table drawn by weight, then the single `bonus` row. Nothing is stored
//!   until something happens. The day turns over at 11:00 UTC
//!   ([`crate::seed::ROLLOVER`]).
//! - **Progress** is kept in delta kind [`KIND`] (`dgoal`): one row per
//!   `(character, day, slot)` holding `[progress, state, earned_isk]`.
//!   Every live-world source calls [`credit`]; see [`Act`].
//! - **The reward is a faucet.** Daily-goal ISK is granted by the daily-goals
//!   system (owner ruling 2026-11-07, spec §0.4): there is no pool and no
//!   reserve. A goal that completes with `isk > 0` holds an earning; the
//!   gateway's `Redeem` pays it as journal ref [`REF_DAILY_GOAL`] (174,
//!   `Flow::System` in `econ_ledger`), through the corporation tax skim like
//!   any other taxed payout.
//! - **The gateway arm** ([`gateway`], one `gateway_ext::CHAIN` row) answers
//!   `GetAllCurrent`, `Get`, `GetAllWithRewards`, `Redeem` and `RedeemAll`,
//!   refuses `PayForCompletion` and the QA-only `admin.Progress` with 403,
//!   and queues the four notices through `gateway_out`.
//!
//! **Bots play by the same rules.** Progress counts only where a player's
//! client could trigger it. The offline settlement of parked bots
//! (`settle_hooks`) has no row, so a bot whose kills are settled while parked
//! earns nothing that day; the claim goes through the gateway (DG-4).
//!
//! **Traps** (spec §0.3): every `Identifier.uuid` must be exactly 16 bytes
//! (`uuid.UUID(bytes=...)` raises otherwise and the whole fetch fails);
//! `isk.Currency.units` is a zigzag `sint64`; a refused redeem is swallowed by
//! the client, so a redeem is never refused for want of funds.
//!
//! `[A]` every number that is not from the client: [`N_DAILY`], [`KEEP_DAYS`],
//! the content table's `isk` and `weight`, "one Method with every matcher
//! `any`" (falsifier: a card whose objective text names a specific system or
//! corporation, or is missing), and "jumps count only through a gate"
//! (falsifier: TQ counting bridges or wormholes).

use crate::content::{Boxed, Origin, Sources, TableSpec};
use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, get_bytes, get_uint, put_bytes, put_varint};
use crate::kill_hooks::KillEvent;
use crate::mining_hooks::MinedEvent;
use crate::rollover::Day;
use crate::seed::{self, Kind};
use crate::service::{Caller, Registry, ServiceError};
use eve_marshal::PyValue;

/// The delta kind (`delta_kinds::KINDS`, owner `daily_goals`).
pub const KIND: &str = "dgoal";
/// Journal ref 174, "Daily Goal Reward" (`ledger.rs`); `Flow::System`.
pub const REF_DAILY_GOAL: i64 = 174;
/// How many `daily` goals a character gets a day `[A]`.
pub const N_DAILY: usize = 3;
/// The bonus goal's slot: right after the dailies.
pub const BONUS_SLOT: usize = N_DAILY;
/// How many days an unclaimed reward stays claimable, and a goal stays
/// readable `[A]`. Falsifier: a pilot who loses an unclaimed reward they
/// expected to keep.
pub const KEEP_DAYS: i64 = 30;

const DG_TAG: u64 = 0x4447_0001;
const DG_UUID_TAG: u64 = 0x4447_0002;
const MAX_WEIGHT: u64 = 1_000_000;

const GOALS: (&str, &str) = (
    "dailygoals/goals.tsv",
    "#key\tcategory\tmethod\ttarget\tcareer\tname_id\tdesc_id\thelp_id\tweight\tisk",
);

const PKG: &str = "eve_public.dailygoal.api.";
const N_PROGRESSED: &str = "eve_public.dailygoal.api.ProgressedNotice";
const N_COMPLETED: &str = "eve_public.dailygoal.api.CompletedNotice";
const N_REDEEMED: &str = "eve_public.dailygoal.api.RedeemedNotice";
const N_CURRENT: &str = "eve_public.dailygoal.api.CurrentGoalsNotice";

/// FILETIME of the Unix epoch.
const UNIX_EPOCH_FT: i64 = 116_444_736_000_000_000;

// ---- the content table -------------------------------------------------------------

/// What a goal asks the pilot to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    KillNpc,
    MineOre,
    SpaceJump,
    SalvageWreck,
    EarnLp,
    /// The bonus: complete the day's dailies.
    CompleteDaily,
}

impl Method {
    fn parse(s: &str) -> Option<Method> {
        Some(match s {
            "kill_npc" => Method::KillNpc,
            "mine_ore" => Method::MineOre,
            "space_jump" => Method::SpaceJump,
            "salvage_wreck" => Method::SalvageWreck,
            "earn_lp" => Method::EarnLp,
            "complete_daily" => Method::CompleteDaily,
            _ => return None,
        })
    }
}

/// `dailygoal.Category` as the content writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Daily,
    Bonus,
}

/// One authored goal, as the bundle holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRow {
    pub key: String,
    pub category: Category,
    pub method: Method,
    /// What the pilot has to reach; 0 on the bonus means "the day's dailies".
    pub target: i64,
    /// `career.Path`: 0 none, 1 explorer, 2 industrialist, 3 enforcer, 4 SoF.
    pub career: u64,
    pub name_id: u64,
    pub desc_id: u64,
    pub help_id: u64,
    pub weight: u64,
    /// The reward in ISK, 0 for none.
    pub isk: i64,
}

/// The `daily_goals` content table.
#[derive(Debug, Default, Clone)]
pub struct Goals {
    rows: Vec<GoalRow>,
}

fn num<T: std::str::FromStr>(f: &str, what: &str, line: usize) -> Result<T, String> {
    f.parse().map_err(|_| {
        format!(
            "{}: data line {line}: {what} {f:?} is not a whole number",
            GOALS.0
        )
    })
}

impl Goals {
    /// Parse the bundle's data lines. Refuses what `tools/content_dailygoals.py`
    /// refuses, so a hand-edited bundle fails loudly at start-up.
    pub fn parse(lines: &[String]) -> Result<Goals, String> {
        let mut rows: Vec<GoalRow> = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let n = i + 1;
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 10 {
                return Err(format!(
                    "{}: data line {n}: {} fields, expected 10",
                    GOALS.0,
                    f.len()
                ));
            }
            let category = match f[1] {
                "daily" => Category::Daily,
                "bonus" => Category::Bonus,
                other => return Err(format!("{}: data line {n}: category {other:?}", GOALS.0)),
            };
            let Some(method) = Method::parse(f[2]) else {
                return Err(format!("{}: data line {n}: method {:?}", GOALS.0, f[2]));
            };
            let career = match f[4] {
                "none" => 0,
                "explorer" => 1,
                "industrialist" => 2,
                "enforcer" => 3,
                "sof" => 4,
                other => return Err(format!("{}: data line {n}: career {other:?}", GOALS.0)),
            };
            let row = GoalRow {
                key: f[0].to_owned(),
                category,
                method,
                target: num(f[3], "target", n)?,
                career,
                name_id: num(f[5], "name_id", n)?,
                desc_id: num(f[6], "desc_id", n)?,
                help_id: num(f[7], "help_id", n)?,
                weight: num(f[8], "weight", n)?,
                isk: num(f[9], "isk", n)?,
            };
            if row.key.is_empty() || rows.iter().any(|r| r.key == row.key) {
                return Err(format!(
                    "{}: data line {n}: empty or duplicate key",
                    GOALS.0
                ));
            }
            if row.target < 0 || row.isk < 0 || row.weight > MAX_WEIGHT {
                return Err(format!(
                    "{}: data line {n}: a number is out of range",
                    GOALS.0
                ));
            }
            if (category == Category::Bonus) != (method == Method::CompleteDaily) {
                return Err(format!(
                    "{}: data line {n}: only the bonus completes dailies",
                    GOALS.0
                ));
            }
            if row.target == 0 && category == Category::Daily {
                return Err(format!("{}: data line {n}: a daily with target 0", GOALS.0));
            }
            if category == Category::Bonus && rows.iter().any(|r| r.category == Category::Bonus) {
                return Err(format!("{}: data line {n}: a second bonus row", GOALS.0));
            }
            rows.push(row);
        }
        Ok(Goals { rows })
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn daily(&self, key: &str) -> Option<&GoalRow> {
        self.rows
            .iter()
            .find(|r| r.category == Category::Daily && r.key == key)
    }

    fn bonus(&self) -> Option<&GoalRow> {
        self.rows.iter().find(|r| r.category == Category::Bonus)
    }
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "daily_goals",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    // The label ids are client data: no shipped copy. No bundle, no goals.
    let Some(b) = src.bundle else {
        return Ok(Box::new(Goals::default()));
    };
    let lines = b.data_lines(GOALS.0, GOALS.1)?;
    if lines.is_empty() {
        src.note(Origin::Fixture);
    }
    Ok(Box::new(Goals::parse(&lines)?))
}

/// Empty on purpose: a test that wants goals inserts its own table, and every
/// other test sees no goal and no `dgoal` row.
fn boxed_fixture() -> Boxed {
    Box::new(Goals::default())
}

fn table(reg: &Registry) -> &Goals {
    static EMPTY: std::sync::OnceLock<Goals> = std::sync::OnceLock::new();
    reg.content()
        .try_get::<Goals>()
        .unwrap_or_else(|| EMPTY.get_or_init(Goals::default))
}

// ---- the day ------------------------------------------------------------------------

/// The game day holding blue FILETIME `t`: days start at 11:00 UTC.
pub fn day_of(t: i64) -> i64 {
    (t - seed::ROLLOVER).div_euclid(seed::DAY)
}

fn day_start(day: i64) -> i64 {
    day * seed::DAY + seed::ROLLOVER
}

fn unix_secs(ft: i64) -> i64 {
    (ft - UNIX_EPOCH_FT).div_euclid(10_000_000)
}

// ---- stored progress ----------------------------------------------------------------

/// Where a goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Still being worked on.
    Open,
    /// Reached its target; the earning (if any) is held.
    Done,
    /// The earning was paid.
    Redeemed,
}

/// One `dgoal` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prog {
    pub progress: i64,
    pub state: State,
    pub earned: i64,
}

fn rkey(ch: i64, day: i64, slot: usize) -> Vec<String> {
    vec![ch.to_string(), day.to_string(), slot.to_string()]
}

fn okey(ch: i64, day: i64, slot: usize) -> Vec<String> {
    let mut k = rkey(ch, day, slot);
    k.push("key".to_owned());
    k
}

/// The stored progress of `(ch, day, slot)`; a goal nobody has touched is
/// open at 0.
pub fn prog(d: &Deltas, ch: i64, day: i64, slot: usize) -> Prog {
    let v = d.of(KIND).get(&rkey(ch, day, slot));
    let n = |i: usize| {
        v.and_then(|v| v.get(i))
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let state = match v.and_then(|v| v.get(1)).map(String::as_str) {
        Some("d") => State::Done,
        Some("r") => State::Redeemed,
        _ => State::Open,
    };
    Prog {
        progress: n(0),
        state,
        earned: n(2),
    }
}

fn put_prog(reg: &mut Registry, ch: i64, day: i64, slot: usize, p: Prog) {
    let s = match p.state {
        State::Open => "o",
        State::Done => "d",
        State::Redeemed => "r",
    };
    reg.deltas_mut().put(
        KIND,
        rkey(ch, day, slot),
        vec![p.progress.to_string(), s.to_owned(), p.earned.to_string()],
    );
}

// ---- generation ---------------------------------------------------------------------

/// One of a character's goals for one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub slot: usize,
    pub day: i64,
    pub row: GoalRow,
    /// The target the pilot has to reach (the bonus's 0 resolved).
    pub target: i64,
}

/// The goals of `ch` on `day`, a pure function of the world seed, the
/// character and the day (plus a GM `swap` override). Empty when the content
/// has no daily row.
pub fn todays(reg: &Registry, ch: i64, day: i64) -> Vec<Goal> {
    let t = table(reg);
    let mut pool: Vec<&GoalRow> = t
        .rows
        .iter()
        .filter(|r| r.category == Category::Daily && r.weight > 0)
        .collect();
    let n = N_DAILY.min(pool.len());
    let mut s = seed::stream(
        reg.world().seed,
        Kind::Loot,
        &[DG_TAG, ch as u64, day as u64],
    );
    let mut out = Vec::new();
    for slot in 0..n {
        let total: u64 = pool.iter().map(|r| r.weight).sum();
        let mut r = s.range(0, total);
        let idx = pool
            .iter()
            .position(|row| {
                if r < row.weight {
                    true
                } else {
                    r -= row.weight;
                    false
                }
            })
            .unwrap_or(0);
        let drawn = pool.remove(idx);
        let row = reg
            .deltas()
            .of(KIND)
            .get(&okey(ch, day, slot))
            .and_then(|v| v.first())
            .and_then(|k| t.daily(k))
            .unwrap_or(drawn);
        out.push(Goal {
            slot,
            day,
            target: row.target,
            row: row.clone(),
        });
    }
    if n > 0
        && let Some(b) = t.bonus()
    {
        out.push(Goal {
            slot: BONUS_SLOT,
            day,
            target: if b.target == 0 { n as i64 } else { b.target },
            row: b.clone(),
        });
    }
    out
}

/// The 16-byte id of `(ch, day, slot)`. The client only compares them.
pub fn goal_uuid(seed: u64, ch: i64, day: i64, slot: usize) -> [u8; 16] {
    let mut s = seed::stream(
        seed,
        Kind::Loot,
        &[DG_UUID_TAG, ch as u64, day as u64, slot as u64],
    );
    let (a, b) = (s.next(), s.next());
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&a.to_be_bytes());
    u[8..].copy_from_slice(&b.to_be_bytes());
    u
}

/// The id a `Payment.asset` carries: stable, 16 bytes, not the goal's own.
fn asset_uuid(u: [u8; 16]) -> [u8; 16] {
    let mut a = u;
    a[15] ^= 0x5a;
    a
}

/// The `(day, slot)` whose id is `uuid`, among the days still kept at `now`.
pub fn find(reg: &Registry, ch: i64, uuid: &[u8], now: i64) -> Option<(i64, usize)> {
    if uuid.len() != 16 {
        return None;
    }
    let seed = reg.world().seed;
    let today = day_of(now);
    (today - KEEP_DAYS..=today)
        .rev()
        .flat_map(|d| (0..=BONUS_SLOT).map(move |s| (d, s)))
        .find(|&(d, s)| goal_uuid(seed, ch, d, s) == uuid)
}

// ---- progress -----------------------------------------------------------------------

/// What a pilot did, as the sources report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    KillNpc,
    /// Units of ore taken.
    Mine(i64),
    /// A gate jump `[A]`.
    Jump,
    Salvage,
    /// Loyalty points earned.
    EarnLp(i64),
}

impl Act {
    fn parts(self) -> (Method, i64) {
        match self {
            Act::KillNpc => (Method::KillNpc, 1),
            Act::Mine(u) => (Method::MineOre, u),
            Act::Jump => (Method::SpaceJump, 1),
            Act::Salvage => (Method::SalvageWreck, 1),
            Act::EarnLp(n) => (Method::EarnLp, n),
        }
    }
}

fn goal_id_msg(reg: &Registry, ch: i64, day: i64, slot: usize) -> Vec<u8> {
    let u = goal_uuid(reg.world().seed, ch, day, slot);
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &u);
    out
}

/// A notice body whose field 1 is the goal `Identifier` as a length-delimited
/// message (`ProgressedNotice`, `CompletedNotice` and `RedeemedNotice` all
/// declare `1 msg Identifier goal`; `[V]` notices_pb2.py, nested descriptor
/// `goal` type 11). Putting the bare uuid in field 1 makes the client's
/// `Unpack` fail with a DecodeError.
fn goal_notice(reg: &Registry, ch: i64, day: i64, slot: usize) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &goal_id_msg(reg, ch, day, slot));
    out
}

fn notify(reg: &mut Registry, ch: i64, ty: &str, payload: Vec<u8>) {
    crate::gateway_out::queue(reg, ty, payload, Target::Character(ch));
}

fn put_field_varint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// Move one goal forward by `by`, capped at its target. Answers whether this
/// call completed it.
fn advance(reg: &mut Registry, ch: i64, g: &Goal, by: i64) -> bool {
    let mut p = prog(reg.deltas(), ch, g.day, g.slot);
    if p.state != State::Open || by <= 0 {
        return false;
    }
    let new = p.progress.saturating_add(by).min(g.target);
    if new == p.progress {
        return false;
    }
    p.progress = new;
    let done = new >= g.target;
    if done {
        p.state = State::Done;
        // The reward is the content's, granted by the daily-goals system.
        p.earned = g.row.isk.max(0);
    }
    put_prog(reg, ch, g.day, g.slot, p);
    let mut m = goal_notice(reg, ch, g.day, g.slot);
    put_field_varint(&mut m, 2, new as u64);
    notify(reg, ch, N_PROGRESSED, m);
    if done {
        let m = goal_notice(reg, ch, g.day, g.slot);
        notify(reg, ch, N_COMPLETED, m);
    }
    done
}

/// **The one entry point every source calls**: `ch` did `act` at `now` (blue
/// FILETIME). Counts toward each of today's open dailies that asks for it, and
/// every daily that completes moves the bonus by one. Does nothing for a
/// pilot the world does not know or one with no live session: a logged-off
/// pilot's drones, and a parked bot, earn nothing (a bot plays through its own
/// client connection, so it has a session while it plays).
pub fn credit(reg: &mut Registry, ch: i64, act: Act, now: i64) {
    if table(reg).is_empty() || reg.character(ch).is_none() || !reg.sessions().is_online(ch) {
        return;
    }
    let (method, by) = act.parts();
    let goals = todays(reg, ch, day_of(now));
    let mut completed = 0;
    for g in goals
        .iter()
        .filter(|g| g.slot < BONUS_SLOT && g.row.method == method)
    {
        if advance(reg, ch, g, by) {
            completed += 1;
        }
    }
    if completed > 0
        && let Some(b) = goals.iter().find(|g| g.slot == BONUS_SLOT)
    {
        advance(reg, ch, b, completed);
    }
}

/// **The `kill_hooks::NPC` row**: the killer's owner killed an NPC.
pub fn on_npc_kill(w: &mut Registry, ev: &KillEvent) {
    let Some(killer) = ev.killer else { return };
    let Some(p) = w.pilot(killer) else { return };
    credit(w, p.owner_id, Act::KillNpc, crate::pyutil::filetime_now());
}

/// **The `mining_hooks::MINED` row**: ore left a rock into a hold.
pub fn on_mined(w: &mut Registry, ev: &MinedEvent) {
    let units = ev.normal.saturating_add(ev.crit);
    credit(w, ev.miner, Act::Mine(units), crate::pyutil::filetime_now());
}

// ---- rewards ------------------------------------------------------------------------

/// Pay the held earning of `(ch, day, slot)`: ISK from the daily-goals system
/// (a faucet, owner ruling 2026-11-07), net of the corporation tax skim,
/// journal ref 174. `false` when there is nothing held.
pub fn redeem(reg: &mut Registry, ch: i64, day: i64, slot: usize) -> bool {
    use crate::service::corp_wallet::{self, Via};
    let mut p = prog(reg.deltas(), ch, day, slot);
    if p.state != State::Done || p.earned <= 0 || reg.character(ch).is_none() {
        return false;
    }
    let gross = p.earned as f64;
    p.state = State::Redeemed;
    put_prog(reg, ch, day, slot, p);
    let reference = day * 10 + slot as i64;
    let skim = corp_wallet::skim(reg, Via::Clock, ch, gross, REF_DAILY_GOAL, reference);
    let net = gross - skim.map_or(0.0, |s| s.tax);
    let description = corp_wallet::stamped("", REF_DAILY_GOAL, skim);
    corp_wallet::char_post(
        reg,
        Via::Clock,
        ch,
        net,
        REF_DAILY_GOAL,
        1,
        ch,
        reference,
        &description,
    );
    let m = goal_notice(reg, ch, day, slot);
    notify(reg, ch, N_REDEEMED, m);
    true
}

/// Every held earning of `ch`, oldest day first: `(day, slot)`.
pub fn held(reg: &Registry, ch: i64, now: i64) -> Vec<(i64, usize)> {
    let today = day_of(now);
    (today - KEEP_DAYS..=today)
        .flat_map(|d| (0..=BONUS_SLOT).map(move |s| (d, s)))
        .filter(|&(d, s)| {
            let p = prog(reg.deltas(), ch, d, s);
            p.state == State::Done && p.earned > 0
        })
        .collect()
}

// ---- the rollover -------------------------------------------------------------------

/// Tell these characters the day turned over; the client flushes its cache.
fn announce_new_day(reg: &mut Registry, to: &[i64]) {
    for &c in to {
        notify(reg, c, N_CURRENT, Vec::new());
    }
}

/// **The `rollover_steps` row**: drop rows older than [`KEEP_DAYS`], then tell
/// every online character. Its count is the rows dropped.
pub fn on_rollover(w: &mut Registry, d: &Day) -> usize {
    let cutoff = day_of(d.now) - KEEP_DAYS;
    let old: Vec<Vec<String>> = w
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| {
            k.get(1)
                .and_then(|s| s.parse::<i64>().ok())
                .is_some_and(|day| day < cutoff)
        })
        .cloned()
        .collect();
    let n = old.len();
    for k in old {
        w.deltas_mut().del(KIND, &k);
    }
    let online = w.sessions().online_characters();
    announce_new_day(w, &online);
    n
}

// ---- the wire -----------------------------------------------------------------------

fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

fn sequential(id: u64) -> Vec<u8> {
    let mut o = Vec::new();
    put_field_varint(&mut o, 1, id);
    o
}

/// `Unit{isk{Currency{units}}}`; `units` is a zigzag `sint64`.
fn unit_isk(isk: i64) -> Vec<u8> {
    let mut cur = Vec::new();
    put_field_varint(&mut cur, 1, zigzag(isk));
    let mut isk_msg = Vec::new();
    put_bytes(&mut isk_msg, 1, &cur);
    let mut unit = Vec::new();
    put_bytes(&mut unit, 1, &isk_msg);
    unit
}

/// A matcher with `any: true`.
fn matcher_any() -> Vec<u8> {
    let mut m = Vec::new();
    put_field_varint(&mut m, 1, 1);
    m
}

/// `ContributionConfiguration` with one Method, every matcher `any` `[A]`.
fn contribution(m: Method) -> Vec<u8> {
    let mut method = Vec::new();
    let (field_no, body): (u64, Option<Vec<u8>>) = match m {
        Method::KillNpc => {
            put_bytes(&mut method, 1, &matcher_any());
            (1, Some(method))
        }
        Method::MineOre => {
            put_bytes(&mut method, 1, &matcher_any());
            put_bytes(&mut method, 2, &matcher_any());
            (3, Some(method))
        }
        Method::SalvageWreck => {
            put_bytes(&mut method, 1, &matcher_any());
            (11, Some(method))
        }
        Method::EarnLp => {
            put_bytes(&mut method, 1, &matcher_any());
            (12, Some(method))
        }
        // `space_jump` is an empty message; its presence is the method.
        Method::SpaceJump => (13, None),
        Method::CompleteDaily => {
            // DailyGoalCategoryMatcher{2 category = DAILY (1)}
            let mut cat = Vec::new();
            put_field_varint(&mut cat, 2, 1);
            put_bytes(&mut method, 2, &cat);
            (10, Some(method))
        }
    };
    let mut cfg = Vec::new();
    match body {
        Some(method) => {
            let mut wrap = Vec::new();
            put_bytes(&mut wrap, 1, &method);
            put_bytes(&mut cfg, field_no, &wrap);
        }
        None => put_bytes(&mut cfg, field_no, &[]),
    }
    cfg
}

fn timestamp(ft: i64) -> Vec<u8> {
    let mut t = Vec::new();
    put_field_varint(&mut t, 1, unix_secs(ft) as u64);
    t
}

/// `Attributes` of one goal.
fn attributes(reg: &Registry, ch: i64, g: &Goal) -> Vec<u8> {
    let r = &g.row;
    let mut a = Vec::new();
    put_bytes(&mut a, 1, &sequential(r.name_id));
    put_bytes(&mut a, 2, &sequential(r.desc_id));
    put_bytes(&mut a, 3, &sequential(r.help_id));
    put_field_varint(
        &mut a,
        5,
        match r.category {
            Category::Daily => 1,
            Category::Bonus => 2,
        },
    );
    put_bytes(&mut a, 6, &contribution(r.method));
    if r.career > 0 {
        put_field_varint(&mut a, 7, r.career);
    }
    if r.isk > 0 {
        let asset = asset_uuid(goal_uuid(reg.world().seed, ch, g.day, g.slot));
        let mut asset_msg = Vec::new();
        put_bytes(&mut asset_msg, 1, &asset);
        let mut pay = Vec::new();
        put_bytes(&mut pay, 1, &asset_msg);
        put_field_varint(&mut pay, 2, 1);
        put_bytes(&mut pay, 3, &unit_isk(r.isk));
        put_bytes(&mut a, 8, &pay);
    }
    put_field_varint(&mut a, 9, g.target.max(0) as u64);
    put_bytes(&mut a, 10, &timestamp(day_start(g.day)));
    put_bytes(&mut a, 11, &timestamp(day_start(g.day + 1)));
    a
}

/// `Earning{unit}` for a held reward, nothing otherwise.
fn earnings(p: &Prog) -> Vec<Vec<u8>> {
    if p.state == State::Done && p.earned > 0 {
        let mut e = Vec::new();
        put_bytes(&mut e, 1, &unit_isk(p.earned));
        vec![e]
    } else {
        Vec::new()
    }
}

/// One `GetAllCurrentResponse.goals` entry.
fn goal_entry(reg: &Registry, ch: i64, g: &Goal) -> Vec<u8> {
    let p = prog(reg.deltas(), ch, g.day, g.slot);
    let mut o = Vec::new();
    put_bytes(&mut o, 1, &goal_id_msg(reg, ch, g.day, g.slot));
    put_bytes(&mut o, 2, &attributes(reg, ch, g));
    put_field_varint(&mut o, 3, p.progress.max(0) as u64);
    for e in earnings(&p) {
        put_bytes(&mut o, 5, &e);
    }
    o
}

fn reply(status: u32, name: Option<String>, body: Vec<u8>) -> Reply {
    Reply {
        status,
        payload: name.map(|n| (n, body)),
    }
}

fn refuse(status: u32) -> Option<Reply> {
    Some(reply(status, None, Vec::new()))
}

/// The goal id inside a request's field 1 (`Identifier{1 uuid}`).
fn request_uuid(body: &[u8]) -> Option<Vec<u8>> {
    get_bytes(body, 1).and_then(|g| get_bytes(&g, 1))
}

/// **The gateway arm** (`gateway_ext::CHAIN`): every
/// `eve_public.dailygoal.api.*` request. `None` for anything else.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    let name = format!("{}Response", ty.strip_suffix("Request").unwrap_or(ty));
    // QA-only (`goalMessenger.set_goal_progress`), and a PLEX sink we do not
    // have: refused, and the client reports the failure.
    if rest.starts_with("admin.") || rest == "PayForCompletionRequest" {
        return refuse(403);
    }
    if !matches!(
        rest,
        "GetAllCurrentRequest"
            | "GetRequest"
            | "GetAllWithRewardsRequest"
            | "RedeemRequest"
            | "RedeemAllRequest"
    ) {
        return None;
    }
    let Ground::Server(w) = g else {
        return if rest == "GetAllCurrentRequest" {
            Some(reply(200, Some(name), Vec::new()))
        } else {
            refuse(404)
        };
    };
    let now = crate::pyutil::filetime_now();
    match rest {
        "GetAllCurrentRequest" => {
            let reg = crate::service::world_read_timed(w, "daily_goals_current");
            let mut out = Vec::new();
            for goal in todays(&reg, character, day_of(now)) {
                put_bytes(&mut out, 1, &goal_entry(&reg, character, &goal));
            }
            Some(reply(200, Some(name), out))
        }
        "GetRequest" => {
            let reg = crate::service::world_read_timed(w, "daily_goals_get");
            let Some((day, slot)) = request_uuid(body).and_then(|u| find(&reg, character, &u, now))
            else {
                return refuse(404);
            };
            let Some(goal) = todays(&reg, character, day)
                .into_iter()
                .find(|g| g.slot == slot)
            else {
                return refuse(404);
            };
            let p = prog(reg.deltas(), character, day, slot);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &attributes(&reg, character, &goal));
            put_field_varint(&mut out, 2, p.progress.max(0) as u64);
            for e in earnings(&p) {
                put_bytes(&mut out, 4, &e);
            }
            Some(reply(200, Some(name), out))
        }
        "GetAllWithRewardsRequest" => {
            let reg = crate::service::world_read_timed(w, "daily_goals_rewards");
            let page = get_bytes(body, 1).unwrap_or_default();
            let size = get_uint(&page, 1).filter(|n| *n > 0).unwrap_or(50) as usize;
            let offset = get_bytes(&page, 2)
                .and_then(|t| String::from_utf8(t).ok())
                .and_then(|t| t.parse::<usize>().ok())
                .unwrap_or(0);
            let all = held(&reg, character, now);
            let mut out = Vec::new();
            for &(d, s) in all.iter().skip(offset).take(size) {
                put_bytes(&mut out, 1, &goal_id_msg(&reg, character, d, s));
            }
            let next = offset.saturating_add(size);
            let mut np = Vec::new();
            if next < all.len() {
                put_bytes(&mut np, 1, next.to_string().as_bytes());
            }
            put_bytes(&mut out, 2, &np);
            Some(reply(200, Some(name), out))
        }
        "RedeemRequest" | "RedeemAllRequest" => {
            let mut reg = crate::service::world_write_timed(w, "daily_goals_redeem");
            let ok = if rest == "RedeemRequest" {
                request_uuid(body)
                    .and_then(|u| find(&reg, character, &u, now))
                    .is_some_and(|(d, s)| redeem(&mut reg, character, d, s))
            } else {
                for (d, s) in held(&reg, character, now) {
                    redeem(&mut reg, character, d, s);
                }
                true
            };
            let batch = crate::gateway_out::take(&mut reg);
            drop(reg);
            crate::gateway_out::flush(batch);
            // Nothing to redeem: 404, which the client swallows (spec trap 5).
            if ok {
                Some(reply(200, Some(name), Vec::new()))
            } else {
                refuse(404)
            }
        }
        _ => None,
    }
}

// ---- GM -----------------------------------------------------------------------------

fn gm_allowed(reg: &Registry) -> bool {
    match reg.caller() {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

fn state_word(s: State) -> &'static str {
    match s {
        State::Open => "open",
        State::Done => "done",
        State::Redeemed => "redeemed",
    }
}

fn gm_run(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    if !gm_allowed(reg) {
        return "/dailygoals is for GMs".to_owned();
    }
    let Some(ch) = actor else {
        return "no character".to_owned();
    };
    let now = crate::pyutil::filetime_now();
    let day = day_of(now);
    match args.first().copied() {
        None => {
            let goals = todays(reg, ch, day);
            if goals.is_empty() {
                return "no daily goals (the content bundle has none)".to_owned();
            }
            goals
                .iter()
                .map(|g| {
                    let p = prog(reg.deltas(), ch, day, g.slot);
                    format!(
                        "{} {} {}/{} {} earned {}",
                        g.slot,
                        g.row.key,
                        p.progress,
                        g.target,
                        state_word(p.state),
                        p.earned
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Some("swap") => {
            let (Some(slot), Some(key)) = (
                args.get(1).and_then(|s| s.parse::<usize>().ok()),
                args.get(2),
            ) else {
                return "usage: /dailygoals swap <slot> <key>".to_owned();
            };
            let goals = todays(reg, ch, day);
            if slot >= N_DAILY || !goals.iter().any(|g| g.slot == slot) {
                return format!("slot {slot} is not one of today's daily slots");
            }
            if table(reg).daily(key).is_none() {
                return format!("no daily goal called {key}");
            }
            if let Some(other) = goals.iter().find(|g| g.slot != slot && g.row.key == *key) {
                return format!("{key} is already in slot {}", other.slot);
            }
            let p = prog(reg.deltas(), ch, day, slot);
            if p.progress > 0 || p.state != State::Open {
                return format!("slot {slot} has progress; a swap would hide it");
            }
            reg.deltas_mut()
                .put(KIND, okey(ch, day, slot), vec![(*key).to_owned()]);
            announce_new_day(reg, &[ch]);
            format!("slot {slot} is now {key}")
        }
        Some(_) => "usage: /dailygoals | /dailygoals swap <slot> <key>".to_owned(),
    }
}

/// `/dailygoals` (a `slash::COMMANDS` row): list today's goals, or `swap` one
/// that has no progress yet (the CI's way to a known goal). GM role only;
/// moves no ISK and no item.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(crate::pyutil::bstr(&gm_run(reg, actor, args)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::field;
    use std::sync::{Arc, RwLock};

    const ALICE: i64 = 1_001;
    const BOB: i64 = 1_002;
    /// A pilot who exists but is never logged in.
    const CAROL: i64 = 1_003;
    /// An arbitrary instant; tests that need "today" use `filetime_now()`.
    const NOW: i64 = 133_000_000_000_000_000;

    /// Five dailies and a bonus, every number invented.
    const BIG: &[&str] = &[
        "ta\tdaily\tkill_npc\t3\tenforcer\t11\t12\t13\t1\t300",
        "tb\tdaily\tmine_ore\t50\tindustrialist\t21\t22\t23\t1\t700",
        "tc\tdaily\tspace_jump\t2\texplorer\t31\t32\t33\t1\t0",
        "td\tdaily\tsalvage_wreck\t2\tindustrialist\t41\t42\t43\t1\t500",
        "te\tdaily\tearn_lp\t10\tsof\t51\t52\t53\t1\t900",
        "tz\tbonus\tcomplete_daily\t0\tnone\t61\t62\t63\t1\t1100",
    ];
    /// Exactly three dailies, so all three are always drawn.
    const SMALL: &[&str] = &[
        "ta\tdaily\tkill_npc\t3\tenforcer\t11\t12\t13\t1\t300",
        "tb\tdaily\tmine_ore\t50\tindustrialist\t21\t22\t23\t1\t700",
        "tc\tdaily\tspace_jump\t2\texplorer\t31\t32\t33\t1\t0",
        "tz\tbonus\tcomplete_daily\t0\tnone\t61\t62\t63\t1\t1100",
    ];

    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|s| (*s).to_owned()).collect()
    }

    fn world(rows: &[&str]) -> Registry {
        let mut reg = crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: ALICE,
                owner_user_id: Some(11),
                ..Default::default()
            };
            let mut b = a.clone();
            b.character_id = BOB;
            b.ship_id = a.ship_id + 1;
            b.owner_user_id = Some(12);
            let mut c = a.clone();
            c.character_id = CAROL;
            c.ship_id = a.ship_id + 2;
            c.owner_user_id = Some(13);
            cfg.characters = vec![a, b, c];
            cfg.catch_all = None;
        });
        let mut content = crate::content::Content::fixture();
        content.insert("daily_goals", Goals::parse(&lines(rows)).unwrap());
        reg = reg.with_content(Arc::new(content));
        // Alice and Bob are playing; Carol is logged off.
        for (client, who, user) in [(10_000_000_001, ALICE, 11), (10_000_000_002, BOB, 12)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: client,
                session_id: client,
                user_id: user,
                character_id: Some(who),
            });
        }
        reg
    }

    /// How many `daily` rows a table has.
    fn dailies(rows: &[&str]) -> usize {
        rows.iter()
            .filter(|r| r.split('\t').nth(1) == Some("daily"))
            .count()
    }

    /// Do what completes `g`.
    fn finish(reg: &mut Registry, ch: i64, g: &Goal, now: i64) {
        let act = match g.row.method {
            Method::KillNpc => Act::KillNpc,
            Method::MineOre => Act::Mine(g.target),
            Method::SpaceJump => Act::Jump,
            Method::SalvageWreck => Act::Salvage,
            Method::EarnLp => Act::EarnLp(g.target),
            Method::CompleteDaily => unreachable!("the bonus is never done by hand"),
        };
        let times = match act {
            Act::Mine(_) | Act::EarnLp(_) => 1,
            _ => g.target,
        };
        for _ in 0..times {
            credit(reg, ch, act, now);
        }
    }

    /// The goal with content key `key` among `ch`'s goals on `day`.
    fn goal_of(reg: &Registry, ch: i64, day: i64, key: &str) -> Goal {
        todays(reg, ch, day)
            .into_iter()
            .find(|g| g.row.key == key)
            .unwrap_or_else(|| panic!("{key} is not among today's goals"))
    }

    fn notices(reg: &mut Registry, ty: &str) -> usize {
        crate::gateway_out::take(reg)
            .iter()
            .filter(|n| n.0 == ty)
            .count()
    }

    fn balance(reg: &Registry, ch: i64) -> f64 {
        reg.character(ch).unwrap().balance
    }

    // ---- generation ----

    #[test]
    fn the_day_is_deterministic_has_three_dailies_and_one_bonus() {
        let reg = world(BIG);
        let day = day_of(NOW);
        let a = todays(&reg, ALICE, day);
        assert_eq!(a, todays(&reg, ALICE, day), "a pure function");
        assert_eq!(a.len(), N_DAILY + 1);
        let keys: Vec<&str> = a[..N_DAILY].iter().map(|g| g.row.key.as_str()).collect();
        for (i, k) in keys.iter().enumerate() {
            assert!(!keys[i + 1..].contains(k), "{keys:?} are distinct");
        }
        assert!(
            a[..N_DAILY]
                .iter()
                .all(|g| g.row.category == Category::Daily)
        );
        assert_eq!(a[N_DAILY].row.key, "tz");
        assert_eq!(a[N_DAILY].slot, BONUS_SLOT);
        assert_eq!(
            a[N_DAILY].target, N_DAILY as i64,
            "target 0 means the dailies"
        );
        // It differs by character and by day somewhere in a window.
        let differs_by_char =
            (0..30).any(|d| todays(&reg, ALICE, day + d) != todays(&reg, BOB, day + d));
        let differs_by_day = (1..30).any(|d| {
            let keys = |v: Vec<Goal>| v.into_iter().map(|g| g.row.key).collect::<Vec<_>>();
            keys(todays(&reg, ALICE, day)) != keys(todays(&reg, ALICE, day + d))
        });
        assert!(differs_by_char && differs_by_day);
    }

    #[test]
    fn an_empty_table_has_no_goals_and_credit_writes_nothing() {
        let mut reg = world(&[]);
        assert!(todays(&reg, ALICE, day_of(NOW)).is_empty());
        credit(&mut reg, ALICE, Act::KillNpc, NOW);
        assert!(reg.deltas().of(KIND).is_empty());
        assert!(crate::gateway_out::take(&mut reg).is_empty());
    }

    #[test]
    fn goal_ids_are_sixteen_bytes_unique_and_found_again() {
        let reg = world(BIG);
        let seed = reg.world().seed;
        let today = day_of(NOW);
        let mut seen = std::collections::BTreeSet::new();
        for ch in [ALICE, BOB] {
            for d in today - 29..=today {
                for s in 0..=BONUS_SLOT {
                    let u = goal_uuid(seed, ch, d, s);
                    assert_eq!(u.len(), 16);
                    assert!(seen.insert(u), "ids must not collide");
                    assert_ne!(asset_uuid(u), u);
                    assert_eq!(find(&reg, ch, &u, NOW), Some((d, s)));
                }
            }
        }
        assert_eq!(find(&reg, ALICE, &[7; 16], NOW), None);
        assert_eq!(
            find(&reg, ALICE, &[7; 4], NOW),
            None,
            "short ids are refused"
        );
        let old = goal_uuid(seed, ALICE, today - KEEP_DAYS - 1, 0);
        assert_eq!(find(&reg, ALICE, &old, NOW), None, "older than KEEP_DAYS");
    }

    #[test]
    fn the_table_refuses_what_the_content_tool_refuses() {
        for bad in [
            "x\tdaily\tkill_npc\t0\tnone\t1\t2\t3\t1\t5",
            "x\tdaily\tflying\t3\tnone\t1\t2\t3\t1\t5",
            "x\tweekly\tkill_npc\t3\tnone\t1\t2\t3\t1\t5",
            "x\tdaily\tkill_npc\t3\tpirate\t1\t2\t3\t1\t5",
            "x\tdaily\tkill_npc\t3\tnone\t1\t2\t3\t1\t-5",
            "x\tbonus\tkill_npc\t0\tnone\t1\t2\t3\t1\t5",
            "x\tdaily\tkill_npc\t3\tnone\t1\t2\t3\t1",
        ] {
            assert!(Goals::parse(&lines(&[bad])).is_err(), "{bad}");
        }
        let twice = [SMALL[3], "y\tbonus\tcomplete_daily\t0\tnone\t1\t2\t3\t1\t5"];
        assert!(Goals::parse(&lines(&twice)).is_err(), "one bonus only");
        assert!(
            Goals::parse(&lines(&[SMALL[0], SMALL[0]])).is_err(),
            "duplicate key"
        );
        assert_eq!(Goals::parse(&lines(SMALL)).unwrap().rows.len(), SMALL.len());
    }

    // ---- progress ----

    #[test]
    fn kills_complete_the_kill_goal_and_move_the_bonus_by_one() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let slot = goal_of(&reg, ALICE, day, "ta").slot;
        for _ in 0..2 {
            credit(&mut reg, ALICE, Act::KillNpc, NOW);
        }
        let p = prog(reg.deltas(), ALICE, day, slot);
        assert_eq!((p.progress, p.state), (2, State::Open));
        assert_eq!(prog(reg.deltas(), ALICE, day, BONUS_SLOT).progress, 0);
        assert_eq!(notices(&mut reg, N_COMPLETED), 0);
        credit(&mut reg, ALICE, Act::KillNpc, NOW);
        let p = prog(reg.deltas(), ALICE, day, slot);
        assert_eq!((p.progress, p.state, p.earned), (3, State::Done, 300));
        let b = prog(reg.deltas(), ALICE, day, BONUS_SLOT);
        assert_eq!((b.progress, b.state), (1, State::Open));
        let theirs = goal_of(&reg, BOB, day, "ta").slot;
        assert_eq!(
            prog(reg.deltas(), BOB, day, theirs).progress,
            0,
            "per character"
        );
    }

    /// `[V]` notices_pb2.py: Progressed/Completed/Redeemed notices all declare
    /// `1 msg dailygoal.Identifier goal`, and `Identifier` is `1 bytes uuid`.
    /// Field 1 must hold a message whose field 1 holds the 16-byte uuid.
    #[test]
    fn the_goal_notices_nest_the_goal_identifier_as_a_message() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let kill = goal_of(&reg, ALICE, day, "ta");
        let want = goal_uuid(reg.world().seed, ALICE, day, kill.slot);
        let nested = |body: &[u8]| {
            let id = get_bytes(body, 1).expect("field 1 is the goal message");
            // Tag 0x0a, length 0x10, 16 uuid bytes: nothing trailing or unknown.
            assert_eq!(id.len(), 18, "Identifier is exactly one uuid field");
            assert_eq!(
                get_bytes(&id, 1).as_deref(),
                Some(&want[..]),
                "Identifier.uuid"
            );
        };
        for _ in 0..3 {
            credit(&mut reg, ALICE, Act::KillNpc, NOW);
        }
        let sent = crate::gateway_out::take(&mut reg);
        let progressed: Vec<_> = sent.iter().filter(|n| n.0 == N_PROGRESSED).collect();
        // Three kill steps, then the bonus moved by one.
        assert_eq!(progressed.len(), 4);
        for (i, n) in progressed.iter().take(3).enumerate() {
            nested(&n.1);
            assert_eq!(get_uint(&n.1, 2), Some(i as u64 + 1), "current_progress");
        }
        // The bonus goal's own notice: its id, progress 1.
        let bonus = get_bytes(&progressed[3].1, 1).expect("the bonus goal message");
        let bonus_want = goal_uuid(reg.world().seed, ALICE, day, BONUS_SLOT);
        assert_eq!(get_bytes(&bonus, 1).as_deref(), Some(&bonus_want[..]));
        assert_eq!(get_uint(&progressed[3].1, 2), Some(1), "bonus progress");
        let done = sent.iter().find(|n| n.0 == N_COMPLETED).expect("completed");
        nested(&done.1);
        assert!(redeem(&mut reg, ALICE, day, kill.slot));
        let sent = crate::gateway_out::take(&mut reg);
        let paid = sent.iter().find(|n| n.0 == N_REDEEMED).expect("redeemed");
        nested(&paid.1);
    }

    #[test]
    fn progress_caps_at_the_target_and_completes_once() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let slot = goal_of(&reg, ALICE, day, "ta").slot;
        for _ in 0..3 {
            credit(&mut reg, ALICE, Act::KillNpc, NOW);
        }
        let _ = crate::gateway_out::take(&mut reg);
        credit(&mut reg, ALICE, Act::KillNpc, NOW);
        credit(&mut reg, ALICE, Act::KillNpc, NOW);
        assert_eq!(prog(reg.deltas(), ALICE, day, slot).progress, 3);
        assert_eq!(
            prog(reg.deltas(), ALICE, day, BONUS_SLOT).progress,
            1,
            "bonus moved once"
        );
        assert!(
            crate::gateway_out::take(&mut reg).is_empty(),
            "no notice over the cap"
        );
    }

    #[test]
    fn mined_units_and_lp_add_by_their_amount() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let slot = goal_of(&reg, ALICE, day, "tb").slot;
        credit(&mut reg, ALICE, Act::Mine(20), NOW);
        credit(&mut reg, ALICE, Act::Mine(25), NOW);
        assert_eq!(prog(reg.deltas(), ALICE, day, slot).progress, 45);
        credit(&mut reg, ALICE, Act::Mine(500), NOW);
        assert_eq!(prog(reg.deltas(), ALICE, day, slot).progress, 50, "capped");
        credit(&mut reg, ALICE, Act::EarnLp(40), NOW);
        credit(&mut reg, ALICE, Act::Mine(0), NOW);
        credit(&mut reg, ALICE, Act::Mine(-4), NOW);
        assert_eq!(prog(reg.deltas(), ALICE, day, slot).progress, 50);
    }

    #[test]
    fn all_dailies_done_completes_the_bonus() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        for g in todays(&reg, ALICE, day)
            .iter()
            .filter(|g| g.slot < BONUS_SLOT)
        {
            finish(&mut reg, ALICE, g, NOW);
        }
        let b = prog(reg.deltas(), ALICE, day, BONUS_SLOT);
        assert_eq!(
            (b.progress, b.state, b.earned),
            (N_DAILY as i64, State::Done, 1100)
        );
        // The jump goal carries no ISK: done, nothing held.
        let j = prog(
            reg.deltas(),
            ALICE,
            day,
            goal_of(&reg, ALICE, day, "tc").slot,
        );
        assert_eq!((j.state, j.earned), (State::Done, 0));
        assert_eq!(held(&reg, ALICE, NOW).len(), 3, "kill, mine and the bonus");
        // One notice per daily and one for the bonus.
        assert_eq!(notices(&mut reg, N_COMPLETED), N_DAILY + 1);
    }

    #[test]
    fn a_pilot_the_world_does_not_know_earns_nothing() {
        let mut reg = world(SMALL);
        credit(&mut reg, 424_242, Act::KillNpc, NOW);
        assert!(reg.deltas().of(KIND).is_empty());
    }

    // ---- rewards ----

    #[test]
    fn redeeming_pays_the_earning_once_as_ref_174() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let kill = goal_of(&reg, ALICE, day, "ta");
        let slot = kill.slot;
        let before = balance(&reg, ALICE);
        finish(&mut reg, ALICE, &kill, NOW);
        assert_eq!(balance(&reg, ALICE), before, "completion pays nothing yet");
        assert!(redeem(&mut reg, ALICE, day, slot));
        let paid = balance(&reg, ALICE) - before;
        assert_eq!(
            paid, 300.0,
            "earned minus the skim; no corporation tax here"
        );
        let rows: Vec<_> = reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == REF_DAILY_GOAL)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].owner_id_2, rows[0].amount), (ALICE, paid));
        assert_eq!(prog(reg.deltas(), ALICE, day, slot).state, State::Redeemed);
        assert!(
            !redeem(&mut reg, ALICE, day, slot),
            "a second redeem pays nothing"
        );
        assert_eq!(balance(&reg, ALICE) - before, paid);
        assert_eq!(notices(&mut reg, N_REDEEMED), 1);
    }

    #[test]
    fn the_reward_is_a_faucet_the_ledger_classes_as_system() {
        assert_eq!(
            crate::econ_ledger::flow(REF_DAILY_GOAL),
            Some(crate::econ_ledger::Flow::System)
        );
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let total = |r: &Registry| balance(r, ALICE) + balance(r, BOB);
        let before = total(&reg);
        for g in todays(&reg, ALICE, day)
            .iter()
            .filter(|g| g.slot < BONUS_SLOT)
        {
            finish(&mut reg, ALICE, g, NOW);
        }
        for (d, s) in held(&reg, ALICE, NOW) {
            assert!(redeem(&mut reg, ALICE, d, s));
        }
        // 300 + 700 + 1100: money was granted from nowhere, nothing else moved.
        assert_eq!(total(&reg) - before, 2100.0);
        assert!(held(&reg, ALICE, NOW).is_empty());
    }

    #[test]
    fn a_goal_with_no_isk_holds_no_earning_and_cannot_be_redeemed() {
        let mut reg = world(SMALL);
        let day = day_of(NOW);
        let jump = goal_of(&reg, ALICE, day, "tc");
        let kill = goal_of(&reg, ALICE, day, "ta").slot;
        finish(&mut reg, ALICE, &jump, NOW);
        assert_eq!(prog(reg.deltas(), ALICE, day, jump.slot).state, State::Done);
        assert!(!redeem(&mut reg, ALICE, day, jump.slot));
        assert!(!redeem(&mut reg, ALICE, day, kill), "an open goal either");
    }

    // ---- the wire ----

    fn ground(reg: Registry) -> (Ground, Arc<RwLock<Registry>>) {
        let w = Arc::new(RwLock::new(reg));
        (Ground::Server(w.clone()), w)
    }

    fn call(g: &Ground, ch: i64, ty: &str, body: &[u8]) -> Reply {
        gateway(&format!("{PKG}{ty}"), body, ch, g).expect("an arm answers it")
    }

    fn id_request(uuid: &[u8; 16]) -> Vec<u8> {
        let mut id = Vec::new();
        put_bytes(&mut id, 1, uuid);
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &id);
        req
    }

    #[test]
    fn get_all_current_encodes_every_id_as_sixteen_bytes_and_isk_as_zigzag() {
        let (g, _w) = ground(world(SMALL));
        let r = call(&g, ALICE, "GetAllCurrentRequest", &[]);
        assert_eq!(r.status, 200);
        let (name, body) = r.payload.expect("a payload");
        assert_eq!(name, format!("{PKG}GetAllCurrentResponse"));
        let goals = field(&body, 1);
        assert_eq!(goals.len(), N_DAILY + 1);
        let (mut daily, mut bonus, mut jump_empty, mut paid) = (0, 0, 0, Vec::new());
        for goal in &goals {
            let id = get_bytes(goal, 1).expect("an id");
            assert_eq!(get_bytes(&id, 1).expect("a uuid").len(), 16);
            let a = get_bytes(goal, 2).expect("attributes");
            match get_uint(&a, 5) {
                Some(1) => daily += 1,
                Some(2) => bonus += 1,
                other => panic!("category {other:?}"),
            }
            let cfg = get_bytes(&a, 6).expect("a contribution");
            if get_bytes(&cfg, 13) == Some(Vec::new()) {
                jump_empty += 1;
            }
            for pay in field(&a, 8) {
                let asset = get_bytes(&pay, 1).and_then(|x| get_bytes(&x, 1)).unwrap();
                assert_eq!(asset.len(), 16, "Payment.asset.uuid");
                assert_ne!(
                    Some(asset),
                    get_bytes(&id, 1),
                    "the asset id is not the goal id"
                );
                let unit = get_bytes(&pay, 3).unwrap();
                let cur = get_bytes(&unit, 1).and_then(|i| get_bytes(&i, 1)).unwrap();
                paid.push(get_uint(&cur, 1).unwrap());
            }
            assert!(get_uint(&a, 9).unwrap() > 0, "a target");
            let after = get_bytes(&a, 10).and_then(|t| get_uint(&t, 1)).unwrap();
            let until = get_bytes(&a, 11).and_then(|t| get_uint(&t, 1)).unwrap();
            assert_eq!(until - after, 86_400);
        }
        assert_eq!((daily, bonus, jump_empty), (N_DAILY, 1, 1));
        paid.sort_unstable();
        // Units 300, 700 and 1100 as zigzag; the jump goal pays nothing.
        assert_eq!(paid, vec![600, 1400, 2200]);
    }

    #[test]
    fn get_returns_a_goal_and_its_progress_and_404_for_a_stranger() {
        let mut reg = world(SMALL);
        let now = crate::pyutil::filetime_now();
        credit(&mut reg, ALICE, Act::KillNpc, now);
        let day = day_of(now);
        let slot = goal_of(&reg, ALICE, day, "ta").slot;
        let uuid = goal_uuid(reg.world().seed, ALICE, day, slot);
        let (g, _w) = ground(reg);
        let r = call(&g, ALICE, "GetRequest", &id_request(&uuid));
        assert_eq!(r.status, 200);
        let (_, body) = r.payload.unwrap();
        assert!(get_bytes(&body, 1).is_some());
        assert_eq!(get_uint(&body, 2), Some(1));
        assert_eq!(
            call(&g, ALICE, "GetRequest", &id_request(&[9; 16])).status,
            404
        );
        assert_eq!(
            call(&g, BOB, "GetRequest", &id_request(&uuid)).status,
            404,
            "another pilot's id"
        );
    }

    #[test]
    fn get_all_with_rewards_pages_by_decimal_offset() {
        let mut reg = world(SMALL);
        let today = day_of(crate::pyutil::filetime_now());
        for d in 0..5 {
            put_prog(
                &mut reg,
                ALICE,
                today - d,
                0,
                Prog {
                    progress: 3,
                    state: State::Done,
                    earned: 300,
                },
            );
        }
        let (g, _w) = ground(reg);
        let page = |size: u64, token: &str| {
            let mut p = Vec::new();
            put_field_varint(&mut p, 1, size);
            if !token.is_empty() {
                put_bytes(&mut p, 2, token.as_bytes());
            }
            let mut req = Vec::new();
            put_bytes(&mut req, 1, &p);
            let (_, body) = call(&g, ALICE, "GetAllWithRewardsRequest", &req)
                .payload
                .unwrap();
            let next = get_bytes(&body, 2)
                .and_then(|n| get_bytes(&n, 1))
                .map(|t| String::from_utf8(t).unwrap())
                .unwrap_or_default();
            (field(&body, 1).len(), next)
        };
        assert_eq!(page(2, ""), (2, "2".to_owned()));
        assert_eq!(page(2, "2"), (2, "4".to_owned()));
        assert_eq!(page(2, "4"), (1, String::new()));
    }

    #[test]
    fn redeem_over_the_gateway_pays_then_says_404() {
        let mut reg = world(SMALL);
        let now = crate::pyutil::filetime_now();
        let day = day_of(now);
        let kill = goal_of(&reg, ALICE, day, "ta");
        finish(&mut reg, ALICE, &kill, now);
        let uuid = goal_uuid(reg.world().seed, ALICE, day, kill.slot);
        let before = balance(&reg, ALICE);
        let (g, w) = ground(reg);
        assert_eq!(
            call(&g, ALICE, "RedeemRequest", &id_request(&uuid)).status,
            200
        );
        assert_eq!(balance(&w.read().unwrap(), ALICE) - before, 300.0);
        assert_eq!(
            call(&g, ALICE, "RedeemRequest", &id_request(&uuid)).status,
            404
        );
        assert_eq!(
            call(&g, BOB, "RedeemRequest", &id_request(&uuid)).status,
            404
        );
        assert_eq!(balance(&w.read().unwrap(), ALICE) - before, 300.0);
        // RedeemAll takes what is left (the bonus is not done; nothing is).
        assert_eq!(call(&g, ALICE, "RedeemAllRequest", &[]).status, 200);
    }

    #[test]
    fn pay_for_completion_and_admin_progress_are_refused_and_other_types_pass() {
        let (g, _w) = ground(world(SMALL));
        assert_eq!(call(&g, ALICE, "PayForCompletionRequest", &[]).status, 403);
        assert_eq!(call(&g, ALICE, "admin.ProgressRequest", &[]).status, 403);
        assert!(gateway("eve_public.skin.api.GetRequest", &[], ALICE, &g).is_none());
        assert!(
            gateway(
                &format!("{PKG}GetAllCompletedWithEntitlementsRequest"),
                &[],
                ALICE,
                &g
            )
            .is_none()
        );
        let local = Ground::local();
        assert_eq!(call(&local, ALICE, "GetAllCurrentRequest", &[]).status, 200);
        assert_eq!(call(&local, ALICE, "RedeemRequest", &[]).status, 404);
        assert_eq!(
            call(&local, ALICE, "PayForCompletionRequest", &[]).status,
            403
        );
    }

    // ---- rollover, bots, GM ----

    #[test]
    fn the_rollover_prunes_old_rows_and_tells_the_pilots() {
        let mut reg = world(SMALL);
        let today = day_of(NOW);
        let row = Prog {
            progress: 1,
            state: State::Open,
            earned: 0,
        };
        put_prog(&mut reg, ALICE, today, 0, row);
        put_prog(&mut reg, ALICE, today - KEEP_DAYS, 0, row);
        put_prog(&mut reg, ALICE, today - KEEP_DAYS - 1, 0, row);
        put_prog(&mut reg, BOB, today - 90, 1, row);
        reg.deltas_mut()
            .put(KIND, okey(BOB, today - 90, 1), vec!["ta".to_owned()]);
        let dropped = on_rollover(
            &mut reg,
            &Day {
                day: 5,
                days: 1,
                now: NOW,
            },
        );
        assert_eq!(dropped, 3);
        assert_eq!(reg.deltas().of(KIND).len(), 2);
        assert_eq!(
            notices(&mut reg, N_CURRENT),
            2,
            "on_rollover itself tells each online pilot once, and not Carol"
        );
    }

    #[test]
    fn the_fixtures_match_n_daily() {
        // The SMALL table is built to draw every daily: it must hold N_DAILY.
        assert_eq!(dailies(SMALL), N_DAILY);
        assert!(dailies(BIG) > N_DAILY);
    }

    #[test]
    fn a_logged_off_pilot_earns_nothing() {
        let mut reg = world(SMALL);
        credit(&mut reg, CAROL, Act::KillNpc, NOW);
        credit(&mut reg, CAROL, Act::Jump, NOW);
        assert!(reg.deltas().of(KIND).is_empty(), "no session, no progress");
        credit(&mut reg, ALICE, Act::Jump, NOW);
        assert_eq!(reg.deltas().of(KIND).len(), 1, "a pilot in play does");
    }

    #[test]
    fn offline_settlement_has_no_daily_goal_row() {
        let plan = crate::module::plan();
        assert!(
            plan.settled
                .iter()
                .all(|r| !r.name.starts_with("daily_goal"))
        );
        assert!(plan.mined.iter().any(|r| r.name == "daily_goal_mined"));
        assert!(
            plan.kill_npc
                .iter()
                .any(|r| r.name == "daily_goal_npc_kill")
        );
        assert!(plan.commands.iter().any(|c| c.name == "dailygoals"));
    }

    #[test]
    fn swap_replaces_a_slot_without_progress_and_refuses_one_with() {
        let mut reg = world(BIG);
        let day = day_of(crate::pyutil::filetime_now());
        let drawn: Vec<String> = todays(&reg, ALICE, day)[..N_DAILY]
            .iter()
            .map(|g| g.row.key.clone())
            .collect();
        let spare = ["ta", "tb", "tc", "td", "te"]
            .into_iter()
            .find(|k| !drawn.iter().any(|d| d == k))
            .unwrap();
        assert!(
            gm_run(&mut reg, Some(ALICE), &["swap", "0", &drawn[1]]).contains("already in slot 1")
        );
        assert!(gm_run(&mut reg, Some(ALICE), &["swap", "0", "nope"]).contains("no daily goal"));
        assert!(
            gm_run(
                &mut reg,
                Some(ALICE),
                &["swap", &N_DAILY.to_string(), spare]
            )
            .contains("not one of")
        );
        assert!(gm_run(&mut reg, Some(ALICE), &["swap", "0", spare]).contains("is now"));
        let after = todays(&reg, ALICE, day);
        assert_eq!(after[0].row.key, spare);
        assert_eq!(after[1].row.key, drawn[1], "the other slots stand");
        assert_eq!(
            goal_uuid(reg.world().seed, ALICE, day, 0),
            goal_uuid(reg.world().seed, ALICE, day, 0),
            "the id depends on the slot, not on the key"
        );
        // Give the swapped slot progress: a further swap is refused.
        finish(&mut reg, ALICE, &after[0], crate::pyutil::filetime_now());
        let other = ["ta", "tb", "tc", "td", "te"]
            .into_iter()
            .find(|k| *k != spare && !drawn.iter().any(|d| d == k))
            .unwrap_or(drawn[0].as_str())
            .to_owned();
        let said = gm_run(&mut reg, Some(ALICE), &["swap", "0", &other]);
        assert!(
            said.contains("has progress") || said.contains("already in slot"),
            "{said}"
        );
        assert!(gm_run(&mut reg, Some(ALICE), &[]).contains("0 "));
    }
}
