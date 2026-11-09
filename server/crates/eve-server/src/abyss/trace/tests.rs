//! The suspect flag, the trace, the client's own departure effect and the
//! small validator gaps. **Every id and number here is invented** (a 0.8
//! system, keys of invented tiers on an invented type); the client's
//! constants appear only by name, and no `[A]` value is asserted.

use super::super::tests::{
    ABY, ALICE, BOB, HOME, NOW, ROOM_A, room, seat, ship, stand, table, world_with,
};
use super::*;
use crate::abyss::{
    AbyssState, Facts, Mode, TIER_0_DEMO, base_errors, deploy_errors, dispatch, enter, err, expire,
    leave, nebula, run_of, runs, slash, tick,
};
use crate::celestial::Universe;
use crate::content::Content;
use crate::jump_drive::{AbyssFx, FX_ABYSS_BETWEEN, FX_ABYSS_OUT};
use crate::notify::Push;
use crate::service::{Request, ServiceError};
use eve_marshal::PyValue;
use std::sync::Arc;
use std::time::Instant;

/// A system of security 0.8, whose limit is a tier the invented keys pass or
/// fail.
const MID: i64 = 30_880_003;
const CAROL: i64 = 90_880_003;
const KEY_TYPE: i64 = 990_941;
const KEY_ITEM: i64 = 100_880_901;
const KEY_GROUP: i64 = crate::abyss::KEY_GROUP;
const G_FRIGATE: i64 = 990_932;
const G_CRUISER: i64 = 990_931;
const T: i64 = 10_000_000;

