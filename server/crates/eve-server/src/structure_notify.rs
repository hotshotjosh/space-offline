//! The notifications an Upwell structure sends its owner's managers (issue
//! #47, lane L4; shared with the destruction lane, which adds `destroyed`
//! for 188 beside these).
//!
//! The keys of each `data` dict are the client formatter's `MakeData` **[V]**
//! (`notifications/common/formatters/structures.py`); the type numbers are
//! `notificationconst.py:172-195` **[V]**. Times in the data are FILETIME
//! ticks (100 ns), as the formatters subtract `blue.os.GetWallclockTime()`.
//!
//! **[A]** Who hears one: every member of the owner corporation who may
//! manage structures ([`crate::service::sov_hub::may_launch`]: director,
//! station manager, CEO), online or not (`notify_at` stores for the offline).
//! Falsifier: a TQ corp member without those roles who receives them, or a
//! manager who does not. The sender is the owner corporation. Falsifier: the
//! notification window naming another sender.
//!
//! **[A]** `vulnerableTime` is the repair time of the state that follows
//! ([`structure::State::repair_secs`]). Falsifier: a TQ notification whose
//! "vulnerable for" text differs.

use crate::notifications::{self as notif, Data, Value, data};
use crate::service::Registry;
use crate::structure::{Event, SECOND, State, Structure};

pub const STRUCTURE_ANCHORING: i64 = 182;
pub const STRUCTURE_UNANCHORING: i64 = 183;
pub const STRUCTURE_UNDER_ATTACK: i64 = 184;
pub const STRUCTURE_ONLINE: i64 = 185;
pub const STRUCTURE_LOST_SHIELDS: i64 = 186;
pub const STRUCTURE_LOST_ARMOR: i64 = 187;
pub const STRUCTURE_DESTROYED: i64 = 188;
pub const STRUCTURES_REINFORCEMENT_CHANGED: i64 = 209;

/// `const.typeCorporation` **[V]** (`inventorycommon/const.py`).
pub const TYPE_CORPORATION: i64 = 2;
/// `const.typeAlliance` **[V]**.
pub const TYPE_ALLIANCE: i64 = 16159;
/// `const.typeFaction` **[V]** (`inventorycommon/const.py:1502`): the link type
/// the 184 formatter uses when the aggressor's alliance id is a faction.
pub const TYPE_FACTION: i64 = 30;

/// `('showinfo', typeID, id)`, the link tuple the formatters build.
pub fn show_info(type_id: i64, id: i64) -> Value {
    Value::Tuple(vec![
        Value::Text("showinfo".to_owned()),
        Value::Int(type_id),
        Value::Int(id),
    ])
}

/// `BaseStructureNotificationFormatter.GetBaseData` **[V]**: `structureID`,
/// `structureShowInfoData`, `solarsystemID` (lower-case s, the file's own
/// constant) and `structureTypeID`.
pub fn base_data(s: &Structure) -> Data {
    data([
        ("structureID", Value::Int(s.id)),
        ("structureShowInfoData", show_info(s.type_id, s.id)),
        ("solarsystemID", Value::Int(s.system)),
        ("structureTypeID", Value::Int(s.type_id)),
    ])
}

/// `ownerCorpName` and `ownerCorpLinkData`, which 182 and 183 add **[V]**.
fn owner_fields(reg: &Registry, corp: i64) -> Data {
    let name = reg
        .corporations()
        .get(corp)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    data([
        ("ownerCorpName", Value::Text(name)),
        ("ownerCorpLinkData", show_info(TYPE_CORPORATION, corp)),
    ])
}

/// Who hears a structure notification of `corp` **[A]**, in id order.
pub fn managers(reg: &Registry, corp: i64) -> Vec<i64> {
    reg.corporations()
        .members_of(corp)
        .into_iter()
        .filter(|c| crate::service::sov_hub::may_launch(reg, *c))
        .collect()
}

/// One notification of `type_id` to every manager of `corp`, the corporation
/// the sender **[A]**.
pub fn tell_corp(reg: &mut Registry, corp: i64, type_id: i64, data: Data) {
    for to in managers(reg, corp) {
        notif::notify_from(reg, to, type_id, corp, data.clone());
    }
}

/// [`tell_corp`] for the owner of `s`.
pub fn tell(reg: &mut Registry, s: &Structure, type_id: i64, data: Data) {
    tell_corp(reg, s.owner_corp, type_id, data);
}

/// `vulnerableTime`: the repair time of `next`, in ticks **[A]**.
fn vulnerable_ticks(next: State) -> i64 {
    next.repair_secs() * SECOND
}

