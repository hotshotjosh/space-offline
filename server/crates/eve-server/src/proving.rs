//! **Proving Grounds, the read side** (issue #64, PG-1b): the event calendar
//! the client's `pvpFilamentSvc` asks `pvpFilamentMgr` for, and the empty
//! leaderboard and statistics it expects pushed back.
//!
//! The wire arms are `service/proving.rs`; everything they decide is here.
//!
//! ## What the client reads
//!
//! - `GetAllEvents` and `GetActiveEvents` answer a **dict**; the client calls
//!   `.values()` on it **[V]** `pvpFilaments/client/pvpFilamentSvc.py`
//!   (`GetAllEvents`, `GetActiveEvents`).
//! - An event is a `KeyVal` with the 25 keys of [`INFO_KEYS`] **[V]**
//!   `pvpFilaments/common/util.py` `_construct_info`.
//! - The window's controller reads `GetActiveEvents()` and, when the list is
//!   empty, never binds `sortedEvents` and dies with an `UnboundLocalError`
//!   **[V]** `event_window/controller.py` (`__init__`). So the active answer is
//!   **never empty**: [`placeholder`] stands in, a QA event the window hides.
//! - Every event the window asks about must be in `GetAllEvents`, or
//!   `GetAllEventDataByEventID()[GetEventID(info)]` is a `KeyError`
//!   **[V]** `pvpFilamentSvc.py`. The placeholder is in it whenever it is
//!   served.
//! - `GetLeaderboard(matchTypeID, scheduleID)` and
//!   `GetCharacterStatistics(matchTypeID, scheduleID)` ignore the answer and
//!   wait for the push `OnPVPFilamentsLeaderboard` /
//!   `OnPVPFilamentsCharacterStatistics` **[V]** `pvpFilamentSvc.py`. Without
//!   it `fetchingLeaderboard` stays set and the window never fills. A rank of
//!   0 means "no rank" **[V]** `event_window/character_statistics.py`
//!   (`update_statistics`).
//!
//! ## When an event is active
//!
//! The client's own rules, read from `util.py` **[V]** and kept as they are,
//! quirks included:
//!
//! - a QA row is always active, never over, never "next";
//! - the day a window opens it starts at downtime, the day it closes it ends
//!   at downtime (`is_downtime_done_today`);
//! - downtime is 11:00 UTC ([`crate::seed::ROLLOVER`], `gametime/downtime.py`);
//! - "the next date" is the start of the first row, by start date, whose start
//!   is after today **or** whose window straddles today (`_next_date`); a
//!   window that starts today is not "next".
//!
//! ## Match types
//!
//! A schedule row says *when*; the match type says *what* (filament, fleets,
//! restrictions, rewards). The match types are client data and arrive with
//! PG-1a (`proving/match_types.tsv`, not yet in the bundle). Until then the
//! catalogue is built with **none**: an event gets a blank match type (id 0,
//! no filament), so [`Catalogue::by_filament`] finds nothing and nothing
//! panics. [`Catalogue::new`] takes the match types as a parameter so that
//! wiring the bundle in is a change in [`load_table`] alone.
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **A blank match type sends `None` for every id the client resolves**
//!   (`eventNameID`, the hint ids, the list ids). Falsified by a traceback in
//!   the Proving Grounds window naming `GetByMessageID` or `GetTypeIDsByListID`
//!   (then send `0`, or load the real match types).
//! - **A `datetime.date` goes over the wire in its `__reduce__` form**: the
//!   callable `datetime.date` (whitelisted **[V]** `eve_rowset::whitelist`) and
//!   one 4-byte state string **[I]**. Falsified by a traceback in
//!   `datetime_to_filetime` or an unmarshal error when the window opens.
//! - **The shipped row is a 2x1 QA event** (`content/pve/proving-events.tsv`):
//!   always active, hidden from the window's list. Falsified by the client
//!   listing the QA event in Neocom (then QA is not what hides it), or the
//!   maintainer wanting a real schedule.
//! - **Join and Leave refuse with code 33 (event not active)** until PG-1c.
//!   Falsified by a Leave raising a dialog for a pilot who is in a queue (it
//!   cannot happen: there is no queue yet).
//! - **A match type is chosen by format and a word of the filament's name**
//!   ([`Catalogue::resolve`]). Falsified by PG-1a's table naming the schedule
//!   in a column of its own.
//! - **The placeholder is dated today**, so `datetime_to_filetime` has a date
//!   to read. Falsified by the window showing the placeholder's dates.

