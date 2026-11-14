//! The Proving Grounds read-side tests. **Every id and date here is
//! invented**: schedules are 880,xxx, match types 881,xxx, filament types
//! 990,880,xxx, the years 2000, 2999, 3001 and 3002. The client's rules appear as rules
//! (downtime at 11:00 UTC, the active window) and its key names as names.

use super::*;
use crate::pyutil::dict_get;
use crate::service::{Registry, Request, ServiceError};

const SCHED_A: i64 = 880_001;
const SCHED_B: i64 = 880_002;
const SCHED_C: i64 = 880_003;
const MATCH_X: i64 = 881_001;
const MATCH_Y: i64 = 881_002;
const FIL_X: i64 = 990_880_001;
const FIL_Y: i64 = 990_880_002;

const HOUR: i64 = crate::seed::HOUR;
const DAY: i64 = crate::seed::DAY;
const DAYS_1601_TO_1970: i64 = 134_774;

/// The FILETIME of `y-m-d hh:00` UTC, by Hinnant's `days_from_civil`: an
/// oracle that shares no code with the module.
fn ft(y: i64, m: i64, d: i64, h: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468 + DAYS_1601_TO_1970) * DAY + h * HOUR
}

fn header() -> String {
    format!("{HEADER}\n")
}

/// One authored row: id, format, name word, start, end, qa.
fn row(id: i64, format: &str, name: &str, start: &str, end: &str, qa: u8) -> String {
    format!("{id}\t{format}\t{name}\t{start}\t{end}\t{qa}\ttest\n")
}

/// A match type for each format the tests use, so a row that is not QA is
/// served (a row with no match type is dropped).
fn types() -> Vec<MatchType> {
    [(2, 1), (4, 4), (4, 1), (3, 5)]
        .iter()
        .zip(MATCH_X..)
        .map(|(&(f, z), id)| match_type(id, 0, "", f, z))
        .collect()
}

fn catalogue(rows: &[String]) -> Catalogue {
    Catalogue::parse(&format!("{}{}", header(), rows.concat()), types())
        .expect("the test rows parse")
}

fn match_type(id: i64, filament: i64, name: &str, fleets: i64, size: i64) -> MatchType {
    MatchType {
        id,
        filament_type_id: filament,
        filament_name: name.to_owned(),
        number_of_fleets: fleets,
        fleet_size: size,
        rewards: vec![(990_880_900, 3)],
        ..MatchType::default()
    }
}

/// A window from 3001-03-10 to 3001-03-12, not QA.
fn window() -> Catalogue {
    catalogue(&[row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 0)])
}

fn get<'a>(info: &'a PyValue, key: &str) -> &'a PyValue {
    match info {
        PyValue::Instance { state, .. } => dict_get(state, key.as_bytes()),
        other => dict_get(other, key.as_bytes()),
    }
    .unwrap_or_else(|| panic!("no key {key} in {info:?}"))
}

fn values(dict: &PyValue) -> Vec<&PyValue> {
    match dict {
        PyValue::Dict(e) => e.iter().map(|(v, _)| v).collect(),
        other => panic!("not a dict: {other:?}"),
    }
}

fn world(cat: Catalogue) -> Registry {
    let mut content = crate::content::Content::fixture();
    content.insert("proving_events", cat);
    Registry::new(crate::model::SandboxConfig::default()).with_content(std::sync::Arc::new(content))
}

fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service: "pvpFilamentMgr",
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: None,
    })
}

fn pushed_names(reg: &mut Registry) -> Vec<&'static str> {
    reg.take_pending_raw()
        .iter()
        .map(|(_, p)| p.to_push().0)
        .collect()
}

// ---- the active window -----------------------------------------------------

#[test]
fn active_spans_downtime_to_downtime() {
    let c = window();
    let on = |y, m, d, h| !c.active(ft(y, m, d, h)).is_empty();
    assert!(!on(3001, 3, 9, 12), "the day before");
    assert!(!on(3001, 3, 10, 10), "opening day, before downtime");
    assert!(on(3001, 3, 10, 11), "opening day, at downtime");
    assert!(on(3001, 3, 11, 3), "the middle");
    assert!(on(3001, 3, 12, 10), "closing day, before downtime");
    assert!(!on(3001, 3, 12, 11), "closing day, at downtime");
    assert!(!on(3001, 3, 13, 12), "the day after");
}

