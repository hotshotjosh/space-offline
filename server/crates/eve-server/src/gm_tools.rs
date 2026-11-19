//! **GM pilot tools** (#201 lane B): `/who`, `/where`, `/heal`, `/hurt` and
//! `/suicide`, the commands a GM tests combat, repair and death flows with
//! without waiting for a fight. Names and behaviour only; nothing here is
//! copied from EvEJS.
//!
//! # The gate rule ("nothing from nowhere")
//!
//! The server cannot see whether the bots run `--closed` or `--society`, so a
//! command that gives a character value uses one of three gates ([`Gate`]):
//!
//! - **read**: `ROLE_GML` only, which `service/gm.rs` already checks for every
//!   slash command; nothing more here.
//! - **reset**: a GM may reset a human, but a bot is refused unless the server
//!   runs `--dev` ([`BOT_REFUSED`]): a healed bot in a society world is value
//!   from nowhere. **[A]** this fits the owner's rule; falsified if the owner
//!   wants humans in a society world refused as well (then it is a world
//!   setting, an owner decision).
//! - **value**: `--dev` only.
//!
//! `/suicide` only destroys value, so it takes no gate beyond GM. `eve-bots`
//! never sends any of these commands.
//!
//! **Open owner question.** Until accounts land every session has `ROLE_GML`
//! (`service/gm.rs`), so in a shared PvP world any player can run these:
//! `/where` and `/who` read anyone, `/heal` mends a ship mid-fight and `/hurt`
//! hurts another human. The gate above does not stop that.
//!
//! **`/suicide` and insurance.** It goes through `combat::self_destruct_kill`
//! but does not set `self_destruct::State.killing`, so `pays_insurance` is
//! always false for it: a GM suicide never pays insurance, unlike a real
//! self-destruct.
//!
//! **`/hurt` and the shield.** The shield is left as it is (its charge at the
//! moment of the command, still recharging along its curve); only armour and
//! hull are set. The ship's own profile (fitted resists) is kept.
//!
//! # What the client is shown
//!
//! `/who` and `/where` answer as a `CustomInfo` dialog (`service/gm.rs`
//! `VISIBLE`). `/heal` and `/hurt` tell the park the new bars and the pilot the
//! ring that moved, through [`crate::remote_rep::announce_layer`].

use std::collections::HashMap;
use std::time::Duration;

use eve_marshal::PyValue;

use crate::damage::{BlueTime, Layer, Profile, ShipDamage, attr as dmg_attr};
use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};

/// The three gate classes of the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Read,
    Reset,
    Value,
}

/// The refusal for a reset on a bot outside `--dev`.
pub const BOT_REFUSED: &str =
    "refused: a bot gets nothing from nowhere; start the server with --dev to test bots";
/// The refusal for a value command outside `--dev` (the text `dev_mode` uses).
pub const DEV_ONLY: &str = "dev only: start the server with --dev";

/// `Ok(())` or the refusal text, for acting on `target`.
pub fn gate(reg: &Registry, gate: Gate, target: i64) -> Result<(), &'static str> {
    let dev = reg.config().dev;
    match gate {
        Gate::Read => Ok(()),
        Gate::Reset => {
            if !dev && reg.character(target).is_some_and(|c| c.is_bot) {
                Err(BOT_REFUSED)
            } else {
                Ok(())
            }
        }
        Gate::Value => {
            if dev {
                Ok(())
            } else {
                Err(DEV_ONLY)
            }
        }
    }
}

/// Most lines `/who` prints before it cuts the list.
const WHO_LINES: usize = 40;

/// One online character, as `/who` prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct WhoRow {
    pub name: String,
    pub bot: bool,
    pub system: String,
    pub state: &'static str,
    pub ship: String,
}

impl WhoRow {
    fn kind(&self) -> &'static str {
        if self.bot { "bot" } else { "human" }
    }
}