use crate::authored::Row;
use crate::content::{Boxed, Sources, TableSpec};
use crate::notify::Push;
use crate::pyutil::{bstr, dict_kv, keyval, pydict};
use eve_marshal::{ObjectForm, PyValue};

/// The services this module answers (`service::DOMAINS`, coordinator-only).
pub const SERVICES: &[&str] = &["pvpFilamentMgr"];

/// The authored schedule, relative to `--authored`.
pub const FILE: &str = "pve/proving-events.tsv";

/// Its header, copied into `docs/content/schema.md`.
pub const HEADER: &str = "schedule_id\tformat\tname_has\tstart\tend\tqa\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/pve/proving-events.tsv");

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "proving_events",
    load: load_table,
    fixture: || Box::new(Catalogue::default()) as Boxed,
};

/// The schedule rows; the match types are PG-1a's and not here yet, so none.
fn load_table(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let schedules = file.parse_rows(Schedule::from_row);
    Ok(Box::new(Catalogue::new(schedules, Vec::new())))
}

/// The `PVPJumpError` code for "the event is not active" **[V]**
/// `pvpFilaments/error.py` (`EVENT_NOT_ACTIVE = 33`).
pub const EVENT_NOT_ACTIVE: i64 = 33;

/// The `UserError` message the client matches **[V]**
/// `activation_window/pvpFilamentController.py` (`JoinQueue`:
/// `error.msg == 'PVPJumpError'`, `error.dict['errors']`).
pub const ERROR: &str = "PVPJumpError";

/// The keys of an event, in the order `_construct_info` builds them **[V]**.
pub const INFO_KEYS: [&str; 25] = [
    "matchTypeID",
    "scheduleID",
    "filamentTypeID",
    "numberOfFleets",
    "fleetSize",
    "shipRestrictions",
    "rewards",
    "eventNameID",
    "eventDescriptionID",
    "filamentDescriptionID",
    "restrictionsDescriptionID",
    "rewardsHintID",
    "shipRestrictionsHintID",
    "rulesHintID",
    "startDate",
    "endDate",
    "QA",
    "moduleAndImplantMetaLevelRestrictions",
    "maxOfShipTypeRestriction",
    "maxOfShipGroupRestriction",
    "minimumQueueSizeMultiplier",
    "provingGroundLootTableID",
    "bannedModulesTypeListID",
    "limitedAmountOfModulesTypeListID",
    "numberOfLimitedModules",
];

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

/// A calendar date `(year, month, day)`. Tuples compare as dates do.
pub type Day = (i64, i64, i64);

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// `YYYY-MM-DD` as a date; `None` for anything else (a bad month or day
/// included, as Python's `datetime.date` refuses them).
pub fn parse_day(s: &str) -> Option<Day> {
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return None;
    }
    (1..=days_in_month(y, m)).contains(&d).then_some((y, m, d))
}

/// Today's date (UTC) for the blue FILETIME `now`.
pub fn today(now: i64) -> Day {
    crate::ledger::civil_from_filetime(now)
}

/// Has today's downtime passed? Downtime is 11:00 UTC **[V]**
/// `gametime/downtime.py` (`is_downtime_done_today`).
pub fn downtime_done(now: i64) -> bool {
    now.rem_euclid(crate::seed::DAY) >= crate::seed::ROLLOVER
}