#[test]
fn downtime_is_eleven_utc() {
    assert!(!downtime_done(ft(3001, 5, 5, 10) + HOUR - 1));
    assert!(downtime_done(ft(3001, 5, 5, 11)));
    assert!(downtime_done(ft(3001, 5, 5, 23)));
    assert!(!downtime_done(ft(3001, 5, 6, 0)));
}

#[test]
fn qa_is_always_active_and_never_over_or_next() {
    let c = catalogue(&[row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 1)]);
    for now in [ft(2000, 1, 1, 0), ft(3001, 3, 11, 12), ft(3999, 1, 1, 0)] {
        assert_eq!(c.active(now).len(), 1);
        assert!(c.over(now).is_empty());
        assert!(c.most_recent(now).is_none());
        assert!(c.next_date(now).is_none());
    }
}

#[test]
fn active_events_never_empty() {
    let empty = catalogue(&[]);
    let now = ft(3001, 3, 11, 12);
    let answer = active_events(&empty, now);
    let events = values(&answer);
    assert_eq!(events.len(), 1, "the placeholder stands in");
    assert_eq!(get(events[0], "QA"), &PyValue::Int(1));
    // The same when a real window is closed.
    let closed = window();
    assert_eq!(values(&active_events(&closed, ft(3002, 1, 1, 12))).len(), 1);
    // And it is not served when something is active.
    let open = values(&active_events(&closed, now)).len();
    assert_eq!(open, 1);
    assert_eq!(
        get(values(&active_events(&closed, now))[0], "scheduleID"),
        &PyValue::Int(SCHED_A)
    );
}

#[test]
fn placeholder_is_in_all_events_when_served() {
    let empty = catalogue(&[]);
    let now = ft(3001, 3, 11, 12);
    assert_eq!(values(&all_events(&empty, now)).len(), 1);
    assert!(empty.known(now, 0, 0));
    // With a real event active the placeholder is not added.
    let open = window();
    assert_eq!(values(&all_events(&open, now)).len(), 1);
    assert!(!open.known(now, 0, 0));
    assert!(open.known(now, MATCH_X, SCHED_A));
}

// ---- over, most recent, next -----------------------------------------------

#[test]
fn over_starts_at_closing_downtime() {
    let c = window();
    assert!(c.over(ft(3001, 3, 12, 10)).is_empty());
    assert_eq!(c.over(ft(3001, 3, 12, 11)).len(), 1);
    assert_eq!(c.over(ft(3001, 4, 1, 0)).len(), 1);
    assert!(c.over(ft(3001, 3, 11, 12)).is_empty());
}

#[test]
fn most_recent_is_the_latest_end() {
    let c = catalogue(&[
        row(SCHED_A, "2x1", "", "3001-01-01", "3001-01-10", 0),
        row(SCHED_B, "2x1", "", "3001-02-01", "3001-02-10", 0),
        row(SCHED_C, "2x1", "", "3001-06-01", "3001-06-10", 0),
    ]);
    let now = ft(3001, 3, 1, 12);
    let e = c.most_recent(now).expect("two are over");
    assert_eq!(e.schedule.id, SCHED_B);
    assert!(c.most_recent(ft(3001, 1, 5, 12)).is_none());
}

#[test]
fn next_date_is_a_future_start_or_a_straddling_start() {
    let c = catalogue(&[
        row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 0),
        row(SCHED_B, "2x1", "", "3001-05-01", "3001-05-03", 0),
    ]);
    let next = |y, m, d| c.next_date(ft(y, m, d, 12));
    assert_eq!(next(3001, 3, 1).as_deref(), Some("3001-03-10"));
    // A window that straddles today is "next" with its own start (a client quirk).
    assert_eq!(next(3001, 3, 11).as_deref(), Some("3001-03-10"));
    // A window that starts today is not.
    assert_eq!(next(3001, 3, 10).as_deref(), Some("3001-05-01"));
    // The first window is over; the second is ahead.
    assert_eq!(next(3001, 4, 1).as_deref(), Some("3001-05-01"));
    assert_eq!(next(3001, 6, 1), None);
}

