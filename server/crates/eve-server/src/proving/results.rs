//! **Proving Grounds results** (issue #66, PG-3): the statistics, the
//! leaderboard and the rewards of an event.
//!
//! The wire is `proving.rs` (the two pushes) and `service/proving.rs` (the
//! reads); the state and the rules are here. Every function over the rows is
//! pure over [`Deltas`]; only [`on_finished`], [`award`], [`on_rollover`] and
//! [`slash`] touch the world.
//!
//! ## What the client does **[V]** (`pvpFilaments/client/pvpFilamentSvc.py`)
//!
//! - It asks `GetCharacterStatistics` only while it holds none, so a result
//!   after a match must be **pushed unasked**: [`on_finished`] pushes the
//!   pilot's statistics and then the board to every participant.
//! - A board entry is `{character_id, rank, wins}`; when the **last** entry
//!   is the caller's own and its rank is above 100, the client takes it as
//!   the caller's and drops it from the list. [`board`] sends the top
//!   [`BOARD_TOP`] and, for a caller below them, their own entry last.
//! - Rank 0 hides the panel's wins, losses and draws, so a pilot who has not
//!   played is answered rank 0 and zeros.
//! - A reward is an icon of a type and an amount; there is no tier, rank or
//!   win count on it. The reward vocabulary is the redeem system's:
//!   `typeKredits` for ISK, a skill point token for skill points, any other
//!   type an item.
//!
//! ## What this server decides
//!
//! - **[A]** Everyone who played an event is ranked: wins descending, losses
//!   ascending, then the pilot who reached the wins first, then the lower
//!   character id. Falsified by TQ material showing winners-only boards or
//!   another order.
//! - **[A]** The rewards of an event are paid **once**, at the event's end
//!   (the rollover after it is over) or by `/pg award`, to every pilot with
//!   at least [`MIN_WINS`] win, the match type's whole list. Falsified by the
//!   `rewardsHintID` text saying "per win" or by TQ paying per match.
//! - **[A]** ISK is the list's amount at the world's mission dial
//!   (`econ_scale`), with no corporation tax. Falsified by an owner ruling.
//! - **[A] Placeholder:** the ISK amount itself, the list's own figure
//!   (100M ISK on the current bundle's lists), is kept for now as a
//!   placeholder until a reward tuning pass sets it (owner, 2026-11-24).
//!   Falsified by that pass or by an owner ruling on the figure.
//! - Nothing is made from nowhere: ISK is posted to the pilot's wallet with
//!   journal ref 147 by an NPC issuer (the same kind of grant as a mission
//!   reward), skill point tokens go to the free pool, and every other reward
//!   is a **redeem token** the pilot claims into a station hangar, as the
//!   client's own redeem window does for TQ's rewards **[I]**.
//! - A skill point token with no entry in [`SpTokens`] is **skipped and logged
//!   once**, never made an item. The table is **not built** (PG-3b): the
//!   content tool reads fsd data, not the client's code, and the points live
//!   in `redeem/data.py`; [`SP_TOKEN_TYPES`] names the bundle's three types so
//!   they are recognised without it. A reward type the world cannot resolve
//!   as an item is skipped and logged too, never queued.
//! - Only the participants of a match are pushed their numbers; another
//!   pilot's cached rank refreshes with the client's one-minute board cache.
//!
//! ## The rows (delta kind [`KIND`])
//!
//! - `s match_type schedule char -> wins losses draws reached_at awarded`
//! - `m seq(12 digits) -> match_type schedule at char:result,char:result,...`
//!   (result is the `MatchResult` number), pruned to [`HISTORY_KEEP`] at the
//!   rollover.
//! - `next -> n`, the next history number.
//! - `e match_type schedule -> awarded_at`, the event's award marker.

use crate::delta::Deltas;
use crate::proving::arena::{MatchFinished, MatchResult};
use crate::proving::{Catalogue, Event, LeaderboardPush, StatisticsPush};
use crate::service::corp_wallet::{self, Via};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "pgstat";

