//! **Corporation voting** (lane `corp-votes`, #103): vote cases, their options,
//! votes weighed by shares, expiry and the result. The wire is
//! `service/corp_votes.rs`; this is the store and the rules, with no registry.
//!
//! One delta kind carries the state, the way `corp_shares` keeps its own:
//!
//! | kind | key | value |
//! | --- | --- | --- |
//! | [`KIND`] | `next` | the next case id |
//! | [`KIND`] | `case id` | `corp type creator start end title description` |
//! | [`KIND`] | `opt id n` | `text parameter parameter1 parameter2`, option `n` of a case |
//! | [`KIND`] | `cast id voter` | `option weight`, one pilot's vote |
//! | [`KIND`] | `snapped id` | marker: the case has a holdings snapshot |
//! | [`KIND`] | `snap id holder` | `shares`, what `holder` held when the case opened |
//! | [`KIND`] | `used id holder` | marker: that holder's snapshot shares were already voted |
//! | [`KIND`] | `acted id` | `result`: the closed case was settled (its effect carried out or skipped), once |
//! | [`KIND`] | `sanction id` | `acted_at`: the CEO put the case's sanctioned action in effect, once (FILETIME) |
//!
//! Times are Windows FILETIMEs, as the client compares `endDateTime` with
//! `GetWallclockTime()` (`voting.py` service `GetVoteCase`) **[V]**.
//!
//! **Open or closed is read from the clock**, never stored: a case is open
//! while `now < end`. Nothing runs at expiry, so a server that was down
//! when a vote ended still shows it closed with its result.
//!
//! **Weight.** A vote weighs the shares the voter held of the corp **when the
//! case opened** ([`vote`]), plus those of that corp held then by any corp
//! the voter is CEO of (`voting.py` `_CanVote` third branch lets such a CEO
//! vote) **[V]**. [`open`] writes the snapshot into the same delta kind (so
//! it is saved and restored with everything else), and each holder's
//! snapshot shares can be voted once: handing shares on, or changing a
//! corp's CEO, after voting cannot vote them twice. A case with no snapshot
//! (opened before this) is weighed from the live holdings ([`weight`]).
//! The weight is stored with the vote, so selling shares later does not
//! change a closed result. **[A]** that a weight is fixed at the open, not
//! at cast or close; falsifier: a reference account of a vote counting
//! shares bought after it opened.
//!
//! **Retention.** [`open`] drops cases that closed more than [`RETENTION`]
//! ago, with their options, votes and snapshot. **[A]** 90 days; falsifier:
//! the owner wanting a longer history.
//!
//! **Result.** The option with the most weight wins ([`outcome`]); a tie or
//! no votes is no result. **[A]** plurality, not a majority of issued
//! shares; falsifier: reference data on a vote's pass rule.
//!
//! **Effect (#153).** A closed case is settled once: [`take_due`] finds the
//! closed cases with no `acted` marker ([`pending`]), writes the marker, and
//! answers each case's [`Effect`]. Only option 0 acts, the "yes" the client
//! builds for every non-general type (`voting.py` `CreateVote`,
//! `GetVoteOptionText` **[V]**): its `parameter` is the candidate for a CEO
//! vote (the proposer's character id), the pilot to expel for a kick vote,
//! the shares to create for a shares vote; `parameter` / `parameter1` /
//! `parameter2` are item id, type id and location for a blueprint lock or
//! unlock. Option 1, a tie, no votes and a general vote do nothing. The
//! marker is written before the effect runs, so a case never acts twice,
//! even when the effect is then refused (the candidate left).
//!
//! **Which results act automatically [I].** The client has no button to
//! carry out a won CEO vote (`corp_ui_sanctionableactions.py` lists the
//! other types, with an Implement button calling
//! `ActivateSanctionedAction`), so a won CEO vote acts by itself. The other
//! won votes (expel, create shares, lock, unlock) are *sanctioned actions*
//! in the client's model and wait for `ActivateSanctionedAction`: [`sanctioned`]
//! lists them and [`put_in_effect`] marks one acted upon, once, within
//! [`SANCTION_WINDOW`] of the case's end; the service runs the effect.
//!
//! **CEO quorum [A].** A CEO vote acts only when option 0 holds more than
//! half of the corp's issued shares ([`ceo_quorum_met`]), not on plurality
//! alone, so a lone small holder cannot take a corp. Falsifier: reference
//! data on the pass rule, or an owner decision. The displayed result stays
//! the plurality [`outcome`]. A case that closed before this existed acts on
//! the first call after it, if it meets the quorum.

use crate::delta::Deltas;

/// The votes delta kind (`delta_kinds::KINDS`).
pub const KIND: &str = "corpvote";

/// `corporation/voting/voting.py` constants **[V]**.
pub const MAX_OPEN_VOTES: usize = 1000;
pub const MAX_VOTE_DURATION: i64 = 30;
pub const MAX_VOTE_OPTIONS: usize = 50;

/// `voteCEO` .. `voteItemUnlock` **[V]** (`voting.py`).
pub const VOTE_CEO: i64 = 0;
pub const VOTE_WAR: i64 = 1;
pub const VOTE_SHARES: i64 = 2;
pub const VOTE_KICK_MEMBER: i64 = 3;
pub const VOTE_GENERAL: i64 = 4;
pub const VOTE_ITEM_LOCKDOWN: i64 = 5;
pub const VOTE_ITEM_UNLOCK: i64 = 6;

/// FILETIME ticks in one day.
pub const DAY: i64 = 864_000_000_000;

/// Longest title and description kept **[V]**: the proposing form caps the
/// title at 100 characters and the description at 1000 (`corp_ui_votes.py`
/// `OnSelectVoteDetails`, `maxLength`).
pub const MAX_TITLE: usize = 100;
pub const MAX_DESCRIPTION: usize = 1000;

/// How long after its end a case is kept **[A]**; falsifier: the owner wanting
/// a longer history.
pub const RETENTION: i64 = 90 * DAY;

/// How long after a case closes its sanctioned action may be put in effect
/// **[A]**; falsifier: reference data or a TQ observation of a different
/// window. The client only compares `expires` with the wall clock
/// (`voting.py:241-246`).
pub const SANCTION_WINDOW: i64 = 30 * DAY;