// ---- match types -----------------------------------------------------------

#[test]
fn by_filament_is_empty_without_match_types() {
    let c = window();
    let now = ft(3001, 3, 11, 12);
    assert!(c.by_filament(now, FIL_X).is_none());
    assert!(
        c.by_filament(now, 0).is_none(),
        "a blank type is not a filament"
    );
    // Nothing panics when asked for every read.
    let _ = (all_events(&c, now), active_events(&c, now));
}

#[test]
fn by_filament_finds_the_active_event_of_the_type() {
    let types = vec![
        match_type(MATCH_X, FIL_X, "alpha", 2, 1),
        match_type(MATCH_Y, FIL_Y, "beta", 3, 5),
    ];
    let text = format!(
        "{}{}{}",
        header(),
        row(SCHED_A, "2x1", "alpha", "3001-03-10", "3001-03-12", 0),
        row(SCHED_B, "3x5", "beta", "3001-05-01", "3001-05-03", 0),
    );
    let c = Catalogue::parse(&text, types).unwrap();
    let now = ft(3001, 3, 11, 12);
    let e = c.by_filament(now, FIL_X).expect("alpha is active");
    assert_eq!((e.match_type.id, e.schedule.id), (MATCH_X, SCHED_A));
    assert!(c.by_filament(now, FIL_Y).is_none(), "beta is not open yet");
    assert!(c.by_filament(ft(3001, 5, 2, 12), FIL_Y).is_some());
    assert!(c.known(now, MATCH_X, SCHED_A));
    assert!(
        !c.known(now, 0, SCHED_A),
        "the match type is part of the key"
    );
}

#[test]
fn resolve_needs_format_and_name() {
    let types = vec![match_type(MATCH_X, FIL_X, "alpha", 2, 1)];
    let c = Catalogue::parse(
        &format!(
            "{}{}{}{}",
            header(),
            row(SCHED_A, "2x1", "ALPHA", "3001-03-10", "3001-03-12", 0),
            row(SCHED_B, "2x1", "beta", "3001-03-10", "3001-03-12", 0),
            row(SCHED_C, "4x1", "", "3001-03-10", "3001-03-12", 0),
        ),
        types,
    )
    .unwrap();
    let served = c.all(ft(3001, 3, 11, 12));
    assert_eq!(served.len(), 1);
    assert_eq!(
        (served[0].schedule.id, served[0].match_type.id),
        (SCHED_A, MATCH_X),
        "the word is case-insensitive"
    );
    assert_eq!(
        c.unserved(),
        [SCHED_B, SCHED_C],
        "no filament name has the word; no match type has the format"
    );
}

#[test]
fn a_row_with_no_match_type_is_not_served() {
    let text = format!(
        "{}{}{}",
        header(),
        row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 0),
        row(SCHED_B, "2x1", "", "", "", 1),
    );
    let c = Catalogue::parse(&text, Vec::new()).unwrap();
    let now = ft(3001, 3, 11, 12);
    assert_eq!(c.unserved(), [SCHED_A]);
    let active = c.active(now);
    assert_eq!(active.len(), 1, "only the QA row");
    assert_eq!(active[0].schedule.id, SCHED_B);
    assert!(c.over(ft(3001, 4, 1, 12)).is_empty());
    assert!(c.most_recent(ft(3001, 4, 1, 12)).is_none());
    assert!(c.next_date(ft(3001, 3, 1, 12)).is_none());
    // With nothing else, the placeholder stands in.
    let only = Catalogue::parse(
        &format!(
            "{}{}",
            header(),
            row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 0)
        ),
        Vec::new(),
    )
    .unwrap();
    let v = active_events(&only, now);
    assert_eq!(get(values(&v)[0], "scheduleID"), &PyValue::Int(0));
}

