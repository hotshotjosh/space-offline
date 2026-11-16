//! **Flying the corp logo** (EM-BOT, #207): a bot that holds Evermarks (a
//! heraldry turn-in pays them, [`crate::missioner`]) buys the corporation-logo
//! licence for the hull it flies and shows the logo on it, so other pilots
//! see it. [`crate::lp_spend::spend`] calls [`run`] when the pilot holds
//! Evermarks; they are never converted to ISK.
//!
//! The calls, all the client's own **[V]** (`eve_server::emblem` header):
//! - the purchase is `LPStoreMgr.TakeOfferForCharacter(1000419, offer, 1)`;
//!   it grants an entitlement, not an item;
//! - the owned licences are `entitlement.character.GetAllRequest` on the
//!   gateway (`GetAllResponse{ repeated Entitlement = 1 }`, `Entitlement{
//!   corporation_logo = 1 | alliance_logo = 2 }`, each an `Identifier{
//!   character = 1, ship_type = 2 }`);
//! - the logo is `cosmetic.ship.logo.DisplayRequest{ id = 1 { ship = 1 {
//!   sequential = 1 }, index = 2 }, attr = 2 { corporation = 2 } }`.
//!
//! A bot never gets a licence from nowhere: it spends Evermarks it earned.
//! Every refusal is a note, never a failure.
//!
//! **[A]** (falsifiers in [`run`]'s doc): the gateway listens on the game
//! server's address; the slot is 0; "the main hull" is the ship the bot is
//! flying.

use crate::clock;
use crate::gateway_client::GatewayClient;
use crate::lp_spend::CORP_HERALDRY;
use crate::plexer::{LP_STORE, LpOffer, read_offers};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::emblem::{CORPORATION_LOGO, Licences};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GET_ALL: &str = "eve_public.entitlement.character.GetAllRequest";
const DISPLAY: &str = "eve_public.cosmetic.ship.logo.DisplayRequest";

/// The backend slot the logo is shown in. **[A]** slot 0 is free on a ship a
/// bot has never decorated. Falsified by a Display refused for slot 0 (the
/// note says so).
pub const SLOT: i64 = 0;

/// How long the gateway may take to answer one request.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How long a missing gateway is left alone.
const NO_GATEWAY_BACKOFF: Duration = Duration::from_secs(600);

