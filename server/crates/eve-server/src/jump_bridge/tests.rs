//! Tests for `jump_bridge`: invented ids throughout, no client values.

use super::*;
use crate::celestial::Catalogue;
use crate::model::{Character, SandboxConfig};
use crate::space_state::{Motion, Presence};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const HERE: i64 = 30_960_001;
const THERE: i64 = 30_960_002;
const ELSEWHERE: i64 = 30_960_003;
/// An invented hull with a base cost, one with none, one on the restricted list.
const SHIP: i64 = 9_960_001;
const SHIP_NO_COST: i64 = 9_960_002;
const SHIP_BANNED: i64 = 9_960_003;
const BANNED_GROUP: i64 = 9_960_100;
const BAN_LIST: i64 = 9_960_200;
/// Invented capacitor figures of the gate type, cost of `SHIP`, the zone
/// multipliers of the invented table, and the light years of the invented
/// centres (every ly figure is HERE-relative, along x).
const CAP: f64 = 400.0;
const RECHARGE_MS: f64 = 3_600_000.0;
const BASE: f64 = 10.0;
const MULTS: [f64; 5] = [1.0, 2.0, 3.0, 4.0, 5.0];
const THERE_LY: f64 = 7.0;
const ELSEWHERE_LY: f64 = 30.0;
const ALICE: i64 = 5_001;
const STRANGER: i64 = 5_002;
const ALICE_SHIP: i64 = 6_001;
const STRANGER_SHIP: i64 = 6_002;
const CORP_A: i64 = 7_001;
const CORP_B: i64 = 7_002;
const CORP_X: i64 = 7_099;
const ALLIANCE: i64 = 8_001;

/// The invented content: the gate type's capacitor, three hulls, the zone
/// table and one restricted list. `table` false leaves out `jump-bridges`.
fn content(table: bool) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}type\t{TYPE_ANSIBLEX}\t9960300\t9960400\t1\t1\t1.0\t1.0\t1.0\t500.0\t0\t\t482={CAP};55={RECHARGE_MS}\tGate\n\
         type\t{SHIP}\t9960301\t6\t1\t1\t1.0\t1.0\t1.0\t10.0\t0\t\t6364={BASE}\tHull\n\
         type\t{SHIP_NO_COST}\t9960301\t6\t1\t1\t1.0\t1.0\t1.0\t10.0\t0\t\t\tFree Hull\n\
         type\t{SHIP_BANNED}\t{BANNED_GROUP}\t6\t1\t1\t1.0\t1.0\t1.0\t10.0\t0\t\t6364=5.0\tBanned Hull\n",
        crate::typedata::FIXTURE
    );
    let t = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(t));
    let at = |id: i64, ly: f64| format!("s\t{id}\t{}\t0.0\t0.0", ly * crate::celestial::LIGHTYEAR);
    let centers = crate::celestial::Centers::parse(&[
        at(HERE, 0.0),
        at(THERE, THERE_LY),
        at(ELSEWHERE, ELSEWHERE_LY),
    ])
    .expect("the test centres parse");
    c.insert("centers", centers);
    let mut d = crate::dungeon_data::Table::fixture();
    d.typelists.insert(
        BAN_LIST,
        crate::dungeon_data::TypeList {
            inc_groups: vec![BANNED_GROUP],
            ..Default::default()
        },
    );
    c.insert("dungeon_data", d);
    if table {
        c.insert(
            "jump bridges",
            Bridges::new([(
                TYPE_ANSIBLEX,
                GateRow {
                    restriction_list: Some(BAN_LIST),
                    zones: MULTS,
                },
            )]),
        );
    }
    c
}

fn world() -> Registry {
    let mut text = String::new();
    for (n, s) in [HERE, THERE, ELSEWHERE].into_iter().enumerate() {
        text += &format!("#system\t{s}\t{}\t{}\n", 92_000 + n, 93_000 + n);
    }
    let ch = |id, ship, corp| Character {
        character_id: id,
        ship_id: ship,
        corporation_id: corp,
        ship_type_id: SHIP,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            ch(ALICE, ALICE_SHIP, CORP_A),
            ch(STRANGER, STRANGER_SHIP, CORP_X),
        ],
        celestials: Catalogue::parse(&text),
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content(true)));
    for (client, who) in [(1, ALICE), (2, STRANGER)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
    }
    reg
}

/// An online gate at `at`, owned by `corp`, aimed at `aimed`.
fn gate(reg: &mut Registry, id: i64, corp: i64, system: i64, at: [f64; 3], aimed: Option<i64>) {
    let e = crate::service::structures::env(crate::pyutil::filetime_now());
    let mut s = Structure::anchored(id, TYPE_ANSIBLEX, corp, system, at, 0.0, "Gate", 1, &e);
    s.set_deed(true, &e);
    s.set_state(structure::State::ShieldVulnerable, &e);
    for _ in 0..8 {
        s.update(&e);
    }
    s.take_events();
    structure::put(reg.deltas_mut(), &s);
    if let Some(a) = aimed {
        set_destination(reg.deltas_mut(), id, a);
    }
}

fn ally(reg: &mut Registry, corps: &[i64]) {
    for c in corps {
        reg.deltas_mut().put(
            crate::alliance::KIND_MEMBER,
            vec![c.to_string()],
            vec![ALLIANCE.to_string(), "0".to_owned()],
        );
    }
}

/// Two gates of one alliance, aimed at each other: 11 in HERE, 22 in THERE,
/// the sov upgrade that enables the bridge Online at both ends.
fn pair(reg: &mut Registry) {
    bare_pair(reg);
    for system in [HERE, THERE] {
        upgrade(reg, system, JUMP_UPGRADE);
    }
}

/// The invented fixture type that enables `jump_bridge`
/// (`sov_upgrade::fixture_upgrades`).
const JUMP_UPGRADE: i64 = 9_204;