/// Why a call is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a vote type the client knows (declaring war is not proposable).
    BadType,
    NoTitle,
    TooLong,
    /// Fewer than two options.
    TooFewOptions,
    TooManyOptions,
    /// An option with no text.
    EmptyOption,
    BadDuration,
    TooManyOpen,
    NoSuchCase,
    /// The vote has ended.
    Closed,
    AlreadyVoted,
    NoSuchOption,
    /// The voter holds no shares of the corp.
    NoWeight,
    /// No such won kick, shares, lock or unlock case.
    NotSanctioned,
    /// The sanctioned action was already put in effect.
    ActedUpon,
    /// The window to put it in effect has passed.
    Expired,
}

/// One vote case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub id: i64,
    pub corp: i64,
    pub vote_type: i64,
    pub creator: i64,
    pub start: i64,
    pub end: i64,
    pub title: String,
    pub description: String,
}

impl Case {
    pub fn is_open(&self, now: i64) -> bool {
        now < self.end
    }
}

/// An option as proposed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptionSpec {
    pub text: String,
    pub parameter: i64,
    pub parameter1: i64,
    pub parameter2: i64,
}

/// An option as stored, with the weight cast for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteOption {
    pub id: i64,
    pub spec: OptionSpec,
    pub votes_for: i64,
}

/// How a closed vote came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// This option has the most weight.
    Won(i64),
    /// The two best options have equal weight.
    Tie,
    NoVotes,
}

fn case_key(id: i64) -> Vec<String> {
    vec!["case".to_owned(), id.to_string()]
}

fn opt_key(id: i64, n: i64) -> Vec<String> {
    vec!["opt".to_owned(), id.to_string(), n.to_string()]
}

fn cast_key(id: i64, voter: i64) -> Vec<String> {
    vec!["cast".to_owned(), id.to_string(), voter.to_string()]
}

fn snap_key(id: i64, holder: i64) -> Vec<String> {
    vec!["snap".to_owned(), id.to_string(), holder.to_string()]
}

fn used_key(id: i64, holder: i64) -> Vec<String> {
    vec!["used".to_owned(), id.to_string(), holder.to_string()]
}

fn snapped_key(id: i64) -> Vec<String> {
    vec!["snapped".to_owned(), id.to_string()]
}

fn num(v: &[String], i: usize) -> Option<i64> {
    v.get(i)?.parse().ok()
}

fn alloc_id(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let n = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    d.put(KIND, key, vec![(n + 1).to_string()]);
    n
}

fn parse_case(id: i64, v: &[String]) -> Option<Case> {
    Some(Case {
        id,
        corp: num(v, 0)?,
        vote_type: num(v, 1)?,
        creator: num(v, 2)?,
        start: num(v, 3)?,
        end: num(v, 4)?,
        title: v.get(5)?.clone(),
        description: v.get(6)?.clone(),
    })
}

/// One case by id.
pub fn get(d: &Deltas, id: i64) -> Option<Case> {
    parse_case(id, d.of(KIND).get(&case_key(id))?)
}

/// A corp's cases, oldest first. `open` picks open or closed ones.
pub fn cases(d: &Deltas, corp: i64, open: bool, now: i64) -> Vec<Case> {
    let mut v: Vec<Case> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "case")
        .filter_map(|(k, row)| parse_case(k[1].parse().ok()?, row))
        .filter(|c| c.corp == corp && c.is_open(now) == open)
        .collect();
    v.sort_by_key(|c| c.id);
    v
}

/// What `voter` weighs in `corp`: their own shares plus those of every corp
/// in `ceo_of` (the corps the voter is CEO of).
pub fn weight(d: &Deltas, corp: i64, voter: i64, ceo_of: &[i64]) -> i64 {
    let mut w = crate::corp_shares::held(d, corp, voter);
    for c in ceo_of {
        if *c != corp {
            w += crate::corp_shares::held(d, corp, *c);
        }
    }
    w
}

/// A proposal: what the proposer asked for.
#[derive(Debug, Clone, Copy)]
pub struct Proposal<'a> {
    pub corp: i64,
    pub creator: i64,
    pub vote_type: i64,
    pub title: &'a str,
    pub description: &'a str,
    /// Whole days, 1 to [`MAX_VOTE_DURATION`].
    pub days: i64,
    pub options: &'a [OptionSpec],
}

/// Open a case. Returns the case id; options get ids 0, 1, ... in the order
/// given.
pub fn open(d: &mut Deltas, p: &Proposal<'_>, now: i64) -> Result<i64, Refusal> {
    let Proposal {
        corp,
        creator,
        vote_type,
        title,
        description,
        days,
        options,
    } = *p;
    if !(VOTE_CEO..=VOTE_ITEM_UNLOCK).contains(&vote_type) || vote_type == VOTE_WAR {
        return Err(Refusal::BadType);
    }
    // A general vote has its title typed in; the others default from the type.
    if title.trim().is_empty() {
        return Err(Refusal::NoTitle);
    }
    if title.chars().count() > MAX_TITLE || description.chars().count() > MAX_DESCRIPTION {
        return Err(Refusal::TooLong);
    }
    if !(1..=MAX_VOTE_DURATION).contains(&days) {
        return Err(Refusal::BadDuration);
    }
    if options.len() < 2 {
        return Err(Refusal::TooFewOptions);
    }
    if options.len() > MAX_VOTE_OPTIONS {
        return Err(Refusal::TooManyOptions);
    }
    if options.iter().any(|o| o.text.trim().is_empty()) {
        return Err(Refusal::EmptyOption);
    }
    if cases(d, corp, true, now).len() >= MAX_OPEN_VOTES {
        return Err(Refusal::TooManyOpen);
    }
    prune(d, now);
    let id = alloc_id(d);
    d.put(
        KIND,
        case_key(id),
        vec![
            corp.to_string(),
            vote_type.to_string(),
            creator.to_string(),
            now.to_string(),
            (now + days * DAY).to_string(),
            title.to_owned(),
            description.to_owned(),
        ],
    );
    for (n, o) in options.iter().enumerate() {
        d.put(
            KIND,
            opt_key(id, n as i64),
            vec![
                o.text.clone(),
                o.parameter.to_string(),
                o.parameter1.to_string(),
                o.parameter2.to_string(),
            ],
        );
    }
    d.put(KIND, snapped_key(id), vec![String::new()]);
    for (holder, n) in crate::corp_shares::holders(d, corp) {
        d.put(KIND, snap_key(id, holder), vec![n.to_string()]);
    }
    Ok(id)
}