fn universe() -> Universe {
    let sys = |id: i64, region: i64, sec: &str, name: &str| {
        format!("{id}\t{region}\t22880001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let mut rows = vec![
        sys(HOME, 10_880_001, "-0.45", "Invented Home"),
        sys(MID, 10_880_001, "0.8", "Invented Mid"),
    ];
    for (n, id) in ABY.iter().enumerate().rev() {
        rows.push(sys(*id, 12_880_001, "-0.99", &format!("Invented AD-{n}")));
    }
    Universe::parse(
        &rows,
        &[
            "10880001\t\t\t\t\tInvented Home".to_owned(),
            "12880001\t\t\t\t\tInvented Depths".to_owned(),
        ],
        &["22880001\t12880001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses")
}

/// The invented key type with `tier` as its `difficultyTier`.
fn typedata(tier: i64) -> crate::typedata::Installed {
    let rows = format!(
        "{}type\t{KEY_TYPE}\t{KEY_GROUP}\t4\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\
         {}={tier}.0;{}=0.0\tInvented Filament\n",
        crate::typedata::FIXTURE,
        crate::abyss::ATTR_TIER,
        crate::abyss::ATTR_WEATHER,
    );
    crate::typedata::Installed::build(
        crate::typedata::Table::parse(&rows).expect("the test rows parse"),
    )
}

/// A world with `keys` invented keys of `tier`, rooms with no waves.
fn world(group: i64, keys: i64, tier: i64) -> Registry {
    let mut c = Content::fixture();
    c.insert("universe", universe());
    c.insert("dungeons", table(vec![room(ROOM_A)]));
    c.insert("typedata", typedata(tier));
    let mut reg = world_with(group, keys).with_content(Arc::new(c));
    reg.ext_mut::<AbyssState>().waves = Some(Vec::new());
    reg
}

fn call(
    reg: &mut Registry,
    who: i64,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    dispatch(
        reg,
        &Request {
            service: "abyssalMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        },
    )
}

fn deploy(reg: &mut Registry, who: i64, mode: Mode) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "AbyssalEntranceDeployment",
        &[PyValue::Int(KEY_ITEM), PyValue::Int(mode.wire())],
    )
}

fn gate(reg: &mut Registry, who: i64, method: &str, id: i64) -> Result<PyValue, ServiceError> {
    call(reg, who, method, &[PyValue::Int(id)])
}

/// The `AbyssError` codes a refusal carries, in order.
fn codes(e: &ServiceError) -> Vec<i64> {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("a raised user error, got {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("header")
    };
    let Some(PyValue::Tuple(a)) = h.get(1) else {
        panic!("args")
    };
    let PyValue::Dict(d) = &a[1] else {
        panic!("dict")
    };
    let Some((PyValue::List(list), _)) = d.first() else {
        panic!("errors")
    };
    list.iter()
        .map(|t| match t {
            PyValue::Tuple(p) => match p[0] {
                PyValue::Int(c) => c,
                _ => panic!("a code"),
            },
            _ => panic!("a pair"),
        })
        .collect()
}

fn suspect_now(reg: &Registry, who: i64) -> bool {
    crate::crimewatch::is_suspect(reg, who, crate::pyutil::filetime_now())
}

fn none_safety(reg: &mut Registry, who: i64) {
    crate::crimewatch::set_safety(reg, who, crate::crimewatch::Safety::None);
}

type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

/// `(event, args, audience)` of what was pushed, in the order it leaves: the
/// answer's pushes, then the clock's zero-delay queue in insertion order.
fn sent(reg: &mut Registry) -> Vec<Sent> {
    let mut out: Vec<Box<dyn Push>> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
    out.extend(reg.take_due(Instant::now() + std::time::Duration::from_secs(3_600)));
    out.into_iter()
        .map(|p| {
            let (event, args) = p.to_push();
            (event, args, p.audience())
        })
        .collect()
}

/// The name of the first Destiny call in a push (`AddBalls2`, `OnSpecialFX`).
fn destiny_call(s: &Sent) -> Option<Vec<u8>> {
    if s.0 != "DoDestinyUpdate" {
        return None;
    }
    let PyValue::List(entries) = &s.1[0] else {
        return None;
    };
    let PyValue::Tuple(entry) = entries.first()? else {
        return None;
    };
    let PyValue::Tuple(call) = &entry[1] else {
        return None;
    };
    match &call[0] {
        PyValue::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

/// The fourteen positionals of the `OnSpecialFX` a `DoDestinyUpdate` carries.
fn fx_args(event: &str, args: &[PyValue]) -> Vec<PyValue> {
    assert_eq!(event, "DoDestinyUpdate");
    let PyValue::List(entries) = &args[0] else {
        panic!("entries")
    };
    let PyValue::Tuple(entry) = &entries[0] else {
        panic!("entry")
    };
    let PyValue::Tuple(call) = &entry[1] else {
        panic!("call")
    };
    assert_eq!(call[0], PyValue::Bytes(b"OnSpecialFX".to_vec()));
    let PyValue::Tuple(a) = &call[1] else {
        panic!("args")
    };
    a.clone()
}

fn guid(name: &str) -> PyValue {
    PyValue::Bytes(name.as_bytes().to_vec())
}

/// The trace ball of Alice's run, from the instance (not from the run).
fn trace_balls(reg: &Registry, system: i64) -> Vec<(i64, i64, [f64; 3])> {
    reg.ext::<crate::site::Sites>()
        .iter()
        .filter(|i| i.system == system)
        .flat_map(|i| i.objects.iter())
        .filter(|o| o.type_id == TRACE || o.type_id == TRACE_SUSPECT)
        .map(|o| (o.item_id, o.type_id, o.position))
        .collect()
}

fn fleet(reg: &mut Registry, members: &[i64]) {
    let f = reg.fleets_mut().create(ALICE).expect("created");
    reg.fleets_mut()
        .init(ALICE, f, Some(670), None, None)
        .expect("init");
    for m in members {
        reg.fleets_mut()
            .invite(ALICE, f, *m, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(*m, f, Some(670), None, None)
            .expect("accepted");
    }
}

#[test]
fn a_key_above_the_systems_tier_makes_the_deployer_suspect() {
    // Green safety: refused, nothing paid, nobody flagged.
    let mut reg = world(G_CRUISER, 2, 4);
    stand(&mut reg, ALICE, MID, [1.0, 2.0, 3.0]);
    let e = deploy(&mut reg, ALICE, Mode::Solo).unwrap_err();
    assert!(codes(&e).contains(&err::SAFETY_ENGAGED));
    assert!(!suspect_now(&reg, ALICE));
    assert_eq!(reg.store().get(KEY_ITEM).map(|i| i.quantity), Some(2));
    // Safety off: the run starts and the deployer is a suspect.
    none_safety(&mut reg, ALICE);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    assert!(run_of(&reg, ALICE).is_some_and(|r| r.suspect));
    assert!(suspect_now(&reg, ALICE));
}

#[test]
fn a_key_within_the_tier_and_a_demo_key_flag_nobody() {
    // A key at the system's limit.
    let mut reg = world(G_CRUISER, 2, crate::abyss::max_tier(0.8));
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    assert!(!suspect_now(&reg, ALICE));
    assert!(run_of(&reg, ALICE).is_some_and(|r| !r.suspect));
    // The demo key, whatever the system allows, is never flagged.
    let mut reg = world(G_CRUISER, 2, TIER_0_DEMO);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    none_safety(&mut reg, ALICE);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    assert!(!suspect_now(&reg, ALICE));
    assert!(run_of(&reg, ALICE).is_some_and(|r| !r.suspect));
}

#[test]
fn an_entrant_through_the_co_op_gate_is_never_flagged() {
    let mut reg = world(G_FRIGATE, 3, 4);
    stand(&mut reg, ALICE, MID, [3.0, 0.0, 0.0]);
    stand(&mut reg, BOB, MID, [4.0, 0.0, 0.0]);
    fleet(&mut reg, &[BOB]);
    none_safety(&mut reg, ALICE);
    deploy(&mut reg, ALICE, Mode::CoOp).expect("deployed");
    assert!(suspect_now(&reg, ALICE), "the deployer");
    let gate_id = runs(&reg).next().unwrap().entrance.clone().unwrap().gate;
    // Bob's safety is green and the gate takes him all the same.
    gate(&mut reg, BOB, "AbyssalEntranceGateActivation", gate_id).expect("bob in");
    assert!(run_of(&reg, BOB).is_some());
    assert!(!suspect_now(&reg, BOB));
}

#[test]
fn a_solo_run_leaves_a_trace_at_the_origin_of_the_suspect_type_when_suspicious() {
    let mut reg = world(G_CRUISER, 2, 4);
    stand(&mut reg, ALICE, MID, [1.0, 2.0, 3.0]);
    none_safety(&mut reg, ALICE);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    let t = run_of(&reg, ALICE).unwrap().trace.clone().expect("a trace");
    assert_eq!((t.system, t.at), (MID, [1.0, 2.0, 3.0]));
    assert_eq!(
        trace_balls(&reg, MID),
        vec![(t.ball, TRACE_SUSPECT, [1.0, 2.0, 3.0])]
    );
    // A key within the limit leaves the plain one.
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [1.0, 2.0, 3.0]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    let t = run_of(&reg, ALICE).unwrap().trace.clone().expect("a trace");
    assert_eq!(
        trace_balls(&reg, MID),
        vec![(t.ball, TRACE, [1.0, 2.0, 3.0])]
    );
    assert_ne!(TRACE, TRACE_SUSPECT);
}

#[test]
fn a_co_op_run_leaves_its_trace_where_the_gate_stood_once_the_gate_goes() {
    // Every seat taken.
    let mut reg = world(G_FRIGATE, 3, 3);
    for (who, x) in [(ALICE, 3.0), (BOB, 4.0), (CAROL, 2.0)] {
        stand(&mut reg, who, MID, [x, 0.0, 0.0]);
    }
    fleet(&mut reg, &[BOB, CAROL]);
    deploy(&mut reg, ALICE, Mode::CoOp).expect("deployed");
    let e = runs(&reg).next().unwrap().entrance.clone().unwrap();
    assert!(
        runs(&reg).next().unwrap().trace.is_none(),
        "the gate stands"
    );
    for who in [ALICE, BOB] {
        gate(&mut reg, who, "AbyssalEntranceGateActivation", e.gate).expect("in");
        assert!(runs(&reg).next().unwrap().trace.is_none(), "a seat is free");
    }
    gate(&mut reg, CAROL, "AbyssalEntranceGateActivation", e.gate).expect("in");
    let run = runs(&reg).next().unwrap().clone();
    assert!(run.entrance.is_none());
    let t = run.trace.expect("the trace");
    assert_eq!((t.system, t.at), (e.system, e.at));
    assert_eq!(trace_balls(&reg, MID), vec![(t.ball, TRACE, e.at)]);
    // Its life over with one inside.
    let mut reg = world(G_FRIGATE, 3, 3);
    stand(&mut reg, ALICE, MID, [3.0, 0.0, 0.0]);
    stand(&mut reg, BOB, MID, [4.0, 0.0, 0.0]);
    fleet(&mut reg, &[BOB]);
    deploy(&mut reg, ALICE, Mode::CoOp).expect("deployed");
    let e = runs(&reg).next().unwrap().entrance.clone().unwrap();
    gate(&mut reg, ALICE, "AbyssalEntranceGateActivation", e.gate).expect("in");
    assert!(runs(&reg).next().unwrap().trace.is_none());
    seat(&mut reg, ALICE);
    // The gate's life ends (moved up: the run's own clock is later still).
    let id = runs(&reg).next().unwrap().id;
    let life = NOW + 10;
    let entrance = reg.ext_mut::<AbyssState>().runs.get_mut(&id).unwrap();
    entrance.entrance.as_mut().unwrap().expires = life;
    tick(&mut reg, Instant::now(), life + 1);
    let t = runs(&reg).next().unwrap().trace.clone().expect("the trace");
    assert_eq!((t.system, t.at), (e.system, e.at));
    assert_eq!(trace_balls(&reg, MID).len(), 1);
}

#[test]
fn the_trace_goes_when_the_run_closes() {
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    assert_eq!(trace_balls(&reg, MID).len(), 1);
    assert!(leave(&mut reg, ALICE));
    assert_eq!(runs(&reg).count(), 0);
    assert!(trace_balls(&reg, MID).is_empty());
}

#[test]
fn the_way_in_names_the_trace_as_its_target_and_the_nebula() {
    // The shape.
    let fx = AbyssFx {
        to: vec![ALICE],
        stamp: 77,
        ship: ship(ALICE),
        target: 555,
        between: false,
        dest_system: ABY[0],
        nebula: 4242,
    };
    let (event, args) = fx.to_push();
    let a = fx_args(event, &args);
    assert_eq!(a.len(), 14);
    assert_eq!(a[0], PyValue::Int(ship(ALICE)));
    assert_eq!(a[3], PyValue::Int(555));
    assert_eq!(a[5], guid(FX_ABYSS_OUT));
    assert_eq!(
        a[13],
        PyValue::Tuple(vec![PyValue::Int(ABY[0]), PyValue::Int(4242)])
    );
    // In a real deploy: the target is the trace's ball, the nebula the tier's.
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    let _ = sent(&mut reg);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    let run = run_of(&reg, ALICE).unwrap().clone();
    let ball = run.trace.as_ref().expect("a trace").ball;
    let out = sent(&mut reg);
    let fx = out
        .iter()
        .filter(|s| destiny_call(s).is_some_and(|c| c == b"OnSpecialFX"))
        .map(|s| fx_args(s.0, &s.1))
        .find(|a| a[5] == guid(FX_ABYSS_OUT))
        .expect("the abyssal effect");
    assert_eq!(fx[3], PyValue::Int(ball));
    assert_eq!(
        fx[13],
        PyValue::Tuple(vec![
            PyValue::Int(run.pockets[0].system),
            PyValue::Int(nebula(run.tier))
        ])
    );
}

#[test]
fn the_traces_ball_leaves_before_the_effect_that_names_it() {
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    let _ = sent(&mut reg);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    let out = sent(&mut reg);
    let at = |name: &[u8]| {
        out.iter()
            .position(|s| destiny_call(s).is_some_and(|c| c == name))
    };
    let ball = at(b"AddBalls2").expect("the trace's ball is sent");
    let fx = out
        .iter()
        .position(|s| {
            destiny_call(s).is_some_and(|c| c == b"OnSpecialFX")
                && fx_args(s.0, &s.1)[5] == guid(FX_ABYSS_OUT)
        })
        .expect("the abyssal effect is sent");
    assert!(ball < fx, "ball at {ball}, effect at {fx}");
}

/// The ids a `RemoveBalls` push carries.
fn removed(s: &Sent) -> Vec<i64> {
    assert_eq!(destiny_call(s).as_deref(), Some(&b"RemoveBalls"[..]));
    let PyValue::List(entries) = &s.1[0] else {
        panic!("entries")
    };
    let PyValue::Tuple(entry) = &entries[0] else {
        panic!("entry")
    };
    let PyValue::Tuple(call) = &entry[1] else {
        panic!("call")
    };
    let PyValue::Tuple(a) = &call[1] else {
        panic!("args")
    };
    let PyValue::List(ids) = &a[0] else {
        panic!("ids")
    };
    ids.iter()
        .map(|i| match i {
            PyValue::Int(n) => *n,
            _ => panic!("an id"),
        })
        .collect()
}

#[test]
fn the_pilot_loses_the_trace_with_the_ship_and_keeps_the_ship_until_then() {
    use std::time::Duration;
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    let _ = sent(&mut reg);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    let ball = run_of(&reg, ALICE).unwrap().trace.as_ref().unwrap().ball;
    // The first flush: the trace and the effect, and no removal of anything.
    let first: Vec<Sent> = reg
        .take_due(Instant::now() + Duration::from_millis(100))
        .into_iter()
        .map(|p| {
            let (event, args) = p.to_push();
            (event, args, p.audience())
        })
        .collect();
    let names: Vec<_> = first.iter().filter_map(destiny_call).collect();
    assert!(names.contains(&b"AddBalls2".to_vec()), "{names:?}");
    assert!(names.contains(&b"OnSpecialFX".to_vec()), "{names:?}");
    assert!(
        !names.contains(&b"RemoveBalls".to_vec()),
        "the ship is still in the park when the trace arrives: {names:?}"
    );
    // The removal comes after the jump's own delay and takes the trace out
    // of the pilot's park with the ship (the overview would otherwise load
    // its row with no ego).
    let later = sent(&mut reg);
    let mine: Vec<Vec<i64>> = later
        .iter()
        .filter(|s| destiny_call(s).is_some_and(|c| c == b"RemoveBalls"))
        .filter(|s| s.2.as_ref().is_some_and(|a| a.contains(&ALICE)))
        .map(removed)
        .collect();
    assert!(
        mine.iter()
            .any(|ids| ids.contains(&ship(ALICE)) && ids.contains(&ball)),
        "{mine:?} must name the ship {} and the trace {ball}",
        ship(ALICE)
    );
}

#[test]
fn between_rooms_is_the_between_effect_with_the_gate_as_target() {
    let fx = AbyssFx {
        to: vec![ALICE],
        stamp: 78,
        ship: ship(ALICE),
        target: 556,
        between: true,
        dest_system: ABY[1],
        nebula: 4242,
    };
    let (event, args) = fx.to_push();
    let a = fx_args(event, &args);
    assert_eq!(a[3], PyValue::Int(556));
    assert_eq!(a[5], guid(FX_ABYSS_BETWEEN));
    assert_eq!(a[13], PyValue::Tuple(vec![PyValue::Int(ABY[1])]));
    // In a real run.
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let one = {
        let r = run_of(&reg, ALICE).unwrap();
        r.pocket_of(r.pilot(ALICE).unwrap()).unwrap().clone()
    };
    let _ = sent(&mut reg);
    gate(&mut reg, ALICE, "AbyssalGateActivation", one.gate).expect("on");
    let out = sent(&mut reg);
    let fx = out
        .iter()
        .filter(|s| destiny_call(s).is_some_and(|c| c == b"OnSpecialFX"))
        .map(|s| fx_args(s.0, &s.1))
        .find(|a| a[5] == guid(FX_ABYSS_BETWEEN))
        .expect("the between effect");
    assert_eq!(fx[3], PyValue::Int(one.gate));
    assert_eq!(fx[13].clone(), PyValue::Tuple(vec![fx_dest(&fx)]));
}

fn fx_dest(a: &[PyValue]) -> PyValue {
    let PyValue::Tuple(info) = &a[13] else {
        panic!("graphicInfo")
    };
    info[0].clone()
}

#[test]
fn a_gate_at_the_end_of_the_clock_refuses_with_pocket_collapsing() {
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let one = {
        let r = run_of(&reg, ALICE).unwrap();
        r.pocket_of(r.pilot(ALICE).unwrap()).unwrap().clone()
    };
    // The clock has run out (its end is long past).
    let id = run_of(&reg, ALICE).unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&id)
        .unwrap()
        .ends_at = 1;
    let e = gate(&mut reg, ALICE, "AbyssalGateActivation", one.gate).unwrap_err();
    assert_eq!(codes(&e), vec![err::POCKET_COLLAPSING]);
    // The exit gate of a one-room run too.
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    enter(&mut reg, ALICE, 3, crate::pyutil::filetime_now()).expect("in");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let last = {
        let r = run_of(&reg, ALICE).unwrap();
        r.pocket_of(r.pilot(ALICE).unwrap()).unwrap().clone()
    };
    let id = run_of(&reg, ALICE).unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&id)
        .unwrap()
        .ends_at = 1;
    let e = gate(&mut reg, ALICE, "AbyssalEndGateActivation", last.gate);
    assert!(e.is_err());
    assert!(run_of(&reg, ALICE).is_some(), "still in: the gate refused");
    // With time left the same gate takes the pilot out.
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&id)
        .unwrap()
        .ends_at = crate::pyutil::filetime_now() + 600 * T;
    gate(&mut reg, ALICE, "AbyssalEndGateActivation", last.gate).expect("out");
    assert!(run_of(&reg, ALICE).is_none());
}

#[test]
fn a_gate_is_refused_from_the_end_of_the_clock_not_from_the_next_tick() {
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let one = {
        let r = run_of(&reg, ALICE).unwrap();
        r.pocket_of(r.pilot(ALICE).unwrap()).unwrap().clone()
    };
    // The last tick was 5 s ago on the stage's clock, 2 s before the end:
    // the end passed 3 s ago, with no tick since.
    let end = NOW + 1_000 * T;
    let id = run_of(&reg, ALICE).unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&id)
        .unwrap()
        .ends_at = end;
    let five_ago = Instant::now()
        .checked_sub(std::time::Duration::from_secs(5))
        .expect("an instant five seconds back");
    tick(&mut reg, five_ago, end - 2 * T);
    let e = gate(&mut reg, ALICE, "AbyssalGateActivation", one.gate).unwrap_err();
    assert_eq!(codes(&e), vec![err::POCKET_COLLAPSING]);
    // The same tick 20 s before the end: the gate still works.
    let mut reg = world(G_CRUISER, 2, 3);
    stand(&mut reg, ALICE, MID, [0.0; 3]);
    deploy(&mut reg, ALICE, Mode::Solo).expect("deployed");
    seat(&mut reg, ALICE);
    tick(&mut reg, Instant::now(), NOW + 1);
    let one = {
        let r = run_of(&reg, ALICE).unwrap();
        r.pocket_of(r.pilot(ALICE).unwrap()).unwrap().clone()
    };
    let id = run_of(&reg, ALICE).unwrap().id;
    reg.ext_mut::<AbyssState>()
        .runs
        .get_mut(&id)
        .unwrap()
        .ends_at = end;
    tick(&mut reg, five_ago, end - 20 * T);
    gate(&mut reg, ALICE, "AbyssalGateActivation", one.gate).expect("on");
}