/// [`pair`] with no sov upgrade anywhere.
fn bare_pair(reg: &mut Registry) {
    ally(reg, &[CORP_A, CORP_B]);
    gate(reg, 11, CORP_A, HERE, [4.0e9, 0.0, 0.0], Some(THERE));
    gate(reg, 22, CORP_B, THERE, [0.0, 0.0, 6.0e9], Some(HERE));
    // The alliance holds both systems, and a third as its capital.
    for system in [HERE, THERE, ELSEWHERE] {
        crate::sov::set_claim(reg.deltas_mut(), system, Some((ALLIANCE, CORP_A)), 1);
    }
    full(reg, 11);
    full(reg, 22);
}

/// Gate `id` holds its full capacity, settled now.
fn full(reg: &mut Registry, id: i64) {
    set_charge(reg, id, CAP, crate::pyutil::filetime_now());
}

/// Gate `id` holds `gj`, settled at `at`, with the etag unchanged.
fn set_charge(reg: &mut Registry, id: i64, gj: f64, at: i64) {
    let counter = cap_row(reg.deltas(), id).map_or(0, |r| r.2);
    put_cap(reg.deltas_mut(), id, gj, at, counter);
}

/// The charge gate `id` holds now.
fn charge(reg: &Registry, id: i64) -> f64 {
    let g = structure::get(reg.deltas(), id).unwrap();
    charge_of(reg, &g, crate::pyutil::filetime_now()).0
}

/// Make `system` the alliance's capital, at once.
fn capital(reg: &mut Registry, system: i64) {
    crate::sov_index::set_capital(reg.deltas_mut(), ALLIANCE, system, 1, 0).unwrap();
}

/// A held hub in `system` with upgrade `ty` Online.
fn upgrade(reg: &mut Registry, system: i64, ty: i64) {
    let hub_id = 8_800_000 + system % 1_000;
    crate::sov_hub::put(
        reg.deltas_mut(),
        &crate::sov_hub::Hub {
            system_id: system,
            hub_id,
            position: [0.0; 3],
            launcher_id: None,
            launch_time: 0,
            offline_days: 0,
        },
    );
    crate::sov::set_claim(reg.deltas_mut(), system, Some((ALLIANCE, CORP_A)), 1);
    crate::sov_upgrade::put_installed(
        reg.deltas_mut(),
        hub_id,
        &[crate::sov_upgrade::Installed {
            type_id: ty,
            online: true,
            state: crate::sov_upgrade::Power::Online,
        }],
    );
}

fn put(reg: &mut Registry, who: i64, ship: i64, system: i64, at: [f64; 3]) {
    reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship,
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
    crate::liveness::refresh(reg, Instant::now(), reg.world().birth);
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    let req = Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    };
    match service {
        "structureJumpBridgeMgr" => dispatch(reg, &req),
        _ => crate::service::structures::dispatch(reg, &req),
    }
}

/// An attribute of a KeyVal (`0x17` state) or of a new-style object
/// (`0x23`, state at `header[1]`).
fn attr(v: &PyValue, name: &str) -> PyValue {
    let state = match v {
        PyValue::Instance { state, .. } => &**state,
        PyValue::ObjectEx { header, .. } => match &**header {
            PyValue::Tuple(parts) if parts.len() == 2 => &parts[1],
            other => panic!("a (args, state) header expected, got {other:?}"),
        },
        other => panic!("a KeyVal or a new-style object expected, got {other:?}"),
    };
    crate::pyutil::dict_get(state, name.as_bytes())
        .cloned()
        .unwrap_or_else(|| panic!("{name} missing"))
}

// -- the link rule ------------------------------------------------------------------

#[test]
fn two_gates_aimed_at_each_other_in_one_alliance_are_linked() {
    let mut reg = world();
    pair(&mut reg);
    let a = structure::get(reg.deltas(), 11).unwrap();
    let b = structure::get(reg.deltas(), 22).unwrap();
    assert_eq!(partner(reg.deltas(), &a).map(|p| p.id), Some(22));
    assert_eq!(partner(reg.deltas(), &b).map(|p| p.id), Some(11));
    let l = links(reg.deltas());
    assert_eq!(l.len(), 1, "once, not once per end");
    assert_eq!((l[0].0.id, l[0].1.id), (11, 22));
}

#[test]
fn a_gate_aimed_one_way_only_links_to_nothing() {
    let mut reg = world();
    pair(&mut reg);
    set_destination(reg.deltas_mut(), 22, ELSEWHERE);
    let a = structure::get(reg.deltas(), 11).unwrap();
    assert!(partner(reg.deltas(), &a).is_none());
    assert!(links(reg.deltas()).is_empty());
    clear_destination(reg.deltas_mut(), 22);
    assert_eq!(destination(reg.deltas(), 22), None);
}

#[test]
fn another_owner_is_not_a_partner_but_one_corporation_is_its_own_group() {
    let mut reg = world();
    // No alliance: two corporations are two owners ...
    gate(&mut reg, 11, CORP_A, HERE, [0.0; 3], Some(THERE));
    gate(&mut reg, 22, CORP_B, THERE, [0.0; 3], Some(HERE));
    let a = structure::get(reg.deltas(), 11).unwrap();
    assert!(
        partner(reg.deltas(), &a).is_none(),
        "different corps, no alliance"
    );
    // ... one corporation with two gates is one.
    gate(&mut reg, 22, CORP_A, THERE, [0.0; 3], Some(HERE));
    assert_eq!(partner(reg.deltas(), &a).map(|p| p.id), Some(22));
    // A stranger's gate in the same system does not displace it.
    gate(&mut reg, 5, CORP_X, THERE, [0.0; 3], Some(HERE));
    assert_eq!(partner(reg.deltas(), &a).map(|p| p.id), Some(22));
}

#[test]
fn only_an_ansiblex_has_a_partner_or_slim_keys() {
    let mut reg = world();
    pair(&mut reg);
    let mut other = structure::get(reg.deltas(), 11).unwrap();
    other.type_id = 990_001; // an invented structure type
    assert!(partner(reg.deltas(), &other).is_none());
    assert!(slim_keys(&reg, &other).is_empty());
}