/// `max(0, end - now)`, the formatters' `timeLeft`.
fn left(end: Option<i64>, now: i64) -> i64 {
    end.map_or(0, |e| (e - now).max(0))
}

/// 182 `StructureAnchoring.MakeData`: the structure has been anchored and
/// its timer runs `timeLeft` more.
pub fn anchoring(reg: &mut Registry, s: &Structure, now: i64) {
    let mut d = base_data(s);
    d.extend(owner_fields(reg, s.owner_corp));
    d.extend(data([
        ("timeLeft", Value::Int(left(s.timer.end, now))),
        (
            "vulnerableTime",
            Value::Int(vulnerable_ticks(State::OnliningVulnerable)),
        ),
    ]));
    tell(reg, s, STRUCTURE_ANCHORING, d);
}

/// 183 `StructureUnanchoring.MakeData`: the decommission countdown started.
pub fn unanchoring(reg: &mut Registry, s: &Structure, now: i64) {
    let mut d = base_data(s);
    d.extend(owner_fields(reg, s.owner_corp));
    d.push(("timeLeft".to_owned(), Value::Int(left(s.unanchoring, now))));
    tell(reg, s, STRUCTURE_UNANCHORING, d);
}

/// 185 `StructureOnline.MakeData`: base data only (`requiresDeedTypeID` is
/// set only when a deed is missing, which never happens today).
pub fn online(reg: &mut Registry, s: &Structure) {
    tell(reg, s, STRUCTURE_ONLINE, base_data(s));
}

/// 186 and 187 (`StructureLostShield`, `StructureLostArmor`): the timer the
/// structure now sits in. `to` is the reinforce state it entered.
pub fn lost_layer(reg: &mut Registry, s: &Structure, to: State, now: i64) {
    let (type_id, next) = match to {
        State::ArmorReinforce => (STRUCTURE_LOST_SHIELDS, State::ArmorVulnerable),
        State::HullReinforce => (STRUCTURE_LOST_ARMOR, State::HullVulnerable),
        _ => return,
    };
    let mut d = base_data(s);
    d.extend(data([
        ("timeLeft", Value::Int(left(s.timer.end, now))),
        ("timestamp", Value::Int(s.timer.end.unwrap_or(now))),
        ("vulnerableTime", Value::Int(vulnerable_ticks(next))),
    ]));
    tell(reg, s, type_id, d);
}

/// 184 `StructureUnderAttack.MakeData` **[V]** (`structures.py:152-171`): the
/// bars as percentages (`100 * fraction`, Real), the aggressor's `charID`,
/// `corpName`, `corpLinkData` ('showinfo', 2, corp), `allianceID` and, when
/// it is set, `allianceName` and `allianceLinkData` ('showinfo', 16159, id), or
/// ('showinfo', 30, id) when the id is an NPC faction.
/// `shooter` is the roster key of the last volley
/// ([`crate::structure_combat::Fire::take_last_shooter`]); damage nobody
/// fired (a GM command) sends no 184 **[V]**: a `charID` of None makes the
/// client's formatter raise (`_GetByMessageID ... int() NoneType`, the
/// 2026-11-10 client-CI shoot run's `client.log:34670`). An NPC's volley sends
/// one with its owner corp as `charID` **[A1]** and its faction as
/// `allianceID` **[A2]**.
///
/// **[A1]** TQ's `charID` for an NPC aggressor is the NPC's owner corp.
/// Falsifier: a TQ 184 for an NPC attack. The label half is settled **[V]**:
/// the body label reads `cfg.eveowners.Get(charID)` and links with that
/// owner's own type, so a corp id renders as the corp's name and link
/// (`localization/propertyHandlers/characterPropertyHandler.py`).
///
/// **[A2]** The NPC corp's faction is the aggressor alliance. Falsifier: a TQ
/// 184 whose `allianceID` is None for a rat attack.
pub fn under_attack(reg: &mut Registry, s: &Structure, shooter: Option<i64>) -> bool {
    let (sh, ar, hu) = s.damage.unwrap_or((1.0, 1.0, 1.0));
    let who = shooter.and_then(|k| reg.pilot(k));
    let Some(p) = who.as_ref() else {
        return false;
    };
    // The NPC shape (`npc::pilot_of`): no character, owner is the corp.
    let npc = p.char_id.is_none() && p.owner_id == p.corporation_id;
    let Some(char_id) = p.char_id.or(npc.then_some(p.corporation_id)) else {
        return false;
    };
    let mut d = base_data(s);
    d.extend(data([
        ("shieldPercentage", Value::Real(100.0 * sh)),
        ("armorPercentage", Value::Real(100.0 * ar)),
        ("hullPercentage", Value::Real(100.0 * hu)),
        ("charID", Value::Int(char_id)),
    ]));
    let corp = p.corporation_id;
    let (mut corp_name, mut alliance, mut alliance_name) = (None, p.alliance_id, None);
    if npc && let Some(t) = reg.content().try_get::<crate::npc_data::Table>() {
        corp_name = t.corporations.get(&corp).map(|c| c.name.clone());
        let faction = t
            .corporations
            .get(&corp)
            .and_then(|c| c.faction_id)
            .or_else(|| {
                t.factions
                    .values()
                    .find(|f| f.owner() == corp)
                    .map(|f| f.faction_id)
            });
        if let Some(f) = faction {
            alliance = Some(f);
            alliance_name = Some(
                t.factions
                    .get(&f)
                    .map(|r| r.name.clone())
                    .unwrap_or_default(),
            );
        }
    }
    let corp_name = corp_name
        .or_else(|| reg.corporations().get(corp).map(|c| c.name.clone()))
        .unwrap_or_default();
    d.extend(data([
        ("corpName", Value::Text(corp_name)),
        ("corpLinkData", show_info(TYPE_CORPORATION, corp)),
    ]));
    d.push((
        "allianceID".to_owned(),
        alliance.map_or(Value::None, Value::Int),
    ));
    if let Some(a) = alliance {
        let (name, ty) = match alliance_name {
            Some(n) => (n, TYPE_FACTION),
            None => (
                crate::alliance::name_of(reg.deltas(), a).unwrap_or_default(),
                TYPE_ALLIANCE,
            ),
        };
        d.extend(data([
            ("allianceName", Value::Text(name)),
            ("allianceLinkData", show_info(ty, a)),
        ]));
    }
    tell(reg, s, STRUCTURE_UNDER_ATTACK, d);
    true
}