/// Drop the cases that closed more than [`RETENTION`] before `now`, with
/// their options, votes and snapshots.
fn prune(d: &mut Deltas, now: i64) {
    let old: Vec<String> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "case")
        .filter_map(|(k, row)| parse_case(k[1].parse().ok()?, row))
        .filter(|c| c.end.saturating_add(RETENTION) < now)
        .map(|c| c.id.to_string())
        .collect();
    if old.is_empty() {
        return;
    }
    let keys: Vec<Vec<String>> = d
        .of(KIND)
        .keys()
        .filter(|k| match k.first().map(String::as_str) {
            Some("case" | "snapped" | "acted" | "sanction") => k.len() == 2 && old.contains(&k[1]),
            Some("opt" | "cast" | "snap" | "used") => k.len() == 3 && old.contains(&k[1]),
            _ => false,
        })
        .cloned()
        .collect();
    for k in keys {
        d.del(KIND, &k);
    }
}

/// What `voter` weighs in case `id`, and the holders whose snapshot shares
/// make it: their own and those of each corp in `ceo_of` (the corps they are
/// CEO of), leaving out holders whose shares were already voted. A case with
/// no snapshot is weighed from the live holdings. `None` for no such case.
pub fn case_weight(d: &Deltas, id: i64, voter: i64, ceo_of: &[i64]) -> Option<(i64, Vec<i64>)> {
    let case = get(d, id)?;
    if !d.of(KIND).contains_key(&snapped_key(id)) {
        return Some((weight(d, case.corp, voter, ceo_of), Vec::new()));
    }
    let mut holders = vec![voter];
    holders.extend(ceo_of.iter().copied().filter(|c| *c != case.corp));
    holders.dedup();
    let mut w = 0;
    let mut counted = Vec::new();
    for h in holders {
        if d.of(KIND).contains_key(&used_key(id, h)) {
            continue;
        }
        let n = d
            .of(KIND)
            .get(&snap_key(id, h))
            .map_or(0, |r| num(r, 0).unwrap_or(0));
        if n > 0 {
            w += n;
            counted.push(h);
        }
    }
    Some((w, counted))
}

/// Cast `voter`'s vote for `option`, weighed from the case's snapshot (see
/// [`case_weight`]); the holders it counted cannot be counted again.
pub fn vote(
    d: &mut Deltas,
    id: i64,
    voter: i64,
    ceo_of: &[i64],
    option: i64,
    now: i64,
) -> Result<(), Refusal> {
    let (w, counted) = case_weight(d, id, voter, ceo_of).ok_or(Refusal::NoSuchCase)?;
    cast(d, id, voter, option, w, now)?;
    for h in counted {
        d.put(KIND, used_key(id, h), vec![String::new()]);
    }
    Ok(())
}

/// `(voter, option, weight)` of every vote on a case, lowest voter first.
pub fn casts(d: &Deltas, id: i64) -> Vec<(i64, i64, i64)> {
    let c = id.to_string();
    let mut v: Vec<(i64, i64, i64)> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3 && k[0] == "cast" && k[1] == c)
        .filter_map(|(k, row)| Some((k[2].parse().ok()?, num(row, 0)?, num(row, 1)?)))
        .collect();
    v.sort();
    v
}

/// What `voter` cast on a case: `(option, weight)`.
pub fn cast_of(d: &Deltas, id: i64, voter: i64) -> Option<(i64, i64)> {
    let row = d.of(KIND).get(&cast_key(id, voter))?;
    Some((num(row, 0)?, num(row, 1)?))
}

/// A case's options with the weight cast for each, in option order.
pub fn options(d: &Deltas, id: i64) -> Vec<VoteOption> {
    let c = id.to_string();
    let votes = casts(d, id);
    let mut v: Vec<VoteOption> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3 && k[0] == "opt" && k[1] == c)
        .filter_map(|(k, row)| {
            let n: i64 = k[2].parse().ok()?;
            Some(VoteOption {
                id: n,
                spec: OptionSpec {
                    text: row.first()?.clone(),
                    parameter: num(row, 1)?,
                    parameter1: num(row, 2)?,
                    parameter2: num(row, 3)?,
                },
                votes_for: votes.iter().filter(|x| x.1 == n).map(|x| x.2).sum(),
            })
        })
        .collect();
    v.sort_by_key(|o| o.id);
    v
}

/// Cast `voter`'s vote for `option` with `weight` (see [`weight`]). One vote
/// per pilot per case; it cannot be changed.
pub fn cast(
    d: &mut Deltas,
    id: i64,
    voter: i64,
    option: i64,
    weight: i64,
    now: i64,
) -> Result<(), Refusal> {
    let case = get(d, id).ok_or(Refusal::NoSuchCase)?;
    if !case.is_open(now) {
        return Err(Refusal::Closed);
    }
    if !d.of(KIND).contains_key(&opt_key(id, option)) {
        return Err(Refusal::NoSuchOption);
    }
    if cast_of(d, id, voter).is_some() {
        return Err(Refusal::AlreadyVoted);
    }
    if weight < 1 {
        return Err(Refusal::NoWeight);
    }
    d.put(
        KIND,
        cast_key(id, voter),
        vec![option.to_string(), weight.to_string()],
    );
    Ok(())
}

/// What a won vote asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing to carry out: a lost, tied or empty vote, "no", a general vote.
    Nothing,
    /// `candidate` becomes the corp's CEO.
    NewCeo { candidate: i64 },
    /// `target` is expelled from the corp.
    Expel { target: i64 },
    /// `shares` more shares are issued.
    CreateShares { shares: i64 },
    /// The blueprint `item` of type `type_id` at `location` is locked.
    LockItem {
        item: i64,
        type_id: i64,
        location: i64,
    },
    /// The same, unlocked.
    UnlockItem {
        item: i64,
        type_id: i64,
        location: i64,
    },
}

/// A closed case that has just been settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub case: Case,
    pub outcome: Outcome,
    pub effect: Effect,
}