#[test]
fn void_space_and_the_banned_system_refuse_with_their_codes() {
    let f = Facts {
        ship_allowed: true,
        void_space: true,
        banned_system: true,
        session_change: true,
        ..Facts::default()
    };
    // The client's order: void space first, the banned system last.
    let got: Vec<i64> = base_errors(Mode::Solo, &f).iter().map(|e| e.0).collect();
    assert_eq!(
        got,
        [
            err::VOID_SPACE_DISALLOWED,
            err::SESSION_CHANGE_IN_PROGRESS,
            err::IN_BANNED_SYSTEM
        ]
    );
    let clean = Facts {
        ship_allowed: true,
        ..Facts::default()
    };
    assert!(
        deploy_errors(Mode::Solo, &clean, &Default::default())
            .iter()
            .all(|e| { e.0 != err::VOID_SPACE_DISALLOWED && e.0 != err::IN_BANNED_SYSTEM })
    );
    // Read from the world: the bands are the client's ids.
    let mut reg = world(G_CRUISER, 2, 3);
    let facts_at = |reg: &mut Registry, system: i64| {
        stand(reg, ALICE, system, [0.0; 3]);
        crate::abyss::facts(reg, Mode::Solo, ALICE, KEY_TYPE, true, NOW)
    };
    let void = crate::celestial::VOID_SPACE_SYSTEMS.start;
    let f = facts_at(&mut reg, void);
    assert!(f.void_space && !f.banned_system);
    let f = facts_at(&mut reg, crate::abyss::ZARZAKH);
    assert!(f.banned_system && !f.void_space);
    let f = facts_at(&mut reg, MID);
    assert!(!f.banned_system && !f.void_space);
}