/// A `datetime.date` as the client unmarshals it: `date(<4-byte state>)`, the
/// `__reduce__` form **[I]** (module doc).
pub fn date((y, m, d): Day) -> PyValue {
    let year = u16::try_from(y).unwrap_or(1);
    let state = vec![
        (year >> 8) as u8,
        (year & 0xff) as u8,
        u8::try_from(m).unwrap_or(1),
        u8::try_from(d).unwrap_or(1),
    ];
    PyValue::ObjectEx {
        form: ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"datetime.date".to_vec()),
            PyValue::Tuple(vec![PyValue::Bytes(state)]),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// The data
// ---------------------------------------------------------------------------

/// A match type: what an event is played with. Every id is optional because
/// the client's loader leaves some unset **[I]**; a blank one has none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchType {
    /// `matchTypeID`; 0 for [`MatchType::blank`].
    pub id: i64,
    /// The filament type; 0 for none.
    pub filament_type_id: i64,
    /// A word of the filament's name, lower case, for [`Catalogue::resolve`].
    pub filament_name: String,
    pub number_of_fleets: i64,
    pub fleet_size: i64,
    pub ship_restrictions: Option<i64>,
    /// `(typeID, amount)`.
    pub rewards: Vec<(i64, i64)>,
    pub event_name_id: Option<i64>,
    pub event_description_id: Option<i64>,
    pub filament_description_id: Option<i64>,
    pub restrictions_description_id: Option<i64>,
    pub rewards_hint_id: Option<i64>,
    pub ship_restrictions_hint_id: Option<i64>,
    pub rules_hint_id: Option<i64>,
    pub module_and_implant_meta_level_restrictions: Option<i64>,
    pub max_of_ship_type_restriction: Option<i64>,
    pub max_of_ship_group_restriction: Option<i64>,
    pub minimum_queue_size_multiplier: Option<f64>,
    pub proving_ground_loot_table_id: Option<i64>,
    pub banned_modules_type_list_id: Option<i64>,
    pub limited_amount_of_modules_type_list_id: Option<i64>,
    pub number_of_limited_modules: Option<i64>,
}

impl MatchType {
    /// The match type of an event whose real one is not loaded **[A]**.
    pub fn blank(number_of_fleets: i64, fleet_size: i64) -> MatchType {
        MatchType {
            number_of_fleets,
            fleet_size,
            ..MatchType::default()
        }
    }
}

/// One row of the authored schedule.
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub id: i64,
    pub fleets: i64,
    pub fleet_size: i64,
    /// A word of the filament's name, lower case, or empty.
    pub name_has: String,
    pub start: Option<Day>,
    pub end: Option<Day>,
    /// The start as written, which `GetNextEventDate` returns as is.
    pub start_text: String,
    pub qa: bool,
}

impl Schedule {
    /// A row of `content/pve/proving-events.tsv`.
    pub fn from_row(r: &Row) -> Result<Schedule, String> {
        let id = r.i64(0)?;
        if id < 1 {
            return Err(format!("schedule_id {id} is not a positive id"));
        }
        let bad_format = || format!("format {:?} is not FLEETSxSIZE", r.field(1));
        let (f, s) = r.field(1).split_once('x').ok_or_else(bad_format)?;
        let fleets: i64 = f.parse().map_err(|_| bad_format())?;
        let fleet_size: i64 = s.parse().map_err(|_| bad_format())?;
        if fleets < 1 || fleet_size < 1 {
            return Err(format!("format {:?} has a zero side", r.field(1)));
        }
        let qa = r.bool(5)?;
        let day = |i: usize| -> Result<Option<Day>, String> {
            let t = r.field(i);
            if t.is_empty() {
                return Ok(None);
            }
            parse_day(t)
                .map(Some)
                .ok_or_else(|| format!("{t:?} is not a YYYY-MM-DD date"))
        };
        let (start, end) = (day(3)?, day(4)?);
        if !qa && (start.is_none() || end.is_none()) {
            return Err("a row that is not QA needs a start and an end".to_owned());
        }
        if let (Some(a), Some(b)) = (start, end)
            && b < a
        {
            return Err("the end is before the start".to_owned());
        }
        Ok(Schedule {
            id,
            fleets,
            fleet_size,
            name_has: r.field(2).trim().to_lowercase(),
            start,
            end,
            start_text: r.field(3).to_owned(),
            qa,
        })
    }
}

/// A schedule row with its match type: what the client calls an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub schedule: Schedule,
    pub match_type: MatchType,
}

impl Event {
    /// The id the client keys an event by **[V]** `util.GetEventID`.
    pub fn event_id(&self) -> String {
        format!("{}_{}", self.match_type.id, self.schedule.id)
    }
}

/// The schedule and the match types, built once at start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalogue {
    schedules: Vec<Schedule>,
    match_types: Vec<MatchType>,
}

