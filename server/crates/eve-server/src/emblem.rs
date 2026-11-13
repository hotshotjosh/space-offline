//! **Ship emblem licences and logos on hulls** (#207, EM-3): the Paragon store
//! grants an entitlement, the gateway lists, displays and clears logos, and a
//! ship's slim carries the enabled logo types so every viewer draws them.
//!
//! ## What the client does **[V]** (paths under the decompiled client's `src/`)
//!
//! - A Paragon purchase is `LPStoreMgr.TakeOfferForCharacter(1000419, offer, 1)`
//!   (`lpOfferEntry.py:350-361`). The licence row turns "owned" only through
//!   the gateway `GrantedNotice` (`cosmetics/client/messengers/entitlements/
//!   character/ship/noticeMessenger.py:8-37`), so **a licence purchase grants
//!   an entitlement, not an item** [I]. Falsifier: a Tranquility capture where
//!   the purchase leaves a licence item in the hangar.
//! - `entitlement.character.GetAllRequest` answers
//!   `GetAllResponse{ repeated Entitlement = 1 }`, `Entitlement{ oneof
//!   corporation_logo = 1 | alliance_logo = 2 }`, each an `Identifier{
//!   character = 1, ship_type = 2 }` (`eveProto/generated/eve_public/
//!   entitlement/character/character_pb2.py`, `ship/corplogo_pb2.py`,
//!   `ship/alliancelogo_pb2.py`); `character.Identifier` and `shiptype.
//!   Identifier` are each `{ sequential = 1 }`. A non-200 answer raises.
//! - `cosmetic.ship.logo.DisplayRequest{ id = 1 { ship = 1 { sequential = 1 },
//!   index = 2 }, attr = 2 { alliance = 1 | corporation = 2 } }` and
//!   `ClearRequest{ logo = 1 }` (`eveProto/generated/eve_public/cosmetic/ship/
//!   logo/logo_pb2.py`, `shipEmblemsRequestMessenger.py:29-39`); the client
//!   then **blocks until** machoNet `OnShipCosmeticChanged(ship, slot, cosmetic)`
//!   arrives for `(ship, slot)` (`shipcosmetics/client/fittingsgateway/
//!   quasar.py:50-86`, 5 s). `OnShipCosmeticsChanged(ship, {slot: type})`
//!   refreshes the hangar view (`quasar.py:80-81`).
//! - `shipCosmeticsMgr.GetEnabledCosmetics(shipID)` is `{backend_slot:
//!   cosmetic_type}` (`quasar.py:32-48`); the key is `slot | group << 6`
//!   (`shipcosmetics/utils/indexcast.py:17-19`).
//! - In space every viewer draws logos from the ship slim's `cosmeticsItems`
//!   (a list of enabled types, 1 corporation, 2 alliance; absent when none)
//!   with the slim's `corpID` and `allianceID` picking the image
//!   (`evegraphics/CosmeticsLoader.py:11-56`, `spaceObject/ship.py:220-224`).
//!
//! ## State (delta kind `emblem`)
//!
//! - `own char ship_type cosmetic_type -> 1`: a licence the pilot owns.
//! - `slot ship_item backend_slot -> cosmetic_type`: an enabled slot. Rows of
//!   an item that no longer exists are ignored on read and dropped by
//!   [`prune`] (run on every Display).
//!
//! ## Assumptions
//!
//! - **[V]** the client never branches on the refusal's status code: any
//!   non-200 raises (`shipEmblemsRequestMessenger.py:44-45`) and becomes a
//!   `RuntimeError` (`fittingsgateway/quasar.py:67-70`). 403 is TQ's code only
//!   by guess; it is cosmetic.
//! - **[V]** `OnShipCosmeticChanged` ignores its cosmetic argument
//!   (`quasar.py:83-86`), so type 0 on a clear cannot be rejected. A Clear of
//!   an empty slot answers 200 and still sends the events (the client waits).
//! - **[A]** any slot index from 0 to 4095 is accepted for any licence; the
//!   licence's `slot_group` is not enforced. Falsifier: a licence of another
//!   group showing in a slot the client says it does not fit.
//! - The alliance-logo rule (a pilot with no alliance is refused) is the
//!   spec's, from the client dropping the logo itself (`shipLogoController.
//!   py:52-59`).

