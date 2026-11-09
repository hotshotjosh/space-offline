//! Tests of a structure's death (#48) and its 184 notice. Every id is
//! invented; the wreck type is whatever the store's own table lists first.

use super::*;
use crate::damage::Damage;
use crate::model::{Character, SandboxConfig};
use crate::notifications::Value;
use crate::space_state::{Motion, Presence};
use crate::structure_combat::tests::{ARMOR, HULL, SHIELD, content_with};
use crate::structure_space::ASTRAHUS;

const BOSS: i64 = 7;
const PAL: i64 = 8;
const FOE: i64 = 9;
const CORP: i64 = 11;
const FOE_CORP: i64 = 13;
const FOE_ALLIANCE: i64 = 14;
const SYSTEM: i64 = 31;
const GUN: i64 = 990_700_002;

fn ship_of(who: i64) -> i64 {
    1_000 + who
}

fn wreck_type() -> i64 {
    crate::item::WRECK_TYPES[0].type_id
}

/// Three pilots in space in one system: the boss and a pal of one
/// corporation (the boss a director), a foe of another. The boss spawns an
/// Astrahus whose hull table names a wreck.
fn world() -> (Registry, i64) {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
        corporation_id: corp,
        solar_system_id: Some(SYSTEM),
        ship_id: ship_of(id),
        ship_type_id: crate::service::TYPE_CAPSULE,
        ..Character::default()
    };
    // No per-second limits: one volley can empty a layer.
    let mut content = content_with(&format!(
        "{ATTR_SHIELD_CAPACITY}={SHIELD};{ATTR_ARMOR_HP}={ARMOR};{ATTR_HULL_HP}={HULL}"
    ));
    content.insert(
        "wrecks",
        crate::combat::Wrecks::from_pairs([(ASTRAHUS, wreck_type())]),
    );
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![ch(BOSS, CORP), ch(PAL, CORP), ch(FOE, FOE_CORP)],
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    reg.corporations_mut()
        .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    reg.corporations_mut().enrol_for_test(PAL, CORP, 0);
    reg.corporations_mut().enrol_for_test(FOE, FOE_CORP, 0);
    for (n, who) in [BOSS, PAL, FOE].into_iter().enumerate() {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship_of(who),
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    crate::service::structures::slash(
        &mut reg,
        Some(BOSS),
        &["spawn", "astrahus", "Invented", "Home"],
    )
    .expect("spawns");
    let id = structure::all(reg.deltas()).last().expect("spawned").id;
    let _ = reg.take_pending();
    (reg, id)
}

fn set_state(reg: &mut Registry, id: i64, state: State) {
    let mut s = structure::get(reg.deltas(), id).unwrap();
    let e = crate::structure_space::env(reg, crate::pyutil::filetime_now());
    s.set_state(state, &e);
    s.vulnerable = Some(true);
    if state == State::HullVulnerable {
        // A hull is only vulnerable once the layers above it are gone.
        s.set_damage(0.0, 0.0, 1.0, &e);
    }
    let _ = s.take_events();
    structure::put(reg.deltas_mut(), &s);
}

fn volley(target: i64, kinetic: f64) -> Volley {
    Volley {
        shooter: FOE,
        attacker_ship: ship_of(FOE),
        weapon_type: GUN,
        target,
        damage: Damage {
            kinetic,
            ..Damage::ZERO
        },
        hit_quality: 3,
        banked: false,
        at: crate::pyutil::filetime_now(),
    }
}

fn received(reg: &Registry, who: i64, type_id: i64) -> Vec<crate::notifications::Notification> {
    crate::notifications::of(reg.deltas(), who)
        .into_iter()
        .filter(|n| n.type_id == type_id)
        .collect()
}

fn get<'a>(n: &'a crate::notifications::Notification, key: &str) -> &'a Value {
    &n.data
        .iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("{key} missing in {:?}", n.data))
        .1
}

/// A volley that empties a whole hull in the hull-vulnerable state.
fn kill(reg: &mut Registry, id: i64) {
    set_state(reg, id, State::HullVulnerable);
    hit(reg, volley(id, 1.0e12), crate::pyutil::filetime_now());
}