impl Catalogue {
    /// The catalogue of these rows. Schedules are kept in id order; a repeated
    /// id keeps its **first** row. Match types are kept in id order.
    pub fn new(schedules: Vec<Schedule>, mut match_types: Vec<MatchType>) -> Catalogue {
        let mut kept: Vec<Schedule> = Vec::new();
        for s in schedules {
            if !kept.iter().any(|k| k.id == s.id) {
                kept.push(s);
            }
        }
        kept.sort_by_key(|s| s.id);
        match_types.sort_by_key(|m| m.id);
        Catalogue {
            schedules: kept,
            match_types,
        }
    }

    /// The catalogue of authored text. A bad header or row is an error naming
    /// its line (the loader instead skips a bad row and says so).
    pub fn parse(text: &str, match_types: Vec<MatchType>) -> Result<Catalogue, String> {
        let (rows, _) = crate::authored::parse(text, HEADER)?;
        let mut out = Vec::new();
        for r in &rows {
            out.push(
                Schedule::from_row(r)
                    .map_err(|e| format!("proving-events line {}: {e}", r.line))?,
            );
        }
        Ok(Catalogue::new(out, match_types))
    }

    /// How many schedule rows there are.
    pub fn len(&self) -> usize {
        self.schedules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.schedules.is_empty()
    }

    /// The match type of a schedule row **[A]**: the first whose format is the
    /// row's and whose filament name has the row's word; else a blank one.
    pub fn resolve(&self, s: &Schedule) -> MatchType {
        self.match_types
            .iter()
            .find(|m| {
                m.number_of_fleets == s.fleets
                    && m.fleet_size == s.fleet_size
                    && (s.name_has.is_empty() || m.filament_name.contains(&s.name_has))
            })
            .cloned()
            .unwrap_or_else(|| MatchType::blank(s.fleets, s.fleet_size))
    }

    fn event(&self, s: &Schedule) -> Event {
        Event {
            schedule: s.clone(),
            match_type: self.resolve(s),
        }
    }

    /// Every event **[V]** `util.get_all_events` (the client's region test is
    /// `boot.region`; every row here is `ccp`), with the [`placeholder`] added
    /// when [`Catalogue::active_or_placeholder`] serves it, so the client can
    /// find any event it was given.
    pub fn all(&self, now: i64) -> Vec<Event> {
        let mut v: Vec<Event> = self.schedules.iter().map(|s| self.event(s)).collect();
        if self.active(now).is_empty() {
            v.push(placeholder(now));
        }
        v
    }

    /// Events active at `now` **[V]** `util._is_event_active`.
    pub fn active(&self, now: i64) -> Vec<Event> {
        let (today, done) = (today(now), downtime_done(now));
        self.schedules
            .iter()
            .filter(|s| is_active(s, today, done))
            .map(|s| self.event(s))
            .collect()
    }

    /// [`Catalogue::active`], and the [`placeholder`] when it is empty: the
    /// answer to `GetActiveEvents`, which must never be empty (module doc).
    pub fn active_or_placeholder(&self, now: i64) -> Vec<Event> {
        let a = self.active(now);
        if a.is_empty() {
            vec![placeholder(now)]
        } else {
            a
        }
    }

    /// Events that are over at `now` **[V]** `util._is_event_over`.
    pub fn over(&self, now: i64) -> Vec<Event> {
        let (today, done) = (today(now), downtime_done(now));
        self.schedules
            .iter()
            .filter(|s| is_over(s, today, done))
            .map(|s| self.event(s))
            .collect()
    }

    /// The event that ended last **[V]** `util.get_most_recent_event`.
    pub fn most_recent(&self, now: i64) -> Option<Event> {
        let (today, done) = (today(now), downtime_done(now));
        let mut by_end: Vec<&Schedule> = self.schedules.iter().collect();
        by_end.sort_by(|a, b| b.end.cmp(&a.end));
        by_end
            .into_iter()
            .find(|s| is_over(s, today, done))
            .map(|s| self.event(s))
    }

    /// The date `GetNextEventDate` answers, as written in the row **[V]**
    /// `util.get_next_active_event_date`.
    pub fn next_date(&self, now: i64) -> Option<String> {
        let today = today(now);
        let mut by_start: Vec<&Schedule> = self.schedules.iter().collect();
        by_start.sort_by(|a, b| a.start.cmp(&b.start));
        by_start.into_iter().find_map(|s| {
            if s.qa {
                return None;
            }
            let (start, end) = (s.start?, s.end?);
            ((start < today && end > today) || start > today).then(|| s.start_text.clone())
        })
    }