#[test]
fn the_slim_carries_the_partner_system_or_none() {
    let mut reg = world();
    pair(&mut reg);
    let a = structure::get(reg.deltas(), 11).unwrap();
    let keys: BTreeMap<_, _> = slim_keys(&reg, &a).into_iter().collect();
    assert_eq!(keys["targetSolarsystemID"], PyValue::Int(THERE));
    assert_eq!(keys["component_jumpBridge"], PyValue::Int(0));
    assert_eq!(keys["activationState"], PyValue::None);
    // `jumps[0]` is read unguarded by the departure effect's handler.
    assert_eq!(
        keys["jumps"],
        PyValue::List(vec![crate::destiny::jumps_row(22, THERE)])
    );
    clear_destination(reg.deltas_mut(), 22);
    let keys: BTreeMap<_, _> = slim_keys(&reg, &a).into_iter().collect();
    assert_eq!(keys["targetSolarsystemID"], PyValue::None, "unlinked");
    assert_eq!(keys["jumps"], PyValue::List(vec![]));
    // And the structure's own extras carry them (the bootstrap and the push).
    let all: Vec<_> = crate::structure_space::extras_of(&reg, &a)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert!(all.contains(&"targetSolarsystemID") && all.contains(&"state"));
}

// -- the calls ------------------------------------------------------------------------

#[test]
fn the_linked_structure_and_the_destination_answer_the_clients_shapes() {
    let mut reg = world();
    pair(&mut reg);
    let linked = call(
        &mut reg,
        ALICE,
        "structureJumpBridgeMgr",
        "GetLinkedStructure",
        &[PyValue::Int(11)],
    )
    .unwrap();
    assert_eq!(
        linked,
        PyValue::Tuple(vec![PyValue::Int(22), PyValue::Int(THERE)])
    );
    let dest = call(
        &mut reg,
        ALICE,
        "structureJumpBridgeMgr",
        "GetJbStructureDestination",
        &[PyValue::Int(11)],
    )
    .unwrap();
    assert_eq!(dest, PyValue::Int(THERE));
    // Unlinked: the destination is still the configured one, the link none.
    clear_destination(reg.deltas_mut(), 22);
    let linked = call(
        &mut reg,
        ALICE,
        "structureJumpBridgeMgr",
        "GetLinkedStructure",
        &[PyValue::Int(11)],
    )
    .unwrap();
    assert_eq!(linked, PyValue::None);
    let dest = call(
        &mut reg,
        ALICE,
        "structureJumpBridgeMgr",
        "GetJbStructureDestination",
        &[PyValue::Int(11)],
    )
    .unwrap();
    assert_eq!(dest, PyValue::Int(THERE));
    // Not a gate, not a structure: falsy.
    for id in [999, 0] {
        for m in ["GetLinkedStructure", "GetJbStructureDestination"] {
            let got = call(
                &mut reg,
                ALICE,
                "structureJumpBridgeMgr",
                m,
                &[PyValue::Int(id)],
            )
            .unwrap();
            assert_eq!(got, PyValue::None, "{m} {id}");
        }
    }
    assert!(
        call(
            &mut reg,
            ALICE,
            "structureJumpBridgeMgr",
            "NoSuchThing",
            &[]
        )
        .is_err()
    );
}

#[test]
fn the_map_lists_the_links_with_the_access_split() {
    let mut reg = world();
    pair(&mut reg);
    let got = call(
        &mut reg,
        ALICE,
        "structureDirectory",
        "GetJumpBridgesWithMyAccess",
        &[],
    )
    .unwrap();
    let PyValue::Tuple(t) = got else {
        panic!("{got:?}")
    };
    assert_eq!(t.len(), 3, "the arity is the requirement");
    let PyValue::List(pairs) = &t[0] else {
        panic!()
    };
    assert_eq!(pairs.len(), 1);
    let PyValue::Tuple(ends) = &pairs[0] else {
        panic!()
    };
    assert_eq!(attr(&ends[0], "solarSystemID"), PyValue::Int(HERE));
    assert_eq!(attr(&ends[0], "structureID"), PyValue::Int(11));
    assert_eq!(attr(&ends[1], "solarSystemID"), PyValue::Int(THERE));
    assert_eq!(
        t[1],
        PyValue::List(vec![PyValue::Int(11), PyValue::Int(22)])
    );
    assert_eq!(t[2], PyValue::List(vec![]));
    // The map draws the pair for a stranger too, without access to it.
    let got = call(
        &mut reg,
        STRANGER,
        "structureDirectory",
        "GetJumpBridgesWithMyAccess",
        &[],
    )
    .unwrap();
    let PyValue::Tuple(t) = got else { panic!() };
    assert_eq!(t[1], PyValue::List(vec![]));
    assert_eq!(
        t[2],
        PyValue::List(vec![PyValue::Int(11), PyValue::Int(22)])
    );
}

#[test]
fn nearby_gates_are_the_groups_with_the_aligned_flag() {
    let mut reg = world();
    pair(&mut reg);
    gate(&mut reg, 33, CORP_X, ELSEWHERE, [0.0; 3], Some(HERE));
    put(&mut reg, ALICE, ALICE_SHIP, HERE, [4.0e9, 0.0, 9000.0]);
    let got = call(
        &mut reg,
        ALICE,
        "structureDirectory",
        "GetNearbyJumpBridges",
        &[],
    )
    .unwrap();
    let PyValue::List(rows) = got else {
        panic!("{got:?}")
    };
    assert_eq!(
        rows.len(),
        2,
        "the alliance's two gates, not the stranger's"
    );
    let by_system: BTreeMap<i64, PyValue> = rows
        .iter()
        .map(|r| match attr(r, "solarSystemID") {
            PyValue::Int(s) => (s, r.clone()),
            o => panic!("{o:?}"),
        })
        .collect();
    // Alice is in HERE: the gate in THERE is aimed at her system, her own is not.
    assert_eq!(
        attr(&by_system[&THERE], "alignedToCurrentSystem"),
        PyValue::Bool(true)
    );
    assert_eq!(
        attr(&by_system[&HERE], "alignedToCurrentSystem"),
        PyValue::Bool(false)
    );
    assert_eq!(attr(&by_system[&THERE], "structureID"), PyValue::Int(22));
    // Nobody in space and no session: an empty list, never a failure.
    let none = nearby(&reg, None);
    assert_eq!(none, PyValue::List(vec![]));
}