/// The `/who` text: a count header, then up to [`WHO_LINES`] lines that match
/// `filter` (a case-insensitive substring over name, kind and system).
pub fn who_text(rows: &[WhoRow], filter: Option<&str>) -> String {
    let humans = rows.iter().filter(|r| !r.bot).count();
    let mut out = format!(
        "{} online ({} humans, {} bots)",
        rows.len(),
        humans,
        rows.len() - humans
    );
    let want = filter.map(str::to_lowercase).filter(|f| !f.is_empty());
    let hits: Vec<&WhoRow> = rows
        .iter()
        .filter(|r| {
            want.as_ref().is_none_or(|f| {
                [r.name.as_str(), r.kind(), r.system.as_str()]
                    .iter()
                    .any(|field| field.to_lowercase().contains(f.as_str()))
            })
        })
        .collect();
    for r in hits.iter().take(WHO_LINES) {
        out.push_str(&format!(
            "\n{}  {}  {}  {}  {}",
            r.name,
            r.kind(),
            r.system,
            r.state,
            r.ship
        ));
    }
    if hits.len() > WHO_LINES {
        out.push_str(&format!(
            "\n... and {} more (filter to narrow)",
            hits.len() - WHO_LINES
        ));
    }
    out
}

/// The system names of the world's universe, by id (empty without one).
fn system_names(reg: &Registry) -> HashMap<i64, String> {
    reg.content()
        .try_get::<crate::celestial::Universe>()
        .map(|u| u.systems().map(|s| (s.id, s.name.clone())).collect())
        .unwrap_or_default()
}

fn system_label(names: &HashMap<i64, String>, id: Option<i64>) -> String {
    match id {
        Some(id) => names.get(&id).cloned().unwrap_or_else(|| id.to_string()),
        None => "?".to_owned(),
    }
}

fn state_of(reg: &Registry, id: i64) -> &'static str {
    if reg.roster().get(id).is_some() {
        "space"
    } else if reg.jumps().contains(id) {
        "jumping"
    } else if reg.sessions().is_online(id) {
        "docked"
    } else {
        "offline"
    }
}

fn ship_name(reg: &Registry, type_id: i64) -> String {
    crate::item::find_in(reg.content(), type_id)
        .map(|t| t.name.to_owned())
        .unwrap_or_else(|| type_id.to_string())
}

/// `/who [filter]`.
pub fn who(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let names = system_names(reg);
    let rows: Vec<WhoRow> = reg
        .sessions()
        .online_characters()
        .into_iter()
        .filter_map(|id| {
            let c = reg.character(id)?;
            Some(WhoRow {
                name: c.name.clone(),
                bot: c.is_bot,
                system: system_label(&names, reg.session_system(id)),
                state: state_of(reg, id),
                ship: ship_name(reg, c.ship_type_id),
            })
        })
        .collect();
    let filter = args.join(" ");
    Ok(bstr(&who_text(&rows, Some(&filter))))
}

/// What `/where` knows about one character.
#[derive(Debug, Clone, PartialEq)]
pub struct WhereRow {
    pub name: String,
    pub state: &'static str,
    pub station: Option<i64>,
    pub system: String,
    pub system_id: Option<i64>,
    /// Kilometres, rounded.
    pub position_km: Option<[i64; 3]>,
    /// Metres, rounded: what `fci_bump_pair` compares to a client's ball.
    pub position_m: Option<[i64; 3]>,
    pub ship_id: i64,
    pub ship: String,
    pub fleet: Option<i64>,
}

/// The `/where` text.
pub fn where_text(r: &WhereRow) -> String {
    let place = match (r.state, r.station) {
        ("docked", Some(st)) => format!("docked at station {st}"),
        ("space", _) => match r.system_id {
            Some(id) => format!("in space in system {} ({id})", r.system),
            None => "in space".to_owned(),
        },
        (other, _) => other.to_owned(),
    };
    let mut out = format!("{}: {place}", r.name);
    if let Some([x, y, z]) = r.position_km {
        out.push_str(&format!("\nposition {x} {y} {z} km"));
    }
    if let Some([x, y, z]) = r.position_m {
        out.push_str(&format!("\nposition {x} {y} {z} m"));
    }
    out.push_str(&format!("\nship {} {}", r.ship_id, r.ship));
    if let Some(sys) = r.system_id {
        out.push_str(&format!("\nsession system {sys}"));
    }
    match r.fleet {
        Some(f) => out.push_str(&format!("\nin fleet {f}")),
        None => out.push_str("\nnot in a fleet"),
    }
    out
}