    /// The active event of a filament type **[V]** `util.get_event_by_filament`.
    /// A blank match type has no filament, so type 0 finds nothing.
    pub fn by_filament(&self, now: i64, filament_type_id: i64) -> Option<Event> {
        if filament_type_id == 0 {
            return None;
        }
        self.active(now)
            .into_iter()
            .find(|e| e.match_type.filament_type_id == filament_type_id)
    }

    /// Is `(match type, schedule)` an event the client can have been given?
    pub fn known(&self, now: i64, match_type_id: i64, schedule_id: i64) -> bool {
        self.all(now)
            .iter()
            .any(|e| e.match_type.id == match_type_id && e.schedule.id == schedule_id)
    }
}

fn is_active(s: &Schedule, today: Day, done: bool) -> bool {
    if s.qa {
        return true;
    }
    let (Some(start), Some(end)) = (s.start, s.end) else {
        return false;
    };
    if start > today || end < today {
        return false;
    }
    if start == today && !done {
        return false;
    }
    !(end == today && done)
}

fn is_over(s: &Schedule, today: Day, done: bool) -> bool {
    if s.qa {
        return false;
    }
    let Some(end) = s.end else {
        return false;
    };
    end < today || (end == today && done)
}

/// The event served when none is active: QA (the window hides it), no
/// filament, dated today **[A]** (module doc).
pub fn placeholder(now: i64) -> Event {
    let t = today(now);
    Event {
        schedule: Schedule {
            id: 0,
            fleets: 0,
            fleet_size: 0,
            name_has: String::new(),
            start: Some(t),
            end: Some(t),
            start_text: String::new(),
            qa: true,
        },
        match_type: MatchType::blank(0, 0),
    }
}

// ---------------------------------------------------------------------------
// The wire values
// ---------------------------------------------------------------------------

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// An event as the client's `KeyVal`, every key of [`INFO_KEYS`].
pub fn event_info(e: &Event) -> PyValue {
    let m = &e.match_type;
    let s = &e.schedule;
    let rewards = m
        .rewards
        .iter()
        .map(|(t, a)| {
            keyval(vec![
                ("typeID", PyValue::Int(*t)),
                ("amount", PyValue::Int(*a)),
            ])
        })
        .collect();
    let day = |d: Option<Day>| d.map_or(PyValue::None, date);
    keyval(vec![
        ("matchTypeID", PyValue::Int(m.id)),
        ("scheduleID", PyValue::Int(s.id)),
        ("filamentTypeID", PyValue::Int(m.filament_type_id)),
        ("numberOfFleets", PyValue::Int(m.number_of_fleets)),
        ("fleetSize", PyValue::Int(m.fleet_size)),
        ("shipRestrictions", opt(m.ship_restrictions)),
        ("rewards", PyValue::List(rewards)),
        ("eventNameID", opt(m.event_name_id)),
        ("eventDescriptionID", opt(m.event_description_id)),
        ("filamentDescriptionID", opt(m.filament_description_id)),
        (
            "restrictionsDescriptionID",
            opt(m.restrictions_description_id),
        ),
        ("rewardsHintID", opt(m.rewards_hint_id)),
        ("shipRestrictionsHintID", opt(m.ship_restrictions_hint_id)),
        ("rulesHintID", opt(m.rules_hint_id)),
        ("startDate", day(s.start)),
        ("endDate", day(s.end)),
        ("QA", PyValue::Int(i64::from(s.qa))),
        (
            "moduleAndImplantMetaLevelRestrictions",
            opt(m.module_and_implant_meta_level_restrictions),
        ),
        (
            "maxOfShipTypeRestriction",
            opt(m.max_of_ship_type_restriction),
        ),
        (
            "maxOfShipGroupRestriction",
            opt(m.max_of_ship_group_restriction),
        ),
        (
            "minimumQueueSizeMultiplier",
            m.minimum_queue_size_multiplier
                .map_or(PyValue::None, PyValue::Real),
        ),
        (
            "provingGroundLootTableID",
            opt(m.proving_ground_loot_table_id),
        ),
        (
            "bannedModulesTypeListID",
            opt(m.banned_modules_type_list_id),
        ),
        (
            "limitedAmountOfModulesTypeListID",
            opt(m.limited_amount_of_modules_type_list_id),
        ),
        ("numberOfLimitedModules", opt(m.number_of_limited_modules)),
    ])
}