#[test]
fn a_dead_hull_removes_the_row_the_item_and_the_fire_record() {
    let (mut reg, id) = world();
    assert!(reg.store().get(id).is_some(), "the spawn made an item");
    kill(&mut reg, id);
    assert!(structure::get(reg.deltas(), id).is_none());
    assert!(reg.store().get(id).is_none());
    assert_eq!(reg.ext_mut::<Fire>().take_last_shooter(id), None);
}

#[test]
fn a_dead_hull_leaves_the_hulls_wreck_owned_by_the_corporation() {
    let (mut reg, id) = world();
    let at = structure::get(reg.deltas(), id).unwrap().pos;
    kill(&mut reg, id);
    let wrecks: Vec<_> = reg.combat().wrecks().copied().collect();
    assert_eq!(wrecks.len(), 1, "{wrecks:?}");
    let wr = wrecks[0];
    assert_eq!(wr.type_id, wreck_type());
    assert_eq!((wr.solar_system_id, wr.ship_id), (SYSTEM, id));
    assert_eq!((wr.owner_id, wr.corp_id), (CORP, CORP));
    assert_eq!(wr.position, at);
}

#[test]
fn a_dead_hull_files_a_killmail_with_a_corporation_victim() {
    let (mut reg, id) = world();
    kill(&mut reg, id);
    let reports = crate::killmail::involving(&reg, FOE, None, 5);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!(r.victim, 0, "no victim character");
    assert_eq!(r.victim_corporation, CORP);
    assert_eq!(r.ship_type, ASTRAHUS);
    assert_eq!(r.system, SYSTEM);
    assert_eq!(r.final_blow.character, Some(FOE));
    assert_eq!(r.final_blow.corporation, Some(FOE_CORP));
    assert!(r.damage_taken > 0);
    // The victim's corporation sees it among its losses, and nobody is
    // indexed under the character 0.
    let losses = crate::killmail::corp_involving(&reg, CORP, true, None, 5);
    assert_eq!(losses.len(), 1);
    assert!(crate::killmail::involving(&reg, 0, None, 5).is_empty());
    // The pilot who fired the last volley is told (141), with a null victim.
    let n = received(
        &reg,
        FOE,
        crate::notifications::kind::KILL_REPORT_FINAL_BLOW,
    );
    assert_eq!(n.len(), 1);
    assert_eq!(get(&n[0], "victimID"), &Value::None);
    assert_eq!(get(&n[0], "killMailID"), &Value::Int(r.kill_id));
    assert_eq!(
        get(&n[0], "killMailHash"),
        &Value::Text(crate::notifications::kill_hash(r))
    );
}

#[test]
fn a_dead_hull_tells_the_managers_with_the_formatters_keys() {
    let (mut reg, id) = world();
    let s = structure::get(reg.deltas(), id).unwrap();
    kill(&mut reg, id);
    let got = received(&reg, BOSS, 188);
    assert_eq!(got.len(), 1);
    assert_eq!(get(&got[0], "structureID"), &Value::Int(id));
    assert_eq!(get(&got[0], "structureTypeID"), &Value::Int(s.type_id));
    assert_eq!(get(&got[0], "solarsystemID"), &Value::Int(SYSTEM));
    assert_eq!(
        get(&got[0], "structureShowInfoData"),
        &Value::Tuple(vec![
            Value::Text("showinfo".to_owned()),
            Value::Int(s.type_id),
            Value::Int(id)
        ])
    );
    assert!(matches!(get(&got[0], "ownerCorpName"), Value::Text(_)));
    assert!(matches!(get(&got[0], "ownerCorpLinkData"), Value::Tuple(t) if t.len() == 3));
    assert_eq!(get(&got[0], "isAbandoned"), &Value::Bool(false));
}

#[test]
fn a_dead_hull_sends_one_kill_batch_and_forgets_it_in_the_directory() {
    let (mut reg, id) = world();
    kill(&mut reg, id);
    let sent: Vec<&'static str> = crate::combat::tests::due(&mut reg)
        .into_iter()
        .map(|(_, n, _)| n)
        .chain(reg.take_pending().iter().map(|(_, p)| p.to_push().0))
        .collect();
    assert!(sent.contains(&"OnStructureRemovedFromSpace"), "{sent:?}");
    assert!(
        sent.contains(&"DoDestinyUpdate"),
        "a destiny batch carries the explosion: {sent:?}"
    );
}

#[test]
fn a_dead_hull_drops_the_locks_on_it() {
    let (mut reg, id) = world();
    reg.locks_mut().add(ship_of(FOE), id);
    reg.locks_mut().add(ship_of(PAL), id);
    kill(&mut reg, id);
    assert!(reg.locks().targeters_of(id).is_empty());
}