/// The character a command acts on: no argument or `me` is the caller; a
/// number is a ship in space (a ball id, the client's GM menu sends
/// `/heal <ballID> 0`) and means that ship's pilot, else a character id.
fn target(reg: &Registry, actor: Option<i64>, arg: Option<&&str>) -> Result<i64, &'static str> {
    match arg {
        Some(a) if a.eq_ignore_ascii_case("me") => actor.ok_or("no character to act on"),
        Some(a) => {
            let n: i64 = a.parse().map_err(|_| "usage: bad characterID")?;
            Ok(reg.roster().by_ship(n).map_or(n, |p| p.character_id))
        }
        None => actor.ok_or("no character to act on"),
    }
}

/// `/where [characterID]`.
pub fn where_cmd(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let id = match target(reg, actor, args.first()) {
        Ok(id) => id,
        Err(e) => return Ok(bstr(&format!("{e}; usage: /where [characterID]"))),
    };
    if reg.character(id).is_none() {
        return Ok(bstr(&format!("no character {id}")));
    }
    // The position as of now, not as of the last time something read it.
    reg.advance_character(id);
    let Some(c) = reg.character(id) else {
        return Ok(bstr(&format!("no character {id}")));
    };
    let names = system_names(reg);
    let presence = reg.roster().get(id).copied();
    let state = state_of(reg, id);
    let system_id = reg.session_system(id);
    let row = WhereRow {
        name: c.name.clone(),
        state,
        station: if state == "docked" {
            c.station_id
        } else {
            None
        },
        system: system_label(&names, system_id),
        system_id,
        position_km: presence.map(|p| p.motion.position.map(|m| (m / 1000.0).round() as i64)),
        position_m: presence.map(|p| p.motion.position.map(|m| m.round() as i64)),
        ship_id: presence.map_or(c.ship_id, |p| p.ship_id),
        ship: ship_name(reg, c.ship_type_id),
        fleet: crate::combat::fleet_of(reg, id),
    };
    Ok(bstr(&where_text(&row)))
}

/// The ship of `target` when it is in space.
fn ship_in_space(reg: &Registry, target: i64) -> Option<i64> {
    reg.roster().get(target).map(|p| p.ship_id)
}

fn name_of(reg: &Registry, id: i64) -> String {
    reg.character(id)
        .map_or_else(|| id.to_string(), |c| c.name.clone())
}

/// `/heal [me|characterID|shipID]`.
pub fn heal(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let id = match target(reg, actor, args.first()) {
        Ok(id) => id,
        Err(e) => return Ok(bstr(&format!("{e}; usage: /heal [me|characterID|shipID]"))),
    };
    // A pilotless ball (an NPC, a drone, a wreck: roster character_id == ship
    // id) has no character; it is not a target.
    if reg.character(id).is_none() {
        return Ok(bstr(&format!("no character or ship {id}")));
    }
    if let Err(refusal) = gate(reg, Gate::Reset, id) {
        return Ok(bstr(refusal));
    }
    let Some(ship) = ship_in_space(reg, id) else {
        return Ok(bstr("not in space"));
    };
    let now: BlueTime = crate::pyutil::filetime_now();
    let mut got = [0.0_f64; 3];
    for (i, layer) in [Layer::Shield, Layer::Armour, Layer::Hull]
        .into_iter()
        .enumerate()
    {
        got[i] = crate::remote_rep::heal(reg, ship, layer, 1.0e12, now);
    }
    let ship_type = reg.pilot(id).map_or(0, |p| p.ship_type_id);
    let (before, capacity) = crate::modules::capacitor_at(reg, ship, ship_type, now);
    let mut cap = 0.0;
    if let Some(moved) = crate::modules::give_capacitor_by(reg, ship, capacity - before, now) {
        cap = moved.amount;
        if let Some(p) = moved.push {
            reg.schedule(Duration::ZERO, p);
        }
    }
    Ok(bstr(&format!(
        "healed {}: shield +{:.0} armour +{:.0} hull +{:.0} capacitor +{:.0}",
        name_of(reg, id),
        got[0],
        got[1],
        got[2],
        cap
    )))
}

/// The ship's damage profile, the order a shot uses (`combat.rs` fire_banked):
/// the record it already has (fitted resists kept), else the dogma sheet's,
/// else the hull table's for the pilot's ship type.
fn hull_profile(reg: &Registry, owner: i64, ship: i64) -> Profile {
    if let Some(rec) = reg.combat().ship(ship) {
        return *rec.profile();
    }
    crate::dogma::profile(reg, ship).unwrap_or_else(|| {
        let ship_type = reg.pilot(owner).map_or(0, |p| p.ship_type_id);
        Profile::from_hull_in(
            reg.content(),
            crate::destiny::hull::of_in(reg.content(), ship_type),
        )
    })
}