use crate::content::{Boxed, Sources, TableSpec};
use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, get_bytes, get_uint, put_bytes, put_varint};
use crate::service::Registry;
use crate::slim_extra::SlimExtras;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// The delta kind (`delta_kinds.rs`).
pub const KIND: &str = "emblem";
const OWN: &str = "own";
const SLOT: &str = "slot";

/// `CosmeticsType.CORPORATION_LOGO` and `ALLIANCE_LOGO` **[V]**
/// (`shipcosmetics/common/const.py:11-12`).
pub const CORPORATION_LOGO: i64 = 1;
pub const ALLIANCE_LOGO: i64 = 2;

const CATEGORY_SHIP: i64 = 6;
/// The highest backend slot accepted: `slot | group << 6` with 6 bits of
/// slot and a small group.
const MAX_SLOT: i64 = 4095;
/// The machoNet events go out after the answer: the client builds the channel
/// it waits on before sending, but the answer must reach it first (as
/// `skins::EVENT_DELAY`).
pub const EVENT_DELAY: Duration = Duration::from_millis(400);

const FILE: (&str, &str) = (
    "cosmetics/ship-licences.tsv",
    "#licence_id\ttype_id\tship_type_id\tcosmetic_type\tslot_group",
);

const GRANT_CORP: &str = "eve_public.entitlement.character.ship.corplogo.GrantedNotice";
const GRANT_ALLIANCE: &str = "eve_public.entitlement.character.ship.alliancelogo.GrantedNotice";
const GET_ALL: &str = "eve_public.entitlement.character.GetAllRequest";
const LOGO_PKG: &str = "eve_public.cosmetic.ship.logo.";

// ---- the table -------------------------------------------------------------------

/// The emblem licences of one client build: licence type -> (hull type,
/// cosmetic type). Only emblem types (1, 2) are kept.
#[derive(Debug, Clone, Default)]
pub struct Licences {
    by_type: BTreeMap<i64, (i64, i64)>,
}

impl Licences {
    /// From the data lines of `cosmetics/ship-licences.tsv`.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Licences, String> {
        let mut t = Licences::default();
        for (i, line) in lines.iter().enumerate() {
            let bad = |e: &str| format!("{}: data line {}: {e}", FILE.0, i + 1);
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [_id, type_id, ship, cosmetic, _group] = f[..] else {
                return Err(bad(&format!("{} fields, want 5", f.len())));
            };
            let n = |s: &str, what: &str| s.parse::<i64>().map_err(|_| bad(what));
            let cosmetic = n(cosmetic, "cosmetic_type")?;
            if matches!(cosmetic, CORPORATION_LOGO | ALLIANCE_LOGO) {
                t.by_type
                    .insert(n(type_id, "type_id")?, (n(ship, "ship_type_id")?, cosmetic));
            }
        }
        Ok(t)
    }

    pub fn is_empty(&self) -> bool {
        self.by_type.is_empty()
    }
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "emblem",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Licences::default()));
    };
    Ok(Box::new(Licences::parse(&b.data_lines(FILE.0, FILE.1)?)?))
}

/// Invented rows for tests: ids are not the client's.
pub mod fixture {
    /// A corporation-logo licence for [`HULL`].
    pub const LIC_CORP: i64 = 981_400_001;
    /// An alliance-logo licence for [`HULL`].
    pub const LIC_ALLIANCE: i64 = 981_400_002;
    /// A corporation-logo licence for [`HULL_OTHER`].
    pub const LIC_OTHER: i64 = 981_400_003;
    pub const HULL: i64 = 981_500_001;
    pub const HULL_OTHER: i64 = 981_500_002;

    /// The rows, with the first two licences on `hull`.
    pub fn lines(hull: i64) -> Vec<String> {
        vec![
            format!("1\t{LIC_CORP}\t{hull}\t1\t0"),
            format!("2\t{LIC_ALLIANCE}\t{hull}\t2\t0"),
            format!("3\t{LIC_OTHER}\t{HULL_OTHER}\t1\t0"),
        ]
    }
}

fn boxed_fixture() -> Boxed {
    Box::new(Licences::parse(&fixture::lines(fixture::HULL)).expect("the fixture rows parse"))
}

fn table(reg: &Registry) -> &Licences {
    static EMPTY: std::sync::OnceLock<Licences> = std::sync::OnceLock::new();
    reg.content()
        .try_get::<Licences>()
        .unwrap_or_else(|| EMPTY.get_or_init(Licences::default))
}