#[test]
fn a_hull_gone_in_a_reinforced_state_is_held_at_the_floor_not_killed() {
    let (mut reg, id) = world();
    set_state(&mut reg, id, State::ShieldVulnerable);
    // One volley through every layer from the shield: the structure
    // reinforces, and the dead hull is not death.
    hit(&mut reg, volley(id, 1.0e12), crate::pyutil::filetime_now());
    let s = structure::get(reg.deltas(), id).expect("still there");
    assert_ne!(s.state, State::ShieldVulnerable);
    assert!(s.damage.unwrap().2 > 0.0);
    assert!(received(&reg, BOSS, 188).is_empty());
    assert!(reg.combat().wrecks().next().is_none());
}

#[test]
fn a_destroyed_structure_restarts_clean() {
    let (mut reg, id) = world();
    kill(&mut reg, id);
    // A real save and load: a new process with the stored world over it.
    let snap = reg.to_snapshot();
    let mut again = Registry::new(SandboxConfig::default());
    again.apply_snapshot(snap);
    assert!(structure::get(again.deltas(), id).is_none());
    assert!(structure::all(again.deltas()).is_empty());
    assert!(crate::structure_space::balls_in(&again, SYSTEM).is_empty());
    assert!(again.store().get(id).is_none());
    // The notice and the report are kept in the saved world.
    assert_eq!(received(&again, BOSS, 188).len(), 1);
    assert_eq!(crate::killmail::involving(&again, FOE, None, 5).len(), 1);
}

#[test]
fn two_volleys_in_one_tick_kill_it_once() {
    let (mut reg, id) = world();
    set_state(&mut reg, id, State::HullVulnerable);
    let now = crate::pyutil::filetime_now();
    hit(&mut reg, volley(id, 1.0e12), now);
    hit(&mut reg, volley(id, 1.0e12), now);
    assert_eq!(crate::killmail::involving(&reg, FOE, None, 5).len(), 1);
    assert_eq!(reg.combat().wrecks().count(), 1);
    assert_eq!(received(&reg, BOSS, 188).len(), 1);
}

#[test]
fn a_kill_between_two_corporations_at_war_is_filed_under_the_war() {
    let (mut reg, id) = world();
    let now = crate::pyutil::filetime_now();
    crate::war::put(
        reg.deltas_mut(),
        &crate::war::War::new(
            1,
            FOE_CORP,
            CORP,
            now - 2 * crate::war::WAR_SPOOLUP,
            crate::war::WAR_SPOOLUP,
            None,
        ),
    );
    kill(&mut reg, id);
    let r = crate::killmail::involving(&reg, FOE, None, 1).remove(0);
    assert_eq!(crate::war::war_of_kill(&reg, r.kill_id), Some(1));
}

#[test]
fn a_kill_with_no_war_is_filed_under_none() {
    let (mut reg, id) = world();
    kill(&mut reg, id);
    let r = crate::killmail::involving(&reg, FOE, None, 1).remove(0);
    assert_eq!(crate::war::war_of_kill(&reg, r.kill_id), None);
}

#[test]
fn the_first_aggression_notifies_the_managers_once_with_the_shooter() {
    let (mut reg, id) = world();
    reg.character_mut(FOE).unwrap().alliance_id = Some(FOE_ALLIANCE);
    set_state(&mut reg, id, State::ShieldVulnerable);
    let now = crate::pyutil::filetime_now();
    // Enough to take the shield well under the notification threshold.
    hit(&mut reg, volley(id, SHIELD * 0.5), now);
    let got = received(&reg, BOSS, 184);
    assert_eq!(got.len(), 1, "{:?}", got);
    let n = &got[0];
    assert_eq!(get(n, "charID"), &Value::Int(FOE));
    assert_eq!(get(n, "structureID"), &Value::Int(id));
    let Value::Real(shield) = get(n, "shieldPercentage") else {
        panic!("a percentage is a real")
    };
    assert!(*shield > 0.0 && *shield < 100.0, "{shield}");
    assert_eq!(get(n, "armorPercentage"), &Value::Real(100.0));
    assert_eq!(get(n, "hullPercentage"), &Value::Real(100.0));
    assert!(matches!(get(n, "corpName"), Value::Text(_)));
    assert_eq!(
        get(n, "corpLinkData"),
        &crate::structure_notify::show_info(crate::structure_notify::TYPE_CORPORATION, FOE_CORP)
    );
    assert_eq!(get(n, "allianceID"), &Value::Int(FOE_ALLIANCE));
    assert!(matches!(get(n, "allianceName"), Value::Text(_)));
    assert_eq!(
        get(n, "allianceLinkData"),
        &crate::structure_notify::show_info(crate::structure_notify::TYPE_ALLIANCE, FOE_ALLIANCE)
    );
    // A second volley in the same window says nothing more.
    hit(&mut reg, volley(id, SHIELD * 0.01), now + structure::SECOND);
    assert_eq!(received(&reg, BOSS, 184).len(), 1, "once");
}