/// What this process has done, so a paid desk visit costs nothing once the
/// logo is up: `(pilot, ship)` pairs whose logo is shown, `(pilot, ship)` ->
/// the Evermarks with which no licence was found (nothing to try again below
/// that), and when the gateway was last missing. Per process, never persisted.
static SHOWN: Mutex<BTreeSet<(i64, i64)>> = Mutex::new(BTreeSet::new());
static BROKE: Mutex<BTreeMap<(i64, i64), i64>> = Mutex::new(BTreeMap::new());
static GATEWAY_DOWN: Mutex<Option<Instant>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the step has nothing to do for `(pilot, ship)` holding `em`.
fn settled(pilot: i64, ship: i64, em: i64) -> bool {
    lock(&SHOWN).contains(&(pilot, ship))
        || lock(&BROKE)
            .get(&(pilot, ship))
            .is_some_and(|tried| em <= *tried)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn read_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let x = *b.get(*p)?;
        *p += 1;
        r |= u64::from(x & 0x7f).checked_shl(s)?;
        if x < 0x80 {
            return Some(r);
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// One top-level field: a varint, or a length-delimited value.
enum Field {
    Int(u64),
    Bytes(Vec<u8>),
}

/// The top-level fields of a message, `(number, value)`; `None` on a
/// malformed message.
fn fields(b: &[u8]) -> Option<Vec<(u64, Field)>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let k = read_varint(b, &mut p)?;
        match k & 7 {
            0 => out.push((k >> 3, Field::Int(read_varint(b, &mut p)?))),
            2 => {
                let l = usize::try_from(read_varint(b, &mut p)?).ok()?;
                let end = p.checked_add(l).filter(|e| *e <= b.len())?;
                out.push((k >> 3, Field::Bytes(b[p..end].to_vec())));
                p = end;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn sub(b: &[u8], n: u64) -> Option<Vec<u8>> {
    fields(b)?.into_iter().find_map(|(k, v)| match v {
        Field::Bytes(x) if k == n => Some(x),
        _ => None,
    })
}

fn uint(b: &[u8], n: u64) -> Option<u64> {
    fields(b)?.into_iter().find_map(|(k, v)| match v {
        Field::Int(x) if k == n => Some(x),
        _ => None,
    })
}

/// **The logo licences a `GetAllResponse` lists**: `(ship type, cosmetic
/// type)`, 1 corporation and 2 alliance. Entitlements of another shape are
/// skipped.
pub fn owned_licences(payload: &[u8]) -> BTreeSet<(i64, i64)> {
    let mut out = BTreeSet::new();
    for (k, v) in fields(payload).unwrap_or_default() {
        let (1, Field::Bytes(entry)) = (k, v) else {
            continue;
        };
        for cosmetic in [1u64, 2] {
            if let Some(ship) = sub(&entry, cosmetic)
                .and_then(|id| sub(&id, 2))
                .and_then(|t| uint(&t, 1))
            {
                out.insert((ship as i64, cosmetic as i64));
            }
        }
    }
    out
}

/// **The `DisplayRequest` body** for the corporation logo on `ship` in
/// `slot`: `id{ ship{sequential}, index }`, and `attr{ corporation = 2 }`
/// empty, the wire the server reads (`emblem::gateway`), with no message.
pub fn display_body(ship: i64, slot: i64) -> Vec<u8> {
    let mut seq = Vec::new();
    put_uint(&mut seq, 1, ship as u64);
    let mut id = Vec::new();
    put_bytes(&mut id, 1, &seq);
    if slot != 0 {
        put_uint(&mut id, 2, slot as u64);
    }
    let mut body = Vec::new();
    put_bytes(&mut body, 1, &id);
    let mut attr = Vec::new();
    put_bytes(&mut attr, 2, &[]);
    put_bytes(&mut body, 2, &attr);
    body
}

/// **The licence to buy**, pure: the corporation-logo licence of `hull` (the
/// table's row), costing no ISK and at most `em` Evermarks, the cheapest
/// (the lower offer id on a tie).
pub fn pick(offers: &[LpOffer], lic: &Licences, hull: i64, em: i64) -> Option<LpOffer> {
    offers
        .iter()
        .filter(|o| {
            lic.get(o.type_id) == Some((hull, CORPORATION_LOGO))
                && o.isk_cost == 0
                && o.req_items.is_empty()
                && o.lp_cost > 0
                && o.lp_cost <= em
        })
        .min_by_key(|o| (o.lp_cost, o.offer_id))
        .cloned()
}

fn value(c: &mut Conn, method: &str, args: Vec<PyValue>) -> Result<PyValue> {
    let outcome = c.call(LP_STORE, method, args)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("[em] {method}: {key}")));
    }
    Ok(outcome.value().unwrap_or(PyValue::None))
}

/// **Buy the corp-logo licence for the flown hull when it is not owned, and
/// show the logo**; the note says what happened. `em` is the Evermark
/// balance. Docked, flying a hull from the hangar.
///
/// **[A]**: the gateway is at the game server's address on
/// [`eve_server::gateway::PORT`] (falsified by the note "no gateway" on a
/// server that has it elsewhere); "the main hull" is the ship flown now
/// (falsified by a bot that should decorate another hull).
pub fn run(c: &mut Conn, em: i64) -> Result<String> {
    let Some(lic) = crate::acquire::with_content(|ct| ct.try_get::<Licences>().cloned()) else {
        return Ok("[em] no licence table".to_owned());
    };
    let (Some(ship), Some(pilot)) = (c.session().ship_id(), c.session().char_id()) else {
        return Ok("[em] not flying a ship".to_owned());
    };
    if settled(pilot, ship, em) {
        return Ok("[em] nothing to do".to_owned());
    }
    if lock(&GATEWAY_DOWN).is_some_and(|t| clock::since(t) < NO_GATEWAY_BACKOFF) {
        return Ok("[em] no gateway".to_owned());
    }
    let Some(hull) = c
        .hangar_items()?
        .iter()
        .find(|r| r.item_id == ship)
        .map(|r| r.type_id)
    else {
        return Ok("[em] the flown hull is not in the hangar".to_owned());
    };
    let peer = c.socket().and_then(|s| s.peer_addr());
    let Ok(peer) = peer else {
        return Ok("[em] no gateway".to_owned());
    };
    let addr = SocketAddr::new(peer.ip(), eve_server::gateway::PORT);
    let Ok(mut gw) = GatewayClient::connect(addr, TIMEOUT) else {
        *lock(&GATEWAY_DOWN) = Some(clock::instant());
        return Ok("[em] no gateway".to_owned());
    };
    let have = match gw.send(pilot, GET_ALL, &[]) {
        Ok((200, Some((_, body)))) => owned_licences(&body),
        Ok((status, _)) => return Ok(format!("[em] GetAll answered {status}")),
        Err(_) => return Ok("[em] no gateway".to_owned()),
    };
    if !have.contains(&(hull, CORPORATION_LOGO)) {
        let store = value(
            c,
            "GetAvailableOffersFromCorp",
            vec![PyValue::Int(CORP_HERALDRY)],
        )?;
        let Some(offer) = pick(&read_offers(&store), &lic, hull, em) else {
            lock(&BROKE).insert((pilot, ship), em);
            return Ok(format!("[em] no licence for hull {hull} within {em} EM"));
        };
        let took = value(
            c,
            "TakeOfferForCharacter",
            vec![
                PyValue::Int(CORP_HERALDRY),
                PyValue::Int(offer.offer_id),
                PyValue::Int(1),
            ],
        )?;
        if took != PyValue::Bool(true) {
            return Ok(format!("[em] the store refused offer {}", offer.offer_id));
        }
    }
    match gw.send(pilot, DISPLAY, &display_body(ship, SLOT)) {
        Ok((200, _)) => {
            lock(&SHOWN).insert((pilot, ship));
            Ok(format!("[em] licence {hull} enabled"))
        }
        Ok((status, _)) => Ok(format!("[em] Display answered {status}")),
        Err(_) => Ok("[em] no gateway".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::emblem::fixture::{HULL, HULL_OTHER, LIC_ALLIANCE, LIC_CORP, LIC_OTHER, lines};
    use eve_server::gateway::Ground;
    use std::sync::{Arc, RwLock};

    fn offer(offer_id: i64, type_id: i64, lp: i64, isk: i64) -> LpOffer {
        LpOffer {
            offer_id,
            type_id,
            qty: 1,
            lp_cost: lp,
            isk_cost: isk,
            req_items: Vec::new(),
        }
    }

    fn table() -> Licences {
        Licences::parse(&lines(HULL)).expect("the fixture rows parse")
    }

    #[test]
    fn pick_takes_the_corp_logo_for_this_hull() {
        let lic = table();
        let offers = vec![
            offer(1, LIC_ALLIANCE, 100, 0),
            offer(2, LIC_OTHER, 100, 0),
            offer(3, LIC_CORP, 900, 0),
            offer(4, LIC_CORP, 700, 0),
            offer(5, LIC_CORP, 600, 5),
            offer(6, 5, 10, 0),
        ];
        // The corp logo of this hull, no ISK, the cheapest within the balance.
        let id = |o: Option<LpOffer>| o.map(|o| o.offer_id);
        assert_eq!(id(pick(&offers, &lic, HULL, 1_000)), Some(4));
        assert_eq!(id(pick(&offers, &lic, HULL, 800)), Some(4));
        assert_eq!(id(pick(&offers, &lic, HULL, 699)), None, "too few");
        // Another hull wants its own licence.
        assert_eq!(id(pick(&offers, &lic, HULL_OTHER, 1_000)), Some(2));
        assert_eq!(id(pick(&offers, &lic, 5, 1_000)), None);
        // A tie goes to the lower offer id.
        let tie = vec![offer(9, LIC_CORP, 50, 0), offer(8, LIC_CORP, 50, 0)];
        assert_eq!(id(pick(&tie, &lic, HULL, 50)), Some(8));
    }

    /// The server's own gateway arm reads the body: a Display on a ship it
    /// does not hold is refused with 403 only after the body parsed (a body
    /// it cannot read is 400).
    #[test]
    fn display_body_is_the_wire_the_server_reads() {
        let reg = eve_server::service::Registry::new(eve_server::model::SandboxConfig::default());
        let g = Ground::Server(Arc::new(RwLock::new(reg)));
        let ask = |body: &[u8]| {
            eve_server::emblem::gateway(DISPLAY, body, 981_001, &g)
                .expect("the emblem arm answers")
                .status
        };
        assert_eq!(ask(&display_body(981_600_001, 0)), 403);
        assert_eq!(ask(&display_body(981_600_001, 7)), 403);
        // id = 1 { ship = 1 { sequential = 1 } }, attr = 2 { 2 = "" }.
        assert_eq!(
            display_body(5, 0),
            vec![0x0a, 0x04, 0x0a, 0x02, 0x08, 0x05, 0x12, 0x02, 0x12, 0x00]
        );
        // The slot is `index = 2` of the id.
        assert_eq!(
            display_body(5, 3),
            vec![
                0x0a, 0x06, 0x0a, 0x02, 0x08, 0x05, 0x10, 0x03, 0x12, 0x02, 0x12, 0x00
            ]
        );
        // A body without an attribute is unreadable to the arm.
        let mut no_attr = display_body(5, 0);
        no_attr.truncate(6);
        assert_eq!(ask(&no_attr), 400);
    }

    /// Once the logo is shown, or no licence was found at this balance, the
    /// step does nothing for that ship until the balance grows.
    #[test]
    fn the_step_runs_once_per_ship_until_the_balance_grows() {
        let (pilot, ship) = (981_701, 981_702);
        assert!(!settled(pilot, ship, 500));
        lock(&BROKE).insert((pilot, ship), 500);
        assert!(settled(pilot, ship, 500) && settled(pilot, ship, 100));
        assert!(!settled(pilot, ship, 501), "more Evermarks, try again");
        lock(&SHOWN).insert((pilot, ship));
        assert!(settled(pilot, ship, 9_999), "shown: never again");
        assert!(!settled(pilot, ship + 1, 9_999), "another ship");
    }

    #[test]
    fn the_server_displays_the_logo_it_is_sent() {
        use eve_server::emblem::{enabled, grant};
        const WHO: i64 = 981_001;
        const STATION: i64 = 981_002;
        const FRIGATE: i64 = 990_000_001;
        let mut content = eve_server::content::Content::fixture();
        content.insert("emblem", Licences::parse(&lines(FRIGATE)).unwrap());
        let mut reg =
            eve_server::service::Registry::new(eve_server::model::SandboxConfig::default())
                .with_content(Arc::new(content));
        let at = eve_server::item::Slot::new(WHO, STATION, eve_server::model::FLAG_HANGAR);
        reg.apply_items(&[eve_server::item::Edit::Create {
            type_id: FRIGATE,
            quantity: 1,
            to: at,
        }])
        .expect("the fixture hull is a known type");
        let ship = reg
            .store()
            .items()
            .iter()
            .find(|i| i.owner_id == WHO && i.type_id == FRIGATE)
            .expect("the hull")
            .item_id;
        assert!(grant(&mut reg, WHO, FRIGATE, CORPORATION_LOGO));
        let reg = Arc::new(RwLock::new(reg));
        let g = Ground::Server(reg.clone());
        let r = eve_server::emblem::gateway(DISPLAY, &display_body(ship, SLOT), WHO, &g)
            .expect("answered");
        assert_eq!(r.status, 200);
        let w = reg.read().unwrap();
        assert_eq!(
            enabled(&w, WHO, ship),
            eve_server::pyutil::dict_kv([(PyValue::Int(SLOT), PyValue::Int(CORPORATION_LOGO))])
        );
    }

    #[test]
    fn the_owned_licences_read_back_from_a_get_all_answer() {
        let ident = |ship: u64| {
            let mut who = Vec::new();
            put_uint(&mut who, 1, 981_001);
            let mut st = Vec::new();
            put_uint(&mut st, 1, ship);
            let mut id = Vec::new();
            put_bytes(&mut id, 1, &who);
            put_bytes(&mut id, 2, &st);
            id
        };
        let mut body = Vec::new();
        for (field, ship) in [(1u64, 300u64), (2, 301)] {
            let mut e = Vec::new();
            put_bytes(&mut e, field, &ident(ship));
            put_bytes(&mut body, 1, &e);
        }
        assert_eq!(owned_licences(&body), BTreeSet::from([(300, 1), (301, 2)]));
        assert!(owned_licences(&[]).is_empty());
        assert!(owned_licences(&[0x0a, 0xff]).is_empty(), "truncated");
    }
}