/// `(hull type, cosmetic type)` when `type_id` is an emblem licence type.
pub fn licence_of(reg: &Registry, type_id: i64) -> Option<(i64, i64)> {
    table(reg).by_type.get(&type_id).copied()
}

// ---- owned licences --------------------------------------------------------------

fn own_key(ch: i64, ship_type: i64, cosmetic: i64) -> Vec<String> {
    vec![
        OWN.to_owned(),
        ch.to_string(),
        ship_type.to_string(),
        cosmetic.to_string(),
    ]
}

/// Whether `ch` owns the licence for `(ship_type, cosmetic)`.
pub fn owns(reg: &Registry, ch: i64, ship_type: i64, cosmetic: i64) -> bool {
    reg.deltas()
        .of(KIND)
        .contains_key(&own_key(ch, ship_type, cosmetic))
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn sequential(id: i64) -> Vec<u8> {
    let mut m = Vec::new();
    put_varint_field(&mut m, 1, id as u64);
    m
}

/// `corplogo.Identifier` / `alliancelogo.Identifier`: `{character, ship_type}`.
fn identifier(ch: i64, ship_type: i64) -> Vec<u8> {
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &sequential(ch));
    put_bytes(&mut m, 2, &sequential(ship_type));
    m
}

/// Grant `ch` the licence for `(ship_type, cosmetic)`: the row, and the
/// gateway `GrantedNotice` queued for the pilot. `false` (nothing written)
/// when the pilot owns it already.
pub fn grant(reg: &mut Registry, ch: i64, ship_type: i64, cosmetic: i64) -> bool {
    if !matches!(cosmetic, CORPORATION_LOGO | ALLIANCE_LOGO) || owns(reg, ch, ship_type, cosmetic) {
        return false;
    }
    reg.deltas_mut()
        .put(KIND, own_key(ch, ship_type, cosmetic), vec!["1".to_owned()]);
    reg.mark_dirty();
    let mut notice = Vec::new();
    put_bytes(&mut notice, 1, &identifier(ch, ship_type));
    let ty = if cosmetic == CORPORATION_LOGO {
        GRANT_CORP
    } else {
        GRANT_ALLIANCE
    };
    crate::gateway_out::queue(reg, ty, notice, Target::Character(ch));
    true
}

/// Every `(ship_type, cosmetic)` `ch` owns, ascending.
fn owned(d: &Deltas, ch: i64) -> Vec<(i64, i64)> {
    let c = ch.to_string();
    let from = vec![OWN.to_owned(), c.clone(), String::new(), String::new()];
    d.of(KIND)
        .range(from..)
        .take_while(|(k, _)| k.len() == 4 && k[0] == OWN && k[1] == c)
        .filter_map(|(k, _)| Some((k[2].parse().ok()?, k[3].parse().ok()?)))
        .collect()
}

// ---- enabled slots ---------------------------------------------------------------

fn slot_key(ship: i64, slot: i64) -> Vec<String> {
    vec![SLOT.to_owned(), ship.to_string(), slot.to_string()]
}

/// `(backend_slot, cosmetic_type)` of `ship`'s enabled slots, ascending slot;
/// nothing for an item the world does not hold.
fn slots_of(reg: &Registry, ship: i64) -> Vec<(i64, i64)> {
    let d = reg.deltas().of(KIND);
    if d.is_empty() || reg.store().get(ship).is_none() {
        return Vec::new();
    }
    let (owner, hull) = reg
        .store()
        .get(ship)
        .map_or((0, 0), |i| (i.owner_id, i.type_id));
    let s = ship.to_string();
    let from = vec![SLOT.to_owned(), s.clone(), String::new()];
    // A row counts only while the ship's CURRENT owner owns the licence: a
    // hull given to a pilot without it shows no logo.
    let mut v: Vec<(i64, i64)> = d
        .range(from..)
        .take_while(|(k, _)| k.len() == 3 && k[0] == SLOT && k[1] == s)
        .filter_map(|(k, val)| Some((k[2].parse().ok()?, val.first()?.parse().ok()?)))
        .filter(|&(_, c)| owns(reg, owner, hull, c))
        .collect();
    v.sort_unstable();
    v
}