/// The effect of `outcome` on a `vote_type` case, from its options.
pub fn effect(vote_type: i64, outcome: Outcome, opts: &[VoteOption]) -> Effect {
    let Outcome::Won(0) = outcome else {
        return Effect::Nothing;
    };
    let Some(p) = opts.iter().find(|o| o.id == 0).map(|o| o.spec.clone()) else {
        return Effect::Nothing;
    };
    match vote_type {
        VOTE_CEO => Effect::NewCeo {
            candidate: p.parameter,
        },
        VOTE_KICK_MEMBER => Effect::Expel {
            target: p.parameter,
        },
        VOTE_SHARES => Effect::CreateShares {
            shares: p.parameter,
        },
        VOTE_ITEM_LOCKDOWN => Effect::LockItem {
            item: p.parameter,
            type_id: p.parameter1,
            location: p.parameter2,
        },
        VOTE_ITEM_UNLOCK => Effect::UnlockItem {
            item: p.parameter,
            type_id: p.parameter1,
            location: p.parameter2,
        },
        _ => Effect::Nothing,
    }
}

/// The rule for a CEO vote to act **[A]**: the yes weight is more than half
/// of the shares issued. Falsifier: reference data on the pass rule, or an
/// owner decision.
pub fn ceo_quorum_met(yes: i64, issued: i64) -> bool {
    issued > 0 && yes.saturating_mul(2) > issued
}

/// Whether the closed case `id` has been settled.
pub fn is_acted(d: &Deltas, id: i64) -> bool {
    d.of(KIND)
        .contains_key(&vec!["acted".to_owned(), id.to_string()])
}

/// The closed cases (`end <= now`) that have not been settled, oldest first.
/// Reads only.
pub fn pending(d: &Deltas, now: i64) -> Vec<Case> {
    let mut closed: Vec<Case> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "case")
        .filter_map(|(k, row)| parse_case(k[1].parse().ok()?, row))
        .filter(|c| !c.is_open(now) && !is_acted(d, c.id))
        .collect();
    closed.sort_by_key(|c| c.id);
    closed
}

/// Settle every [`pending`] case: write its `acted` marker and answer what
/// it asks for, oldest first. A second call answers nothing for the same
/// cases, also after a reload.
pub fn take_due(d: &mut Deltas, now: i64) -> Vec<Due> {
    let closed = pending(d, now);
    let mut out = Vec::new();
    for case in closed {
        let outcome = outcome(d, case.id);
        let opts = options(d, case.id);
        let mut effect = effect(case.vote_type, outcome, &opts);
        if matches!(effect, Effect::NewCeo { .. }) {
            let yes = opts.iter().find(|o| o.id == 0).map_or(0, |o| o.votes_for);
            if !ceo_quorum_met(yes, crate::corp_shares::issued(d, case.corp)) {
                effect = Effect::Nothing;
            }
        }
        let result = match outcome {
            Outcome::Won(n) => format!("won {n}"),
            Outcome::Tie => "tie".to_owned(),
            Outcome::NoVotes => "none".to_owned(),
        };
        d.put(
            KIND,
            vec!["acted".to_owned(), case.id.to_string()],
            vec![result],
        );
        out.push(Due {
            case,
            outcome,
            effect,
        });
    }
    out
}

/// Close the open case `id` now: its `end` becomes `now`, so the next
/// [`take_due`] settles it. `false` for a missing or already closed case.
/// A GM tool; no client call reaches it.
pub fn close_now(d: &mut Deltas, id: i64, now: i64) -> bool {
    let Some(case) = get(d, id) else {
        return false;
    };
    if !case.is_open(now) {
        return false;
    }
    let Some(mut row) = d.of(KIND).get(&case_key(id)).cloned() else {
        return false;
    };
    row[4] = now.to_string();
    d.put(KIND, case_key(id), row);
    true
}

const SLASH_USAGE: &str = "usage: /vote close <voteCaseID>";

/// GM: `/vote close <voteCaseID>` ends an open vote now. It does not settle:
/// the next `voteManager` call does.
pub fn slash(
    reg: &mut crate::service::Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<eve_marshal::PyValue, crate::service::ServiceError> {
    let text = |s: String| Ok(crate::pyutil::bstr(&s));
    let [verb, id] = args else {
        return text(SLASH_USAGE.to_owned());
    };
    let (true, Ok(id)) = (verb.eq_ignore_ascii_case("close"), id.parse::<i64>()) else {
        return text(SLASH_USAGE.to_owned());
    };
    if close_now(reg.deltas_mut(), id, crate::pyutil::filetime_now()) {
        text(format!("vote {id} closed; it acts on the next vote call"))
    } else {
        text(format!("vote {id} is missing or already closed"))
    }
}

/// A won kick, shares, lock or unlock vote, as the client lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanction {
    pub case: Case,
    /// Always 0: the winning "yes".
    pub option: i64,
    pub parameter: i64,
    pub parameter1: i64,
    pub parameter2: i64,
    /// FILETIME: `case.end + SANCTION_WINDOW`.
    pub expires: i64,
    /// FILETIME the CEO put it in effect, if they did.
    pub acted_at: Option<i64>,
}

fn sanction_key(id: i64) -> Vec<String> {
    vec!["sanction".to_owned(), id.to_string()]
}

fn is_sanctionable(vote_type: i64) -> bool {
    matches!(
        vote_type,
        VOTE_KICK_MEMBER | VOTE_SHARES | VOTE_ITEM_LOCKDOWN | VOTE_ITEM_UNLOCK
    )
}

fn sanction_of(d: &Deltas, case: Case) -> Option<Sanction> {
    if !is_sanctionable(case.vote_type) {
        return None;
    }
    if !is_acted(d, case.id) || outcome(d, case.id) != Outcome::Won(0) {
        return None;
    }
    let spec = options(d, case.id).into_iter().find(|o| o.id == 0)?.spec;
    let acted_at = d
        .of(KIND)
        .get(&sanction_key(case.id))
        .map(|r| num(r, 0).unwrap_or(0));
    Some(Sanction {
        expires: case.end.saturating_add(SANCTION_WINDOW),
        option: 0,
        parameter: spec.parameter,
        parameter1: spec.parameter1,
        parameter2: spec.parameter2,
        acted_at,
        case,
    })
}

/// The sanctioned actions of `corp`: its settled, closed cases that were won
/// with option 0 and are a kick, shares, lock or unlock vote, oldest first.
/// [I] The client also renders general and CEO rows, but only the typed ones
/// carry an action, so they are left out.
pub fn sanctioned(d: &Deltas, corp: i64, now: i64) -> Vec<Sanction> {
    let mut v: Vec<Sanction> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "case")
        .filter_map(|(k, row)| parse_case(k[1].parse().ok()?, row))
        .filter(|c| c.corp == corp && !c.is_open(now))
        .filter_map(|c| sanction_of(d, c))
        .collect();
    v.sort_by_key(|s| s.case.id);
    v
}