/// Events as the dict the client calls `.values()` on, keyed by
/// [`Event::event_id`].
pub fn events_dict(events: &[Event]) -> PyValue {
    dict_kv(events.iter().map(|e| (bstr(&e.event_id()), event_info(e))))
}

/// `GetAllEvents`.
pub fn all_events(cat: &Catalogue, now: i64) -> PyValue {
    events_dict(&cat.all(now))
}

/// `GetActiveEvents`: never empty.
pub fn active_events(cat: &Catalogue, now: i64) -> PyValue {
    events_dict(&cat.active_or_placeholder(now))
}

/// `GetMostRecentEvent`: the event or `None`.
pub fn most_recent_event(cat: &Catalogue, now: i64) -> PyValue {
    cat.most_recent(now)
        .map_or(PyValue::None, |e| event_info(&e))
}

/// `GetNextEventDate`: the date as written, or `None`.
pub fn next_event_date(cat: &Catalogue, now: i64) -> PyValue {
    cat.next_date(now).map_or(PyValue::None, |d| bstr(&d))
}

/// The refusal a join gets until PG-1c: `UserError('PVPJumpError',
/// {'errors': [(code, ())]})` **[V]** `pvpFilamentController.py` (`JoinQueue`).
pub fn refusal(code: i64) -> crate::service::ServiceError {
    crate::service::ServiceError::Raise(eve_proto::user_error(
        ERROR,
        crate::pyutil::pypairs(vec![(
            "errors",
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(code),
                PyValue::Tuple(Vec::new()),
            ])]),
        )]),
    ))
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnPVPFilamentsLeaderboard(info)`: `info['entries']`, keyed by event
/// **[V]** `pvpFilamentSvc.OnPVPFilamentsLeaderboard`. Empty: nobody has played.
#[derive(Debug, Clone)]
pub struct LeaderboardPush {
    pub match_type_id: i64,
    pub schedule_id: i64,
}

impl Push for LeaderboardPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnPVPFilamentsLeaderboard",
            vec![pydict(vec![
                ("matchTypeID", PyValue::Int(self.match_type_id)),
                ("scheduleID", PyValue::Int(self.schedule_id)),
                ("entries", PyValue::List(Vec::new())),
            ])],
        )
    }
}

/// `OnPVPFilamentsCharacterStatistics(info)`: `info['statistics']` with
/// `rank`, `wins`, `losses` and `draws` **[V]** `character_statistics.py`.
/// All zero: rank 0 is "unranked" and stops the client asking for the
/// leaderboard again.
#[derive(Debug, Clone)]
pub struct StatisticsPush {
    pub match_type_id: i64,
    pub schedule_id: i64,
}

impl Push for StatisticsPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnPVPFilamentsCharacterStatistics",
            vec![pydict(vec![
                ("matchTypeID", PyValue::Int(self.match_type_id)),
                ("scheduleID", PyValue::Int(self.schedule_id)),
                (
                    "statistics",
                    pydict(vec![
                        ("rank", PyValue::Int(0)),
                        ("wins", PyValue::Int(0)),
                        ("losses", PyValue::Int(0)),
                        ("draws", PyValue::Int(0)),
                    ]),
                ),
            ])],
        )
    }
}

/// Queue the empty leaderboard for `(match type, schedule)` if the client can
/// know it. Returns whether it was queued.
pub fn push_leaderboard(
    reg: &mut crate::service::Registry,
    cat: &Catalogue,
    now: i64,
    match_type_id: i64,
    schedule_id: i64,
) -> bool {
    let known = cat.known(now, match_type_id, schedule_id);
    if known {
        reg.push(LeaderboardPush {
            match_type_id,
            schedule_id,
        });
    }
    known
}

/// Queue the zero statistics for `(match type, schedule)` if the client can
/// know it. Returns whether it was queued.
pub fn push_statistics(
    reg: &mut crate::service::Registry,
    cat: &Catalogue,
    now: i64,
    match_type_id: i64,
    schedule_id: i64,
) -> bool {
    let known = cat.known(now, match_type_id, schedule_id);
    if known {
        reg.push(StatisticsPush {
            match_type_id,
            schedule_id,
        });
    }
    known
}

#[cfg(test)]
mod tests;