#[test]
fn next_date_is_canonical_text() {
    let c = catalogue(&[row(SCHED_A, "2x1", "", "3001-3-1", "3001-03-12", 0)]);
    assert_eq!(
        c.next_date(ft(3001, 1, 1, 12)).as_deref(),
        Some("3001-03-01")
    );
}

#[test]
fn the_placeholder_has_no_dates() {
    let v = active_events(&catalogue(&[]), ft(3001, 3, 11, 12));
    let e = values(&v)[0];
    assert_eq!(get(e, "startDate"), &PyValue::None);
    assert_eq!(get(e, "endDate"), &PyValue::None);
}

// ---- parsing ---------------------------------------------------------------

#[test]
fn parse_rejects_bad_rows() {
    let bad = |r: String| Catalogue::parse(&format!("{}{r}", header()), Vec::new());
    assert!(bad(row(SCHED_A, "2", "", "3001-03-10", "3001-03-12", 0)).is_err());
    assert!(bad(row(SCHED_A, "0x1", "", "3001-03-10", "3001-03-12", 0)).is_err());
    assert!(bad(row(SCHED_A, "2x1", "", "3001-02-30", "3001-03-12", 0)).is_err());
    assert!(bad(row(SCHED_A, "2x1", "", "3001-03-13", "3001-03-12", 0)).is_err());
    assert!(
        bad(row(SCHED_A, "2x1", "", "", "", 0)).is_err(),
        "a real row needs dates"
    );
    assert!(bad(row(0, "2x1", "", "3001-03-10", "3001-03-12", 0)).is_err());
    assert!(bad(row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 2)).is_err());
    assert!(
        bad(row(SCHED_A, "2x1", "", "", "", 1)).is_ok(),
        "a QA row may have none"
    );
    assert!(Catalogue::parse("wrong\theader\n", Vec::new()).is_err());
}

#[test]
fn leap_day_is_a_date_only_in_a_leap_year() {
    assert_eq!(parse_day("3004-02-29"), Some((3004, 2, 29)));
    assert_eq!(parse_day("3001-02-29"), None);
    assert_eq!(parse_day("3000-02-29"), None, "3000 is not a leap year");
    assert_eq!(parse_day("3001-13-01"), None);
    assert_eq!(parse_day("3001-1-1-1"), None);
}

#[test]
fn duplicate_schedule_id_keeps_the_first_row() {
    let c = catalogue(&[
        row(SCHED_A, "2x1", "", "3001-03-10", "3001-03-12", 0),
        row(SCHED_A, "4x4", "", "3001-05-10", "3001-05-12", 0),
    ]);
    assert_eq!(c.len(), 1);
    let e = &c.all(ft(3001, 3, 11, 12))[0];
    assert_eq!((e.schedule.fleets, e.schedule.fleet_size), (2, 1));
}

#[test]
fn shipped_file_parses() {
    let c = Catalogue::parse(SHIPPED, Vec::new()).expect("the shipped file parses");
    assert!(!c.is_empty());
}

// ---- the wire --------------------------------------------------------------

