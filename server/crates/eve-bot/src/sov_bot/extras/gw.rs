//! **The gateway seam** of the sov extras: the gateway's address and session,
//! a small protobuf codec (the server's helpers are `pub(crate)`), and the two
//! ballpark verbs the sov kit needs (a component call, a launch).
//!
//! **[A] B0-2**: [`GW_BACKOFF`] is 600 s (as `emblem_use`). Falsified by soak
//! notes of `MISSING gateway` lasting past a gateway restart.
//!
//! **[A] B0-4**: the gateway is at the game server's IP on
//! [`eve_server::gateway::PORT`] (inherited from `emblem_use`). Falsified by a
//! deployment with the gateway elsewhere; the note `MISSING gateway` says so.

use super::Mind;
use crate::clock;
use crate::gateway_client::GatewayClient;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

pub use eve_server::merc_den::MERCENARY_DEN;
pub use eve_server::skyhook::ORBITAL_SKYHOOK;
pub use eve_server::skyhook_theft::LINK_WITH_SHIP;

/// How long a missing gateway is left alone (**[A] B0-2**).
pub const GW_BACKOFF: Duration = Duration::from_secs(600);

/// How long the gateway may take to answer one request.
const TIMEOUT: Duration = Duration::from_secs(5);

pub const SKYHOOK_PKG: &str = "eve_public.sovereignty.skyhook.api.";
pub const FUEL_PKG: &str = "eve_public.sovereignty.hub.fuel.api.";
pub const HUB_PKG: &str = "eve_public.sovereignty.hub.api.";
pub const PLANET_PKG: &str = "eve_public.sovereignty.resource.planet.api.";
pub const UPGRADE_PKG: &str = "eve_public.sovereignty.hub.upgrade.api.";
pub const DEN_PKG: &str = "eve_public.sovereignty.mercenaryden.api.";
pub const ACTIVITY_PKG: &str = "eve_public.sovereignty.mercenaryden.activity.api.";

/// Game address -> gateway address. Tests only; production has none.
static OVERRIDES: Mutex<BTreeMap<SocketAddr, SocketAddr>> = Mutex::new(BTreeMap::new());

/// Tests only: the gateway of the game server at `game` is at `gateway`.
/// Keyed by the game address, so parallel tests with their own servers never
/// share one.
pub fn set_gateway_for(game: SocketAddr, gateway: SocketAddr) {
    OVERRIDES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(game, gateway);
}

/// Tests only: forget the override for `game`.
pub fn clear_gateway_for(game: SocketAddr) {
    OVERRIDES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&game);
}

/// The gateway of the game server at `game`: the override, else its IP on the
/// gateway port.
pub fn gateway_for(game: SocketAddr) -> SocketAddr {
    OVERRIDES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&game)
        .copied()
        .unwrap_or_else(|| SocketAddr::new(game.ip(), eve_server::gateway::PORT))
}

/// The gateway of the server this connection is to; `None` without a socket.
pub fn gateway_of(c: &Conn) -> Option<SocketAddr> {
    let peer = c.socket().ok()?.peer_addr().ok()?;
    Some(gateway_for(peer))
}

/// One open gateway session for one pilot.
pub struct Gw {
    client: GatewayClient,
    me: i64,
}

impl Gw {
    /// Connect; `None` (and `mind.gw_down` stamped) when the connect fails,
    /// and `None` without trying while the last failure is younger than
    /// [`GW_BACKOFF`].
    pub fn open(c: &Conn, mind: &mut Mind) -> Option<Gw> {
        if mind.gw_down.is_some_and(|t| clock::since(t) < GW_BACKOFF) {
            return None;
        }
        let me = c.session().char_id()?;
        let client = gateway_of(c).and_then(|a| GatewayClient::connect(a, TIMEOUT).ok());
        match client {
            Some(client) => Some(Gw { client, me }),
            None => {
                mind.gw_down = Some(clock::instant());
                None
            }
        }
    }

    /// One request for this pilot: the status and the payload bytes (empty
    /// when the answer carries none). A transport error stamps `mind.gw_down`
    /// and is the note `MISSING gateway`, not a dead game connection.
    pub fn ask(&mut self, mind: &mut Mind, ty: &str, body: &[u8]) -> Result<(u32, Vec<u8>)> {
        match self.client.send(self.me, ty, body) {
            Ok((status, payload)) => Ok((status, payload.map(|(_, b)| b).unwrap_or_default())),
            Err(_) => {
                mind.gw_down = Some(clock::instant());
                Err(Error::NotSent("MISSING gateway".to_owned()))
            }
        }
    }
}

// ---- the codec (protobuf wire format, std only) ----------------------------------------

/// Append a base-128 varint.
pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Append a varint field; a zero is elided, as the server's encoders do.
pub fn put_uint(out: &mut Vec<u8>, field: u32, v: u64) {
    if v == 0 {
        return;
    }
    put_varint(out, u64::from(field) << 3);
    put_varint(out, v);
}