// -- the jump -------------------------------------------------------------------------

fn jump(reg: &mut Registry, who: i64, gate_id: i64) -> Result<PyValue, ServiceError> {
    call(
        reg,
        who,
        "structureJumpBridgeMgr",
        "CmdJumpThroughStructureStargate",
        &[PyValue::Int(gate_id)],
    )
}

fn near_gate_a(reg: &mut Registry, who: i64, ship: i64) {
    let g = structure::get(reg.deltas(), 11).unwrap();
    let r = crate::structure_space::radius(reg.content(), g.type_id);
    put(reg, who, ship, HERE, [g.pos[0] - r - 800.0, 0.0, 0.0]);
}

fn change_map(change: &[(PyValue, PyValue)]) -> BTreeMap<String, (i64, i64)> {
    change
        .iter()
        .filter_map(|(v, k)| {
            let (PyValue::Bytes(k), PyValue::Tuple(p)) = (k, v) else {
                return None;
            };
            let (PyValue::Int(a), PyValue::Int(b)) = (&p[0], &p[1]) else {
                return None;
            };
            Some((String::from_utf8_lossy(k).into_owned(), (*a, *b)))
        })
        .collect()
}

/// A jump moves the ship out of one system and into the partner's with the
/// gate jump's session change and bind, beside the partner gate.
#[test]
fn a_jump_lands_the_ship_beside_the_partner_gate() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    let _ = reg.take_pending();

    let got = jump(&mut reg, ALICE, 11);
    assert_eq!(got, Ok(PyValue::Bool(true)), "didJumpThrough");
    assert!(!reg.roster().is_in_space(ALICE), "in the tunnel");
    assert!(reg.jumps().contains(ALICE));
    assert!(reg.take_session_changes().is_empty(), "not with the answer");

    // The departure effect: 14 operands, the gate as targetID, the far system last.
    let fx: Vec<Vec<PyValue>> = reg
        .take_pending()
        .iter()
        .filter_map(|(_, p)| {
            let (ev, args) = p.to_push();
            if ev != "DoDestinyUpdate" {
                return None;
            }
            let PyValue::List(es) = args.first()? else {
                return None;
            };
            let PyValue::Tuple(e) = es.first()? else {
                return None;
            };
            let PyValue::Tuple(named) = e.get(1)? else {
                return None;
            };
            let (Some(PyValue::Bytes(n)), Some(PyValue::Tuple(ops))) =
                (named.first(), named.get(1))
            else {
                return None;
            };
            (n == b"OnSpecialFX").then(|| ops.clone())
        })
        .collect();
    assert_eq!(fx.len(), 1);
    assert_eq!(fx[0].len(), 14);
    assert_eq!(fx[0][0], PyValue::Int(ALICE_SHIP));
    assert_eq!(fx[0][3], PyValue::Int(11));
    assert_eq!(fx[0][13], PyValue::Tuple(vec![PyValue::Int(THERE)]));

    let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    let _ = reg.take_due(late);
    crate::jump::tick(&mut reg, late);
    let changes = reg.take_session_changes();
    assert_eq!(changes.len(), 1);
    let map = change_map(&changes[0].1);
    assert_eq!(map.get("solarsystemid"), Some(&(HERE, THERE)));
    assert_eq!(map.get("solarsystemid2"), Some(&(HERE, THERE)));
    assert!(map.contains_key("constellationid") && map.contains_key("regionid"));
    assert!(!map.contains_key("shipid"), "the invCache trap");

    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    let p = *reg.roster().get(ALICE).expect("in space again");
    assert_eq!(p.solar_system_id, THERE);
    let there = structure::get(reg.deltas(), 22).unwrap();
    let r = crate::structure_space::radius(reg.content(), there.type_id);
    let d = crate::space_state::distance(p.motion.position, there.pos) - r;
    assert!(d > 0.0, "outside the gate");
    assert!(
        d <= MAX_JUMP_DISTANCE,
        "inside the jump range, so the way back is legal: {d} m"
    );
}

#[test]
fn the_way_back_works_from_the_arrival() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    jump(&mut reg, ALICE, 11).unwrap();
    let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    crate::jump::tick(&mut reg, late);
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    assert_eq!(jump(&mut reg, ALICE, 22), Ok(PyValue::Bool(true)));
}

#[test]
fn a_jump_out_of_range_is_refused_with_the_clients_key_and_nothing_moves() {
    let mut reg = world();
    pair(&mut reg);
    put(&mut reg, ALICE, ALICE_SHIP, HERE, [0.0, 0.0, 0.0]);
    let _ = reg.take_pending();
    assert_eq!(
        jump(&mut reg, ALICE, 11),
        Err(user_error(TOO_FAR, vec![("maxdist", PyValue::Int(2500))]))
    );
    assert!(reg.roster().is_in_space(ALICE));
    assert!(reg.jumps().is_empty());
    assert!(reg.take_pending().is_empty());
}