/// `AWARDED_REWARDS` **[V]** (`pvpFilaments/common/constants.py`).
pub const AWARDED_REWARDS: i64 = 1;
/// `UN_REWARDED` **[V]** (`pvpFilaments/common/constants.py`).
pub const UN_REWARDED: i64 = 0;
/// The entries the client's board draws before the caller's own **[V]**
/// (`pvpFilamentSvc.OnPVPFilamentsLeaderboard`).
pub const BOARD_TOP: usize = 100;
/// `appConst.refRedeemedISKToken` **[V]** (`appConst.py:1010`).
pub const REF_REDEEMED_ISK_TOKEN: i64 = 147;
/// `typeKredits`, the redeem system's ISK type **[V]**
/// (`inventorycommon/const.py:1657`; `redeem/data.py` `TYPE_THAT_GIVES_ISK`).
pub const TYPE_KREDITS: i64 = 52996;
/// **[A]** The wins a pilot needs to be rewarded. Falsified by TQ rewarding a
/// pilot who only played.
pub const MIN_WINS: i64 = 1;
/// **[A]** The match history rows kept. Falsified by an owner ruling (Q8).
pub const HISTORY_KEEP: usize = 2000;
/// **[A]** The issuer on the journal row of a reward. Falsified by TQ's
/// journal showing another issuer.
pub const ISSUER: i64 = crate::concord::CONCORD_CORP;

// ---------------------------------------------------------------------------
// The statistics
// ---------------------------------------------------------------------------

/// A pilot's tally in one event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stat {
    pub wins: i64,
    pub losses: i64,
    pub draws: i64,
    /// The FILETIME of the last change of `wins`; breaks a tie.
    pub reached_at: i64,
    /// The rewards were paid ([`AWARDED_REWARDS`]).
    pub awarded: bool,
}

fn stat_key(mt: i64, sched: i64, ch: i64) -> Vec<String> {
    vec![
        "s".to_owned(),
        mt.to_string(),
        sched.to_string(),
        ch.to_string(),
    ]
}

fn parse_stat(v: &[String]) -> Option<Stat> {
    let n = |i: usize| v.get(i)?.parse::<i64>().ok();
    Some(Stat {
        wins: n(0)?,
        losses: n(1)?,
        draws: n(2)?,
        reached_at: n(3)?,
        awarded: n(4)? == AWARDED_REWARDS,
    })
}

fn put_stat(d: &mut Deltas, mt: i64, sched: i64, ch: i64, s: Stat) {
    d.put(
        KIND,
        stat_key(mt, sched, ch),
        vec![
            s.wins.to_string(),
            s.losses.to_string(),
            s.draws.to_string(),
            s.reached_at.to_string(),
            if s.awarded {
                AWARDED_REWARDS
            } else {
                UN_REWARDED
            }
            .to_string(),
        ],
    );
}

/// A pilot's tally in an event; all zero for a pilot who never played it.
pub fn stat(d: &Deltas, mt: i64, sched: i64, ch: i64) -> Stat {
    d.of(KIND)
        .get(&stat_key(mt, sched, ch))
        .and_then(|v| parse_stat(v))
        .unwrap_or_default()
}

/// Every pilot who played the event, with their tally, in key order.
fn stats_of(d: &Deltas, mt: i64, sched: i64) -> Vec<(i64, Stat)> {
    let (m, s) = (mt.to_string(), sched.to_string());
    let from = vec!["s".to_owned(), m.clone(), s.clone(), String::new()];
    d.of(KIND)
        .range(from..)
        .take_while(|(k, _)| k.len() == 4 && k[0] == "s" && k[1] == m && k[2] == s)
        .filter_map(|(k, v)| Some((k[3].parse().ok()?, parse_stat(v)?)))
        .collect()
}

fn next_seq(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let n = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first()?.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    d.put(KIND, key, vec![(n + 1).to_string()]);
    n
}