#[test]
fn abyss_expire_moves_the_end_and_tells_each_pilot() {
    let mut reg = world(G_FRIGATE, 3, 3);
    stand(&mut reg, ALICE, MID, [3.0, 0.0, 0.0]);
    stand(&mut reg, BOB, MID, [4.0, 0.0, 0.0]);
    fleet(&mut reg, &[BOB]);
    deploy(&mut reg, ALICE, Mode::CoOp).expect("deployed");
    let g = runs(&reg).next().unwrap().entrance.clone().unwrap().gate;
    for who in [ALICE, BOB] {
        gate(&mut reg, who, "AbyssalEntranceGateActivation", g).expect("in");
    }
    let _ = sent(&mut reg);
    let ft = crate::pyutil::filetime_now();
    let said = expire(&mut reg, ALICE, 90, ft);
    assert!(said.contains("90"), "{said}");
    let end = ft + 90 * T;
    assert_eq!(run_of(&reg, ALICE).unwrap().ends_at, end);
    let told: Vec<_> = sent(&mut reg)
        .into_iter()
        .filter(|s| s.0 == "OnAddExpiryDurationForAbyssalContent")
        .collect();
    assert_eq!(told.len(), 2);
    for s in &told {
        assert_eq!(s.1, vec![PyValue::Int(end)]);
    }
    let mut to: Vec<i64> = told.iter().flat_map(|s| s.2.clone().unwrap()).collect();
    to.sort();
    assert_eq!(to, vec![ALICE, BOB]);
    // The slash word; a bad number is the usage; no run is no pocket.
    let say = |r: Result<PyValue, ServiceError>| match r.expect("answers") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => format!("{other:?}"),
    };
    let out = say(slash(&mut reg, Some(ALICE), &["expire", "x"]));
    assert!(out.contains("usage"), "{out}");
    let out = say(slash(&mut reg, Some(ALICE), &["expire", "30"]));
    assert!(out.contains("30"), "{out}");
    assert!(run_of(&reg, ALICE).unwrap().ends_at < end);
    let out = say(slash(&mut reg, Some(CAROL), &["expire", "30"]));
    assert!(out.contains("not in a pocket"), "{out}");
}
