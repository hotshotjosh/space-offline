//! `/entity siege`: GM-ordered NPCs besiege an Upwell structure, on a real
//! `Registry` with invented ids and rows.

use super::tests::{ALICE, ALICE_SHIP, SYSTEM, hijacker_near, hijacker_near_kind, world};
use super::*;
use crate::notifications as notif;
use crate::structure::{self, State, Structure, Upkeep};
use crate::structure_combat::tests::{OWNER_CORP, astrahus_row, attr_text};
use crate::structure_space::ASTRAHUS;

/// The structure's id; no roster ship has it.
const STRUCTURE: i64 = 7;
/// An invented tackling type (scram, web and neutralizer effects).
const TACKLER: i64 = 990_300_002;

/// Invented rows, **not the client's numbers**: a hull with a scram (563), a
/// web (575) and a neutralizer (6691), each costing 5 capacitor a cycle
/// (discharge attribute 990001).
const TACKLER_ROWS: &str = "\
attribute\t114\t0.0\temDamage
effect\t563\t2\t1\t0\t990001\t0\t0\t0\tfixtureScram\t\t0
effect\t575\t2\t1\t0\t990001\t0\t0\t0\tfixtureWeb\t\t0
effect\t6691\t2\t1\t0\t990001\t0\t0\t0\tfixtureNeut\t\t0
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10,563,575,6691\t114=2.0;103=10000.0;105=2.0;504=1.0;505=4000.0;512=1.0;20=-40.0;513=4000.0;514=10000.0;97=30.0;98=8000.0;931=1.0;942=4000.0;990001=5.0;482=40.0;55=400000.0\tFixture Tackler
";

fn tackler() -> &'static NpcType {
    static KIND: std::sync::OnceLock<NpcType> = std::sync::OnceLock::new();
    KIND.get_or_init(|| NpcType {
        type_id: TACKLER,
        hull: Hull {
            type_id: TACKLER,
            ..FIXTURE_RAT.hull
        },
        ..FIXTURE_RAT
    })
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}{}{}",
        crate::typedata::FIXTURE,
        astrahus_row(&attr_text("")),
        TACKLER_ROWS
    );
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c.insert("npc_data", crate::npc_data::Table::fixture());
    c
}

fn now() -> BlueTime {
    crate::pyutil::filetime_now()
}

