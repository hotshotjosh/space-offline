//! **The voting wire** (lane `corp-votes`, #103): the `voteManager` calls
//! `services/corporation/voting.py` makes, answered from `crate::corp_votes`.
//! `service/corp.rs`'s `voteManager` arm routes every method here.
//!
//! | call | answer (client use) |
//! | --- | --- |
//! | `CanViewVotes(corpID)` | bool: a shareholder, or the corp's CEO (`voting.py` `CanViewVotes`; the panel shows "only shareholders" otherwise) |
//! | `CanRunForCEO()` | bool, cached as `canRunForCEO` |
//! | `GetVoteCasesByCorporation(corpID[, status])` | a dict `status -> CRowset` for the open (2) and closed (1) cases: `voteCaseID corporationID voteType voteCaseText description startDateTime endDateTime` |
//! | `GetVoteCase(corpID, voteCaseID)` | one `KeyVal` with those columns |
//! | `GetVoteCaseOptions(corpID, voteCaseID)` | a `CRowset`: `optionID voteCaseID optionText parameter parameter1 parameter2 votesFor` |
//! | `GetVotes(voteCaseID)` | the caller's own vote as a list of `KeyVal` (`optionID voteCaseID characterID weight`), empty before they vote |
//! | `InsertVoteCase(title, description, type, options, days)` | `None`; pushes `OnVoteCaseChanged` |
//! | `InsertVote(corpID, voteCaseID, optionID)` | `None`; pushes `OnVoteCast` and `OnVoteCaseChanged` |
//! | `GetCorpSanctionedActions()` | a dict `voteCaseID -> KeyVal` of the corp's won kick, shares, lock and unlock votes (`voteCaseID voteType optionID parameter parameter1 parameter2 expires actedUpon inEffect timeActedUpon timeRescended`), empty for a non-viewer |
//! | `ActivateSanctionedAction(voteCaseID)` | `None`; the CEO puts a won expulsion in effect; pushes `OnSanctionedActionChanged` and `OnVoteCaseChanged` |
//!
//! **Shapes [V]** (`voting.py`, `corp_ui_votes.py`): `voteCases` is indexed by
//! status and its values iterate as rows and carry `.header`; a vote case
//! row is read by attribute (`vc.endDateTime`); `GetVotes` is measured with
//! `len` and read as `[0].optionID`. `InsertVoteCase`'s `options` is, for a
//! general vote, a list of option texts, and for the other types the tuple
//! `(yesLabel, noLabel, parameter, parameter1, parameter2)` where each label
//! is a `(labelPath, kwargs)` tuple (`CreateVote`). The client words options
//! of every type but a general vote itself (`voting.GetVoteOptionText`), so
//! the label path is only stored.
//!
//! **Who may do what.** A shareholder (or a CEO of a corp that holds shares)
//! votes; the Director role or the CEO proposes anything but a CEO vote, as
//! the client offers `ProposeVote` to a Director only (`corp_ui_votes.py`
//! `Load`). Any member who is not the CEO and holds shares may run for CEO
//! (`CanRunForCEO`, **[A]**: the real rule is not in the client; falsifier:
//! reference data).
//!
//! **A won vote acts (#153).** [`settle`] runs at the start of every
//! `voteManager` call and settles each closed, unsettled case once
//! (`crate::corp_votes::take_due`). A **CEO vote acts automatically [I]**
//! (the client has no button for it): when option 0 holds more than half of
//! the issued shares (`corp_votes::ceo_quorum_met`, **[A]**) the seat goes to
//! the candidate through `corp::hand_over`, the path `ResignFromCEO` takes.
//! The candidate is always the proposer: `insert_case` overwrites the
//! option's parameter with the caller. The **sanctioned actions** (expel,
//! create shares, lock or unlock a blueprint) are listed by
//! `GetCorpSanctionedActions`, and the client's Implement button calls
//! `ActivateSanctionedAction(voteCaseID)` (`corp_ui_sanctionableactions.py`
//! **[V]**): the CEO puts a won expulsion in effect through
//! `corp::expel`, keeping every `can_be_kicked_out` refusal. Create shares
//! and blueprint lock or unlock are refused until built, and stay listed and
//! not acted upon. Nobody is told when
//! nothing is called, so a closed vote acts when the next vote call comes
//! in, from anyone.

use super::args::Args;
use super::*;
use crate::corp_shares as cs;
use crate::corp_votes::{self as cv, OptionSpec, Refusal};
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue};

/// `VOTECASE_STATUS_*` **[V]** (`voting.py`).
const STATUS_CLOSED: i64 = 1;
const STATUS_OPEN: i64 = 2;