const HURT_USAGE: &str = "usage: /hurt <armour %> <hull %> [me|characterID|shipID] (0-100; hull stops at 99, \
     so it never kills; shield damage is not supported)";

/// A percentage of 0 to 100, or `None`.
fn percent(word: &str) -> Option<f64> {
    word.parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
}

/// The damage `/hurt` records: `armour_pct` and `hull_pct` (hull clamped to
/// 99) of the profile's hit points.
pub fn hurt_damage(profile: &Profile, armour_pct: f64, hull_pct: f64) -> (f64, f64) {
    (
        profile.armor_hp * armour_pct / 100.0,
        profile.hp * hull_pct.min(99.0) / 100.0,
    )
}

/// `/hurt <armour %> <hull %> [me|characterID|shipID]`.
pub fn hurt(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let (Some(a), Some(h), rest) = (
        args.first().and_then(|w| percent(w)),
        args.get(1).and_then(|w| percent(w)),
        args.get(2..).unwrap_or(&[]),
    ) else {
        return Ok(bstr(HURT_USAGE));
    };
    if rest.len() > 1 {
        return Ok(bstr(HURT_USAGE));
    }
    let id = match target(reg, actor, rest.first()) {
        Ok(id) => id,
        Err(_) => return Ok(bstr(HURT_USAGE)),
    };
    if reg.character(id).is_none() {
        return Ok(bstr(&format!("no character or ship {id}")));
    }
    if let Err(refusal) = gate(reg, Gate::Reset, id) {
        return Ok(bstr(refusal));
    }
    let Some(ship) = ship_in_space(reg, id) else {
        return Ok(bstr("not in space"));
    };
    let now: BlueTime = crate::pyutil::filetime_now();
    let profile = hull_profile(reg, id, ship);
    let before_rec = reg
        .combat()
        .ship(ship)
        .copied()
        .unwrap_or_else(|| ShipDamage::full(profile, now));
    let before = before_rec.rings(now);
    let (armour, hull) = hurt_damage(&profile, a, h);
    // Keep the shield as it is now: build the record with that charge as its
    // capacity (so `with_damage` fills it to exactly that), then restore the
    // real profile, which clamps and re-stamps along the curve.
    let mut p0 = profile;
    p0.shield_capacity = before_rec.shield_at(now);
    let mut rec = ShipDamage::with_damage(p0, now, armour, hull);
    rec.set_profile(profile, now);
    reg.combat_mut().seed_ship(ship, rec);
    let after = reg.combat().ship(ship).map(|s| s.rings(now));
    if let Some(after) = after {
        // One DamageStateChange for the park; the rings that moved to the
        // pilot (`announce_layers` skips a ring that did not change).
        crate::remote_rep::announce_layers(
            reg,
            ship,
            &[
                (
                    dmg_attr::SHIELD_CHARGE,
                    before.shield_charge,
                    after.shield_charge,
                ),
                (
                    dmg_attr::ARMOR_DAMAGE,
                    before.armor_damage,
                    after.armor_damage,
                ),
                (dmg_attr::DAMAGE, before.damage, after.damage),
            ],
            now,
        );
    }
    Ok(bstr(&format!(
        "hurt {}: armour {a}%, hull {}% damaged",
        name_of(reg, id),
        h.min(99.0)
    )))
}