/// Append a length-delimited field.
pub fn put_bytes(out: &mut Vec<u8>, field: u32, v: &[u8]) {
    put_varint(out, u64::from(field) << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

/// The message `{1 seq}`: an identifier.
pub fn id(seq: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_uint(&mut out, 1, seq);
    out
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// One wire field: `(number, wire type, varint value, bytes)`.
type Field<'a> = (u64, u8, u64, &'a [u8]);

/// Every field of `b` as `(number, wire type, varint value, bytes)`; `None`
/// when `b` is malformed.
fn walk(b: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let key = varint(b, &mut at)?;
        let (num, wire) = (key >> 3, (key & 7) as u8);
        match wire {
            0 => out.push((num, wire, varint(b, &mut at)?, &b[..0])),
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                let end = at.checked_add(n).filter(|e| *e <= b.len())?;
                out.push((num, wire, 0, &b[at..end]));
                at = end;
            }
            2 => {
                let n = usize::try_from(varint(b, &mut at)?).ok()?;
                let end = at.checked_add(n).filter(|e| *e <= b.len())?;
                out.push((num, wire, 0, &b[at..end]));
                at = end;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Every length-delimited occurrence of field `n`, in order; empty when `b` is
/// malformed.
pub fn fields(b: &[u8], n: u32) -> Vec<&[u8]> {
    walk(b)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.0 == u64::from(n) && f.1 == 2)
        .map(|f| f.3)
        .collect()
}

/// The first varint occurrence of field `n` (the server's `get_uint`); `None` when absent or malformed.
pub fn uint(b: &[u8], n: u32) -> Option<u64> {
    walk(b)?
        .into_iter()
        .find(|f| f.0 == u64::from(n) && f.1 == 0)
        .map(|f| f.2)
}

/// The first length-delimited occurrence of field `n`.
pub fn bytes(b: &[u8], n: u32) -> Option<&[u8]> {
    fields(b, n).into_iter().next()
}

/// An identifier at field `n`: `{1 seq}`. An elided zero reads `None` (the
/// server's `sov_proto` semantics).
pub fn read_id(b: &[u8], n: u32) -> Option<u64> {
    uint(bytes(b, n)?, 1)
}

// ---- the ballpark verbs ----------------------------------------------------------------

/// **`beyonce.CallComponentFromClient(item, component, method, args...)`**
/// (the client's own component call, `space_component.rs`): binds the park if
/// it is not bound. A `CustomNotify` or a raise is in the outcome, as
/// `outcome.refusal()`, never an `Err`.
pub fn component_call(
    c: &mut Conn,
    item: i64,
    component: &str,
    method: &str,
    args: Vec<PyValue>,
) -> Result<crate::CallOutcome> {
    if c.ballpark().is_none() {
        crate::capital::bind(c)?;
    }
    let mut all = vec![
        PyValue::Int(item),
        PyValue::Bytes(component.as_bytes().to_vec()),
        PyValue::Bytes(method.as_bytes().to_vec()),
    ];
    all.extend(args);
    c.bound_call("CallComponentFromClient", all)
}

/// What a `ship.Drop` answer says of `item`: `Ok(new id)` from `{item:
/// [newID]}`, `Err(text)` from `{item: [(label, args)]}`, `Err` of a note
/// when it says nothing.
pub fn launch_result(answer: &PyValue, item: i64) -> std::result::Result<i64, String> {
    if let Some(id) = crate::ore::launched(answer, item).first() {
        return Ok(*id);
    }
    let label = answer
        .dict_as_kv()
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| **k == PyValue::Int(item))
        .find_map(|(_, v)| match v {
            PyValue::List(v) | PyValue::Tuple(v) => v.iter().find_map(|e| match e {
                PyValue::Tuple(t) | PyValue::List(t) => match t.first() {
                    Some(PyValue::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
                    Some(PyValue::Unicode(s)) => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            }),
            _ => None,
        });
    Err(label.unwrap_or_else(|| "no answer for the item".to_owned()))
}

/// **Launch** `item` from the ship: `ship.Drop([(item, 1)], me, False)`, the
/// client's Launch for Self (the argument shape of the driver's camp unit
/// launch). `Ok(Ok(new id))`, or `Ok(Err(text))` when the server refused it.
pub fn launch(c: &mut Conn, item: i64) -> Result<std::result::Result<i64, String>> {
    let Some(params) = c.session().location_moniker() else {
        return Err(Error::NotSent("launch: nowhere to launch".to_owned()));
    };
    let me = c.session().char_id().map_or(PyValue::None, PyValue::Int);
    let answer = c.moniker_call(
        "ship",
        params,
        "Drop",
        vec![
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(item),
                PyValue::Int(1),
            ])]),
            me,
            PyValue::Bool(false),
        ],
        vec![],
    )?;
    if let Some(key) = answer.refusal() {
        return Ok(Err(key));
    }
    Ok(match answer.value.as_ref() {
        Some(v) => launch_result(v, item),
        None => Err("no answer".to_owned()),
    })
}