/// The checks of [`put_in_effect`] without the write: the case must be a
/// closed sanctioned one, not yet acted upon, and inside its window.
pub fn check_put_in_effect(d: &Deltas, id: i64, now: i64) -> Result<Sanction, Refusal> {
    let case = get(d, id).ok_or(Refusal::NotSanctioned)?;
    if case.is_open(now) {
        return Err(Refusal::NotSanctioned);
    }
    let s = sanction_of(d, case).ok_or(Refusal::NotSanctioned)?;
    if s.acted_at.is_some() {
        return Err(Refusal::ActedUpon);
    }
    if now >= s.expires {
        return Err(Refusal::Expired);
    }
    Ok(s)
}

/// Mark case `id`'s sanctioned action as put in effect at `now`, once and
/// within the window. It does not run the effect; the service does, between
/// [`check_put_in_effect`] and this call.
pub fn put_in_effect(d: &mut Deltas, id: i64, now: i64) -> Result<Sanction, Refusal> {
    let mut s = check_put_in_effect(d, id, now)?;
    d.put(KIND, sanction_key(id), vec![now.to_string()]);
    s.acted_at = Some(now);
    Ok(s)
}

/// The result: the option with the most weight.
pub fn outcome(d: &Deltas, id: i64) -> Outcome {
    let mut v: Vec<(i64, i64)> = options(d, id)
        .iter()
        .map(|o| (o.votes_for, o.id))
        .filter(|(w, _)| *w > 0)
        .collect();
    v.sort_by(|a, b| b.cmp(a));
    match v.as_slice() {
        [] => Outcome::NoVotes,
        [(_, id)] => Outcome::Won(*id),
        [(a, id), (b, _), ..] if a > b => Outcome::Won(*id),
        _ => Outcome::Tie,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corp_shares as cs;

    // Invented ids: nothing here is a client row.
    const CORP: i64 = 98_000_701;
    const OTHER: i64 = 98_000_702;
    const ALICE: i64 = 90_000_801;
    const BOB: i64 = 90_000_802;
    const CAROL: i64 = 90_000_803;
    const T0: i64 = 1_000_000;

    fn yes_no() -> Vec<OptionSpec> {
        ["Yes", "No"]
            .iter()
            .map(|t| OptionSpec {
                text: (*t).to_owned(),
                ..OptionSpec::default()
            })
            .collect()
    }

    fn shares(d: &mut Deltas) {
        cs::found(d, CORP, ALICE);
        cs::move_shares(d, CORP, ALICE, BOB, 300).unwrap();
    }

    fn prop(d: &mut Deltas, title: &str, days: i64, now: i64) -> i64 {
        open(
            d,
            &Proposal {
                corp: CORP,
                creator: ALICE,
                vote_type: VOTE_GENERAL,
                title,
                description: "",
                days,
                options: &yes_no(),
            },
            now,
        )
        .unwrap()
    }

    fn opened(d: &mut Deltas, days: i64) -> i64 {
        prop(d, "Move HQ", days, T0)
    }

    #[test]
    fn open_stores_the_case_and_its_options() {
        let mut d = Deltas::default();
        let id = opened(&mut d, 2);
        let c = get(&d, id).unwrap();
        assert_eq!(
            (c.corp, c.vote_type, c.creator),
            (CORP, VOTE_GENERAL, ALICE)
        );
        assert_eq!((c.start, c.end), (T0, T0 + 2 * DAY));
        let o = options(&d, id);
        assert_eq!(o.len(), 2);
        assert_eq!((o[0].id, o[0].spec.text.as_str()), (0, "Yes"));
        assert_eq!(o[1].spec.text, "No");
        assert_eq!(opened(&mut d, 1), id + 1);
    }

    #[test]
    fn open_refuses_a_bad_proposal() {
        let mut d = Deltas::default();
        let mut try_open = |t: i64, title: &str, days: i64, o: &[OptionSpec]| {
            open(
                &mut d,
                &Proposal {
                    corp: CORP,
                    creator: ALICE,
                    vote_type: t,
                    title,
                    description: "",
                    days,
                    options: o,
                },
                T0,
            )
        };
        assert_eq!(try_open(VOTE_WAR, "x", 1, &yes_no()), Err(Refusal::BadType));
        assert_eq!(try_open(99, "x", 1, &yes_no()), Err(Refusal::BadType));
        assert_eq!(
            try_open(VOTE_GENERAL, " ", 1, &yes_no()),
            Err(Refusal::NoTitle)
        );
        assert_eq!(
            try_open(VOTE_GENERAL, "x", 0, &yes_no()),
            Err(Refusal::BadDuration)
        );
        assert_eq!(
            try_open(VOTE_GENERAL, "x", 31, &yes_no()),
            Err(Refusal::BadDuration)
        );
        assert_eq!(
            try_open(VOTE_GENERAL, "x", 1, &yes_no()[..1]),
            Err(Refusal::TooFewOptions)
        );
        let many = vec![yes_no()[0].clone(); MAX_VOTE_OPTIONS + 1];
        assert_eq!(
            try_open(VOTE_GENERAL, "x", 1, &many),
            Err(Refusal::TooManyOptions)
        );
        let mut blank = yes_no();
        blank[1].text = " ".to_owned();
        assert_eq!(
            try_open(VOTE_GENERAL, "x", 1, &blank),
            Err(Refusal::EmptyOption)
        );
        let long = "a".repeat(MAX_TITLE + 1);
        assert_eq!(
            try_open(VOTE_GENERAL, &long, 1, &yes_no()),
            Err(Refusal::TooLong)
        );
        assert!(d.of(KIND).is_empty(), "a refusal writes nothing");
    }

    #[test]
    fn weight_is_the_shares_held_plus_a_ceos_corp_shares() {
        let mut d = Deltas::default();
        shares(&mut d);
        assert_eq!(weight(&d, CORP, BOB, &[]), 300);
        assert_eq!(weight(&d, CORP, ALICE, &[]), 700);
        assert_eq!(weight(&d, CORP, CAROL, &[]), 0);
        cs::move_shares(&mut d, CORP, ALICE, OTHER, 100).unwrap();
        assert_eq!(weight(&d, CORP, CAROL, &[OTHER]), 100);
        assert_eq!(
            weight(&d, CORP, CAROL, &[CORP]),
            0,
            "a corp's own is not counted"
        );
    }

    #[test]
    fn votes_are_weighed_by_shares_and_summed_per_option() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = opened(&mut d, 1);
        let w = weight(&d, CORP, ALICE, &[]);
        cast(&mut d, id, ALICE, 1, w, T0 + 1).unwrap();
        let w = weight(&d, CORP, BOB, &[]);
        cast(&mut d, id, BOB, 0, w, T0 + 2).unwrap();
        let o = options(&d, id);
        assert_eq!((o[0].votes_for, o[1].votes_for), (300, 700));
        assert_eq!(outcome(&d, id), Outcome::Won(1));
        assert_eq!(cast_of(&d, id, BOB), Some((0, 300)));
    }

    #[test]
    fn a_pilot_votes_once_and_only_while_open() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = opened(&mut d, 1);
        cast(&mut d, id, BOB, 0, 300, T0).unwrap();
        assert_eq!(
            cast(&mut d, id, BOB, 1, 300, T0),
            Err(Refusal::AlreadyVoted)
        );
        assert_eq!(
            cast(&mut d, id, ALICE, 0, 700, T0 + DAY),
            Err(Refusal::Closed)
        );
        assert_eq!(
            cast(&mut d, id, ALICE, 7, 700, T0),
            Err(Refusal::NoSuchOption)
        );
        assert_eq!(cast(&mut d, id, CAROL, 0, 0, T0), Err(Refusal::NoWeight));
        assert_eq!(cast(&mut d, 999, ALICE, 0, 1, T0), Err(Refusal::NoSuchCase));
        assert_eq!(casts(&d, id).len(), 1);
    }

    #[test]
    fn a_vote_is_open_until_its_end_then_closed() {
        let mut d = Deltas::default();
        let id = opened(&mut d, 1);
        assert_eq!(cases(&d, CORP, true, T0 + DAY - 1).len(), 1);
        assert!(cases(&d, CORP, false, T0 + DAY - 1).is_empty());
        assert!(cases(&d, CORP, true, T0 + DAY).is_empty());
        assert_eq!(cases(&d, CORP, false, T0 + DAY)[0].id, id);
        assert!(
            cases(&d, OTHER, true, T0).is_empty(),
            "another corp's is not listed"
        );
    }

    #[test]
    fn the_result_is_a_win_a_tie_or_nothing() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = opened(&mut d, 1);
        assert_eq!(outcome(&d, id), Outcome::NoVotes);
        cast(&mut d, id, BOB, 0, 300, T0).unwrap();
        assert_eq!(outcome(&d, id), Outcome::Won(0));
        cast(&mut d, id, ALICE, 1, 300, T0).unwrap();
        assert_eq!(outcome(&d, id), Outcome::Tie);
    }

    #[test]
    fn the_open_count_is_capped() {
        let mut d = Deltas::default();
        for _ in 0..MAX_OPEN_VOTES {
            opened(&mut d, 1);
        }
        let mut one = |now| {
            open(
                &mut d,
                &Proposal {
                    corp: CORP,
                    creator: ALICE,
                    vote_type: VOTE_GENERAL,
                    title: "x",
                    description: "",
                    days: 1,
                    options: &yes_no(),
                },
                now,
            )
        };
        assert_eq!(one(T0), Err(Refusal::TooManyOpen));
        // A closed one does not count.
        assert!(one(T0 + 2 * DAY).is_ok());
    }

    #[test]
    fn shares_handed_on_after_voting_are_not_voted_twice() {
        let mut d = Deltas::default();
        shares(&mut d); // ALICE 700, BOB 300
        let id = opened(&mut d, 5);
        vote(&mut d, id, ALICE, &[], 0, T0).unwrap();
        cs::move_shares(&mut d, CORP, ALICE, CAROL, 700).unwrap();
        // CAROL held nothing when the case opened.
        assert_eq!(vote(&mut d, id, CAROL, &[], 1, T0), Err(Refusal::NoWeight));
        vote(&mut d, id, BOB, &[], 1, T0).unwrap();
        assert_eq!(cast_of(&d, id, ALICE), Some((0, 700)));
        assert_eq!(outcome(&d, id), Outcome::Won(0));
    }

    #[test]
    fn shares_bought_after_the_case_opened_weigh_nothing() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = opened(&mut d, 5);
        cs::move_shares(&mut d, CORP, ALICE, CAROL, 100).unwrap();
        assert_eq!(vote(&mut d, id, CAROL, &[], 0, T0), Err(Refusal::NoWeight));
        // ALICE weighs what she held at the open, not what she has left.
        vote(&mut d, id, ALICE, &[], 0, T0).unwrap();
        assert_eq!(cast_of(&d, id, ALICE), Some((0, 700)));
    }

    #[test]
    fn a_corps_shares_are_voted_once_whoever_is_its_ceo() {
        let mut d = Deltas::default();
        shares(&mut d);
        cs::move_shares(&mut d, CORP, ALICE, OTHER, 200).unwrap();
        let id = opened(&mut d, 5);
        // BOB is CEO of OTHER and votes its 200 with his own 300.
        vote(&mut d, id, BOB, &[OTHER], 0, T0).unwrap();
        assert_eq!(cast_of(&d, id, BOB), Some((0, 500)));
        // OTHER changes CEO to CAROL, who has no shares of her own.
        assert_eq!(
            vote(&mut d, id, CAROL, &[OTHER], 0, T0),
            Err(Refusal::NoWeight)
        );
    }

    #[test]
    fn a_closed_case_is_dropped_after_the_retention() {
        let mut d = Deltas::default();
        shares(&mut d);
        let old = opened(&mut d, 1);
        vote(&mut d, old, ALICE, &[], 0, T0).unwrap();
        // Still kept just after it ends.
        let fresh = prop(&mut d, "Later", 1, T0 + 2 * DAY);
        assert!(get(&d, old).is_some());
        let last = prop(&mut d, "Last", 1, T0 + DAY + RETENTION + 1);
        assert!(get(&d, old).is_none());
        assert!(options(&d, old).is_empty());
        assert!(casts(&d, old).is_empty());
        assert!(get(&d, last).is_some());
        let _ = fresh;
    }

    fn typed(d: &mut Deltas, vote_type: i64, param: i64) -> i64 {
        open(
            d,
            &Proposal {
                corp: CORP,
                creator: BOB,
                vote_type,
                title: "t",
                description: "",
                days: 1,
                options: &[
                    OptionSpec {
                        text: "Yes".to_owned(),
                        parameter: param,
                        parameter1: 7,
                        parameter2: 8,
                    },
                    OptionSpec {
                        text: "No".to_owned(),
                        parameter: param,
                        ..OptionSpec::default()
                    },
                ],
            },
            T0,
        )
        .unwrap()
    }

    #[test]
    fn a_won_ceo_vote_asks_for_the_candidate() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = typed(&mut d, VOTE_CEO, BOB);
        vote(&mut d, id, ALICE, &[], 0, T0).unwrap();
        assert!(take_due(&mut d, T0 + DAY - 1).is_empty(), "still open");
        let due = take_due(&mut d, T0 + DAY);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].case.id, id);
        assert_eq!(due[0].outcome, Outcome::Won(0));
        assert_eq!(due[0].effect, Effect::NewCeo { candidate: BOB });
    }

    #[test]
    fn a_lost_tied_empty_or_general_vote_asks_for_nothing() {
        let mut d = Deltas::default();
        shares(&mut d);
        let no = typed(&mut d, VOTE_CEO, BOB);
        vote(&mut d, no, ALICE, &[], 1, T0).unwrap();
        let tie = typed(&mut d, VOTE_KICK_MEMBER, CAROL);
        cast(&mut d, tie, ALICE, 0, 5, T0).unwrap();
        cast(&mut d, tie, BOB, 1, 5, T0).unwrap();
        let empty = typed(&mut d, VOTE_SHARES, 10);
        let general = opened(&mut d, 1);
        vote(&mut d, general, ALICE, &[], 0, T0).unwrap();
        let due = take_due(&mut d, T0 + DAY);
        assert_eq!(due.len(), 4);
        assert!(due.iter().all(|x| x.effect == Effect::Nothing));
        let of = |id| due.iter().find(|x| x.case.id == id).unwrap().outcome;
        assert_eq!(of(no), Outcome::Won(1));
        assert_eq!(of(tie), Outcome::Tie);
        assert_eq!(of(empty), Outcome::NoVotes);
    }

    #[test]
    fn each_type_names_its_effect_and_a_case_acts_once() {
        let mut d = Deltas::default();
        shares(&mut d);
        let kick = typed(&mut d, VOTE_KICK_MEMBER, CAROL);
        let mk = typed(&mut d, VOTE_SHARES, 50);
        let lock = typed(&mut d, VOTE_ITEM_LOCKDOWN, 11);
        let unlock = typed(&mut d, VOTE_ITEM_UNLOCK, 12);
        for id in [kick, mk, lock, unlock] {
            vote(&mut d, id, ALICE, &[], 0, T0).unwrap();
        }
        let due = take_due(&mut d, T0 + 2 * DAY);
        let eff: Vec<Effect> = due.iter().map(|x| x.effect).collect();
        assert_eq!(
            eff,
            vec![
                Effect::Expel { target: CAROL },
                Effect::CreateShares { shares: 50 },
                Effect::LockItem {
                    item: 11,
                    type_id: 7,
                    location: 8
                },
                Effect::UnlockItem {
                    item: 12,
                    type_id: 7,
                    location: 8
                },
            ]
        );
        assert!(is_acted(&d, kick));
        // A second pass, and one after the deltas are saved and loaded again,
        // act on nothing.
        assert!(take_due(&mut d, T0 + 3 * DAY).is_empty());
        let mut reloaded = Deltas::default();
        for (k, v) in d.of(KIND).clone() {
            reloaded.put(KIND, k, v);
        }
        assert!(take_due(&mut reloaded, T0 + 3 * DAY).is_empty());
    }

    #[test]
    fn the_acted_marker_goes_with_the_case_at_retention() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = typed(&mut d, VOTE_CEO, BOB);
        take_due(&mut d, T0 + DAY);
        prop(&mut d, "Last", 1, T0 + DAY + RETENTION + 1);
        assert!(get(&d, id).is_none());
        assert!(!is_acted(&d, id));
    }

    #[test]
    fn a_ceo_vote_needs_more_than_half_the_issued_shares() {
        let mut d = Deltas::default();
        shares(&mut d); // ALICE 700, BOB 300 of 1000
        cs::move_shares(&mut d, CORP, ALICE, CAROL, 1).unwrap();
        // A lone 1-share voter wins the plurality but does not take the corp.
        let lone = typed(&mut d, VOTE_CEO, CAROL);
        vote(&mut d, lone, CAROL, &[], 0, T0).unwrap();
        // Exactly half is not more than half.
        let half = typed(&mut d, VOTE_CEO, BOB);
        cast(&mut d, half, ALICE, 0, 500, T0).unwrap();
        // A majority holder does.
        let most = typed(&mut d, VOTE_CEO, BOB);
        vote(&mut d, most, ALICE, &[], 0, T0).unwrap();
        let due = take_due(&mut d, T0 + DAY);
        let eff = |id| due.iter().find(|x| x.case.id == id).unwrap().effect;
        assert_eq!(
            due.iter().find(|x| x.case.id == lone).unwrap().outcome,
            Outcome::Won(0)
        );
        assert_eq!(eff(lone), Effect::Nothing);
        assert_eq!(eff(half), Effect::Nothing);
        assert_eq!(eff(most), Effect::NewCeo { candidate: BOB });
        assert!(ceo_quorum_met(501, 1000) && !ceo_quorum_met(500, 1000) && !ceo_quorum_met(1, 0));
    }

    #[test]
    fn pending_reads_without_writing() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = typed(&mut d, VOTE_CEO, BOB);
        assert!(pending(&d, T0).is_empty());
        assert_eq!(pending(&d, T0 + DAY)[0].id, id);
        assert!(!is_acted(&d, id));
    }

    /// A won typed case, settled after it closed.
    fn won(d: &mut Deltas, vote_type: i64, param: i64) -> i64 {
        let id = typed(d, vote_type, param);
        vote(d, id, ALICE, &[], 0, T0).unwrap();
        take_due(d, T0 + 2 * DAY);
        id
    }

    #[test]
    fn a_won_kick_vote_is_listed_with_its_target_and_window() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = won(&mut d, VOTE_KICK_MEMBER, CAROL);
        let now = T0 + 2 * DAY;
        assert!(sanctioned(&d, OTHER, now).is_empty());
        let list = sanctioned(&d, CORP, now);
        assert_eq!(list.len(), 1);
        let s = &list[0];
        assert_eq!(s.case.id, id);
        assert_eq!((s.option, s.parameter), (0, CAROL));
        assert_eq!((s.parameter1, s.parameter2), (7, 8));
        assert_eq!(s.expires, s.case.end + SANCTION_WINDOW);
        assert_eq!(s.acted_at, None);
    }

    #[test]
    fn won_general_and_ceo_lost_empty_and_unsettled_kick_votes_are_not_listed() {
        let mut d = Deltas::default();
        shares(&mut d);
        let ceo = won(&mut d, VOTE_CEO, BOB);
        let general = won(&mut d, VOTE_GENERAL, 0);
        // A kick vote that went to "No".
        let no = typed(&mut d, VOTE_KICK_MEMBER, CAROL);
        vote(&mut d, no, ALICE, &[], 1, T0).unwrap();
        // A kick vote nobody voted on.
        let empty = typed(&mut d, VOTE_KICK_MEMBER, CAROL);
        take_due(&mut d, T0 + 2 * DAY);
        assert!(is_acted(&d, empty));
        // A won kick vote not yet settled is not listed either.
        let unsettled = typed(&mut d, VOTE_KICK_MEMBER, CAROL);
        vote(&mut d, unsettled, ALICE, &[], 0, T0).unwrap();
        assert!(sanctioned(&d, CORP, T0 + 2 * DAY).is_empty());
        // Settling hands the won kick votes over: the earlier unsettled one
        // and a new one are listed, nothing else.
        let kick = won(&mut d, VOTE_KICK_MEMBER, CAROL);
        let ids: Vec<i64> = sanctioned(&d, CORP, T0 + 2 * DAY)
            .iter()
            .map(|s| s.case.id)
            .collect();
        assert_eq!(ids, vec![unsettled, kick]);
        let _ = (ceo, general);
    }

    #[test]
    fn put_in_effect_once_then_acted_upon() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = won(&mut d, VOTE_KICK_MEMBER, CAROL);
        let end = get(&d, id).unwrap().end;
        let at = end + SANCTION_WINDOW / 2;
        let s = put_in_effect(&mut d, id, at).unwrap();
        assert_eq!(s.acted_at, Some(at));
        assert_eq!(sanctioned(&d, CORP, at)[0].acted_at, Some(at));
        assert_eq!(put_in_effect(&mut d, id, at + 1), Err(Refusal::ActedUpon));
        // A second case, past its window.
        let late = won(&mut d, VOTE_KICK_MEMBER, BOB);
        let expires = get(&d, late).unwrap().end + SANCTION_WINDOW;
        assert_eq!(put_in_effect(&mut d, late, expires), Err(Refusal::Expired));
        assert!(put_in_effect(&mut d, late, expires - 1).is_ok());
        // Not a sanctioned case.
        let ceo = won(&mut d, VOTE_CEO, BOB);
        assert_eq!(put_in_effect(&mut d, ceo, at), Err(Refusal::NotSanctioned));
        assert_eq!(
            put_in_effect(&mut d, 9_999, at),
            Err(Refusal::NotSanctioned)
        );
    }

    #[test]
    fn the_sanction_marker_goes_with_its_case_at_retention() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = won(&mut d, VOTE_KICK_MEMBER, CAROL);
        put_in_effect(&mut d, id, T0 + 2 * DAY).unwrap();
        prop(&mut d, "Last", 1, T0 + DAY + RETENTION + 1);
        assert!(get(&d, id).is_none());
        assert!(!d.of(KIND).contains_key(&sanction_key(id)));
    }

    #[test]
    fn close_now_ends_an_open_case_and_take_due_settles_it() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = typed(&mut d, VOTE_CEO, BOB);
        vote(&mut d, id, ALICE, &[], 0, T0).unwrap();
        assert!(pending(&d, T0 + 5).is_empty(), "still open");
        assert!(close_now(&mut d, id, T0 + 5));
        assert_eq!(get(&d, id).unwrap().end, T0 + 5);
        assert_eq!(pending(&d, T0 + 5).len(), 1);
        let due = take_due(&mut d, T0 + 5);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].effect, Effect::NewCeo { candidate: BOB });
    }

    #[test]
    fn close_now_refuses_a_closed_or_missing_case() {
        let mut d = Deltas::default();
        shares(&mut d);
        let id = opened(&mut d, 1);
        assert!(!close_now(&mut d, id + 99, T0));
        assert!(!close_now(&mut d, id, T0 + DAY), "already closed");
        assert_eq!(get(&d, id).unwrap().end, T0 + DAY, "left alone");
        assert!(close_now(&mut d, id, T0 + 1));
        assert!(!close_now(&mut d, id, T0 + 2), "closed by the first call");
    }

    #[test]
    fn the_vote_slash_closes_by_id_and_explains_its_usage() {
        use crate::service::Registry;
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let now = crate::pyutil::filetime_now();
        shares(reg.deltas_mut());
        let id = open(
            reg.deltas_mut(),
            &Proposal {
                corp: CORP,
                creator: ALICE,
                vote_type: VOTE_GENERAL,
                title: "t",
                description: "",
                days: 1,
                options: &yes_no(),
            },
            now,
        )
        .unwrap();
        let say = |reg: &mut Registry, a: &[&str]| match slash(reg, None, a).unwrap() {
            eve_marshal::PyValue::Bytes(b) => String::from_utf8(b).unwrap(),
            other => panic!("text expected, got {other:?}"),
        };
        assert_eq!(say(&mut reg, &[]), SLASH_USAGE);
        assert_eq!(say(&mut reg, &["close"]), SLASH_USAGE);
        assert_eq!(say(&mut reg, &["close", "x"]), SLASH_USAGE);
        assert_eq!(say(&mut reg, &["open", "1"]), SLASH_USAGE);
        assert_eq!(
            say(&mut reg, &["close", &id.to_string()]),
            format!("vote {id} closed; it acts on the next vote call")
        );
        assert!(
            !get(reg.deltas(), id)
                .unwrap()
                .is_open(crate::pyutil::filetime_now())
        );
        assert_eq!(
            say(&mut reg, &["close", &id.to_string()]),
            format!("vote {id} is missing or already closed")
        );
    }
}