/// 188 `StructureDestroyed.MakeData` **[V]** (`structures.py:294-300`): the
/// base data and the owner corporation's name and link, and `isAbandoned`
/// (an abandoned structure's own label).
pub fn destroyed(reg: &Registry, s: &Structure) -> Data {
    let mut d = base_data(s);
    d.extend(owner_fields(reg, s.owner_corp));
    d.push((
        "isAbandoned".to_owned(),
        Value::Bool(s.upkeep == Some(crate::structure::Upkeep::Abandoned)),
    ));
    d
}

/// What `structure_space::react` owes the managers for the events one
/// structure raised: 184 on the first aggression of a window, 186/187 on
/// entering a reinforce state, 185 when the onlining ends.
pub fn on_events(reg: &mut Registry, s: &Structure, events: &[Event], now: i64) {
    for ev in events {
        if *ev == Event::FirstAggressed {
            let shooter = reg
                .ext_mut::<crate::structure_combat::Fire>()
                .take_last_shooter(s.id);
            if !under_attack(reg, s, shooter) {
                // Nothing went out (damage no pilot fired): let the next
                // volley raise the first aggression again with its pilot.
                if let Some(mut row) = crate::structure::get(reg.deltas(), s.id) {
                    row.notified = false;
                    crate::structure::put(reg.deltas_mut(), &row);
                }
            }
        }
        if let Event::StateChanged { from, to } = ev {
            match to {
                State::ArmorReinforce | State::HullReinforce => lost_layer(reg, s, *to, now),
                State::ShieldVulnerable if *from == State::OnliningVulnerable => online(reg, s),
                _ => {}
            }
        }
    }
}