#[test]
fn info_has_every_client_key() {
    // The names `util._construct_info` sets; restated here on purpose.
    let wanted = [
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
    let c = window();
    let e = &c.active(ft(3001, 3, 11, 12))[0];
    let info = event_info(e);
    let PyValue::Instance { class, state } = &info else {
        panic!("not a KeyVal: {info:?}");
    };
    assert_eq!(class.as_slice(), b"utillib.KeyVal");
    let PyValue::Dict(entries) = &**state else {
        panic!("no state dict");
    };
    assert_eq!(
        entries.len(),
        wanted.len(),
        "no key the client does not read"
    );
    for k in wanted {
        assert!(dict_get(state, k.as_bytes()).is_some(), "missing {k}");
    }
    assert_eq!(get(&info, "scheduleID"), &PyValue::Int(SCHED_A));
    assert_eq!(get(&info, "matchTypeID"), &PyValue::Int(MATCH_X));
    assert_eq!(get(&info, "numberOfFleets"), &PyValue::Int(2));
    assert_eq!(get(&info, "fleetSize"), &PyValue::Int(1));
    assert_eq!(get(&info, "QA"), &PyValue::Int(0));
}

#[test]
fn info_carries_the_match_type_and_its_rewards() {
    let types = vec![match_type(MATCH_X, FIL_X, "alpha", 2, 1)];
    let c = Catalogue::parse(
        &format!(
            "{}{}",
            header(),
            row(SCHED_A, "2x1", "alpha", "3001-03-10", "3001-03-12", 0)
        ),
        types,
    )
    .unwrap();
    let info = event_info(&c.active(ft(3001, 3, 11, 12))[0]);
    assert_eq!(get(&info, "matchTypeID"), &PyValue::Int(MATCH_X));
    assert_eq!(get(&info, "filamentTypeID"), &PyValue::Int(FIL_X));
    let PyValue::List(rewards) = get(&info, "rewards") else {
        panic!("rewards is not a list");
    };
    assert_eq!(rewards.len(), 1);
    assert_eq!(get(&rewards[0], "typeID"), &PyValue::Int(990_880_900));
    assert_eq!(get(&rewards[0], "amount"), &PyValue::Int(3));
}

#[test]
fn dates_go_as_datetime_date_with_a_four_byte_state() {
    let v = date((3001, 3, 10));
    let PyValue::ObjectEx { form, header, .. } = v else {
        panic!("not an object");
    };
    assert_eq!(form, ObjectForm::Call);
    let PyValue::Tuple(h) = *header else {
        panic!("header is not a tuple");
    };
    assert_eq!(h[0], PyValue::Global(b"datetime.date".to_vec()));
    // 3001 = 0x0BB9.
    assert_eq!(
        h[1],
        PyValue::Tuple(vec![PyValue::Bytes(vec![0x0B, 0xB9, 3, 10])])
    );
    assert!(eve_rowset::is_whitelisted(b"datetime.date"));
}

#[test]
fn next_date_answers_the_row_text_or_none() {
    let c = window();
    assert_eq!(
        next_event_date(&c, ft(3001, 3, 1, 12)),
        PyValue::Bytes(b"3001-03-10".to_vec())
    );
    assert_eq!(next_event_date(&c, ft(3001, 6, 1, 12)), PyValue::None);
}

#[test]
fn most_recent_answers_none_with_nothing_over() {
    let c = window();
    assert_eq!(most_recent_event(&c, ft(3001, 3, 1, 12)), PyValue::None);
    let v = most_recent_event(&c, ft(3001, 4, 1, 12));
    assert_eq!(get(&v, "scheduleID"), &PyValue::Int(SCHED_A));
}

#[test]
fn the_pushes_are_empty_and_zero() {
    let (name, args) = LeaderboardPush {
        match_type_id: MATCH_X,
        schedule_id: SCHED_A,
    }
    .to_push();
    assert_eq!(name, "OnPVPFilamentsLeaderboard");
    assert_eq!(dict_get(&args[0], b"entries"), Some(&PyValue::List(vec![])));
    assert_eq!(
        dict_get(&args[0], b"matchTypeID"),
        Some(&PyValue::Int(MATCH_X))
    );
    assert_eq!(
        dict_get(&args[0], b"scheduleID"),
        Some(&PyValue::Int(SCHED_A))
    );
    let (name, args) = StatisticsPush {
        match_type_id: MATCH_X,
        schedule_id: SCHED_A,
    }
    .to_push();
    assert_eq!(name, "OnPVPFilamentsCharacterStatistics");
    let stats = dict_get(&args[0], b"statistics").expect("a statistics dict");
    for k in ["rank", "wins", "losses", "draws"] {
        assert_eq!(dict_get(stats, k.as_bytes()), Some(&PyValue::Int(0)), "{k}");
    }
}

#[test]
fn no_push_for_unknown_event() {
    let c = catalogue(&[row(SCHED_A, "2x1", "", "2000-01-01", "2999-12-31", 0)]);
    let mut reg = world(c);
    let unknown = [PyValue::Int(MATCH_Y), PyValue::Int(SCHED_B)];
    assert_eq!(
        call(&mut reg, "GetLeaderboard", &unknown).unwrap(),
        PyValue::None
    );
    assert_eq!(
        call(&mut reg, "GetCharacterStatistics", &unknown).unwrap(),
        PyValue::None
    );
    assert!(pushed_names(&mut reg).is_empty());
}

#[test]
fn known_event_gets_both_pushes() {
    let c = catalogue(&[row(SCHED_A, "2x1", "", "2000-01-01", "2999-12-31", 0)]);
    let mut reg = world(c);
    let known = [PyValue::Int(MATCH_X), PyValue::Int(SCHED_A)];
    assert_eq!(
        call(&mut reg, "GetLeaderboard", &known).unwrap(),
        PyValue::None
    );
    assert_eq!(
        call(&mut reg, "GetCharacterStatistics", &known).unwrap(),
        PyValue::None
    );
    assert_eq!(
        pushed_names(&mut reg),
        [
            "OnPVPFilamentsLeaderboard",
            "OnPVPFilamentsCharacterStatistics"
        ]
    );
}

#[test]
fn dispatch_answers_all_six_reads() {
    let c = catalogue(&[row(SCHED_A, "2x1", "", "2000-01-01", "2999-12-31", 0)]);
    let mut reg = world(c);
    let ids = [PyValue::Int(MATCH_X), PyValue::Int(SCHED_A)];
    for m in ["GetAllEvents", "GetActiveEvents"] {
        let v = call(&mut reg, m, &[]).unwrap();
        assert_eq!(values(&v).len(), 1, "{m}");
    }
    // Both lists answer a dict with the event in it.
    let v = call(&mut reg, "GetActiveEvents", &[]).unwrap();
    assert_eq!(get(values(&v)[0], "scheduleID"), &PyValue::Int(SCHED_A));
    assert_eq!(
        call(&mut reg, "GetMostRecentEvent", &[]).unwrap(),
        PyValue::None
    );
    // The window straddles any real today, so its start is "next" (the
    // client's quirk, `next_date_is_a_future_start_or_a_straddling_start`).
    assert_eq!(
        call(&mut reg, "GetNextEventDate", &[]).unwrap(),
        PyValue::Bytes(b"2000-01-01".to_vec())
    );
    for m in ["GetLeaderboard", "GetCharacterStatistics"] {
        assert_eq!(call(&mut reg, m, &ids).unwrap(), PyValue::None, "{m}");
    }
}

#[test]
fn a_read_with_missing_arguments_is_a_bad_call() {
    let mut reg = world(catalogue(&[]));
    for m in ["GetLeaderboard", "GetCharacterStatistics"] {
        assert!(
            matches!(
                call(&mut reg, m, &[PyValue::Int(1)]),
                Err(ServiceError::BadArguments { .. })
            ),
            "{m}"
        );
    }
    assert!(pushed_names(&mut reg).is_empty());
}

#[test]
fn unknown_method_is_no_such_method() {
    let mut reg = world(catalogue(&[]));
    assert!(matches!(
        call(&mut reg, "NoSuchProbe", &[]),
        Err(ServiceError::NoSuchMethod { .. })
    ));
}

#[test]
fn join_is_refused_with_event_not_active_and_leave_answers_none() {
    let mut reg = world(catalogue(&[]));
    let args = [PyValue::Int(100_880_001), PyValue::Int(FIL_X)];
    let Err(ServiceError::Raise(v)) = call(&mut reg, "JoinPVPQueue", &args) else {
        panic!("Join did not raise");
    };
    let expected = eve_proto::user_error(
        "PVPJumpError",
        crate::pyutil::pypairs(vec![(
            "errors",
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(33),
                PyValue::Tuple(vec![]),
            ])]),
        )]),
    );
    assert_eq!(v, expected);
    assert_eq!(
        call(&mut reg, "LeavePVPQueue", &args).unwrap(),
        PyValue::None,
        "the client's LeaveQueue does not catch an error"
    );
}