/// `shipCosmeticsMgr.GetEnabledCosmetics(shipID)` for `actor`: the dict
/// `{backend_slot: cosmetic_type}`; empty for a ship that is not the actor's.
pub fn enabled(reg: &Registry, actor: i64, ship: i64) -> PyValue {
    let mine = reg.store().get(ship).is_some_and(|i| i.owner_id == actor);
    PyValue::Dict(if mine {
        slots_of(reg, ship)
            .into_iter()
            .map(|(s, c)| (PyValue::Int(s), PyValue::Int(c)))
            .collect()
    } else {
        Vec::new()
    })
}

/// Drop the slot rows of ships that no longer exist.
pub fn prune(reg: &mut Registry) {
    let gone: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| k.len() == 3 && k[0] == SLOT)
        .filter(|k| {
            k[1].parse::<i64>()
                .ok()
                .is_none_or(|s| reg.store().get(s).is_none())
        })
        .cloned()
        .collect();
    if gone.is_empty() {
        return;
    }
    for k in gone {
        reg.deltas_mut().del(KIND, &k);
    }
    reg.mark_dirty();
}

/// The slim attribute of the ships among `ids` (`slim_extra::SOURCES`):
/// `cosmeticsItems`, the sorted distinct enabled types. A ship whose owner
/// owns an emblem licence for its hull gets `[]` when no slot is enabled, never
/// a missing key: the client's slim returns `None` for a missing name, and
/// `CosmeticsLoader.Update` returns on `None` before it resets the logo
/// paths, so a cleared logo would stay drawn [V `slimItem.py:23-46`,
/// `spaceObject/ship.py:269-271`, `CosmeticsLoader.py:35-38`]. A ship of an
/// owner with no licence keeps no key.
pub fn slim(reg: &Registry, ids: &[i64]) -> SlimExtras {
    if reg.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    ids.iter()
        .filter_map(|&id| {
            let it = reg.store().get(id)?;
            let licensed = [CORPORATION_LOGO, ALLIANCE_LOGO]
                .iter()
                .any(|&c| owns(reg, it.owner_id, it.type_id, c));
            if !licensed {
                return None;
            }
            let mut types: Vec<i64> = slots_of(reg, id).into_iter().map(|(_, c)| c).collect();
            types.sort_unstable();
            types.dedup();
            Some((
                id,
                vec![(
                    "cosmeticsItems",
                    PyValue::List(types.into_iter().map(PyValue::Int).collect()),
                )],
            ))
        })
        .collect()
}

// ---- Display and Clear -----------------------------------------------------------

/// Why a Display or Clear was refused (all answer 403 [A]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    NoSuchShip,
    NotYours,
    NotAShip,
    NotLicensed,
    NoAlliance,
    BadSlot,
}

fn my_ship(reg: &Registry, actor: i64, ship: i64) -> Result<i64, Refused> {
    let it = reg.store().get(ship).ok_or(Refused::NoSuchShip)?;
    if it.owner_id != actor {
        return Err(Refused::NotYours);
    }
    if it.category_id != CATEGORY_SHIP {
        return Err(Refused::NotAShip);
    }
    Ok(it.type_id)
}

/// Show `cosmetic` in `slot` of `actor`'s `ship`. The ship must be the
/// actor's; the actor must own the licence for the hull and type, and have an
/// alliance for type 2. Writes nothing on `Err`.
pub fn display(
    reg: &mut Registry,
    actor: i64,
    ship: i64,
    slot: i64,
    cosmetic: i64,
) -> Result<(), Refused> {
    let hull = my_ship(reg, actor, ship)?;
    if !(0..=MAX_SLOT).contains(&slot) {
        return Err(Refused::BadSlot);
    }
    if !matches!(cosmetic, CORPORATION_LOGO | ALLIANCE_LOGO) || !owns(reg, actor, hull, cosmetic) {
        return Err(Refused::NotLicensed);
    }
    if cosmetic == ALLIANCE_LOGO && reg.character(actor).and_then(|c| c.alliance_id).is_none() {
        return Err(Refused::NoAlliance);
    }
    prune(reg);
    reg.deltas_mut()
        .put(KIND, slot_key(ship, slot), vec![cosmetic.to_string()]);
    reg.mark_dirty();
    announce(reg, actor, ship, slot, cosmetic);
    Ok(())
}