/// 209 `StructuresReinforcementChanged.MakeData` **[V]**: no base data. `info`
/// is `(itemID, name, typeID)` per structure; `timestamp` is when the change
/// applies.
pub fn reinforcement_changed(
    reg: &mut Registry,
    corp: i64,
    info: &[(i64, String, i64)],
    weekday: i64,
    hour: i64,
    timestamp: i64,
) {
    let list = info
        .iter()
        .map(|(id, name, type_id)| {
            Value::Tuple(vec![
                Value::Int(*id),
                Value::Text(name.clone()),
                Value::Int(*type_id),
            ])
        })
        .collect();
    let d = data([
        ("numStructures", Value::Int(info.len() as i64)),
        ("allStructureInfo", Value::List(list)),
        ("weekday", Value::Int(weekday)),
        ("hour", Value::Int(hour)),
        ("timestamp", Value::Int(timestamp)),
    ]);
    tell_corp(reg, corp, STRUCTURES_REINFORCEMENT_CHANGED, d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::structure;

    const DIRECTOR: i64 = 7;
    const MEMBER: i64 = 8;
    const MANAGER: i64 = 9;
    const OTHER: i64 = 10;
    const CORP: i64 = 11;
    const OTHER_CORP: i64 = 12;
    const T0: i64 = 134_000_000_000_000_000;

    fn world() -> Registry {
        let ch = |id: i64, corp: i64| Character {
            character_id: id,
            corporation_id: corp,
            ship_id: 1_000 + id,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(DIRECTOR, CORP),
                ch(MEMBER, CORP),
                ch(MANAGER, CORP),
                ch(OTHER, OTHER_CORP),
            ],
            ..SandboxConfig::default()
        });
        let c = reg.corporations_mut();
        c.enrol_for_test(DIRECTOR, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        c.enrol_for_test(MEMBER, CORP, 0);
        c.enrol_for_test(MANAGER, CORP, crate::sov_hub::CORP_ROLE_STATION_MANAGER);
        c.enrol_for_test(OTHER, OTHER_CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        reg
    }

    fn structure_at(state: State) -> Structure {
        let e = structure::Env::new(T0);
        let mut s = Structure::anchored(4_001, 35_832, CORP, 31, [0.0; 3], 0.0, "Invented", 1, &e);
        s.set_state(state, &e);
        s.take_events();
        s
    }

    fn got(reg: &Registry, who: i64) -> Vec<notif::Notification> {
        notif::of(reg.deltas(), who)
    }

    fn val<'a>(d: &'a Data, key: &str) -> &'a Value {
        &d.iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("{key}"))
            .1
    }

    #[test]
    fn only_the_owner_corps_managers_hear_it() {
        let mut reg = world();
        assert_eq!(managers(&reg, CORP), vec![DIRECTOR, MANAGER]);
        let s = structure_at(State::ShieldVulnerable);
        tell(&mut reg, &s, STRUCTURE_ONLINE, base_data(&s));
        assert_eq!(got(&reg, DIRECTOR).len(), 1);
        assert_eq!(got(&reg, MANAGER).len(), 1);
        assert!(got(&reg, MEMBER).is_empty(), "no role, no notification");
        assert!(got(&reg, OTHER).is_empty(), "another corp's director");
        let n = &got(&reg, DIRECTOR)[0];
        assert_eq!(n.type_id, STRUCTURE_ONLINE);
        assert_eq!(n.sender, CORP, "the sender is the owner corp");
    }

    #[test]
    fn the_base_data_has_the_formatters_keys_and_a_showinfo_tuple() {
        let s = structure_at(State::ShieldVulnerable);
        let d = base_data(&s);
        let keys: Vec<&str> = d.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "structureID",
                "structureShowInfoData",
                "solarsystemID",
                "structureTypeID"
            ]
        );
        assert_eq!(
            val(&d, "structureShowInfoData"),
            &Value::Tuple(vec![
                Value::Text("showinfo".to_owned()),
                Value::Int(s.type_id),
                Value::Int(s.id)
            ])
        );
        assert_eq!(val(&d, "solarsystemID"), &Value::Int(31));
    }

    #[test]
    fn first_aggression_with_no_shooter_sends_no_184() {
        let mut reg = world();
        let s = structure_at(State::ShieldVulnerable);
        on_events(&mut reg, &s, &[Event::FirstAggressed], T0);
        assert!(
            got(&reg, DIRECTOR).is_empty(),
            "a charID of None raises in the client's formatter"
        );
    }

    #[test]
    fn first_aggression_by_a_character_sends_a_184_with_its_char_id() {
        let mut reg = world();
        let s = structure_at(State::ShieldVulnerable);
        reg.ext_mut::<crate::structure_combat::Fire>()
            .note_shooter(s.id, OTHER);
        on_events(&mut reg, &s, &[Event::FirstAggressed], T0);
        let n = got(&reg, DIRECTOR);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].type_id, STRUCTURE_UNDER_ATTACK);
        assert_eq!(val(&n[0].data, "charID"), &Value::Int(OTHER));
    }

    #[test]
    fn gm_damage_does_not_swallow_a_pilots_later_184() {
        let mut reg = world();
        let mut s = structure_at(State::ShieldVulnerable);
        structure::put(reg.deltas_mut(), &s);
        let e = crate::structure_space::env(&reg, T0);
        s.set_damage(0.2, 1.0, 1.0, &e);
        let events = s.take_events();
        structure::put(reg.deltas_mut(), &s);
        crate::structure_space::react_at(&mut reg, s.id, &events, T0);
        assert!(got(&reg, DIRECTOR).is_empty(), "no pilot, no 184");
        // The pilot's volley, same window.
        reg.ext_mut::<crate::structure_combat::Fire>()
            .note_shooter(s.id, OTHER);
        let mut s = structure::get(reg.deltas(), s.id).unwrap();
        let fire = structure::Env {
            under_fire: true,
            ..e
        };
        s.set_damage(0.1, 1.0, 1.0, &fire);
        let events = s.take_events();
        structure::put(reg.deltas_mut(), &s);
        crate::structure_space::react_at(&mut reg, s.id, &events, T0);
        let n = got(&reg, DIRECTOR);
        assert_eq!(n.len(), 1, "exactly one 184");
        assert_eq!(val(&n[0].data, "charID"), &Value::Int(OTHER));
    }

    #[test]
    fn first_aggression_by_an_npc_sends_a_184_naming_its_corp() {
        // An empty NPC table: no faction row, so no alliance.
        let mut c = crate::content::Content::fixture();
        c.insert("npc_data", crate::npc_data::Table::default());
        let mut reg = world().with_content(std::sync::Arc::new(c));
        let mut s = structure_at(State::ShieldVulnerable);
        s.notified = true;
        structure::put(reg.deltas_mut(), &s);
        let npc = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, 31, [0.0; 3]).unwrap();
        reg.ext_mut::<crate::structure_combat::Fire>()
            .note_shooter(s.id, npc);
        on_events(&mut reg, &s, &[Event::FirstAggressed], T0);
        let n = got(&reg, DIRECTOR);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].type_id, STRUCTURE_UNDER_ATTACK);
        assert_eq!(
            val(&n[0].data, "charID"),
            &Value::Int(crate::npc::FIXTURE_CORP)
        );
        assert_eq!(val(&n[0].data, "allianceID"), &Value::None);
        let row = structure::get(reg.deltas(), s.id).expect("the row");
        assert!(row.notified, "no retry on the next volley");
    }

    #[test]
    fn an_npc_shooters_faction_rides_as_the_alliance() {
        const FACTION: i64 = 599_901;
        let mut t = crate::npc_data::Table::default();
        t.corporations.insert(
            crate::npc::FIXTURE_CORP,
            crate::npc_data::Corporation {
                corp_id: crate::npc::FIXTURE_CORP,
                faction_id: Some(FACTION),
                ..Default::default()
            },
        );
        t.factions.insert(
            FACTION,
            crate::npc_data::Faction {
                faction_id: FACTION,
                name: "Invented Faction".to_owned(),
                ..Default::default()
            },
        );
        let mut c = crate::content::Content::fixture();
        c.insert("npc_data", t);
        let mut reg = world().with_content(std::sync::Arc::new(c));
        let s = structure_at(State::ShieldVulnerable);
        structure::put(reg.deltas_mut(), &s);
        let npc = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, 31, [0.0; 3]).unwrap();
        reg.ext_mut::<crate::structure_combat::Fire>()
            .note_shooter(s.id, npc);
        on_events(&mut reg, &s, &[Event::FirstAggressed], T0);
        let n = got(&reg, DIRECTOR);
        assert_eq!(n.len(), 1);
        assert_eq!(val(&n[0].data, "allianceID"), &Value::Int(FACTION));
        assert_eq!(
            val(&n[0].data, "allianceName"),
            &Value::Text("Invented Faction".to_owned())
        );
        assert_eq!(
            val(&n[0].data, "allianceLinkData"),
            &show_info(TYPE_FACTION, FACTION)
        );
    }

    #[test]
    fn an_npc_corp_with_no_row_finds_its_faction_by_owner() {
        const FACTION: i64 = 599_902;
        let mut t = crate::npc_data::Table::default();
        t.factions.insert(
            FACTION,
            crate::npc_data::Faction {
                faction_id: FACTION,
                corporation_id: crate::npc::FIXTURE_CORP,
                name: "Invented Owner Faction".to_owned(),
                ..Default::default()
            },
        );
        let mut c = crate::content::Content::fixture();
        c.insert("npc_data", t);
        let mut reg = world().with_content(std::sync::Arc::new(c));
        let s = structure_at(State::ShieldVulnerable);
        structure::put(reg.deltas_mut(), &s);
        let npc = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, 31, [0.0; 3]).unwrap();
        reg.ext_mut::<crate::structure_combat::Fire>()
            .note_shooter(s.id, npc);
        on_events(&mut reg, &s, &[Event::FirstAggressed], T0);
        let n = got(&reg, DIRECTOR);
        assert_eq!(n.len(), 1);
        assert_eq!(val(&n[0].data, "allianceID"), &Value::Int(FACTION));
    }
}