#[test]
fn the_other_refusals_change_nothing() {
    let mut reg = world();
    pair(&mut reg);
    // Not in space.
    assert!(jump(&mut reg, ALICE, 11).is_err());
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    // Not a gate / not in this system.
    assert!(jump(&mut reg, ALICE, 999).is_err());
    assert!(
        jump(&mut reg, ALICE, 22).is_err(),
        "the far gate is not in this system"
    );
    // Another group's pilot is refused with the access setting's key.
    near_gate_a(&mut reg, STRANGER, STRANGER_SHIP);
    assert_eq!(
        jump(&mut reg, STRANGER, 11),
        Err(user_error(
            crate::structure_profile::access_error(SETTING_ACTIVATE),
            vec![]
        ))
    );
    // The far end offline: refused.
    let mut far = structure::get(reg.deltas(), 22).unwrap();
    far.state = structure::State::Anchoring;
    structure::put(reg.deltas_mut(), &far);
    assert!(!usable(&far), "{:?}", far.state);
    assert!(jump(&mut reg, ALICE, 11).is_err());
    assert!(reg.roster().is_in_space(ALICE));
    assert!(reg.jumps().is_empty());
    // Unlinked: refused.
    clear_destination(reg.deltas_mut(), 22);
    assert!(jump(&mut reg, ALICE, 11).is_err());
}

#[test]
fn a_second_jump_while_in_the_tunnel_is_refused() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    jump(&mut reg, ALICE, 11).unwrap();
    assert!(jump(&mut reg, ALICE, 11).is_err());
}

// -- aiming a gate ----------------------------------------------------------------------

#[test]
fn the_anchor_extra_config_aims_the_gate() {
    let mut reg = world();
    gate(&mut reg, 11, CORP_A, HERE, [0.0; 3], None);
    let s = structure::get(reg.deltas(), 11).unwrap();
    let extra = crate::pyutil::keyval(vec![("destinationSolarsystemID", PyValue::Int(THERE))]);
    on_anchored(&mut reg, &s, Some(&extra));
    assert_eq!(destination(reg.deltas(), 11), Some(THERE));
    // No config, or a gate-less type: nothing is written.
    gate(&mut reg, 12, CORP_A, HERE, [0.0; 3], None);
    let s = structure::get(reg.deltas(), 12).unwrap();
    on_anchored(&mut reg, &s, None);
    on_anchored(&mut reg, &s, Some(&crate::pyutil::keyval(vec![])));
    assert_eq!(destination(reg.deltas(), 12), None);
    let mut other = s.clone();
    other.type_id = 990_001;
    on_anchored(&mut reg, &other, Some(&extra));
    assert_eq!(destination(reg.deltas(), 12), None);
}

#[test]
fn the_gm_command_links_and_unlinks() {
    let mut reg = world();
    ally(&mut reg, &[CORP_A, CORP_B]);
    gate(&mut reg, 11, CORP_A, HERE, [0.0; 3], None);
    gate(&mut reg, 22, CORP_B, THERE, [0.0; 3], None);
    let say = |v: PyValue| match v {
        PyValue::Bytes(b) => String::from_utf8(b).unwrap(),
        o => panic!("{o:?}"),
    };
    let r = say(slash(
        &mut reg,
        Some(ALICE),
        &["link", "11", THERE.to_string().as_str()],
    )
    .unwrap());
    assert!(r.contains("no gate aimed back"), "{r}");
    let r = say(slash(
        &mut reg,
        Some(ALICE),
        &["link", "22", HERE.to_string().as_str()],
    )
    .unwrap());
    assert!(r.contains("linked to gate 11"), "{r}");
    assert_eq!(links(reg.deltas()).len(), 1);
    let r = say(slash(&mut reg, Some(ALICE), &["list"]).unwrap());
    assert!(r.contains("gate 11") && r.contains("partner 22"), "{r}");
    // Refused: a system that is its own, a non-gate.
    assert!(
        say(slash(
            &mut reg,
            Some(ALICE),
            &["link", "11", HERE.to_string().as_str()]
        )
        .unwrap())
        .starts_with("/jbridge")
    );
    assert!(
        say(slash(
            &mut reg,
            Some(ALICE),
            &["link", "5", THERE.to_string().as_str()]
        )
        .unwrap())
        .contains("not a jump gate")
    );
    slash(&mut reg, Some(ALICE), &["unlink", "22"]).unwrap();
    assert!(links(reg.deltas()).is_empty());
}

/// Turns the gate on for this thread and back off on drop, panic or not.
struct GateOn;

impl GateOn {
    fn new() -> Self {
        FORCE_GATE.with(|f| f.set(true));
        GateOn
    }
}

impl Drop for GateOn {
    fn drop(&mut self) {
        FORCE_GATE.with(|f| f.set(false));
    }
}

#[test]
fn the_gate_guard_resets_the_switch_even_on_a_panic() {
    let r = std::panic::catch_unwind(|| {
        let _gate = GateOn::new();
        assert!(FORCE_GATE.with(std::cell::Cell::get));
        panic!("on purpose");
    });
    assert!(r.is_err());
    assert!(!FORCE_GATE.with(std::cell::Cell::get));
}

#[test]
fn a_gate_needs_the_bridge_upgrade_at_both_ends() {
    let _gate = GateOn::new();
    let mut reg = world();
    bare_pair(&mut reg);
    let g = structure::get(reg.deltas(), 11).unwrap();
    let t = crate::sov_upgrade::table(reg.content()).clone();
    assert!(!upgraded(&reg, &g), "no hub, no upgrade");
    assert!(!may_use(&reg, ALICE, &g));
    // The switch off is the old rule.
    assert!(upgraded_with(reg.deltas(), &t, &g, false));
    // One end only.
    upgrade(&mut reg, HERE, JUMP_UPGRADE);
    assert!(!upgraded(&reg, &g));
    let far = structure::get(reg.deltas(), 22).unwrap();
    assert!(!upgraded(&reg, &far), "the far end asks the near end too");
    // An upgrade that enables something else does not count.
    upgrade(&mut reg, THERE, 9_203);
    assert!(!upgraded(&reg, &g));
    // Both ends.
    upgrade(&mut reg, THERE, JUMP_UPGRADE);
    assert!(upgraded(&reg, &g) && upgraded(&reg, &far));
    assert!(may_use(&reg, ALICE, &g));
}