/// A vote push: the event, its arguments, and who hears it.
#[derive(Debug, Clone, PartialEq)]
pub struct VotePush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for VotePush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn custom(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

const NOT_CEO: &str = "Only the CEO can put a sanctioned action in effect.";

fn denied(reason: &str) -> ServiceError {
    raise(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn refusal(r: Refusal) -> ServiceError {
    custom(match r {
        Refusal::BadType => "That is not a kind of vote a corporation can hold.",
        Refusal::NoTitle => "A vote needs a title.",
        Refusal::TooLong => "That vote's title or description is too long.",
        Refusal::TooFewOptions => "A vote needs at least two options.",
        Refusal::TooManyOptions => "That vote has too many options.",
        Refusal::EmptyOption => "Every option needs some text.",
        Refusal::BadDuration => "A vote can run from one to thirty days.",
        Refusal::TooManyOpen => "The corporation already has the most open votes it can have.",
        Refusal::NoSuchCase => "That vote does not exist.",
        Refusal::Closed => "That vote has ended.",
        Refusal::AlreadyVoted => "You have already voted on this.",
        Refusal::NoSuchOption => "That is not an option of this vote.",
        Refusal::NoWeight => "Only shareholders can vote.",
        Refusal::NotSanctioned => "That vote has no action to put in effect.",
        Refusal::ActedUpon => "That action has already been put in effect.",
        Refusal::Expired => "The time to put that action in effect has passed.",
    })
}

fn live_corp(reg: &Registry, corp: i64) -> bool {
    reg.corporations().get(corp).is_some_and(|c| !c.deleted)
}

/// Issue `corp`'s shares if it has none yet (a corp founded before shares).
fn ensure_founded(reg: &mut Registry, corp: i64) {
    if cs::is_founded(reg.deltas(), corp) {
        return;
    }
    if let Some(ceo) = reg
        .corporations()
        .get(corp)
        .filter(|c| !c.deleted)
        .map(|c| c.ceo_id)
    {
        cs::found(reg.deltas_mut(), corp, ceo);
    }
}

fn ceo_of(reg: &Registry, me: i64) -> Vec<i64> {
    reg.corporations()
        .all()
        .filter(|c| c.ceo_id == me)
        .map(|c| c.corporation_id)
        .collect()
}

fn weight(reg: &mut Registry, corp: i64, me: i64) -> i64 {
    if !live_corp(reg, corp) {
        return 0;
    }
    ensure_founded(reg, corp);
    cv::weight(reg.deltas(), corp, me, &ceo_of(reg, me))
}

fn can_view(reg: &mut Registry, corp: i64, me: i64) -> bool {
    weight(reg, corp, me) > 0 || ceo_of(reg, me).contains(&corp)
}

/// The caller's own corp when they are in one.
fn my_corp(reg: &Registry, me: i64) -> Option<i64> {
    let m = reg.corporations().membership(me)?;
    live_corp(reg, m.corporation_id).then_some(m.corporation_id)
}

/// Members of `corp` and its personal shareholders, once each.
fn listeners(reg: &Registry, corp: i64) -> Vec<i64> {
    let mut v = reg.corporations().members_of(corp);
    for (holder, _) in cs::holders(reg.deltas(), corp) {
        if reg.character(holder).is_some() && !v.contains(&holder) {
            v.push(holder);
        }
    }
    v
}

fn tell(reg: &mut Registry, corp: i64, case: i64, events: &[&'static str]) {
    let to = listeners(reg, corp);
    if to.is_empty() {
        return;
    }
    for event in events {
        reg.schedule(
            std::time::Duration::ZERO,
            VotePush {
                event,
                args: vec![PyValue::Int(corp), PyValue::Int(case)],
                to: to.clone(),
            },
        );
    }
}

fn describe(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::new(
        columns
            .iter()
            .map(|(name, db_type)| Column {
                name: name.as_bytes().to_vec(),
                db_type: *db_type,
            })
            .collect(),
    )
    .expect("the vote columns are a valid layout")
}

fn text(s: &str) -> DbValue {
    DbValue::Object(PyValue::Unicode(s.to_owned()))
}

const CASE_COLUMNS: &[(&str, DbType)] = &[
    ("voteCaseID", DbType::I4),
    ("corporationID", DbType::I4),
    ("voteType", DbType::I4),
    ("voteCaseText", DbType::WStr),
    ("description", DbType::WStr),
    ("startDateTime", DbType::FileTime),
    ("endDateTime", DbType::FileTime),
];

fn cases_rowset(list: &[cv::Case]) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(describe(CASE_COLUMNS));
    for c in list {
        rs.push_values(vec![
            DbValue::Int(c.id),
            DbValue::Int(c.corp),
            DbValue::Int(c.vote_type),
            text(&c.title),
            text(&c.description),
            DbValue::Int(c.start),
            DbValue::Int(c.end),
        ])
        .expect("the vote case row matches its descriptor");
    }
    rs.to_pyvalue()
}

fn options_rowset(case: i64, list: &[cv::VoteOption]) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(describe(&[
        ("optionID", DbType::I4),
        ("voteCaseID", DbType::I4),
        ("optionText", DbType::WStr),
        ("parameter", DbType::I8),
        ("parameter1", DbType::I8),
        ("parameter2", DbType::I8),
        ("votesFor", DbType::I8),
    ]));
    for o in list {
        rs.push_values(vec![
            DbValue::Int(o.id),
            DbValue::Int(case),
            text(&o.spec.text),
            DbValue::Int(o.spec.parameter),
            DbValue::Int(o.spec.parameter1),
            DbValue::Int(o.spec.parameter2),
            DbValue::Int(o.votes_for),
        ])
        .expect("the vote option row matches its descriptor");
    }
    rs.to_pyvalue()
}

fn case_keyval(c: &cv::Case) -> PyValue {
    crate::pyutil::keyval(vec![
        ("voteCaseID", PyValue::Int(c.id)),
        ("corporationID", PyValue::Int(c.corp)),
        ("voteType", PyValue::Int(c.vote_type)),
        ("voteCaseText", PyValue::Unicode(c.title.clone())),
        ("description", PyValue::Unicode(c.description.clone())),
        ("startDateTime", PyValue::Int(c.start)),
        ("endDateTime", PyValue::Int(c.end)),
    ])
}

/// One sanctioned action as the client reads it **[V]**
/// (`corp_ui_sanctionableactions.py:93-116`, `:136-149`, `:212`;
/// `voting.py:241-267`). In effect means acted upon. [I] The client indexes
/// `[No, Yes][row.actedUpon]`, so the flags are ints 0 or 1, not bools.
fn sanction_row(s: &cv::Sanction) -> PyValue {
    let flag = i64::from(s.acted_at.is_some());
    crate::pyutil::keyval(vec![
        ("voteCaseID", PyValue::Int(s.case.id)),
        ("voteType", PyValue::Int(s.case.vote_type)),
        ("optionID", PyValue::Int(s.option)),
        ("parameter", PyValue::Int(s.parameter)),
        ("parameter1", PyValue::Int(s.parameter1)),
        ("parameter2", PyValue::Int(s.parameter2)),
        ("expires", PyValue::Int(s.expires)),
        ("actedUpon", PyValue::Int(flag)),
        ("inEffect", PyValue::Int(flag)),
        (
            "timeActedUpon",
            s.acted_at.map_or(PyValue::None, PyValue::Int),
        ),
        ("timeRescended", PyValue::None),
    ])
}

/// `GetCorpSanctionedActions`: `{voteCaseID: row}`, as (value, key) pairs.
fn get_sanctioned(reg: &mut Registry, me: i64) -> PyValue {
    let Some(corp) = my_corp(reg, me).filter(|c| can_view(reg, *c, me)) else {
        return PyValue::Dict(vec![]);
    };
    let now = crate::pyutil::filetime_now();
    PyValue::Dict(
        cv::sanctioned(reg.deltas(), corp, now)
            .iter()
            .map(|s| (sanction_row(s), PyValue::Int(s.case.id)))
            .collect(),
    )
}

/// `ActivateSanctionedAction(voteCaseID)`: the CEO puts a won sanctioned
/// action in effect **[V]** (the client gates on `UserIsActiveCEO`).
fn activate_sanctioned(
    reg: &mut Registry,
    req: &Request<'_>,
    me: i64,
) -> Result<PyValue, ServiceError> {
    let id = Args::new(req).int_or_kw(0, "voteCaseID").unwrap_or(0);
    // An unknown id answers like another corporation's case, so a stranger
    // cannot tell whether a case exists.
    let Some(case) = cv::get(reg.deltas(), id) else {
        return Err(denied(NOT_CEO));
    };
    let corp = case.corp;
    let is_ceo =
        live_corp(reg, corp) && reg.corporations().get(corp).is_some_and(|c| c.ceo_id == me);
    if !is_ceo {
        return Err(denied(NOT_CEO));
    }
    // Check the type first: a kind not built yet stays activatable.
    if case.vote_type != cv::VOTE_KICK_MEMBER {
        return Err(
            if cv::sanctioned(reg.deltas(), corp, i64::MAX)
                .iter()
                .any(|s| s.case.id == id)
            {
                custom("That action cannot be put in effect yet.")
            } else {
                refusal(Refusal::NotSanctioned)
            },
        );
    }
    let now = crate::pyutil::filetime_now();
    // Check, run the effect, then mark: a refused expulsion leaves the case
    // there for the CEO to try again.
    let s = cv::check_put_in_effect(reg.deltas(), id, now).map_err(refusal)?;
    if reg
        .corporations()
        .membership(s.parameter)
        .is_none_or(|m| m.corporation_id != corp)
    {
        return Err(custom(
            "That pilot is no longer a member of the corporation.",
        ));
    }
    super::corp::expel(reg, me, s.parameter)?;
    cv::put_in_effect(reg.deltas_mut(), id, now).map_err(refusal)?;
    tell(
        reg,
        corp,
        id,
        &["OnSanctionedActionChanged", "OnVoteCaseChanged"],
    );
    Ok(PyValue::None)
}

/// `{status: CRowset}` for a corp, the shape `voteCases[status]` indexes.
fn get_cases(reg: &mut Registry, req: &Request<'_>, me: i64) -> PyValue {
    let a = Args::new(req);
    let corp = a.int_or_kw(0, "corporationID").unwrap_or(0);
    let now = crate::pyutil::filetime_now();
    let (open, closed) = if can_view(reg, corp, me) {
        (
            cv::cases(reg.deltas(), corp, true, now),
            cv::cases(reg.deltas(), corp, false, now),
        )
    } else {
        (vec![], vec![])
    };
    // A `Dict` pair is (value, key): the wire order.
    PyValue::Dict(vec![
        (cases_rowset(&closed), PyValue::Int(STATUS_CLOSED)),
        (cases_rowset(&open), PyValue::Int(STATUS_OPEN)),
    ])
}

fn case_of(reg: &mut Registry, me: i64, corp: i64, id: i64) -> Option<cv::Case> {
    if !can_view(reg, corp, me) {
        return None;
    }
    cv::get(reg.deltas(), id).filter(|c| c.corp == corp)
}

/// A label's path from a `(path, kwargs)` tuple, or a bare string.
fn label(v: Option<&PyValue>) -> String {
    match v {
        Some(PyValue::Tuple(t) | PyValue::List(t)) => label(t.first()),
        other => super::args::as_text(other).unwrap_or_default(),
    }
}

/// The options a proposal came with: texts for a general vote, the
/// `(yes, no, parameter, parameter1, parameter2)` tuple for the others.
fn parse_options(vote_type: i64, v: Option<&PyValue>) -> Vec<OptionSpec> {
    let items: &[PyValue] = match v {
        Some(PyValue::Tuple(t) | PyValue::List(t)) => t,
        _ => return vec![],
    };
    if vote_type == cv::VOTE_GENERAL {
        return items
            .iter()
            .map(|t| OptionSpec {
                text: label(Some(t)),
                ..OptionSpec::default()
            })
            .collect();
    }
    let n = |i: usize| super::args::as_int(items.get(i)).unwrap_or(0);
    let spec = |i: usize| OptionSpec {
        text: label(items.get(i)),
        parameter: n(2),
        parameter1: n(3),
        parameter2: n(4),
    };
    vec![spec(0), spec(1)]
}

fn default_title(vote_type: i64) -> &'static str {
    match vote_type {
        cv::VOTE_CEO => "New CEO",
        cv::VOTE_SHARES => "Creation of shares",
        cv::VOTE_KICK_MEMBER => "Expel member",
        cv::VOTE_ITEM_LOCKDOWN => "Lock down blueprint",
        cv::VOTE_ITEM_UNLOCK => "Unlock blueprint",
        _ => "",
    }
}

fn insert_case(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let Some(corp) = my_corp(reg, me) else {
        return Err(denied("You are not in a corporation."));
    };
    ensure_founded(reg, corp);
    let a = Args::new(req);
    let vote_type = a.int_or_kw(2, "voteType").unwrap_or(-1);
    let m = reg.corporations().membership(me);
    let is_ceo = reg.corporations().get(corp).is_some_and(|c| c.ceo_id == me);
    let director = is_ceo || m.is_some_and(|m| m.roles & super::corp::CORP_ROLE_DIRECTOR != 0);
    if vote_type == cv::VOTE_CEO {
        if is_ceo || weight(reg, corp, me) < 1 {
            return Err(denied("You cannot run for CEO."));
        }
    } else if !director {
        return Err(denied("Only Directors can propose votes."));
    }
    let mut title = a.text_or_kw(0, "voteCaseText").unwrap_or_default();
    if title.trim().is_empty() && vote_type != cv::VOTE_GENERAL {
        title = default_title(vote_type).to_owned();
    }
    let description = a.text_or_kw(1, "description").unwrap_or_default();
    let days = a.int_or_kw(4, "duration").unwrap_or(0);
    let mut options = parse_options(vote_type, a.get_or_kw(3, "options"));
    if vote_type == cv::VOTE_CEO {
        // The client always sends the proposer as the candidate
        // (`CreateVote`); a crafted call cannot name anyone else.
        for o in &mut options {
            o.parameter = me;
        }
    }
    if vote_type == cv::VOTE_KICK_MEMBER {
        // The member to expel must be in the corp and not its CEO **[A]**:
        // the client's picker offers members only.
        let target = options.first().map_or(0, |o| o.parameter);
        let in_corp = reg.corporations().members_of(corp).contains(&target);
        let target_is_ceo = reg
            .corporations()
            .get(corp)
            .is_some_and(|c| c.ceo_id == target);
        if !in_corp || target_is_ceo {
            return Err(custom("That pilot cannot be put to an expulsion vote."));
        }
    }
    let now = crate::pyutil::filetime_now();
    let id = cv::open(
        reg.deltas_mut(),
        &cv::Proposal {
            corp,
            creator: me,
            vote_type,
            title: &title,
            description: &description,
            days,
            options: &options,
        },
        now,
    )
    .map_err(refusal)?;
    tell(reg, corp, id, &["OnVoteCaseChanged"]);
    Ok(PyValue::None)
}

fn insert_vote(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    let corp = a.int_or_kw(0, "corporationID").unwrap_or(0);
    let id = a.int_or_kw(1, "voteCaseID").unwrap_or(0);
    let option = a.int_or_kw(2, "voteValue").unwrap_or(-1);
    // `weight` also issues the corp's shares if it has none yet.
    let _ = weight(reg, corp, me);
    if case_of(reg, me, corp, id).is_none() {
        return Err(refusal(Refusal::NoSuchCase));
    }
    if !live_corp(reg, corp) {
        return Err(refusal(Refusal::NoWeight));
    }
    let now = crate::pyutil::filetime_now();
    let ceo_of = ceo_of(reg, me);
    cv::vote(reg.deltas_mut(), id, me, &ceo_of, option, now).map_err(refusal)?;
    tell(reg, corp, id, &["OnVoteCast", "OnVoteCaseChanged"]);
    Ok(PyValue::None)
}

/// Carry out every closed vote that has not acted, once each (`now` is a
/// FILETIME). The case is marked before its effect runs, so an effect the
/// corp can no longer take (the candidate left) is skipped, not retried.
pub(crate) fn settle(reg: &mut Registry, now: i64) {
    // Read first: a call with nothing due must not dirty the world.
    if cv::pending(reg.deltas(), now).is_empty() {
        return;
    }
    for due in cv::take_due(reg.deltas_mut(), now) {
        let (corp, id) = (due.case.corp, due.case.id);
        match due.effect {
            cv::Effect::Nothing => {}
            cv::Effect::NewCeo { candidate } => {
                let old = reg.corporations().get(corp).map(|c| (c.ceo_id, c.deleted));
                let member = reg
                    .corporations()
                    .membership(candidate)
                    .is_some_and(|m| m.corporation_id == corp);
                match old {
                    Some((old, false)) if member && old != candidate => {
                        super::corp::hand_over(reg, corp, old, candidate);
                    }
                    _ => {
                        eprintln!(
                            "[votes] case {id}: new CEO {candidate} skipped: not a member of a live corp {corp}, or already its CEO"
                        );
                    }
                }
            }
            cv::Effect::Expel { target } => {
                eprintln!("[votes] case {id}: expelling {target} is a sanctioned action");
            }
            cv::Effect::CreateShares { shares } => {
                eprintln!(
                    "[votes] case {id}: creating {shares} shares awaits ActivateSanctionedAction"
                );
            }
            cv::Effect::LockItem { item, .. } => {
                eprintln!("[votes] case {id}: locking item {item} awaits ActivateSanctionedAction");
            }
            cv::Effect::UnlockItem { item, .. } => {
                eprintln!(
                    "[votes] case {id}: unlocking item {item} awaits ActivateSanctionedAction"
                );
            }
        }
        // Every settled case moved from open to closed, acted on or not.
        tell(reg, corp, id, &["OnVoteCaseChanged"]);
    }
}

/// A `voteManager` call.
pub(crate) fn vote_manager(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    settle(reg, crate::pyutil::filetime_now());
    let a = Args::new(req);
    let Some(me) = req.actor else {
        return match req.method {
            "CanViewVotes" | "CanRunForCEO" => Ok(PyValue::Bool(false)),
            "GetCorpSanctionedActions" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        };
    };
    match req.method {
        "CanViewVotes" => {
            let corp = a
                .int_or_kw(0, "corporationID")
                .filter(|c| *c != 0)
                .or_else(|| my_corp(reg, me))
                .unwrap_or(0);
            Ok(PyValue::Bool(can_view(reg, corp, me)))
        }
        "CanRunForCEO" => {
            let ok = my_corp(reg, me).is_some_and(|corp| {
                let is_ceo = reg.corporations().get(corp).is_some_and(|c| c.ceo_id == me);
                !is_ceo && weight(reg, corp, me) > 0
            });
            Ok(PyValue::Bool(ok))
        }
        "GetVoteCasesByCorporation" => Ok(get_cases(reg, req, me)),
        "GetVoteCase" => {
            let corp = a.int_or_kw(0, "corporationID").unwrap_or(0);
            let id = a.int_or_kw(1, "voteCaseID").unwrap_or(0);
            match case_of(reg, me, corp, id) {
                Some(c) => Ok(case_keyval(&c)),
                None => Err(refusal(Refusal::NoSuchCase)),
            }
        }
        "GetVoteCaseOptions" => {
            let corp = a.int_or_kw(0, "corporationID").unwrap_or(0);
            let id = a.int_or_kw(1, "voteCaseID").unwrap_or(0);
            let list = case_of(reg, me, corp, id)
                .map(|_| cv::options(reg.deltas(), id))
                .unwrap_or_default();
            Ok(options_rowset(id, &list))
        }
        "GetVotes" => {
            let id = a.int_or_kw(0, "voteCaseID").unwrap_or(0);
            let corp = cv::get(reg.deltas(), id).map_or(0, |c| c.corp);
            let mine = case_of(reg, me, corp, id)
                .and_then(|_| cv::cast_of(reg.deltas(), id, me))
                .map(|(option, w)| {
                    crate::pyutil::keyval(vec![
                        ("optionID", PyValue::Int(option)),
                        ("voteCaseID", PyValue::Int(id)),
                        ("characterID", PyValue::Int(me)),
                        ("weight", PyValue::Int(w)),
                    ])
                });
            Ok(PyValue::List(mine.into_iter().collect()))
        }
        "InsertVoteCase" => insert_case(reg, req, me),
        "InsertVote" => insert_vote(reg, req, me),
        "GetCorpSanctionedActions" => Ok(get_sanctioned(reg, me)),
        "ActivateSanctionedAction" => activate_sanctioned(reg, req, me),
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const ALICE: i64 = 90_000_811;
    const BOB: i64 = 90_000_812;
    const CAROL: i64 = 90_000_813;
    const DAVE: i64 = 90_000_814;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ..crate::model::Character::default()
        }
    }

    /// ALICE the CEO, BOB and CAROL members, DAVE outside; BOB holds 300.
    fn world() -> (Registry, i64) {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
                pilot(DAVE, "Dave"),
            ];
        });
        super::super::corp::dev_join(&mut reg, ALICE, "TST");
        super::super::corp::dev_join(&mut reg, BOB, "TST");
        super::super::corp::dev_join(&mut reg, CAROL, "TST");
        let corp = reg.corporations().membership(ALICE).unwrap().corporation_id;
        ensure_founded(&mut reg, corp);
        cs::move_shares(reg.deltas_mut(), corp, ALICE, BOB, 300).unwrap();
        (reg, corp)
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "voteManager",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn general(reg: &mut Registry, who: i64, days: i64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            who,
            "InsertVoteCase",
            &[
                u("Move HQ"),
                u("Closer to the gate"),
                i(cv::VOTE_GENERAL),
                PyValue::List(vec![u("Yes"), u("No")]),
                i(days),
            ],
        )
    }

    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    /// The single text a refusal carries (`notify` or `reason`).
    fn text_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        let PyValue::Dict(d) = &args[1] else {
            panic!("kwargs")
        };
        match &d[0].0 {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(t) => t.clone(),
            other => panic!("a text: {other:?}"),
        }
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .map(|p| (p.to_push().0, p.audience().unwrap_or_default()))
            .collect()
    }

    fn cases_now(reg: &Registry, corp: i64) -> Vec<cv::Case> {
        cv::cases(reg.deltas(), corp, true, crate::pyutil::filetime_now())
    }

    #[test]
    fn only_a_shareholder_can_view_votes() {
        let (mut reg, _) = world();
        for (who, want) in [(ALICE, true), (BOB, true), (CAROL, false), (DAVE, false)] {
            let r = call(&mut reg, who, "CanViewVotes", &[i(0)]);
            assert_eq!(r.unwrap(), PyValue::Bool(want), "pilot {who}");
        }
    }

    #[test]
    fn a_director_proposes_and_the_members_hear_of_it() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        let open = cases_now(&reg, corp);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "Move HQ");
        assert_eq!(cv::options(reg.deltas(), open[0].id).len(), 2);
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "OnVoteCaseChanged");
        for who in [ALICE, BOB, CAROL] {
            assert!(p[0].1.contains(&who), "{who} hears");
        }
        assert!(!p[0].1.contains(&DAVE));
    }

    #[test]
    fn a_plain_member_cannot_propose_but_may_run_for_ceo() {
        let (mut reg, corp) = world();
        let e = general(&mut reg, BOB, 3).unwrap_err();
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert!(cases_now(&reg, corp).is_empty());
        assert_eq!(
            call(&mut reg, BOB, "CanRunForCEO", &[]).unwrap(),
            PyValue::Bool(true)
        );
        assert_eq!(
            call(&mut reg, ALICE, "CanRunForCEO", &[]).unwrap(),
            PyValue::Bool(false)
        );
        // No shares, no run.
        assert_eq!(
            call(&mut reg, CAROL, "CanRunForCEO", &[]).unwrap(),
            PyValue::Bool(false)
        );
        let ceo_vote = |reg: &mut Registry, who| {
            call(
                reg,
                who,
                "InsertVoteCase",
                &[
                    u(""),
                    u(""),
                    i(cv::VOTE_CEO),
                    PyValue::Tuple(vec![
                        PyValue::Tuple(vec![u("UI/Some/Yes"), PyValue::Dict(vec![])]),
                        PyValue::Tuple(vec![u("UI/Some/No")]),
                        i(who),
                        PyValue::None,
                        PyValue::None,
                    ]),
                    i(1),
                ],
            )
        };
        assert_eq!(
            key_of(&ceo_vote(&mut reg, CAROL).unwrap_err()),
            "CrpAccessDenied"
        );
        ceo_vote(&mut reg, BOB).unwrap();
        let open = cases_now(&reg, corp);
        assert_eq!(open[0].vote_type, cv::VOTE_CEO);
        assert_eq!(open[0].title, "New CEO", "an empty title takes the type's");
        let o = cv::options(reg.deltas(), open[0].id);
        assert_eq!(
            (o[0].spec.text.as_str(), o[0].spec.parameter),
            ("UI/Some/Yes", BOB)
        );
    }

    #[test]
    fn a_vote_weighs_the_shares_and_is_cast_once() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        let id = cases_now(&reg, corp)[0].id;
        call(&mut reg, BOB, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(id), i(1)]).unwrap();
        let o = cv::options(reg.deltas(), id);
        assert_eq!((o[0].votes_for, o[1].votes_for), (300, 700));
        // Once only.
        let e = call(&mut reg, BOB, "InsertVote", &[i(corp), i(id), i(1)]).unwrap_err();
        assert_eq!(key_of(&e), "CustomNotify");
        assert_eq!(cv::options(reg.deltas(), id)[1].votes_for, 700);
        // GetVotes is the caller's own, empty before they vote.
        let PyValue::List(mine) = call(&mut reg, BOB, "GetVotes", &[i(id)]).unwrap() else {
            panic!("a list")
        };
        assert_eq!(mine.len(), 1);
        let PyValue::List(none) = call(&mut reg, CAROL, "GetVotes", &[i(id)]).unwrap() else {
            panic!("a list")
        };
        assert!(none.is_empty());
    }

    #[test]
    fn a_non_shareholder_cannot_vote_or_read_the_case() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        let id = cases_now(&reg, corp)[0].id;
        for who in [CAROL, DAVE] {
            let e = call(&mut reg, who, "InsertVote", &[i(corp), i(id), i(0)]).unwrap_err();
            assert_eq!(key_of(&e), "CustomNotify", "{who}");
            assert!(call(&mut reg, who, "GetVoteCase", &[i(corp), i(id)]).is_err());
        }
        assert!(cv::casts(reg.deltas(), id).is_empty());
    }

    #[test]
    fn a_vote_cast_pushes_to_the_corp_and_a_closed_vote_takes_no_more() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        let id = cases_now(&reg, corp)[0].id;
        pushes(&mut reg);
        call(&mut reg, BOB, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        let events: Vec<_> = pushes(&mut reg).into_iter().map(|p| p.0).collect();
        assert_eq!(events, ["OnVoteCast", "OnVoteCaseChanged"]);
        // A vote that ended a day ago.
        let past = crate::pyutil::filetime_now() - 10 * cv::DAY;
        let two = [
            OptionSpec {
                text: "A".into(),
                ..OptionSpec::default()
            },
            OptionSpec {
                text: "B".into(),
                ..OptionSpec::default()
            },
        ];
        let old = cv::open(
            reg.deltas_mut(),
            &cv::Proposal {
                corp,
                creator: ALICE,
                vote_type: cv::VOTE_GENERAL,
                title: "Old",
                description: "",
                days: 1,
                options: &two,
            },
            past,
        )
        .unwrap();
        let e = call(&mut reg, ALICE, "InsertVote", &[i(corp), i(old), i(0)]).unwrap_err();
        assert_eq!(key_of(&e), "CustomNotify");
        assert!(cv::casts(reg.deltas(), old).is_empty());
        assert_eq!(cases_now(&reg, corp).len(), 1, "the old one is closed");
    }

    #[test]
    fn expelling_needs_a_member_who_is_not_the_ceo() {
        let (mut reg, corp) = world();
        let kick = |reg: &mut Registry, target| {
            call(
                reg,
                ALICE,
                "InsertVoteCase",
                &[
                    u(""),
                    u(""),
                    i(cv::VOTE_KICK_MEMBER),
                    PyValue::Tuple(vec![
                        u("a"),
                        u("b"),
                        i(target),
                        PyValue::None,
                        PyValue::None,
                    ]),
                    i(2),
                ],
            )
        };
        assert!(kick(&mut reg, DAVE).is_err(), "not a member");
        assert!(kick(&mut reg, ALICE).is_err(), "the CEO");
        kick(&mut reg, CAROL).unwrap();
        assert_eq!(cases_now(&reg, corp).len(), 1);
    }

    #[test]
    fn the_lists_answer_in_the_shape_the_client_indexes() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        let id = cases_now(&reg, corp)[0].id;
        let PyValue::Dict(by_status) =
            call(&mut reg, BOB, "GetVoteCasesByCorporation", &[i(corp), i(0)]).unwrap()
        else {
            panic!("a dict of status to rowset")
        };
        let keys: Vec<_> = by_status.iter().map(|p| p.1.clone()).collect();
        assert_eq!(keys, [i(STATUS_CLOSED), i(STATUS_OPEN)]);
        assert!(matches!(
            call(&mut reg, BOB, "GetVoteCaseOptions", &[i(corp), i(id)]),
            Ok(PyValue::ObjectEx { .. })
        ));
        assert!(matches!(
            call(&mut reg, BOB, "GetVoteCase", &[i(corp), i(id)]),
            Ok(PyValue::Instance { .. })
        ));
        assert_eq!(
            call(&mut reg, BOB, "GetCorpSanctionedActions", &[]).unwrap(),
            PyValue::Dict(vec![])
        );
    }

    fn run_for_ceo(reg: &mut Registry, who: i64) {
        call(
            reg,
            who,
            "InsertVoteCase",
            &[
                u(""),
                u(""),
                i(cv::VOTE_CEO),
                PyValue::Tuple(vec![u("a"), u("b"), i(who), PyValue::None, PyValue::None]),
                i(2),
            ],
        )
        .unwrap();
    }

    fn ceo(reg: &Registry, corp: i64) -> i64 {
        reg.corporations().get(corp).unwrap().ceo_id
    }

    fn later() -> i64 {
        crate::pyutil::filetime_now() + 5 * cv::DAY
    }

    #[test]
    fn a_won_ceo_vote_changes_the_ceo_once() {
        let (mut reg, corp) = world();
        run_for_ceo(&mut reg, BOB);
        let id = cases_now(&reg, corp)[0].id;
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        settle(&mut reg, crate::pyutil::filetime_now());
        assert_eq!(ceo(&reg, corp), ALICE, "still open: nothing yet");
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), BOB);
        // The old CEO is a plain member now; a second settle changes nothing,
        // also when the seat is moved on by other means in between.
        super::super::corp::hand_over(&mut reg, corp, BOB, CAROL);
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), CAROL);
        assert!(cv::is_acted(reg.deltas(), id));
    }

    #[test]
    fn a_lost_or_empty_ceo_vote_changes_nothing() {
        let (mut reg, corp) = world();
        run_for_ceo(&mut reg, BOB);
        run_for_ceo(&mut reg, BOB);
        let (a, b) = (cases_now(&reg, corp)[0].id, cases_now(&reg, corp)[1].id);
        // Lost: the CEO's 700 against BOB.
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(a), i(1)]).unwrap();
        call(&mut reg, BOB, "InsertVote", &[i(corp), i(a), i(0)]).unwrap();
        // b gets no votes at all.
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), ALICE);
        assert!(cv::is_acted(reg.deltas(), a) && cv::is_acted(reg.deltas(), b));
    }

    #[test]
    fn settling_a_lost_vote_still_tells_the_clients_the_case_closed() {
        let (mut reg, corp) = world();
        run_for_ceo(&mut reg, BOB);
        let id = cases_now(&reg, corp)[0].id;
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(id), i(1)]).unwrap();
        pushes(&mut reg);
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), ALICE, "the vote lost");
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "OnVoteCaseChanged");
        settle(&mut reg, later());
        assert!(pushes(&mut reg).is_empty(), "told once");
    }

    #[test]
    fn a_ceo_vote_for_someone_who_left_is_skipped() {
        let (mut reg, corp) = world();
        run_for_ceo(&mut reg, BOB);
        let id = cases_now(&reg, corp)[0].id;
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        on_leave(&mut reg, BOB);
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), ALICE);
    }

    fn on_leave(reg: &mut Registry, who: i64) {
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "KickOutMember",
            args: &[PyValue::Int(who)],
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
        .unwrap();
    }

    #[test]
    fn the_candidate_is_the_proposer_whatever_the_call_says() {
        let (mut reg, corp) = world();
        call(
            &mut reg,
            BOB,
            "InsertVoteCase",
            &[
                u(""),
                u(""),
                i(cv::VOTE_CEO),
                PyValue::Tuple(vec![u("a"), u("b"), i(CAROL), PyValue::None, PyValue::None]),
                i(2),
            ],
        )
        .unwrap();
        let id = cases_now(&reg, corp)[0].id;
        let o = cv::options(reg.deltas(), id);
        assert_eq!((o[0].spec.parameter, o[1].spec.parameter), (BOB, BOB));
    }

    #[test]
    fn a_ceo_vote_below_the_quorum_changes_nothing() {
        let (mut reg, corp) = world();
        run_for_ceo(&mut reg, BOB);
        let id = cases_now(&reg, corp)[0].id;
        // BOB's 300 of 1000 for himself: the plurality, not a majority.
        call(&mut reg, BOB, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        settle(&mut reg, later());
        assert_eq!(ceo(&reg, corp), ALICE);
    }

    #[test]
    fn a_won_expulsion_waits_for_its_activation() {
        let (mut reg, corp) = world();
        call(
            &mut reg,
            ALICE,
            "InsertVoteCase",
            &[
                u(""),
                u(""),
                i(cv::VOTE_KICK_MEMBER),
                PyValue::Tuple(vec![u("a"), u("b"), i(CAROL), PyValue::None, PyValue::None]),
                i(2),
            ],
        )
        .unwrap();
        let id = cases_now(&reg, corp)[0].id;
        call(&mut reg, ALICE, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        settle(&mut reg, later());
        assert!(
            reg.corporations().membership(CAROL).is_some(),
            "not expelled by itself"
        );
        assert!(cv::is_acted(reg.deltas(), id));
    }

    #[test]
    fn a_call_with_nothing_due_does_not_dirty_the_world() {
        let (mut reg, corp) = world();
        general(&mut reg, ALICE, 3).unwrap();
        assert!(!cases_now(&reg, corp).is_empty());
        reg.dirty = false;
        settle(&mut reg, crate::pyutil::filetime_now());
        assert!(!reg.dirty);
    }

    /// A won expulsion of `target` (ALICE votes yes), closed and settled.
    fn won_kick(reg: &mut Registry, corp: i64, vote_type: i64, target: i64) -> i64 {
        call(
            reg,
            ALICE,
            "InsertVoteCase",
            &[
                u(""),
                u(""),
                i(vote_type),
                PyValue::Tuple(vec![
                    u("a"),
                    u("b"),
                    i(target),
                    PyValue::None,
                    PyValue::None,
                ]),
                i(1),
            ],
        )
        .unwrap();
        let id = cases_now(reg, corp).last().unwrap().id;
        call(reg, ALICE, "InsertVote", &[i(corp), i(id), i(0)]).unwrap();
        // Close it for real: the calls read the wall clock.
        let mut row = reg
            .deltas()
            .of(cv::KIND)
            .get(&vec!["case".to_owned(), id.to_string()])
            .unwrap()
            .clone();
        let past =
            (crate::pyutil::filetime_now() - cv::DAY.min(cv::SANCTION_WINDOW / 2)).to_string();
        row[4] = past;
        reg.deltas_mut()
            .put(cv::KIND, vec!["case".to_owned(), id.to_string()], row);
        settle(reg, crate::pyutil::filetime_now());
        id
    }

    /// The reply's rows as `(voteCaseID key, row)`.
    fn listed(reg: &mut Registry, who: i64) -> Vec<(PyValue, PyValue)> {
        let PyValue::Dict(rows) = call(reg, who, "GetCorpSanctionedActions", &[]).unwrap() else {
            panic!("a dict")
        };
        rows.into_iter().map(|(v, k)| (k, v)).collect()
    }

    fn attr(row: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = row else {
            panic!("a KeyVal")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("{name} missing"))
            .clone()
    }

    #[test]
    fn the_ceo_puts_a_won_expulsion_in_effect_and_the_member_leaves() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        let _ = pushes(&mut reg);
        assert!(reg.corporations().membership(CAROL).is_some());
        call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap();
        assert!(reg.corporations().membership(CAROL).is_none());
        let p = pushes(&mut reg);
        let names: Vec<&str> = p.iter().map(|x| x.0).collect();
        assert!(names.contains(&"OnSanctionedActionChanged"));
        assert!(names.contains(&"OnVoteCaseChanged"));
        for (_, to) in p.iter().filter(|x| x.0 == "OnSanctionedActionChanged") {
            assert!(to.contains(&ALICE) && to.contains(&BOB));
        }
    }

    #[test]
    fn a_director_who_is_not_ceo_is_refused_activation() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        reg.corporations_mut().member_mut(BOB).unwrap().roles =
            super::super::corp::CORP_ROLE_DIRECTOR;
        let e = call(&mut reg, BOB, "ActivateSanctionedAction", &[i(id)]).unwrap_err();
        assert_eq!(key_of(&e), "CrpAccessDenied");
        assert!(reg.corporations().membership(CAROL).is_some());
        // The case is still there to be put in effect by the CEO.
        call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap();
        assert!(reg.corporations().membership(CAROL).is_none());
    }

    #[test]
    fn a_second_activation_is_refused() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap();
        let e = call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap_err();
        assert_eq!(key_of(&e), "CustomNotify");
        assert_eq!(text_of(&e), "That action has already been put in effect.");
        let rows = listed(&mut reg, ALICE);
        assert_eq!(attr(&rows[0].1, "actedUpon"), i(1));
    }

    #[test]
    fn a_refused_expulsion_leaves_the_action_to_try_again() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        // The target is gone before the CEO acts.
        super::super::corp::expel(&mut reg, ALICE, CAROL).unwrap();
        let e = call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap_err();
        assert_eq!(
            text_of(&e),
            "That pilot is no longer a member of the corporation."
        );
        let rows = listed(&mut reg, ALICE);
        assert_eq!(attr(&rows[0].1, "actedUpon"), i(0));
        assert_eq!(attr(&rows[0].1, "inEffect"), i(0));
        assert!(
            cv::sanctioned(reg.deltas(), corp, i64::MAX)[0]
                .acted_at
                .is_none()
        );
        // Back as a plain member: the same action now goes through.
        super::super::corp::dev_join(&mut reg, CAROL, "TST");
        assert!(reg.corporations().membership(CAROL).is_some());
        call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap();
        assert!(reg.corporations().membership(CAROL).is_none());
        let rows = listed(&mut reg, ALICE);
        assert_eq!(attr(&rows[0].1, "actedUpon"), i(1));
    }

    #[test]
    fn an_unknown_case_and_another_corps_case_are_refused_alike() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        let a = call(&mut reg, DAVE, "ActivateSanctionedAction", &[i(id)]).unwrap_err();
        let b = call(&mut reg, DAVE, "ActivateSanctionedAction", &[i(id + 9_999)]).unwrap_err();
        assert_eq!(key_of(&a), key_of(&b));
        assert_eq!(text_of(&a), text_of(&b));
        assert!(reg.corporations().membership(CAROL).is_some());
    }

    #[test]
    fn a_won_shares_vote_cannot_be_put_in_effect_yet_and_stays_listed_not_acted() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_SHARES, 50);
        let e = call(&mut reg, ALICE, "ActivateSanctionedAction", &[i(id)]).unwrap_err();
        assert_eq!(key_of(&e), "CustomNotify");
        assert_eq!(text_of(&e), "That action cannot be put in effect yet.");
        let rows = listed(&mut reg, ALICE);
        assert_eq!(rows.len(), 1);
        assert_eq!(attr(&rows[0].1, "actedUpon"), i(0));
        assert_eq!(attr(&rows[0].1, "timeActedUpon"), PyValue::None);
        assert!(
            cv::sanctioned(reg.deltas(), corp, i64::MAX)[0]
                .acted_at
                .is_none()
        );
    }

    #[test]
    fn get_corp_sanctioned_actions_answers_value_key_pairs_with_int_flags() {
        let (mut reg, corp) = world();
        let id = won_kick(&mut reg, corp, cv::VOTE_KICK_MEMBER, CAROL);
        let PyValue::Dict(pairs) = call(&mut reg, ALICE, "GetCorpSanctionedActions", &[]).unwrap()
        else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, i(id), "the key is the case id");
        let row = &pairs[0].0;
        assert_eq!(attr(row, "voteCaseID"), i(id));
        assert_eq!(attr(row, "voteType"), i(cv::VOTE_KICK_MEMBER));
        assert_eq!(attr(row, "optionID"), i(0));
        assert_eq!(attr(row, "parameter"), i(CAROL));
        assert_eq!(attr(row, "actedUpon"), i(0));
        assert_eq!(attr(row, "inEffect"), i(0));
        assert_eq!(attr(row, "timeActedUpon"), PyValue::None);
        assert_eq!(attr(row, "timeRescended"), PyValue::None);
        let case = cv::get(reg.deltas(), id).unwrap();
        assert_eq!(attr(row, "expires"), i(case.end + cv::SANCTION_WINDOW));
        // A pilot who is not a shareholder or CEO sees none.
        assert!(listed(&mut reg, DAVE).is_empty());
        assert!(listed(&mut reg, CAROL).is_empty());
    }
}