#[test]
fn a_gm_damage_first_aggression_still_sends_a_formatted_184() {
    let (mut reg, id) = world();
    set_state(&mut reg, id, State::ShieldVulnerable);
    let mut s = structure::get(reg.deltas(), id).unwrap();
    let e = crate::structure_space::env(&reg, crate::pyutil::filetime_now());
    s.set_damage(0.2, 1.0, 1.0, &e);
    let events = s.take_events();
    structure::put(reg.deltas_mut(), &s);
    crate::structure_space::react(&mut reg, id, &events);
    let got = received(&reg, BOSS, 184);
    assert_eq!(got.len(), 1);
    // Every key the formatter reads is there, whoever shot.
    for key in [
        "structureID",
        "shieldPercentage",
        "armorPercentage",
        "hullPercentage",
        "charID",
        "corpName",
        "corpLinkData",
    ] {
        let _ = get(&got[0], key);
    }
}

/// One attribute of a `KeyVal` (its state dict is stored value first).
fn attr(v: &eve_marshal::PyValue, name: &str) -> eve_marshal::PyValue {
    let eve_marshal::PyValue::Instance { state, .. } = v else {
        panic!("not a KeyVal: {v:?}")
    };
    state
        .dict_as_kv()
        .and_then(|kv| {
            kv.into_iter().find_map(|(k, val)| {
                (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then(|| val.clone())
            })
        })
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn the_client_reads_the_report_with_a_null_victim_and_opens_it_by_its_own_hash() {
    use eve_marshal::PyValue;
    let (mut reg, id) = world();
    kill(&mut reg, id);
    let r = crate::killmail::involving(&reg, FOE, None, 1).remove(0);
    let call = |reg: &mut Registry, service: &str, method: &str, args: &[PyValue]| {
        reg.dispatch(&crate::service::Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(FOE),
        })
        .expect("answered")
    };
    let PyValue::List(rows) = call(
        &mut reg,
        "charMgr",
        "GetRecentShipKillsAndLosses",
        &[PyValue::Int(25), PyValue::None],
    ) else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(attr(&rows[0], "victimCharacterID"), PyValue::None);
    assert_eq!(attr(&rows[0], "victimCorporationID"), PyValue::Int(CORP));
    assert_eq!(attr(&rows[0], "victimShipTypeID"), PyValue::Int(ASTRAHUS));
    // `GetKillReportHashValue` on the row as the client holds it: Python
    // prints the null victim as `None`.
    let PyValue::Int(time) = attr(&rows[0], "killTime") else {
        panic!("killTime")
    };
    let hash = crate::notifications::kill_hash_of(0, Some(FOE), ASTRAHUS, time);
    assert_eq!(hash, crate::notifications::kill_hash(&r));
    let opened = call(
        &mut reg,
        "warStatisticMgr",
        "GetKillMail",
        &[
            PyValue::Int(r.kill_id),
            PyValue::Bytes(hash.clone().into_bytes()),
        ],
    );
    assert_eq!(attr(&opened, "killID"), PyValue::Int(r.kill_id));
    // And a hash made with the victim printed as `0` opens nothing.
    let wrong = crate::accounts::hex(&crate::notifications::sha1(
        format!("0{FOE}{ASTRAHUS}{time}").as_bytes(),
    ));
    assert_ne!(wrong, hash);
    let nothing = call(
        &mut reg,
        "warStatisticMgr",
        "GetKillMail",
        &[PyValue::Int(r.kill_id), PyValue::Bytes(wrong.into_bytes())],
    );
    assert_eq!(nothing, PyValue::None);
}