#[test]
fn a_jump_without_the_upgrade_is_refused_and_nothing_moves() {
    let _gate = GateOn::new();
    let mut reg = world();
    bare_pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    assert!(jump(&mut reg, ALICE, 11).is_err());
    assert!(reg.roster().is_in_space(ALICE));
    assert!(reg.jumps().is_empty());
    // The map keeps drawing the pair but lists the gates as not usable.
    let got = call(
        &mut reg,
        ALICE,
        "structureDirectory",
        "GetJumpBridgesWithMyAccess",
        &[],
    )
    .unwrap();
    let PyValue::Tuple(t) = got else { panic!() };
    assert_eq!(t[1], PyValue::List(vec![]));
    assert_eq!(
        t[2],
        PyValue::List(vec![PyValue::Int(11), PyValue::Int(22)])
    );
    // With the upgrade at both ends the same jump goes through.
    for system in [HERE, THERE] {
        upgrade(&mut reg, system, JUMP_UPGRADE);
    }
    assert!(jump(&mut reg, ALICE, 11).is_ok());
}

// -- ANSI-2a: the capacitor, the zone cost, the access rule -------------------------

/// The gate's charge in a test is exact to this.
const EPS: f64 = 1e-9;

#[test]
fn charge_grows_from_zero_toward_the_capacity_and_never_past_it() {
    let tick = 10_000.0;
    let at = |ms: f64| (ms * tick) as i64;
    let mut last = 0.0;
    for ms in [0.0, 1.0e5, 1.0e6, 5.0e6, 2.0e7, 1.0e9] {
        let c = charge_at(CAP, RECHARGE_MS, 0.0, 0, at(ms));
        assert!(c >= last, "monotonic at {ms} ms: {c} < {last}");
        assert!((0.0..=CAP).contains(&c));
        last = c;
    }
    assert_eq!(charge_at(CAP, RECHARGE_MS, 0.0, 0, 0), 0.0);
    assert!(last > CAP * 0.999, "asymptotically full: {last}");
    // Full stays full; a time before the settle counts as none.
    assert!((charge_at(CAP, RECHARGE_MS, CAP, 0, at(1.0e6)) - CAP).abs() < EPS);
    assert!((charge_at(CAP, RECHARGE_MS, 50.0, 100, 0) - 50.0).abs() < 1e-6);
    // c0 above the capacity is clamped.
    assert!((charge_at(CAP, RECHARGE_MS, CAP * 3.0, 0, 0) - CAP).abs() < EPS);
}

#[test]
fn charge_at_one_tau_matches_the_formula() {
    let tau = RECHARGE_MS / 5.0;
    let c0 = 100.0;
    let got = charge_at(CAP, RECHARGE_MS, c0, 7, 7 + (tau * 10_000.0) as i64);
    let want = (1.0 + ((c0 / CAP).sqrt() - 1.0) * (-1.0f64).exp()).powi(2) * CAP;
    assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    // A gate with no capacity holds nothing.
    assert_eq!(charge_at(0.0, RECHARGE_MS, 0.0, 0, 1_000_000_000), 0.0);
}

#[test]
fn zone_bounds_and_cost() {
    assert_eq!(zone_index(Some(0.0)), 0);
    assert_eq!(zone_index(Some(5.0)), 0, "the bound belongs to its zone");
    assert_eq!(zone_index(Some(5.01)), 1);
    assert_eq!(zone_index(Some(10.0)), 1);
    assert_eq!(zone_index(Some(15.0)), 2);
    assert_eq!(zone_index(Some(20.0)), 3);
    assert_eq!(zone_index(Some(25.0)), 4);
    assert_eq!(zone_index(None), 4, "no capital is the last zone");
    assert_eq!(zone_cost(10.0, MULTS, 0), 10.0);
    assert_eq!(zone_cost(10.0, MULTS, 3), 40.0);
    assert_eq!(
        zone_cost(10.0, MULTS, 9),
        50.0,
        "an index past the end clamps"
    );
}

/// A jump takes `base x multiplier` of the destination's zone from the gate
/// and lands the ship.
#[test]
fn a_jump_takes_the_zone_cost_and_lands() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    // Capital in HERE: THERE is 7 ly away, zone 2.
    capital(&mut reg, HERE);
    set_charge(&mut reg, 11, 100.0, crate::pyutil::filetime_now());
    assert_eq!(jump(&mut reg, ALICE, 11), Ok(PyValue::Bool(true)));
    assert!(reg.jumps().contains(ALICE));
    let want = 100.0 - BASE * MULTS[1];
    assert!(
        (charge(&reg, 11) - want).abs() < 0.5,
        "{}",
        charge(&reg, 11)
    );
    // The far gate's charge is its own.
    assert!((charge(&reg, 22) - CAP).abs() < EPS);
}

#[test]
fn the_zone_follows_the_capital() {
    let mut reg = world();
    pair(&mut reg);
    let now = crate::pyutil::filetime_now();
    // No capital among three held systems: the last zone.
    assert_eq!(zone_of(&reg, ALLIANCE, THERE, now), 4);
    capital(&mut reg, HERE);
    assert_eq!(zone_of(&reg, ALLIANCE, THERE, now), 1, "7 ly");
    capital(&mut reg, ELSEWHERE);
    assert_eq!(zone_of(&reg, ALLIANCE, THERE, now), 4, "23 ly");
    // A system the centres do not hold: the last zone.
    assert_eq!(zone_of(&reg, ALLIANCE, 30_960_099, now), 4);
}