/// Empty `slot` of `actor`'s `ship`.
pub fn clear(reg: &mut Registry, actor: i64, ship: i64, slot: i64) -> Result<(), Refused> {
    my_ship(reg, actor, ship)?;
    if !(0..=MAX_SLOT).contains(&slot) {
        return Err(Refused::BadSlot);
    }
    if reg.deltas_mut().del(KIND, &slot_key(ship, slot)).is_some() {
        reg.mark_dirty();
    }
    announce(reg, actor, ship, slot, 0);
    Ok(())
}

#[derive(Debug, Clone)]
struct Event {
    to: i64,
    name: &'static str,
    args: Vec<PyValue>,
}

impl crate::notify::Push for Event {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.name, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// The two machoNet events to the pilot (after the answer), and a fresh slim
/// of the ship to its grid when the pilot is flying it, so viewers redraw.
fn announce(reg: &mut Registry, actor: i64, ship: i64, slot: i64, cosmetic: i64) {
    let now: Vec<(PyValue, PyValue)> = slots_of(reg, ship)
        .into_iter()
        .map(|(s, c)| (PyValue::Int(s), PyValue::Int(c)))
        .collect();
    reg.push_later(
        EVENT_DELAY,
        Event {
            to: actor,
            name: "OnShipCosmeticChanged",
            args: vec![
                PyValue::Int(ship),
                PyValue::Int(slot),
                PyValue::Int(cosmetic),
            ],
        },
    );
    reg.push_later(
        EVENT_DELAY,
        Event {
            to: actor,
            name: "OnShipCosmeticsChanged",
            args: vec![PyValue::Int(ship), PyValue::Dict(now)],
        },
    );
    if reg.roster().get(actor).is_some_and(|p| p.ship_id == ship) {
        crate::stance::refresh_slim(reg, actor);
    }
}

// ---- the gateway -----------------------------------------------------------------

fn status(code: u32) -> Reply {
    Reply {
        status: code,
        payload: None,
    }
}

fn typed(name: &str, payload: Vec<u8>) -> Reply {
    Reply {
        status: 200,
        payload: Some((name.to_owned(), payload)),
    }
}

/// `GetAllResponse` for `ch`: one `Entitlement` per owned licence.
fn get_all(reg: &Registry, ch: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for (ship_type, cosmetic) in owned(reg.deltas(), ch) {
        let mut e = Vec::new();
        let field = if cosmetic == CORPORATION_LOGO { 1 } else { 2 };
        put_bytes(&mut e, field, &identifier(ch, ship_type));
        put_bytes(&mut out, 1, &e);
    }
    out
}

/// The gateway arm (`gateway_ext::CHAIN`): `entitlement.character.GetAllRequest`
/// and the ship logo `DisplayRequest` and `ClearRequest`. `None` for the rest.
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    if ty == GET_ALL {
        let name = "eve_public.entitlement.character.GetAllResponse";
        let Ground::Server(w) = g else {
            return Some(typed(name, Vec::new()));
        };
        let reg = crate::service::world_read_timed(w, "emblem_gateway");
        return Some(typed(name, get_all(&reg, character)));
    }
    let rest = ty.strip_prefix(LOGO_PKG)?;
    if !matches!(rest, "DisplayRequest" | "ClearRequest") {
        return None;
    }
    let name = format!("{}Response", ty.strip_suffix("Request").unwrap_or(ty));
    let is_display = rest == "DisplayRequest";
    // Display: `id = 1`, `attr = 2`; Clear: `logo = 1` (the same Identifier).
    let Some(id) = get_bytes(body, 1) else {
        return Some(status(400));
    };
    let Some(ship) = get_bytes(&id, 1).and_then(|s| get_uint(&s, 1)) else {
        return Some(status(400));
    };
    let slot = get_uint(&id, 2).unwrap_or(0);
    let Ground::Server(w) = g else {
        return Some(status(403));
    };
    let (ship, slot) = (ship as i64, slot.min(i64::MAX as u64) as i64);
    let mut reg = crate::service::world_write_timed(w, "emblem_gateway_write");
    let done = if is_display {
        let Some(attr) = get_bytes(body, 2) else {
            return Some(status(400));
        };
        let cosmetic = if get_bytes(&attr, 1).is_some() {
            ALLIANCE_LOGO
        } else if get_bytes(&attr, 2).is_some() {
            CORPORATION_LOGO
        } else {
            return Some(status(400));
        };
        display(&mut reg, character, ship, slot, cosmetic)
    } else {
        clear(&mut reg, character, ship, slot)
    };
    Some(match done {
        Ok(()) => typed(&name, Vec::new()),
        Err(_) => status(403),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::*;
    use std::sync::{Arc, RwLock};

    const ALICE: i64 = 1_001;
    const BOB: i64 = 1_002;
    const STATION: i64 = 600_001;
    const SYSTEM: i64 = 3_001;
    const ALLIANCE: i64 = 99_000_777;

    /// Two docked pilots (Bob is in an alliance); the table fits Alice's hull.
    fn world() -> Registry {
        let mut reg = crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: ALICE,
                owner_user_id: Some(11),
                station_id: Some(STATION),
                ..Default::default()
            };
            let a_ship = a.ship_id;
            let mut b = a.clone();
            b.character_id = BOB;
            b.ship_id = a_ship + 1;
            b.owner_user_id = Some(12);
            b.alliance_id = Some(ALLIANCE);
            cfg.characters = vec![a, b];
            cfg.catch_all = None;
        });
        let mut content = crate::content::Content::fixture();
        content.insert("emblem", Licences::parse(&lines(FRIGATE)).unwrap());
        reg = reg.with_content(Arc::new(content));
        for who in [ALICE, BOB] {
            let at = crate::item::Slot::new(who, STATION, crate::model::FLAG_HANGAR);
            reg.apply_items(&[crate::item::Edit::Create {
                type_id: FRIGATE,
                quantity: 1,
                to: at,
            }])
            .expect("the typedata fixture frigate is a known type");
        }
        reg
    }

    /// The typedata fixture's invented frigate (category 6).
    const FRIGATE: i64 = 990_000_001;

    /// `who`'s fixture frigate (not the capsule, whose type is a real id).
    fn ship_of(reg: &Registry, who: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .find(|i| i.owner_id == who && i.type_id == FRIGATE)
            .expect("the world seeds a frigate per pilot")
            .item_id
    }

    fn hull_of(_reg: &Registry, _who: i64) -> i64 {
        FRIGATE
    }

    fn arrive(reg: &mut Registry, who: i64) {
        let ship = ship_of(reg, who);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                who,
                ship,
                SYSTEM,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
    }

    fn cosmetics_items(reg: &Registry, ship: i64) -> Option<PyValue> {
        slim(reg, &[ship])
            .into_iter()
            .find(|(id, _)| *id == ship)
            .and_then(|(_, v)| v.into_iter().find(|(k, _)| *k == "cosmeticsItems"))
            .map(|(_, v)| v)
    }

    fn ground(reg: Registry) -> Ground {
        Ground::Server(Arc::new(RwLock::new(reg)))
    }

    fn id_bytes(ship: i64, slot: i64) -> Vec<u8> {
        let mut id = Vec::new();
        put_bytes(&mut id, 1, &sequential(ship));
        if slot != 0 {
            put_varint_field(&mut id, 2, slot as u64);
        }
        id
    }

    fn display_body(ship: i64, slot: i64, cosmetic: i64) -> Vec<u8> {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &id_bytes(ship, slot));
        let mut attr = Vec::new();
        put_bytes(
            &mut attr,
            if cosmetic == ALLIANCE_LOGO { 1 } else { 2 },
            &[],
        );
        put_bytes(&mut b, 2, &attr);
        b
    }

    #[test]
    fn a_grant_is_listed_by_get_all_and_queues_the_notice() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        assert_eq!(licence_of(&reg, LIC_CORP), Some((hull, CORPORATION_LOGO)));
        assert_eq!(licence_of(&reg, 5), None);
        assert!(grant(&mut reg, ALICE, hull, CORPORATION_LOGO));
        let q = crate::gateway_out::take(&mut reg);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, GRANT_CORP);
        assert!(matches!(q[0].2, Target::Character(c) if c == ALICE));
        let ent = get_bytes(&q[0].1, 1).expect("an identifier");
        assert_eq!(
            get_bytes(&ent, 1).and_then(|c| get_uint(&c, 1)),
            Some(ALICE as u64)
        );
        assert_eq!(
            get_bytes(&ent, 2).and_then(|c| get_uint(&c, 1)),
            Some(hull as u64)
        );
        let g = ground(reg);
        let r = gateway(GET_ALL, &[], ALICE, &g).expect("answered");
        assert_eq!(r.status, 200);
        let (name, bytes) = r.payload.expect("a payload");
        assert!(name.ends_with("GetAllResponse"));
        let entry = get_bytes(&bytes, 1).expect("one entitlement");
        let logo = get_bytes(&entry, 1).expect("a corporation logo");
        assert_eq!(
            get_bytes(&logo, 2).and_then(|c| get_uint(&c, 1)),
            Some(hull as u64)
        );
        let other = gateway(GET_ALL, &[], BOB, &g)
            .and_then(|r| r.payload)
            .unwrap();
        assert!(other.1.is_empty(), "Bob owns nothing");
    }

    #[test]
    fn an_owned_licence_is_not_granted_twice() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        assert!(grant(&mut reg, ALICE, hull, ALLIANCE_LOGO));
        let _ = crate::gateway_out::take(&mut reg);
        assert!(!grant(&mut reg, ALICE, hull, ALLIANCE_LOGO));
        assert!(crate::gateway_out::take(&mut reg).is_empty());
        assert!(!grant(&mut reg, ALICE, hull, 3), "only emblem types");
    }

    #[test]
    fn display_on_a_hull_of_another_type_is_refused() {
        let mut reg = world();
        grant(&mut reg, ALICE, HULL_OTHER, CORPORATION_LOGO);
        let ship = ship_of(&reg, ALICE);
        assert_eq!(
            display(&mut reg, ALICE, ship, 0, CORPORATION_LOGO),
            Err(Refused::NotLicensed)
        );
        assert!(slots_of(&reg, ship).is_empty());
        // Nor someone else's ship, nor a licence not owned.
        let hull = hull_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        assert_eq!(
            display(&mut reg, BOB, ship, 0, CORPORATION_LOGO),
            Err(Refused::NotYours)
        );
        assert_eq!(
            display(&mut reg, ALICE, ship, 0, ALLIANCE_LOGO),
            Err(Refused::NotLicensed)
        );
        assert_eq!(
            display(&mut reg, ALICE, ship, 5000, CORPORATION_LOGO),
            Err(Refused::BadSlot)
        );
    }

    #[test]
    fn display_then_the_slim_carries_the_type_and_clear_removes_it() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        assert_eq!(
            cosmetics_items(&reg, ship),
            Some(PyValue::List(vec![])),
            "a licensed ship with no slot carries []"
        );
        display(&mut reg, ALICE, ship, 0, CORPORATION_LOGO).unwrap();
        assert_eq!(
            cosmetics_items(&reg, ship),
            Some(PyValue::List(vec![PyValue::Int(1)]))
        );
        assert_eq!(
            enabled(&reg, ALICE, ship),
            PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(1))])
        );
        assert_eq!(enabled(&reg, BOB, ship), PyValue::Dict(vec![]), "not Bob's");
        // The two machoNet events follow, to the pilot only.
        let told: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| n.starts_with("OnShipCosmetic"))
            .collect();
        assert_eq!(
            told[0],
            (
                "OnShipCosmeticChanged",
                vec![PyValue::Int(ship), PyValue::Int(0), PyValue::Int(1)]
            )
        );
        assert_eq!(told[1].0, "OnShipCosmeticsChanged");
        clear(&mut reg, ALICE, ship, 0).unwrap();
        // [] and never a missing key: the client keeps the old logo on `None`.
        assert_eq!(cosmetics_items(&reg, ship), Some(PyValue::List(vec![])));
        assert_eq!(enabled(&reg, ALICE, ship), PyValue::Dict(vec![]));
    }

    #[test]
    fn two_slots_of_one_type_list_it_once_and_both_types_sort() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        grant(&mut reg, ALICE, hull, ALLIANCE_LOGO);
        reg.character_mut(ALICE).unwrap().alliance_id = Some(ALLIANCE);
        display(&mut reg, ALICE, ship, 1, ALLIANCE_LOGO).unwrap();
        display(&mut reg, ALICE, ship, 2, CORPORATION_LOGO).unwrap();
        display(&mut reg, ALICE, ship, 3, CORPORATION_LOGO).unwrap();
        assert_eq!(
            cosmetics_items(&reg, ship),
            Some(PyValue::List(vec![PyValue::Int(1), PyValue::Int(2)]))
        );
    }

    #[test]
    fn an_alliance_logo_with_no_alliance_is_refused() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, ALLIANCE_LOGO);
        assert_eq!(
            display(&mut reg, ALICE, ship, 0, ALLIANCE_LOGO),
            Err(Refused::NoAlliance)
        );
        assert!(slots_of(&reg, ship).is_empty());
        reg.character_mut(ALICE).unwrap().alliance_id = Some(ALLIANCE);
        assert_eq!(display(&mut reg, ALICE, ship, 0, ALLIANCE_LOGO), Ok(()));
    }

    #[test]
    fn a_ship_given_to_a_pilot_without_the_licence_shows_no_logo() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, FRIGATE, CORPORATION_LOGO);
        display(&mut reg, ALICE, ship, 0, CORPORATION_LOGO).unwrap();
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: ship,
            to: crate::item::Slot::new(BOB, STATION, crate::model::FLAG_HANGAR),
        }])
        .expect("moves");
        assert_eq!(reg.store().get(ship).unwrap().owner_id, BOB);
        assert_eq!(enabled(&reg, BOB, ship), PyValue::Dict(vec![]));
        assert_eq!(cosmetics_items(&reg, ship), None, "Bob has no licence");
        assert_eq!(enabled(&reg, ALICE, ship), PyValue::Dict(vec![]));
    }

    #[test]
    fn a_destroyed_ship_drops_its_slot_rows() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        display(&mut reg, ALICE, ship, 0, CORPORATION_LOGO).unwrap();
        assert_eq!(
            reg.deltas()
                .of(KIND)
                .keys()
                .filter(|k| k[0] == SLOT)
                .count(),
            1
        );
        reg.apply_items(&[crate::item::Edit::Destroy { item_id: ship }])
            .expect("the ship goes");
        assert_eq!(cosmetics_items(&reg, ship), None, "a gone ship has no slim");
        prune(&mut reg);
        assert_eq!(
            reg.deltas()
                .of(KIND)
                .keys()
                .filter(|k| k[0] == SLOT)
                .count(),
            0
        );
        assert!(
            owns(&reg, ALICE, hull, CORPORATION_LOGO),
            "the licence stays"
        );
    }

    #[test]
    fn the_gateway_displays_and_clears_and_refuses_with_403() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        arrive(&mut reg, ALICE);
        let g = ground(reg);
        let ty = |s: &str| format!("{LOGO_PKG}{s}");
        let ok = gateway(&ty("DisplayRequest"), &display_body(ship, 3, 1), ALICE, &g).unwrap();
        assert_eq!(ok.status, 200);
        assert!(ok.payload.is_some_and(|p| p.0.ends_with("DisplayResponse")));
        let Ground::Server(w) = &g else {
            unreachable!()
        };
        assert_eq!(
            enabled(&w.read().unwrap(), ALICE, ship),
            PyValue::Dict(vec![(PyValue::Int(3), PyValue::Int(1))])
        );
        // Not Bob's ship, not the alliance licence, not a malformed body.
        for (body, who) in [
            (display_body(ship, 0, 1), BOB),
            (display_body(ship, 0, 2), ALICE),
        ] {
            let r = gateway(&ty("DisplayRequest"), &body, who, &g).unwrap();
            assert_eq!(r.status, 403);
        }
        assert_eq!(
            gateway(&ty("DisplayRequest"), &[], ALICE, &g)
                .unwrap()
                .status,
            400
        );
        let mut clear_body = Vec::new();
        put_bytes(&mut clear_body, 1, &id_bytes(ship, 3));
        let r = gateway(&ty("ClearRequest"), &clear_body, ALICE, &g).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(
            enabled(&w.read().unwrap(), ALICE, ship),
            PyValue::Dict(vec![])
        );
        assert_eq!(gateway("eve_public.other.Thing", &[], ALICE, &g), None);
    }

    #[test]
    fn a_ship_flown_in_space_gets_a_fresh_slim_scheduled() {
        let mut reg = world();
        let hull = hull_of(&reg, ALICE);
        let ship = ship_of(&reg, ALICE);
        grant(&mut reg, ALICE, hull, CORPORATION_LOGO);
        arrive(&mut reg, ALICE);
        arrive(&mut reg, BOB);
        let before = reg.scheduled_len();
        display(&mut reg, ALICE, ship, 0, CORPORATION_LOGO).unwrap();
        assert!(
            reg.scheduled_len() > before,
            "an OnSlimItemChange is scheduled"
        );
    }
}