/// Add a finished match to every participant's tally and to the history.
/// `at` is the FILETIME it ended.
pub fn record(d: &mut Deltas, m: &MatchFinished, at: i64) {
    let mut played = Vec::new();
    for (chars, result) in &m.results {
        for &ch in chars {
            let mut s = stat(d, m.match_type_id, m.schedule_id, ch);
            match result {
                MatchResult::Win => {
                    s.wins += 1;
                    s.reached_at = at;
                }
                MatchResult::Loss => s.losses += 1,
                MatchResult::Draw => s.draws += 1,
            }
            put_stat(d, m.match_type_id, m.schedule_id, ch, s);
            played.push(format!("{ch}:{}", *result as i64));
        }
    }
    if played.is_empty() {
        return;
    }
    let seq = next_seq(d);
    d.put(
        KIND,
        vec!["m".to_owned(), format!("{seq:012}")],
        vec![
            m.match_type_id.to_string(),
            m.schedule_id.to_string(),
            at.to_string(),
            played.join(","),
        ],
    );
}

/// The event's pilots by rank: `(character, rank, tally)`, ranks from 1 and
/// unique **[A]** (module doc).
pub fn ranked(d: &Deltas, mt: i64, sched: i64) -> Vec<(i64, i64, Stat)> {
    let mut v = stats_of(d, mt, sched);
    v.sort_by(|a, b| {
        b.1.wins
            .cmp(&a.1.wins)
            .then(a.1.losses.cmp(&b.1.losses))
            .then(a.1.reached_at.cmp(&b.1.reached_at))
            .then(a.0.cmp(&b.0))
    });
    v.into_iter()
        .enumerate()
        .map(|(i, (ch, s))| (ch, i as i64 + 1, s))
        .collect()
}

/// A pilot's rank and tally; rank 0 and zeros for one who never played.
pub fn standing(d: &Deltas, mt: i64, sched: i64, ch: i64) -> (i64, Stat) {
    ranked(d, mt, sched)
        .into_iter()
        .find(|(c, _, _)| *c == ch)
        .map_or((0, Stat::default()), |(_, rank, s)| (rank, s))
}

/// One line of the board **[V]** `leaderboard.py`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardEntry {
    pub character_id: i64,
    pub rank: i64,
    pub wins: i64,
}

impl BoardEntry {
    pub fn to_value(&self) -> PyValue {
        crate::pyutil::pydict(vec![
            ("character_id", PyValue::Int(self.character_id)),
            ("rank", PyValue::Int(self.rank)),
            ("wins", PyValue::Int(self.wins)),
        ])
    }
}

/// The board `caller` is sent: the top [`BOARD_TOP`], and the caller's own
/// entry **last** when their rank is below them **[V]** (module doc).
pub fn board(d: &Deltas, mt: i64, sched: i64, caller: Option<i64>) -> Vec<BoardEntry> {
    let all = ranked(d, mt, sched);
    let entry = |(ch, rank, s): &(i64, i64, Stat)| BoardEntry {
        character_id: *ch,
        rank: *rank,
        wins: s.wins,
    };
    let mut out: Vec<BoardEntry> = all.iter().take(BOARD_TOP).map(entry).collect();
    if let Some(own) = caller.and_then(|c| all.iter().skip(BOARD_TOP).find(|r| r.0 == c)) {
        out.push(entry(own));
    }
    out
}

/// A match a pilot played, newest first from [`history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    pub match_type_id: i64,
    pub schedule_id: i64,
    pub at: i64,
    pub result: i64,
}

/// The last `n` matches `ch` played, newest first.
pub fn history(d: &Deltas, ch: i64, n: usize) -> Vec<Played> {
    let from = vec!["m".to_owned()];
    let to = vec!["n".to_owned()];
    d.of(KIND)
        .range(from..to)
        .rev()
        .filter_map(|(_, v)| {
            let num = |i: usize| v.get(i)?.parse::<i64>().ok();
            let result = v.get(3)?.split(',').find_map(|p| {
                let (c, r) = p.split_once(':')?;
                (c.parse::<i64>().ok()? == ch).then(|| r.parse::<i64>().ok())?
            })?;
            Some(Played {
                match_type_id: num(0)?,
                schedule_id: num(1)?,
                at: num(2)?,
                result,
            })
        })
        .take(n)
        .collect()
}

/// Drop the oldest history rows beyond `keep`; the count dropped.
pub fn prune_history(d: &mut Deltas, keep: usize) -> usize {
    let from = vec!["m".to_owned()];
    let to = vec!["n".to_owned()];
    let keys: Vec<Vec<String>> = d.of(KIND).range(from..to).map(|(k, _)| k.clone()).collect();
    let drop = keys.len().saturating_sub(keep);
    for k in &keys[..drop] {
        d.del(KIND, k);
    }
    drop
}