/// Too little charge refuses the jump: the ship stays, nothing is taken.
#[test]
fn too_little_charge_refuses_and_costs_nothing() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    capital(&mut reg, HERE);
    let cost = BASE * MULTS[1];
    let now = crate::pyutil::filetime_now();
    set_charge(&mut reg, 11, cost - 1.0, now);
    let etag = cap_row(reg.deltas(), 11).unwrap().2;
    assert_eq!(jump(&mut reg, ALICE, 11), Err(notify(NO_CHARGE)));
    assert!(reg.roster().is_in_space(ALICE));
    assert!(reg.jumps().is_empty());
    let (held, at, counter) = cap_row(reg.deltas(), 11).unwrap();
    assert_eq!((held, at, counter), (cost - 1.0, now, etag), "untouched");
    // A new gate starts empty: no row, anchored just now.
    reg.deltas_mut().del(KIND_CHARGE, &cap_key(11));
    assert!(charge(&reg, 11) < 1.0);
    assert!(jump(&mut reg, ALICE, 11).is_err());
}

#[test]
fn a_gate_recharges_over_time() {
    let mut reg = world();
    pair(&mut reg);
    let long_ago = crate::pyutil::filetime_now() - 3 * 3_600 * 10_000_000;
    set_charge(&mut reg, 11, 0.0, long_ago);
    // Three hours on a one-hour recharge time: past 99 percent.
    assert!(charge(&reg, 11) > CAP * 0.99, "{}", charge(&reg, 11));
}

#[test]
fn a_restricted_hull_is_refused_before_any_charge_is_taken() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    assert!(restricted_hull(&reg, SHIP_BANNED));
    assert!(!restricted_hull(&reg, SHIP));
    reg.character_mut(ALICE).unwrap().ship_type_id = SHIP_BANNED;
    let before = cap_row(reg.deltas(), 11);
    assert_eq!(
        jump(&mut reg, ALICE, 11),
        Err(restricted_error(reg.content(), SHIP_BANNED))
    );
    assert_eq!(cap_row(reg.deltas(), 11), before);
    assert!(reg.roster().is_in_space(ALICE));
    // A hull with no base cost is refused the same way.
    reg.character_mut(ALICE).unwrap().ship_type_id = SHIP_NO_COST;
    assert_eq!(
        jump(&mut reg, ALICE, 11),
        Err(restricted_error(reg.content(), SHIP_NO_COST))
    );
    assert_eq!(cap_row(reg.deltas(), 11), before);
}

#[test]
fn without_the_table_nothing_is_restricted_and_every_multiplier_is_one() {
    let mut reg = world().with_content(std::sync::Arc::new(content(false)));
    pair(&mut reg);
    assert!(!restricted_hull(&reg, SHIP_BANNED));
    let g = structure::get(reg.deltas(), 11).unwrap();
    capital(&mut reg, ELSEWHERE);
    let now = crate::pyutil::filetime_now();
    assert_eq!(cost_of(&reg, ALICE, &g, SHIP, now), Some(BASE));
}

#[test]
fn the_access_rule_wants_the_holder_alliance_and_the_profile_setting() {
    let mut reg = world();
    pair(&mut reg);
    let g = structure::get(reg.deltas(), 11).unwrap();
    // A member of the holder alliance, owner group, no profile: in.
    assert!(may_use(&reg, ALICE, &g));
    // Outside the alliance: out.
    assert!(!may_use(&reg, STRANGER, &g));
    // A member of another corporation of the owner alliance is in: the
    // interim rule is the owner group, which is the alliance.
    reg.character_mut(STRANGER).unwrap().corporation_id = CORP_B;
    assert!(
        may_use(&reg, STRANGER, &g),
        "CORP_B is in the owner alliance"
    );
    // A holder that is another alliance refuses the owner's pilot.
    crate::sov::set_claim(reg.deltas_mut(), HERE, Some((ALLIANCE + 1, CORP_X)), 2);
    assert!(!may_use(&reg, ALICE, &g));
}

#[test]
fn the_profile_setting_decides_once_the_owner_has_a_profile() {
    let mut reg = world();
    pair(&mut reg);
    let g = structure::get(reg.deltas(), 11).unwrap();
    assert!(!profile_decides(reg.deltas(), &g));
    assert!(
        may_use(&reg, ALICE, &g),
        "no profile: the owner group passes"
    );
    // The owner's default profile, as anchoring and /upwell spawn create it,
    // holds no activation row: it does not decide, the owner group passes.
    let pid = crate::structure_profile::ensure_default(&mut reg, CORP_A);
    let g = structure::get(reg.deltas(), 11).unwrap();
    assert!(!profile_decides(reg.deltas(), &g));
    assert!(
        may_use(&reg, ALICE, &g),
        "a default profile with no activation row lets the owner group through"
    );
    // A group holding the setting lets its members in, and no one else.
    let group = crate::owner_groups::create_for(
        &mut reg,
        CORP_A,
        "Invented",
        "",
        &[(CORP_A, crate::owner_groups::MEMBER)],
    );
    crate::structure_profile::save_settings(
        reg.deltas_mut(),
        pid,
        &[(SETTING_ACTIVATE, group.group_id, 0.0)],
    );
    assert!(profile_decides(reg.deltas(), &g));
    assert!(may_use(&reg, ALICE, &g));
    assert!(!may_use(&reg, STRANGER, &g));
}

#[test]
fn the_info_is_none_for_a_stranger_and_an_instance_for_a_member() {
    let mut reg = world();
    pair(&mut reg);
    capital(&mut reg, HERE);
    let g = structure::get(reg.deltas(), 11).unwrap();
    let now = crate::pyutil::filetime_now();
    set_charge(&mut reg, 11, 123.0, now);
    assert_eq!(info_at(&mut reg, STRANGER, &g, now), PyValue::None);
    let got = info_at(&mut reg, ALICE, &g, now);
    assert_eq!(attr(&got, "_valid_ship_type"), PyValue::Bool(true));
    assert_eq!(attr(&got, "_sim_timestamp"), PyValue::Long(now));
    let PyValue::Real(c) = attr(&got, "_structure_capacitor") else {
        panic!("a Real charge");
    };
    assert!((c - 123.0).abs() < 0.5, "{c}");
    // A banned hull is not a valid ship type.
    reg.character_mut(ALICE).unwrap().ship_type_id = SHIP_BANNED;
    let got = info_at(&mut reg, ALICE, &g, now);
    assert_eq!(attr(&got, "_valid_ship_type"), PyValue::Bool(false));
}