/// Alice in space, a shield-vulnerable structure of `OWNER_CORP` 5 km off
/// her, Alice a director of that corporation.
fn siege_world() -> Registry {
    let mut reg = world(&[ALICE]).with_content(std::sync::Arc::new(content()));
    let at = reg.roster().get(ALICE).expect("in space").motion.position;
    let e = crate::structure_space::env(&reg, now());
    let mut s = Structure::anchored(
        STRUCTURE,
        ASTRAHUS,
        OWNER_CORP,
        SYSTEM,
        [at[0] + 5_000.0, at[1], at[2]],
        0.0,
        "Target",
        1,
        &e,
    );
    s.set_state(State::ShieldVulnerable, &e);
    s.upkeep = Some(Upkeep::Full);
    structure::put(reg.deltas_mut(), &s);
    reg.corporations_mut()
        .enrol_for_test(ALICE, OWNER_CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    let _ = reg.take_pending();
    reg
}

fn ids(v: &View) -> Vec<i64> {
    v.candidates.iter().map(|c| c.ship_id).collect()
}

fn view(reg: &Registry, rat: i64) -> View {
    let n = *reg.npcs().get(rat).expect("live");
    view_at(reg, &n, now(), crate::space_state::now_seconds()).expect("a view")
}

fn shield(reg: &Registry) -> f64 {
    structure::get(reg.deltas(), STRUCTURE)
        .and_then(|s| s.damage)
        .map_or(1.0, |d| d.0)
}

#[test]
fn a_besieging_npc_sees_the_structure_and_one_without_an_order_does_not() {
    let mut reg = siege_world();
    let besieger = hijacker_near(&mut reg, ALICE, 3_000.0);
    let bystander = hijacker_near(&mut reg, ALICE, 4_000.0);
    reg.npcs_mut().set_siege(besieger, Some(STRUCTURE));
    // Alice has done nothing to it: it sees the structure and not her.
    assert_eq!(ids(&view(&reg, besieger)), vec![STRUCTURE]);
    // The one with no order sees her and not the structure.
    assert_eq!(ids(&view(&reg, bystander)), vec![ALICE_SHIP]);
    // The structure's candidate is at its surface, not its centre.
    let v = view(&reg, besieger);
    let c = v
        .candidates
        .iter()
        .find(|c| c.ship_id == STRUCTURE)
        .unwrap();
    let gap = crate::structure_combat::surface_gap(&reg, v.me, STRUCTURE).unwrap();
    assert!((crate::space_state::distance(v.me, c.position) - gap).abs() < 1e-6);
    // Once she acts on the besieger it sees her, and the structure is left
    // out so that she comes first.
    crate::npc_threat::on_act(&mut reg, ALICE, besieger, 5.0, now());
    let v = view(&reg, besieger);
    assert_eq!(ids(&v), vec![ALICE_SHIP]);
    let n = *reg.npcs().get(besieger).expect("live");
    assert_eq!(
        brain::decide(&n, &v).0.target,
        Some(ALICE_SHIP),
        "the player who shot it comes first"
    );
    // The order lifted, it sees what it always saw.
    reg.npcs_mut().set_siege(besieger, None);
    assert_eq!(ids(&view(&reg, besieger)), vec![ALICE_SHIP]);
    reg.npcs_mut().set_siege(besieger, Some(STRUCTURE));
    // The order is dropped with the NPC.
    reg.npcs_mut().remove(besieger);
    assert_eq!(reg.npcs().siege_of(besieger), None);
    // And a besieger decides to lock and shoot the structure when alone.
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    let n = *reg.npcs().get(rat).expect("live");
    let (after, out) = brain::decide(&n, &view(&reg, rat));
    assert_eq!(after.target, Some(STRUCTURE));
    assert!(out.contains(&Intent::Lock(STRUCTURE)));
}

#[test]
fn a_besieging_npc_volley_takes_shield_and_tells_the_owner_once() {
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    let n = *reg.npcs().get(rat).expect("live");
    let t = now();
    let mut actions = Vec::new();
    // The notification waits for the shield to fall under its threshold, and
    // the structure takes only so much a second: four volleys, a few
    // seconds apart, each far over the cap.
    for i in 0..4 {
        brain::apply(
            &mut reg,
            &n,
            Intent::Shoot {
                target: STRUCTURE,
                at: t + 3 * i * structure::SECOND,
                mult: 1_000.0,
            },
            t,
            &mut actions,
        );
    }
    assert!(shield(&reg) < 1.0, "the shield moved: {}", shield(&reg));
    let got: Vec<_> = notif::of(reg.deltas(), ALICE)
        .into_iter()
        .filter(|n| n.type_id == crate::structure_notify::STRUCTURE_UNDER_ATTACK)
        .collect();
    assert_eq!(got.len(), 1, "one 184 for four volleys");
    let char_id = got[0]
        .data
        .iter()
        .find(|(k, _)| k == "charID")
        .map(|(_, v)| v.clone());
    assert_eq!(char_id, Some(notif::Value::Int(FIXTURE_CORP)));
}

#[test]
fn a_besieger_does_not_scram_a_structure() {
    let mut reg = siege_world();
    let rat = hijacker_near_kind(&mut reg, ALICE, 3_000.0, tackler());
    let n = *reg.npcs().get(rat).expect("live");
    let t = now();
    let cap = |reg: &Registry| crate::modules::capacitor_at(reg, rat, TACKLER, t).0;
    // The type does tackle: a scram on Alice costs its capacitor.
    let full = cap(&reg);
    let mut actions = Vec::new();
    brain::apply(
        &mut reg,
        &n,
        Intent::Neut {
            target: ALICE_SHIP,
            at: t,
        },
        t,
        &mut actions,
    );
    assert!(cap(&reg) < full, "a neut at a pilot is paid for");
    let before = cap(&reg);
    for intent in [
        Intent::Scram {
            target: STRUCTURE,
            at: t,
        },
        Intent::Web {
            target: STRUCTURE,
            at: t,
        },
        Intent::Neut {
            target: STRUCTURE,
            at: t,
        },
    ] {
        brain::apply(&mut reg, &n, intent, t, &mut actions);
    }
    assert_eq!(cap(&reg), before, "nothing was paid for a structure");
}

#[test]
fn entity_siege_orders_only_gm_spawned_npcs_in_the_system() {
    let mut reg = siege_world();
    let gm = hijacker_near(&mut reg, ALICE, 3_000.0);
    let belt = hijacker_near(&mut reg, ALICE, 4_000.0);
    let gm_away = hijacker_near(&mut reg, ALICE, 5_000.0);
    reg.npcs_mut().note_gm(&[gm, gm_away]);
    // One GM NPC is somewhere else: not in the actor's system.
    reg.npcs_mut()
        .get_mut(gm_away)
        .expect("live")
        .solar_system_id = SYSTEM + 1;
    let id = STRUCTURE.to_string();
    let _ = entity_slash(&mut reg, Some(ALICE), &["siege", &id]);
    assert_eq!(reg.npcs().siege_of(gm), Some(STRUCTURE));
    assert_eq!(reg.npcs().siege_of(belt), None, "not GM-spawned");
    assert_eq!(reg.npcs().siege_of(gm_away), None, "another system");
    // A structure that is not there, or not a structure, orders nothing.
    let _ = entity_slash(&mut reg, Some(ALICE), &["siege", "off"]);
    assert_eq!(reg.npcs().siege_of(gm), None, "off lifts it");
    for bad in ["424242", "not-a-number", &ALICE_SHIP.to_string()] {
        let _ = entity_slash(&mut reg, Some(ALICE), &["siege", bad]);
        assert_eq!(reg.npcs().siege_of(gm), None, "{bad} orders nothing");
    }
    // Nobody in space: nothing.
    let _ = entity_slash(&mut reg, None, &["siege", &id]);
    assert_eq!(reg.npcs().siege_of(gm), None);
}

#[test]
fn a_sieging_npc_is_not_idle_alone() {
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    let n = *reg.npcs().get(rat).expect("live");
    assert!(idle_alone(&reg, &n, now()), "an idle rat is skipped alone");
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    assert!(!idle_alone(&reg, &n, now()), "a besieger is not");
}

#[test]
fn the_surface_point_is_the_surface_gap_away_and_none_for_a_non_structure() {
    let reg = siege_world();
    let from = [0.0, 0.0, 0.0];
    let p = crate::structure_combat::surface_point(&reg, from, STRUCTURE).expect("a point");
    let gap = crate::structure_combat::surface_gap(&reg, from, STRUCTURE).expect("a gap");
    assert!((crate::space_state::distance(from, p) - gap).abs() < 1e-6);
    // Inside the radius: where it already is.
    let t = crate::structure_combat::fixed_target(&reg, STRUCTURE).unwrap();
    assert_eq!(
        crate::structure_combat::surface_point(&reg, t.position, STRUCTURE),
        Some(t.position)
    );
    assert_eq!(
        crate::structure_combat::surface_point(&reg, from, 424_242),
        None
    );
}

#[test]
fn a_besieger_on_the_structure_turns_on_a_player_who_shoots_it_whatever_its_type() {
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    // The fixture type has no switch timer: only the view can turn it.
    assert!(
        brain::Behaviour::of(&reg, FIXTURE_RAT.type_id)
            .switch_every
            .is_none()
    );
    let n = *reg.npcs().get(rat).expect("live");
    let (on_structure, _) = brain::decide(&n, &view(&reg, rat));
    assert_eq!(on_structure.target, Some(STRUCTURE));
    crate::npc_threat::on_act(&mut reg, ALICE, rat, 5.0, now());
    let v = view(&reg, rat);
    let (first, _) = brain::decide(&on_structure, &v);
    let (second, _) = brain::decide(&first, &v);
    assert_eq!(second.target, Some(ALICE_SHIP), "within two decides");
}

#[test]
fn an_order_outlives_its_structure_as_a_passive_one() {
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    let n = *reg.npcs().get(rat).expect("live");
    assert!(!idle_alone(&reg, &n, now()));
    assert!(structure::remove(reg.deltas_mut(), STRUCTURE));
    // No structure to look at: idle as any rat, and not turned hostile to
    // the bystander in range.
    assert!(idle_alone(&reg, &n, now()));
    assert!(view(&reg, rat).candidates.is_empty());
}

#[test]
fn the_tick_loop_locks_keeps_the_lock_fires_and_takes_the_shield() {
    let mut reg = siege_world();
    let rat = hijacker_near(&mut reg, ALICE, 3_000.0);
    reg.npcs_mut().set_siege(rat, Some(STRUCTURE));
    let t0 = now();
    let mut locked = false;
    for i in 0..90 {
        tick(&mut reg, t0 + i * structure::SECOND);
        let _ = crate::targeting::sweep_out_of_range(&mut reg);
        locked |= reg.locks().targets_of(rat).contains(&STRUCTURE);
        if shield(&reg) < 1.0 {
            break;
        }
    }
    assert!(locked, "its lock on the structure was added");
    assert!(
        reg.locks().targets_of(rat).contains(&STRUCTURE),
        "and the range sweep kept it"
    );
    assert!(shield(&reg) < 1.0, "the shield dropped");
}

#[test]
fn the_reply_counts_the_ordered_that_are_too_far_to_notice_the_structure() {
    let mut reg = siege_world();
    let near = hijacker_near(&mut reg, ALICE, 3_000.0);
    let far = hijacker_near(&mut reg, ALICE, -60_000.0);
    reg.npcs_mut().note_gm(&[near, far]);
    let reply = entity_slash(&mut reg, Some(ALICE), &["siege", &STRUCTURE.to_string()]);
    let text = match reply {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(text.starts_with("2 NPCs ordered"), "{text}");
    assert!(text.contains("1 of them beyond 20 km"), "{text}");
}