/// The event of the pilot's last match.
fn last_event(d: &Deltas, ch: i64) -> Option<(i64, i64)> {
    history(d, ch, 1)
        .first()
        .map(|p| (p.match_type_id, p.schedule_id))
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnPVPFilamentsCharacterStatistics` for `who` (all zero with `None` or a
/// pilot who never played), addressed to `to` (`None`: the acting session).
pub fn statistics_push(
    d: &Deltas,
    mt: i64,
    sched: i64,
    who: Option<i64>,
    to: Option<i64>,
) -> StatisticsPush {
    let (rank, s) = who.map_or((0, Stat::default()), |c| standing(d, mt, sched, c));
    StatisticsPush {
        match_type_id: mt,
        schedule_id: sched,
        rank,
        wins: s.wins,
        losses: s.losses,
        draws: s.draws,
        to,
    }
}

/// `OnPVPFilamentsLeaderboard` for `caller`, addressed to `to`.
pub fn leaderboard_push(
    d: &Deltas,
    mt: i64,
    sched: i64,
    caller: Option<i64>,
    to: Option<i64>,
) -> LeaderboardPush {
    LeaderboardPush {
        match_type_id: mt,
        schedule_id: sched,
        entries: board(d, mt, sched, caller),
        to,
    }
}

/// **The `arena::ON_FINISHED` row**: count the match, then push every
/// participant their statistics and the board, on the clock (the match ends
/// in a tick or in `/pg decide`, and neither has an answer to ride).
pub fn on_finished(reg: &mut Registry, m: &MatchFinished) {
    // A pilot whose character is gone has nothing to be told or paid.
    let kept = MatchFinished {
        match_type_id: m.match_type_id,
        schedule_id: m.schedule_id,
        results: m
            .results
            .iter()
            .map(|(chars, r)| {
                let alive: Vec<i64> = chars
                    .iter()
                    .copied()
                    .filter(|c| reg.character(*c).is_some())
                    .collect();
                (alive, *r)
            })
            .filter(|(chars, _)| !chars.is_empty())
            .collect(),
    };
    if kept.results.is_empty() {
        return;
    }
    let at = crate::proving::arena::now(reg);
    record(reg.deltas_mut(), &kept, at);
    let (mt, sched) = (m.match_type_id, m.schedule_id);
    for ch in kept.results.iter().flat_map(|(c, _)| c.iter().copied()) {
        let stats = statistics_push(reg.deltas(), mt, sched, Some(ch), Some(ch));
        reg.schedule(Duration::ZERO, stats);
        let lb = leaderboard_push(reg.deltas(), mt, sched, Some(ch), Some(ch));
        reg.schedule(Duration::ZERO, lb);
    }
}

// ---------------------------------------------------------------------------
// The rewards
// ---------------------------------------------------------------------------

/// The skill point token types of the bundle's Proving Grounds rewards **[V]**:
/// keys of `get_redeem_data` (`redeem/data.pyc`; points 250000, 500000 and
/// 5000). Recognised so they are never made items; their points are PG-3b's.
pub const SP_TOKEN_TYPES: [i64; 3] = [43_680, 52_269, 54_650];

/// The points of each skill point token type, from the bundle. **Not built**
/// (PG-3b, module doc): no loader fills it, so a world has none and the
/// [`SP_TOKEN_TYPES`] are skipped. A test inserts its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpTokens(pub BTreeMap<i64, i64>);

fn sp_points(reg: &Registry, type_id: i64) -> Option<i64> {
    reg.content()
        .try_get::<SpTokens>()?
        .0
        .get(&type_id)
        .copied()
}

/// Log a skipped reward type once per process.
fn skip_once(type_id: i64, why: &str) {
    static SEEN: std::sync::Mutex<BTreeSet<i64>> = std::sync::Mutex::new(BTreeSet::new());
    let first = SEEN.lock().map(|mut s| s.insert(type_id)).unwrap_or(false);
    if first {
        crate::outln!("[proving] {why}");
    }
}

/// Can the world make this type as an item (what `nes::claim` needs)?
fn is_item(reg: &Registry, type_id: i64) -> bool {
    crate::item::find_in(reg.content(), type_id).is_some()
        || crate::skins::item_types_in(reg.content())
            .iter()
            .any(|t| t.type_id == type_id)
}

fn event_key(mt: i64, sched: i64) -> Vec<String> {
    vec!["e".to_owned(), mt.to_string(), sched.to_string()]
}

/// Has the event's award run?
pub fn awarded(d: &Deltas, mt: i64, sched: i64) -> bool {
    d.of(KIND).contains_key(&event_key(mt, sched))
}

/// Pay `ch` the event's reward list.
fn grant(reg: &mut Registry, ch: i64, ev: &Event, via: Via) {
    let sched = ev.schedule.id;
    let mut tokens = false;
    for &(type_id, amount) in &ev.match_type.rewards {
        if amount <= 0 {
            continue;
        }
        if type_id == TYPE_KREDITS {
            let isk =
                crate::econ_scale::scaled_isk(amount as f64, crate::econ_scale::of(reg).mission());
            corp_wallet::char_post(
                reg,
                via,
                ch,
                isk,
                REF_REDEEMED_ISK_TOKEN,
                ISSUER,
                ch,
                sched,
                "",
            );
        } else if let Some(points) = sp_points(reg, type_id) {
            crate::free_sp::grant(reg, ch, points.saturating_mul(amount));
        } else if SP_TOKEN_TYPES.contains(&type_id) {
            skip_once(
                type_id,
                &format!("SP reward {type_id} skipped: no points table"),
            );
        } else if !is_item(reg, type_id) {
            skip_once(
                type_id,
                &format!("reward {type_id} skipped: not a type the world can make"),
            );
        } else {
            let user = crate::nes_vault::key_of(reg, ch);
            let name = crate::implants::type_name(reg, type_id);
            crate::nes::add_token(
                reg.deltas_mut(),
                crate::nes::Token {
                    id: 0,
                    user,
                    to: ch,
                    type_id,
                    qty: amount,
                    offer: format!("pg:{}", ev.event_id()),
                    name,
                    message: String::new(),
                    created: crate::game_clock::clock::unix_secs() as i64,
                    from: 0,
                },
            );
            tokens = true;
        }
    }
    if tokens {
        let user = crate::nes_vault::key_of(reg, ch);
        let to = crate::nes_vault::pilots_of(reg, user);
        reg.schedule(Duration::ZERO, crate::nes::QueueUpdated { to });
    }
}

/// Pay the event's rewards once to every pilot with [`MIN_WINS`] wins who has
/// not been paid (each pilot's own flag makes it idempotent), and record the
/// event's marker. The count of pilots paid.
pub(crate) fn award(reg: &mut Registry, ev: &Event, now: i64, via: Via) -> usize {
    let (mt, sched) = (ev.match_type.id, ev.schedule.id);
    let due: Vec<i64> = stats_of(reg.deltas(), mt, sched)
        .into_iter()
        .filter(|(_, s)| s.wins >= MIN_WINS && !s.awarded)
        .map(|(c, _)| c)
        .filter(|c| reg.character(*c).is_some())
        .collect();
    for &ch in &due {
        grant(reg, ch, ev, via);
        let mut s = stat(reg.deltas(), mt, sched, ch);
        s.awarded = true;
        put_stat(reg.deltas_mut(), mt, sched, ch, s);
    }
    // The marker is a record for the GM and the log, never a gate.
    reg.deltas_mut()
        .put(KIND, event_key(mt, sched), vec![now.to_string()]);
    if !due.is_empty() {
        crate::outln!(
            "[proving] awarded {} pilot(s) for {}",
            due.len(),
            ev.event_id()
        );
    }
    due.len()
}

/// **The `rollover_steps` row `proving_awards`**: pay every event that is over
/// (a pilot who won after an earlier payout is paid now), then prune the
/// history. QA events are never over, so they are paid by `/pg award` only.
/// The count is the pilots paid.
pub fn on_rollover(w: &mut Registry, d: &crate::rollover::Day) -> usize {
    let over = w
        .content()
        .try_get::<Catalogue>()
        .map_or_else(Vec::new, |c| c.over(d.now));
    let mut paid = 0;
    for ev in over {
        paid += award(w, &ev, d.now, Via::Clock);
    }
    prune_history(w.deltas_mut(), HISTORY_KEEP);
    paid
}

// ---------------------------------------------------------------------------
// The GM door
// ---------------------------------------------------------------------------

/// The `/pg` usage line, shared with `arena::slash` and `slash.rs`.
pub const USAGE: &str = "usage: /pg status | expire <seconds> | decide | stats [charID] | board | history | award [scheduleID]";

/// Whether `/pg <word>` is one of this module's subcommands.
pub fn is_subcommand(word: &str) -> bool {
    matches!(word, "stats" | "board" | "history" | "award")
}

/// `/pg stats [charID]`, `/pg board`, `/pg history`, `/pg award
/// [scheduleID]`. Each works for the pilot's last match event; `award` takes
/// a schedule id to pay a QA event, which never ends by itself.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |t: String| Ok(crate::pyutil::bstr(&t));
    let Some(actor) = actor else {
        return say("no character".to_owned());
    };
    match args {
        ["stats"] | ["stats", _] => {
            let who = match args.get(1) {
                None => actor,
                Some(s) => match s.parse::<i64>() {
                    Ok(c) => c,
                    Err(_) => return say(USAGE.to_owned()),
                },
            };
            let Some((mt, sched)) = last_event(reg.deltas(), who) else {
                return say(format!("{who} has played no match"));
            };
            let (rank, s) = standing(reg.deltas(), mt, sched, who);
            say(format!(
                "{who} in {mt}_{sched}: rank {rank}, {} win(s), {} loss(es), {} draw(s), rewards {}",
                s.wins,
                s.losses,
                s.draws,
                if s.awarded { "paid" } else { "not paid" }
            ))
        }
        ["board"] => {
            let Some((mt, sched)) = last_event(reg.deltas(), actor) else {
                return say("you have played no match".to_owned());
            };
            let lines: Vec<String> = board(reg.deltas(), mt, sched, Some(actor))
                .iter()
                .take(10)
                .map(|e| format!("{}. {} wins {}", e.rank, e.character_id, e.wins))
                .collect();
            say(format!("{mt}_{sched}: {}", lines.join("; ")))
        }
        ["history"] => {
            let rows = history(reg.deltas(), actor, 10);
            if rows.is_empty() {
                return say("you have played no match".to_owned());
            }
            let name = |r: i64| match r {
                x if x == MatchResult::Win as i64 => "win",
                x if x == MatchResult::Draw as i64 => "draw",
                _ => "loss",
            };
            let lines: Vec<String> = rows
                .iter()
                .map(|p| format!("{}_{} {}", p.match_type_id, p.schedule_id, name(p.result)))
                .collect();
            say(lines.join("; "))
        }
        ["award"] | ["award", _] => {
            let sched = match args.get(1) {
                None => last_event(reg.deltas(), actor).map(|(_, s)| s),
                Some(s) => match s.parse::<i64>() {
                    Ok(id) => Some(id),
                    Err(_) => return say(USAGE.to_owned()),
                },
            };
            let now = crate::proving::arena::now(reg);
            let found = sched.and_then(|id| {
                let cat = reg.content().try_get::<Catalogue>()?;
                let over = cat.over(now).iter().any(|e| e.schedule.id == id);
                let ev = cat.all(now).into_iter().find(|e| e.schedule.id == id)?;
                Some((ev, over))
            });
            let Some((ev, over)) = found else {
                return say("no such event".to_owned());
            };
            // The GM door pays a QA event, which never ends; any other event
            // is paid by the rollover after it is over.
            if !ev.schedule.qa && !over {
                return say("that event is not over yet".to_owned());
            }
            let n = award(reg, &ev, now, Via::Call);
            say(format!("awarded {n} pilot(s) for {}", ev.event_id()))
        }
        _ => say(USAGE.to_owned()),
    }
}

#[cfg(test)]
mod tests;