#[test]
fn the_component_call_answers_through_the_router_shape() {
    let mut reg = world();
    pair(&mut reg);
    let g = structure::get(reg.deltas(), 11).unwrap();
    assert_eq!(locate(&reg, 11), Some(g.system));
    assert_eq!(locate(&reg, 9_999), None);
    let call = crate::space_component::CompCall {
        actor: ALICE,
        system: HERE,
        item_id: 11,
        args: &[],
        kwargs: &[],
    };
    let got = info(&mut reg, &call).unwrap();
    assert!(matches!(got, PyValue::ObjectEx { .. }), "{got:?}");
}

/// The exact wire shape the client unmarshals: `JumpBridgeAuthorizedInfo`
/// derives from `object`, so it is tag `0x23` with the header
/// `((Global(class),), {state})` and empty list and dict streams. The old
/// shape, an old-style instance (`0x17`), the client refuses as "should be of
/// class type but is of <type 'type'>".
#[test]
fn the_info_is_a_new_style_object_with_the_init_keys() {
    let mut reg = world();
    pair(&mut reg);
    capital(&mut reg, HERE);
    let g = structure::get(reg.deltas(), 11).unwrap();
    let now = crate::pyutil::filetime_now();
    set_charge(&mut reg, 11, 50.0, now);
    let got = info_at(&mut reg, ALICE, &g, now);
    let PyValue::ObjectEx {
        form,
        header,
        list,
        dict,
    } = &got
    else {
        panic!("tag 0x23 expected, got {got:?}")
    };
    assert_eq!(*form, eve_marshal::ObjectForm::New);
    assert!(list.is_empty() && dict.is_empty());
    let PyValue::Tuple(parts) = &**header else {
        panic!("{header:?}")
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0],
        PyValue::Tuple(vec![PyValue::Global(INFO_CLASS.as_bytes().to_vec())])
    );
    assert_eq!(
        INFO_CLASS,
        "jumpbridge.common.jump_bridge_authorized_info.JumpBridgeAuthorizedInfo"
    );
    let PyValue::Dict(pairs) = &parts[1] else {
        panic!("{:?}", parts[1])
    };
    let mut keys: Vec<String> = pairs
        .iter()
        // Stored `(value, key)` (`pyutil::pypairs`).
        .map(|(_, k)| match k {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(u) => u.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["_sim_timestamp", "_structure_capacitor", "_valid_ship_type"]
    );
    // On the wire: tag 0x23 right after the 5-byte `0x7E` header, not 0x17.
    let bytes = crate::dumps(&got);
    assert_eq!(bytes[5] & 0x3f, 0x23, "{:02x?}", &bytes[..8]);
}

#[test]
fn a_jump_and_the_gm_command_change_the_etag() {
    let mut reg = world();
    pair(&mut reg);
    near_gate_a(&mut reg, ALICE, ALICE_SHIP);
    let g = structure::get(reg.deltas(), 11).unwrap();
    let etag = |reg: &Registry| {
        slim_keys(reg, &structure::get(reg.deltas(), 11).unwrap())
            .into_iter()
            .find(|(k, _)| *k == "component_jumpBridge")
            .map(|(_, v)| v)
            .unwrap()
    };
    let e0 = etag(&reg);
    let say = |v: PyValue| match v {
        PyValue::Bytes(b) => String::from_utf8(b).unwrap(),
        o => panic!("{o:?}"),
    };
    let r = say(slash(&mut reg, Some(ALICE), &["charge", "11", "50"]).unwrap());
    assert!(r.contains("holds 50.0"), "{r}");
    let e1 = etag(&reg);
    assert_ne!(e0, e1, "the GM set changes the etag");
    assert!((charge(&reg, 11) - 50.0).abs() < 0.5);
    capital(&mut reg, HERE);
    set_charge(&mut reg, 11, 100.0, crate::pyutil::filetime_now());
    assert!(jump(&mut reg, ALICE, g.id).is_ok());
    assert_ne!(etag(&reg), e1, "a jump changes the etag");
    // `full`, here, and a bad amount.
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        ALICE_SHIP,
        HERE,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let r = say(slash(&mut reg, Some(ALICE), &["charge", "here", "full"]).unwrap());
    assert!(r.contains(&format!("holds {CAP:.1}")), "{r}");
    assert!(
        say(slash(&mut reg, Some(ALICE), &["charge", "here", "lots"]).unwrap())
            .starts_with("/jbridge")
    );
    assert!(
        say(slash(&mut reg, Some(ALICE), &["charge", "5", "1"]).unwrap())
            .contains("not a jump gate")
    );
    // Over the capacity is held at it.
    let r = say(slash(&mut reg, Some(ALICE), &["charge", "11", "99999"]).unwrap());
    assert!(r.contains(&format!("holds {CAP:.1}")), "{r}");
}

#[test]
fn the_table_parses_its_lines_and_refuses_a_bad_one() {
    let (id, row) = Bridges::parse_line("9960999\t900\t1\t2.5\t3\t4\t5").unwrap();
    assert_eq!((id, row.restriction_list), (9_960_999, Some(900)));
    assert_eq!(row.zones, [1.0, 2.5, 3.0, 4.0, 5.0]);
    let (_, none) = Bridges::parse_line("9960999\t-\t1\t1\t1\t1\t1").unwrap();
    assert_eq!(none.restriction_list, None);
    assert!(Bridges::parse_line("9960999\t900\t1\t2").is_err());
    assert!(Bridges::parse_line("0\t900\t1\t1\t1\t1\t1").is_err());
    assert!(Bridges::parse_line("9960999\t900\t1\tx\t1\t1\t1").is_err());
    assert!(Bridges::parse_line("9960999\t900\t-1\t1\t1\t1\t1").is_err());
}