/// `/suicide`: the caller's ship blows up now, through the death a shot
/// causes (wreck, capsule, kill hooks, killmail).
pub fn suicide(
    reg: &mut Registry,
    actor: Option<i64>,
    _args: &[&str],
) -> Result<PyValue, ServiceError> {
    let Some(actor) = actor else {
        return Ok(bstr("no character to act on"));
    };
    let now: BlueTime = crate::pyutil::filetime_now();
    if crate::combat::self_destruct_kill(reg, actor, now) {
        Ok(bstr("self-destruct: your ship is destroyed"))
    } else {
        Ok(bstr("you must be in space"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Damage;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_001;
    const BOT: i64 = 90_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOT_SHIP: i64 = 100_000_002;
    const SYS: i64 = 30_990_003;
    /// The content fixture's invented frigate.
    const HULL: i64 = 990_000_001;

    fn row(name: &str, bot: bool) -> WhoRow {
        WhoRow {
            name: name.to_owned(),
            bot,
            system: "Testsys".to_owned(),
            state: "space",
            ship: "Hull".to_owned(),
        }
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Alice (human) and Bot (bot) both in space; `dev` as given.
    fn world(dev: bool) -> Registry {
        let ch = |id: i64, ship: i64, name: &str, is_bot: bool| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: HULL,
            is_bot,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice", false),
                ch(BOT, BOT_SHIP, "Botty", true),
            ],
            dev,
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(crate::content::Content::fixture()));
        for c in [ALICE, BOT] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    fn text(r: Result<PyValue, ServiceError>) -> String {
        match r.expect("answers") {
            PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            other => panic!("not text: {other:?}"),
        }
    }

    fn bars(reg: &Registry, ship: i64) -> crate::destiny::DamageState {
        crate::combat::damage_state(reg, ship, crate::pyutil::filetime_now())
    }

    #[test]
    fn who_text_counts_and_filters() {
        let rows = vec![row("Alice", false), row("Bob", false), row("Botty", true)];
        let all = who_text(&rows, None);
        assert!(all.starts_with("3 online (2 humans, 1 bots)"), "{all}");
        assert_eq!(all.lines().count(), 4);
        let bots = who_text(&rows, Some("BOT"));
        // The header still counts everyone; "bot" matches the kind and "Botty"
        // the name, both the same one row.
        assert!(bots.starts_with("3 online"), "{bots}");
        assert_eq!(bots.lines().count(), 2, "{bots}");
        assert!(bots.contains("Botty"));
        let many: Vec<WhoRow> = (0..45).map(|i| row(&format!("P{i}"), false)).collect();
        let cut = who_text(&many, None);
        assert_eq!(cut.lines().count(), 1 + 40 + 1, "{cut}");
        assert!(cut.contains("and 5 more"), "{cut}");
    }

    #[test]
    fn who_lists_the_online_characters_with_their_kind() {
        let mut reg = world(false);
        for (n, id) in [(1, ALICE), (2, BOT)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        let all = text(who(&mut reg, Some(ALICE), &[]));
        assert!(all.starts_with("2 online (1 humans, 1 bots)"), "{all}");
        assert!(all.contains("Alice  human"), "{all}");
        assert!(all.contains("Botty  bot"), "{all}");
        let one = text(who(&mut reg, Some(ALICE), &["botty"]));
        assert_eq!(one.lines().count(), 2, "{one}");
    }

    #[test]
    fn where_text_names_the_place_and_the_fleet() {
        let mut r = WhereRow {
            name: "Alice".to_owned(),
            state: "space",
            station: None,
            system: "Testsys".to_owned(),
            system_id: Some(SYS),
            position_km: Some([1, -2, 3]),
            position_m: Some([1_200, -2_000, 3_400]),
            ship_id: ALICE_SHIP,
            ship: "Hull".to_owned(),
            fleet: Some(77),
        };
        let t = where_text(&r);
        assert!(t.contains(&format!("system Testsys ({SYS})")), "{t}");
        assert!(t.contains("position 1 -2 3 km"), "{t}");
        assert!(t.contains("position 1200 -2000 3400 m"), "{t}");
        assert!(t.contains(&format!("ship {ALICE_SHIP}")), "{t}");
        assert!(t.contains("in fleet 77"), "{t}");
        r.state = "docked";
        r.station = Some(60_000_009);
        r.fleet = None;
        r.position_km = None;
        r.position_m = None;
        let t = where_text(&r);
        assert!(t.contains("docked at station 60000009"), "{t}");
        assert!(t.contains("not in a fleet"), "{t}");
        assert!(!t.contains("position"), "{t}");
    }

    #[test]
    fn where_reports_the_session_system_of_a_pilot_in_space() {
        let mut reg = world(false);
        let sys = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let said = text(where_cmd(&mut reg, Some(ALICE), &[]));
        assert!(said.contains(&format!("system {sys}")), "{said}");
        assert!(said.contains(&format!("ship {ALICE_SHIP}")), "{said}");
        assert!(said.contains("Alice"), "{said}");
        assert!(text(where_cmd(&mut reg, Some(ALICE), &["1"])).contains("no character 1"));
    }

    #[test]
    fn heal_restores_a_hurt_ship_and_tells_the_pilot() {
        let mut reg = world(false);
        let said = text(hurt(&mut reg, Some(ALICE), &["50", "50"]));
        assert!(said.starts_with("hurt Alice"), "{said}");
        let hurt_bars = bars(&reg, ALICE_SHIP);
        assert!(
            hurt_bars.armour < 0.6 && hurt_bars.hull < 0.6,
            "{hurt_bars:?}"
        );
        let told = |reg: &mut Registry| {
            reg.take_due(far())
                .into_iter()
                .map(|p| (p.to_push(), p.audience()))
                .collect::<Vec<_>>()
        };
        let is_bars = |(push, who): &((&'static str, Vec<PyValue>), Option<Vec<i64>>)| {
            push.0 == "DoDestinyUpdate"
                && format!("{:?}", push.1).contains(&format!("{:?}", bstr("OnDamageStateChange")))
                && who.as_ref().is_some_and(|a| a.contains(&ALICE))
        };
        let is_ring = |attr: i64| {
            move |(push, who): &((&'static str, Vec<PyValue>), Option<Vec<i64>>)| {
                push.0 == "OnModuleAttributeChange"
                    && push.1.get(2) == Some(&PyValue::Int(attr))
                    && *who == Some(vec![ALICE])
            }
        };
        let sent = told(&mut reg);
        assert!(sent.iter().any(is_bars), "hurt tells the park");
        assert!(
            sent.iter().any(is_ring(dmg_attr::ARMOR_DAMAGE)),
            "armour ring"
        );
        assert!(sent.iter().any(is_ring(dmg_attr::DAMAGE)), "hull ring");
        let said = text(heal(&mut reg, Some(ALICE), &[]));
        assert!(
            said.starts_with("healed Alice: shield +0 armour +"),
            "{said}"
        );
        let whole = bars(&reg, ALICE_SHIP);
        assert!(whole.armour > 0.999 && whole.hull > 0.999, "{whole:?}");
        let sent = told(&mut reg);
        assert!(sent.iter().any(is_bars), "heal tells the park");
        assert!(
            sent.iter().any(is_ring(dmg_attr::ARMOR_DAMAGE)),
            "armour ring back"
        );
        assert!(sent.iter().any(is_ring(dmg_attr::DAMAGE)), "hull ring back");
    }

    #[test]
    fn heal_on_a_bot_is_refused_without_dev() {
        let mut reg = world(false);
        // Hurt the bot's record directly (the command would refuse too).
        let profile = hull_profile(&reg, BOT, BOT_SHIP);
        let now = crate::pyutil::filetime_now();
        reg.combat_mut().seed_ship(
            BOT_SHIP,
            ShipDamage::with_damage(profile, now, profile.armor_hp / 2.0, 0.0),
        );
        let before = bars(&reg, BOT_SHIP);
        assert!(before.armour < 0.9, "{before:?}");
        let id = BOT.to_string();
        assert_eq!(text(heal(&mut reg, Some(ALICE), &[&id])), BOT_REFUSED);
        assert_eq!(
            text(hurt(&mut reg, Some(ALICE), &["10", "10", &id])),
            BOT_REFUSED
        );
        assert_eq!(bars(&reg, BOT_SHIP), before, "unchanged");

        let mut dev = world(true);
        let now = crate::pyutil::filetime_now();
        dev.combat_mut().seed_ship(
            BOT_SHIP,
            ShipDamage::with_damage(profile, now, profile.armor_hp / 2.0, 0.0),
        );
        let said = text(heal(&mut dev, Some(ALICE), &[&id]));
        assert!(said.starts_with("healed Botty"), "{said}");
        assert!(bars(&dev, BOT_SHIP).armour > 0.999);
    }

    #[test]
    fn gate_classes() {
        let off = world(false);
        let on = world(true);
        assert_eq!(gate(&off, Gate::Read, BOT), Ok(()));
        assert_eq!(gate(&off, Gate::Reset, ALICE), Ok(()));
        assert_eq!(gate(&off, Gate::Reset, BOT), Err(BOT_REFUSED));
        assert_eq!(gate(&on, Gate::Reset, BOT), Ok(()));
        assert_eq!(gate(&off, Gate::Value, ALICE), Err(DEV_ONLY));
        assert_eq!(gate(&on, Gate::Value, ALICE), Ok(()));
    }

    #[test]
    fn hurt_never_kills() {
        let mut reg = world(false);
        let said = text(hurt(&mut reg, Some(ALICE), &["100", "100"]));
        assert!(said.contains("hull 99%"), "{said}");
        let b = bars(&reg, ALICE_SHIP);
        assert!(b.hull > 0.0 && b.hull < 0.05, "{b:?}");
        assert!(b.armour <= 1e-9, "{b:?}");
        let rec = reg.combat().ship(ALICE_SHIP).expect("a record");
        assert!(!rec.is_destroyed());
        assert!(
            reg.roster()
                .get(ALICE)
                .is_some_and(|p| p.ship_id == ALICE_SHIP)
        );
    }

    #[test]
    fn hurt_damage_is_the_percentage_of_the_profile() {
        let mut p = Profile::from_hull_in(
            &crate::content::Content::fixture(),
            &crate::destiny::hull::FALLBACK,
        );
        p.armor_hp = 1_000.0;
        p.hp = 400.0;
        assert_eq!(hurt_damage(&p, 25.0, 50.0), (250.0, 200.0));
        assert_eq!(hurt_damage(&p, 100.0, 100.0), (1_000.0, 396.0));
    }

    #[test]
    fn docked_says_not_in_space_and_bad_numbers_change_nothing() {
        let mut reg = world(false);
        reg.roster_mut().leave(ALICE);
        assert_eq!(text(heal(&mut reg, Some(ALICE), &[])), "not in space");
        assert_eq!(
            text(hurt(&mut reg, Some(ALICE), &["50", "50"])),
            "not in space"
        );
        let mut reg = world(false);
        for args in [
            vec![],
            vec!["50"],
            vec!["x", "50"],
            vec!["50", "150"],
            vec!["-1", "5"],
            vec!["50", "50", "1", "2"],
            vec!["50", "50", "nope"],
        ] {
            let said = text(hurt(&mut reg, Some(ALICE), &args));
            assert!(said.starts_with("usage: /hurt"), "{args:?}: {said}");
        }
        assert!(reg.combat().ship(ALICE_SHIP).is_none(), "nothing recorded");
        assert!(reg.take_due(far()).is_empty(), "and nothing sent");
    }

    #[test]
    fn suicide_destroys_the_ship_and_needs_space() {
        let mut reg = world(false);
        assert_eq!(
            text(suicide(&mut reg, Some(ALICE), &[])),
            "self-destruct: your ship is destroyed"
        );
        assert!(
            reg.roster()
                .get(ALICE)
                .is_none_or(|p| p.ship_id != ALICE_SHIP),
            "the old ship is gone from the pilot"
        );
        // The wreck and capsule need a wreck type the fixture lacks; the
        // client scenario checks them.
        reg.roster_mut().leave(BOT);
        assert_eq!(
            text(suicide(&mut reg, Some(BOT), &[])),
            "you must be in space"
        );
        assert_eq!(text(suicide(&mut reg, None, &[])), "no character to act on");
    }

    /// A fitted record for `ship`: a non-default resonance and shield, the
    /// shield shot down to nothing.
    fn seed_fitted(reg: &mut Registry, owner: i64, ship: i64) -> Profile {
        let mut profile = hull_profile(reg, owner, ship);
        profile.armor_resonance = Damage {
            em: 0.123,
            ..Damage::UNRESISTED
        };
        profile.shield_capacity = 800.0;
        profile.shield_resonance = Damage::UNRESISTED;
        profile.shield_recharge_rate = 600_000.0;
        let now = crate::pyutil::filetime_now();
        let mut rec = ShipDamage::full(profile, now);
        rec.apply(
            Damage {
                em: 1.0e9,
                ..Damage::ZERO
            },
            now,
        );
        // Whatever the volley left, the shield is down and the record fitted.
        reg.combat_mut().seed_ship(ship, rec);
        profile
    }

    #[test]
    fn hurt_keeps_the_fitted_profile_of_the_ships_record() {
        let mut reg = world(false);
        let fitted = seed_fitted(&mut reg, ALICE, ALICE_SHIP);
        let said = text(hurt(&mut reg, Some(ALICE), &["10", "10"]));
        assert!(said.starts_with("hurt Alice"), "{said}");
        let rec = reg.combat().ship(ALICE_SHIP).expect("a record");
        assert_eq!(*rec.profile(), fitted, "resists and shield survive /hurt");
    }

    #[test]
    fn hurt_leaves_the_shield_as_it_is_and_tells_the_rest_in_one_bars_update() {
        let mut reg = world(false);
        seed_fitted(&mut reg, ALICE, ALICE_SHIP);
        let _ = reg.take_due(far());
        let now = crate::pyutil::filetime_now();
        let before = reg.combat().ship(ALICE_SHIP).expect("rec").shield_at(now);
        assert!(before < 1.0, "the shield is down: {before}");
        text(hurt(&mut reg, Some(ALICE), &["10", "10"]));
        let later = crate::pyutil::filetime_now();
        let after = reg.combat().ship(ALICE_SHIP).expect("rec").shield_at(later);
        assert!(
            (after - before).abs() < 1.0,
            "the shield is not refilled: {before} -> {after}"
        );
        let sent: Vec<_> = reg
            .take_due(far())
            .into_iter()
            .map(|p| p.to_push())
            .collect();
        let bars = format!("{:?}", bstr("OnDamageStateChange"));
        let states = sent
            .iter()
            .filter(|p| p.0 == "DoDestinyUpdate" && format!("{:?}", p.1).contains(&bars))
            .count();
        assert_eq!(states, 1, "one bars update, not one per ring");
        let ring = |attr: i64| {
            sent.iter()
                .filter(|p| {
                    p.0 == "OnModuleAttributeChange" && p.1.get(2) == Some(&PyValue::Int(attr))
                })
                .count()
        };
        assert_eq!(ring(dmg_attr::SHIELD_CHARGE), 0, "no shield ring moved");
        assert_eq!(ring(dmg_attr::ARMOR_DAMAGE), 1, "the armour ring did");
    }

    #[test]
    fn heal_and_hurt_refuse_a_pilotless_ball() {
        let mut reg = world(true);
        let npc_id =
            crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, SYS, [0.0; 3]).expect("an npc");
        assert!(reg.character(npc_id).is_none());
        let id = npc_id.to_string();
        let want = format!("no character or ship {npc_id}");
        assert_eq!(text(heal(&mut reg, Some(ALICE), &[&id])), want);
        assert_eq!(text(hurt(&mut reg, Some(ALICE), &["50", "50", &id])), want);
    }

    #[test]
    fn heal_takes_me_a_ship_id_and_the_clients_trailing_arguments() {
        let mut reg = world(false);
        for args in [vec!["me"], vec!["ME", "capac=1"], vec!["100000001", "0"]] {
            text(hurt(&mut reg, Some(ALICE), &["50", "50"]));
            let said = text(heal(&mut reg, Some(ALICE), &args));
            assert!(said.starts_with("healed Alice"), "{args:?}: {said}");
            assert!(bars(&reg, ALICE_SHIP).armour > 0.999, "{args:?}");
        }
        // A ship id gates on its pilot: the bot's ship is refused too.
        let ship = BOT_SHIP.to_string();
        assert_eq!(
            text(heal(&mut reg, Some(ALICE), &[&ship, "0"])),
            BOT_REFUSED
        );
        // A module item id is neither a ship nor a character.
        let said = text(heal(&mut reg, Some(ALICE), &["555000111"]));
        assert_eq!(said, "no character or ship 555000111");
        assert_eq!(
            text(heal(&mut reg, None, &["me"])),
            "no character to act on; usage: /heal [me|characterID|shipID]"
        );
    }

    #[test]
    fn where_says_offline_for_a_character_with_no_session() {
        let mut reg = world(false);
        reg.roster_mut().leave(ALICE);
        let said = text(where_cmd(&mut reg, Some(ALICE), &[]));
        assert!(said.starts_with("Alice: offline"), "{said}");
        assert!(!said.contains("docked"), "{said}");
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: 1,
            character_id: Some(ALICE),
        });
        let said = text(where_cmd(&mut reg, Some(ALICE), &[]));
        assert!(said.starts_with("Alice: docked"), "{said}");
    }
}
