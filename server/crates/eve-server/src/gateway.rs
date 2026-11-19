//! The public gateway, as far as the client needs it to stop waiting.
//!
//! **Why this exists.** The client sends gRPC requests to a "public gateway"
//! beside machoNet (`publicGateway/`, `eve_public_grpc_client.pyd`). Nothing
//! answered, so each request ran into its *local* deadline (3 s for a ship
//! SKIN, 10 s for most), and two things the owner uses all the time sit behind
//! such a wait: the fitting window (`cosmeticsSvc.GetAppliedSkinState`, 3 s a
//! time, three times) and the right-click menu in space (the mercenary-den
//! activities request). Evidence: `docs/lanes/ui-lag.md`.
//!
//! **What it is.** A std-only HTTP/2 (h2c, no TLS: the client takes
//! `EVE_PUBLIC_GATEWAY_ADDRESS` as insecure, `tools/eve_patch.py` launch env)
//! server for the three gRPC services in the client's own `gateway.proto`
//! **[V]** (read from the `.pyd`): `eve_public.gateway.Requests/Send`
//! (`Request` -> `Response`, bidirectional), `Notices/Consume` (we hold the
//! stream open and send nothing), `Events/Publish` (an empty message back per
//! event), and the three `Ping`s. The `Request`/`Response` field numbers are
//! the client's `public_pb2` **[V]**.
//!
//! **What it answers.** Instantly, from the pilot the request names
//! (`authoritative_context.identity.character`): see [`answer`]. Most classes
//! get the empty answer a pilot with none of the thing would get; personal
//! skill plans are real and persisted in the world file
//! ([`crate::gateway_plans`]). It is still not a gateway implementation;
//! each request class that matters gets its own arm.
//!
//! **Notices.** `Notices/Consume` is answered with response headers and then
//! held open and never ended. Since GW-NOTICE it is written to on demand:
//! see [`NoticeHub`] and [`publish`]. The client's consumer
//! (`publicGateway/grpc/noticeConsumer.py`) polls its *local* queue with a
//! 1 ms sleep, about 15 times a second in the logs, whatever the server does;
//! with the stream held it logs `consumer_state Active`, `channel_state
//! Ready`, no status breadcrumbs, and `Consumed no notices` every 5 s **[V]**
//! (client log 21-300). Ending the stream would make it reconnect in a loop.
//!
//! **Writes** are bounded: a write to a client takes at most [`WRITE_TIMEOUT`]
//! in all (a peer that reads a trickle does not extend it), and a failed one
//! marks the connection dead and removes it from the hub. Nothing but the
//! connection's own writer thread touches the socket for writing (#233):
//! [`publish`] and the serve loop append whole frames to the connection's
//! bounded [`Outbox`], in the order the connection lock gives them, and the
//! writer sends them in that order. A stuck peer therefore costs a caller
//! only the copy into the queue; a peer whose queue passes [`MAX_QUEUED`] is
//! marked dead like one whose write times out. Concurrent publishes may reach
//! different connections in different orders; notices are refresh hints, so
//! order across connections does not matter **[I]**.
//!
//! HPACK (RFC 7541) is decoded in full, dynamic table included, because the
//! only header we need, `:path`, is usually an index into it.

use crate::delta::Deltas;
use crate::service::Registry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

/// The port the client dials (`EVE_PUBLIC_GATEWAY_ADDRESS=<host>:8443`).
pub const PORT: u16 = 8443;
/// Connections at once. The client opens one.
const CAP: usize = 8;
const IDLE: Duration = Duration::from_secs(3600);
const MAX_FRAME: usize = 1 << 20;
/// How long one write to a client socket may block before the connection is
/// given up as dead (#229). A peer that opens big h2 windows and then stops
/// reading TCP would otherwise block the write, and the connection lock with
/// it, forever. **[A]** 10 s is generous for a LAN or home link. Falsifier: a
/// real client on a slow link that loses its connection or its notices at this
/// value (the gateway log line "write to ... failed" shows it).
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes that may wait in one connection's [`Outbox`] before the
/// writer has put them on the wire (#233). A frame is queued only inside the
/// h2 windows and the held-reply caps, so a healthy peer's queue holds at
/// most one burst of replies (under `MAX_HELD_CONN`); twice that is slack.
/// **[A]** 16 MiB. Falsifier: a real client dropped with the log line "write
/// queue ... is full" while reading normally (raise it), or a stuck peer that
/// pins enough memory across `CAP` connections to hurt (lower it).
const MAX_QUEUED: usize = 16 << 20;
const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

// ---- protobuf, just enough ------------------------------------------------

fn varint(b: &[u8], p: &mut usize) -> Option<u64> {
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

/// `(field, bytes)` for every length-delimited field, and `(field, value)`
/// as 8 little-endian bytes for varints; other wire types end the walk.
fn fields(b: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let Some(k) = varint(b, &mut p) else { break };
        match k & 7 {
            0 => {
                let Some(v) = varint(b, &mut p) else { break };
                out.push((k >> 3, v.to_le_bytes().to_vec()));
            }
            2 => {
                let Some(l) = varint(b, &mut p) else { break };
                let Some(end) = p.checked_add(l as usize).filter(|e| *e <= b.len()) else {
                    break;
                };
                out.push((k >> 3, b[p..end].to_vec()));
                p = end;
            }
            _ => break,
        }
    }
    out
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub(crate) fn put_bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

/// A decoded field: a varint, or a length-delimited value.
enum Val {
    Int(u64),
    Bytes(Vec<u8>),
}

fn walk(b: &[u8]) -> Vec<(u64, Val)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let Some(k) = varint(b, &mut p) else { break };
        match k & 7 {
            0 => {
                let Some(v) = varint(b, &mut p) else { break };
                out.push((k >> 3, Val::Int(v)));
            }
            2 => {
                let Some(l) = varint(b, &mut p) else { break };
                let Some(end) = p.checked_add(l as usize).filter(|e| *e <= b.len()) else {
                    break;
                };
                out.push((k >> 3, Val::Bytes(b[p..end].to_vec())));
                p = end;
            }
            _ => break,
        }
    }
    out
}

/// The first length-delimited value of field `n`.
pub(crate) fn get_bytes(b: &[u8], n: u64) -> Option<Vec<u8>> {
    walk(b).into_iter().find_map(|(k, v)| match v {
        Val::Bytes(x) if k == n => Some(x),
        _ => None,
    })
}

/// Every length-delimited value of field `n` (a repeated message).
pub(crate) fn field(b: &[u8], n: u64) -> Vec<Vec<u8>> {
    walk(b)
        .into_iter()
        .filter_map(|(k, v)| match v {
            Val::Bytes(x) if k == n => Some(x),
            _ => None,
        })
        .collect()
}

/// The first varint of field `n`.
pub(crate) fn get_uint(b: &[u8], n: u64) -> Option<u64> {
    walk(b).into_iter().find_map(|(k, v)| match v {
        Val::Int(x) if k == n => Some(x),
        _ => None,
    })
}

/// The pilot a request is for: `Request.authoritative_context.identity
/// .character.sequential` (fields 6, 10, 1, 1), which the client fills in
/// from the session (`publicGateway/grpc/requestBroker.py`) **[V]**. 0 when
/// absent.
pub(crate) fn request_character(request: &[u8]) -> i64 {
    get_bytes(request, 6)
        .and_then(|c| get_bytes(&c, 10))
        .and_then(|i| get_bytes(&i, 1))
        .and_then(|c| get_uint(&c, 1))
        .map_or(0, |n| n as i64)
}

/// The client process that sent a request: `Request.application_instance_uuid`
/// (field 5), a uuid4 the client makes once per process **[V]**
/// (`publicGateway/grpc/requestBroker.pyc`). Empty when absent.
pub(crate) fn request_instance(request: &[u8]) -> Vec<u8> {
    get_bytes(request, 5).unwrap_or_default()
}

/// The account a request claims: `authoritative_context.authenticated_user
/// .sequential` (fields 6, 5, 1), a zigzag `sint32` **[V]**
/// (`eveProto/generated/eve_public/public_pb2.py`, `user/user_pb2.py`).
/// `None` when absent.
pub(crate) fn request_user(request: &[u8]) -> Option<i64> {
    let v = get_bytes(request, 6)
        .and_then(|c| get_bytes(&c, 5))
        .and_then(|i| get_uint(&i, 1))?;
    Some(((v >> 1) as i64) ^ -((v & 1) as i64))
}

/// Where the gateway's stored state lives: the server's world (its delta
/// rows, under the world lock), or a bare set of rows for a test.
#[derive(Clone)]
pub enum Ground {
    Server(Arc<RwLock<Registry>>),
    Local(Arc<Mutex<Deltas>>),
}

impl Ground {
    /// A ground that owns its rows (tests, tools).
    pub fn local() -> Ground {
        Ground::Local(Arc::new(Mutex::new(Deltas::default())))
    }

    pub(crate) fn read<R>(&self, f: impl FnOnce(&Deltas) -> R) -> R {
        match self {
            Ground::Server(w) => f(crate::service::world_read_timed(w, "gateway_read").deltas()),
            Ground::Local(d) => f(&d.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }

    pub(crate) fn write<R>(&self, f: impl FnOnce(&mut Deltas) -> R) -> R {
        match self {
            Ground::Server(w) => {
                f(crate::service::world_write_timed(w, "gateway_write").deltas_mut())
            }
            Ground::Local(d) => f(&mut d.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }
}

/// What the gateway says to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u32,
    /// `(full type name, encoded message)` for the `Any` payload; `None`
    /// leaves it unset.
    pub payload: Option<(String, Vec<u8>)>,
}

/// The type name inside an `Any`'s `type_url`, as the proto package spells it.
fn type_name(url: &[u8]) -> String {
    let s = String::from_utf8_lossy(url);
    let name = s.rsplit('/').next().unwrap_or("");
    name.replace(".requests_pb2.", ".")
        .replace(".api_pb2.", ".")
}

fn put_uint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

/// Seconds since the epoch.
fn now_secs() -> u64 {
    crate::game_clock::clock::unix_secs()
}

/// `google.protobuf.Timestamp { seconds }`.
fn timestamp(secs: u64) -> Vec<u8> {
    let mut ts = Vec::new();
    put_uint(&mut ts, 1, secs);
    ts
}

/// The omega licence's expiry, from now. Nine years, not 2100: the client
/// turns it into a `datetime` with `fromtimestamp`, and a 32-bit `time_t`
/// ends in 2038 **[A]** (falsifier: `ValueError` from `user_license.py`
/// with a later date; the machoNet side, `GetSubscriptionTime`, uses 2100
/// because only blue formats that one).
const LICENSE_YEARS: u64 = 9;
/// `plex.vault` balance in cents of PLEX **[A]**: this server has no PLEX
/// store, so the vault is empty (falsifier: a window that wants to spend it).
pub const PLEX_BALANCE_CENTS: u64 = 0;

/// Requests the client reads as "none", with a `404` and no payload: a
/// missing single record where an empty message would read as a real one
/// with default fields (a black corporation palette, a lock on gate 0, a
/// licence that expires in 1970). Each is `[V]` from the messenger that sends
/// it.
fn is_not_found(ty: &str) -> bool {
    matches!(
        ty,
        // the ship SKIN: `shipSkinStateRequestMessenger` returns `(response, None)`
        "eve_public.cosmetic.ship.api.GetRequest"
            // `corpColorsRequestMessenger`: NotFoundException -> no palette
            | "eve_public.cosmetic.corporation.palette.api.GetRequest"
            | "eve_public.cosmetic.corporation.palette.api.GetOwnRequest"
            // `structure ... requestMessenger.get_request`: `(None, None)`
            | "eve_public.cosmetic.structure.paintwork.api.GetRequest"
            // `gateLockMessenger.get_request`: NOT_FOUND -> no lock
            | "eve_public.stargate.lock.api.GetRequest"
            // `request_messenger` (mercenary den): NOT_FOUND -> UnknownMercenaryDen
            | "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest"
            | "eve_public.sovereignty.mercenaryden.activity.api.GetForMercenaryDenRequest"
            | "eve_public.sovereignty.mercenaryden.activity.api.StartRequest"
    ) || is_content_checkpoint(ty)
}

/// The flatbuffer content classes (`content/client/flatbuffer/provider.py`):
/// the reply is a CDN checkpoint (an https url) and there is no CDN, so there
/// is nothing right to say. The provider then raises whatever we answer
/// (`IndexError` for an empty checkpoint, `GenericNotFoundError` for a 404);
/// both end that repository's update task, which keeps its default (empty)
/// data. A `404` is the cleaner of the two and names the cause in the log.
fn is_content_checkpoint(ty: &str) -> bool {
    ty.ends_with(".cdn.GetLatestRequest")
        || ty.ends_with(".cdn.GetUpdateRequest")
        || ty.ends_with(".definition.api.GetLatestRequest")
}

/// How to answer a request class. **Everything here is a choice made to
/// keep the UI moving, and every arm not marked `[V]` is [A]** until a
/// client run shows what it does with it; the falsifier is the same for all:
/// a new traceback from the messenger that sent it (`evecap --alerts`).
///
/// - Skill plans and their milestones are real and per pilot:
///   [`crate::gateway_plans`].
/// - [`is_not_found`] classes: **404**, no payload.
/// - `user.license.api.GetRequest`, the omega licence: this server's pilots
///   are omega (`subscriptionMgr.GetCloneGrade`), so the licence exists and
///   runs [`LICENSE_YEARS`]. Without one the client's neocom upsell button
///   is built from `has_license`; with an empty `Attributes` it raised
///   `can't compare datetime to NoneType` (`user_license.is_expired`).
/// - `cosmetic.character.title.api.GetEquippedForCharacterRequest`: the
///   `no_title` branch of the oneof.
/// - `plex.vault.api.BalanceRequest`: [`PLEX_BALANCE_CENTS`].
/// - `faction.activity.GetSolarSystemScoresRequest`: the asked system, no
///   scores (the client builds `SolarSystemScores` from the echoed id).
/// - `pirate.{corruption,suppression}.api.GetSystemInfoRequest`: progress 0/1.
/// - `sovereignty.hub.workforce.api.{GetConfigurationRequest,GetStateRequest}`:
///   the `inactive` mode set. Left empty, `create_from_proto` returns None and
///   `SovereigntyHub.UpdateWorkforceTransportState` raises on `.get_mode()`.
/// - `chat.local.api.*`: untouched. The client sends none of them while the
///   local-chat feature flag is off, and it is off (XMPP carries local);
///   if it came on, the empty member list is the honest answer.
/// - every other `*Request`: 200 with an *empty typed* `*Response` (no dens,
///   no thresholds, no rewards, no daily goals: what a pilot with none would
///   get). Payload unset reads as `None` in the client
///   (`ResponseHandler.handle` unpacks only when the `type_url` is set,
///   `docs/guides/debugging-the-client.md` §3c) and ~15 messengers raised on
///   it (sysinfo lane, client log 20:23).
pub fn answer(ty: &str, body: &[u8], character: i64, g: &Ground) -> Reply {
    let reply = |status, payload| Reply { status, payload };
    if let Some(r) = crate::gateway_ext::handle(ty, body, character, g) {
        return r;
    }
    if is_not_found(ty) {
        return reply(404, None);
    }
    let base = ty.strip_suffix("Request");
    let typed = |value: Vec<u8>| reply(200, base.map(|b| (format!("{b}Response"), value)));
    match ty {
        "eve_public.user.license.api.GetRequest" => {
            // `Identifier.license_type` 1; only the omega licence exists.
            let kind = get_bytes(body, 1)
                .and_then(|l| get_bytes(&l, 1))
                .unwrap_or_default();
            if kind != b"eve_clonestate_omega" {
                return reply(404, None);
            }
            let now = now_secs();
            let mut attrs = Vec::new();
            put_bytes(
                &mut attrs,
                2,
                &timestamp(now + LICENSE_YEARS * 365 * 86_400),
            );
            put_bytes(&mut attrs, 3, &timestamp(now));
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &attrs);
            typed(out)
        }
        "eve_public.cosmetic.character.title.api.GetEquippedForCharacterRequest" => {
            let mut out = Vec::new();
            put_uint(&mut out, 2, 1); // no_title
            typed(out)
        }
        "eve_public.plex.vault.api.BalanceRequest" => {
            let mut cur = Vec::new();
            put_uint(&mut cur, 1, PLEX_BALANCE_CENTS);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &cur);
            typed(out)
        }
        "eve_public.faction.activity.GetSolarSystemScoresRequest" => {
            let mut out = Vec::new();
            if let Some(system) = get_bytes(body, 1) {
                put_bytes(&mut out, 1, &system);
            }
            typed(out)
        }
        // A system with no corruption or suppression: progress 0/1 (the
        // insurgency panel divides numerator by denominator, so 0/0 would
        // raise), stage 0. Corruption and suppression `GetSystemInfoResponse`
        // share one layout (`total_progress` = field 1, a `Fraction` of
        // numerator 1, denominator 2; `[V]` from both `requests_pb2`
        // descriptors).
        "eve_public.pirate.corruption.api.GetSystemInfoRequest"
        | "eve_public.pirate.suppression.api.GetSystemInfoRequest" => {
            let mut fraction = Vec::new();
            put_uint(&mut fraction, 2, 1);
            let mut value = Vec::new();
            put_bytes(&mut value, 1, &fraction);
            typed(value)
        }
        // A hub with no workforce set up is idle [A: a fresh hub reads
        // inactive; falsifier: a hub panel showing another mode with nothing
        // configured]. The `mode` oneof must be set: unset, `create_from_proto`
        // returns None for the configuration and the state alike, and the
        // client catches only `SovereigntyWorkforceError`. `inactive` = 1 in
        // both, the message is response field 1 (`[V]` `workforce_pb2`).
        "eve_public.sovereignty.hub.workforce.api.GetConfigurationRequest"
        | "eve_public.sovereignty.hub.workforce.api.GetStateRequest" => {
            let mut mode = Vec::new();
            put_uint(&mut mode, 1, 1);
            let mut out = Vec::new();
            put_bytes(&mut out, 1, &mode);
            typed(out)
        }
        // Writes nothing here can honour: refused, so the client shows its
        // own "could not" instead of believing a save that never happened.
        "eve_public.cosmetic.corporation.palette.api.SetRequest" => reply(403, None),
        // Everything else: an empty typed response. The client reads a
        // missing payload as `None` and the messengers then raise
        // `AttributeError` on it (the location panel,
        // `infoPanelLocationInfo.UpdateMainLocationInfo` -> `GetCorruption`
        // -> `.total_progress`, only catches `TimeoutException`, so the whole
        // panel failed to build). An empty message reads as defaults.
        _ => typed(Vec::new()),
    }
}

/// Turn one `eve_public.Request` into its `eve_public.Response` bytes, the
/// request's type name and the status it got.
pub fn respond(request: &[u8], instance: &[u8; 16], g: &Ground) -> (Vec<u8>, String, u32) {
    let served = crate::gateway_auth::check(g, request, None);
    respond_as(request, instance, g, served.as_ref())
}

/// [`respond`] with the pilot check already made (`None`: refused, 403).
fn respond_as(
    request: &[u8],
    instance: &[u8; 16],
    g: &Ground,
    served: Option<&crate::gateway_auth::Served>,
) -> (Vec<u8>, String, u32) {
    let f = fields(request);
    let get = |n: u64| f.iter().find(|(k, _)| *k == n).map(|(_, v)| v.as_slice());
    let any = get(3).unwrap_or(&[]);
    let ty = get_bytes(any, 1).map(|v| type_name(&v)).unwrap_or_default();
    let value = get_bytes(any, 2).unwrap_or_default();
    let r = match served {
        Some(s) => answer(&ty, &value, s.character, g),
        None => Reply {
            status: 403,
            payload: None,
        },
    };
    let mut out = Vec::new();
    // dispatched: Timestamp { seconds }
    put_bytes(&mut out, 1, &timestamp(now_secs()));
    put_bytes(&mut out, 2, get(2).unwrap_or(&[]));
    put_varint(&mut out, 3 << 3);
    put_varint(&mut out, u64::from(r.status));
    if let Some((name, value)) = &r.payload {
        let mut any = Vec::new();
        put_bytes(
            &mut any,
            1,
            format!("type.googleapis.com/{name}").as_bytes(),
        );
        put_bytes(&mut any, 2, value);
        put_bytes(&mut out, 5, &any);
    }
    put_bytes(&mut out, 6, b"eve-offline gateway stub");
    put_bytes(&mut out, 7, get(5).unwrap_or(&[]));
    put_bytes(&mut out, 8, instance);
    (out, ty, r.status)
}

// ---- HPACK ------------------------------------------------------------------

/// RFC 7541 appendix B: (bits, code), symbol 256 is EOS.
const HUFFMAN: [(u8, u32); 257] = [
    (13, 0x1ff8),
    (23, 0x7fffd8),
    (28, 0xfffffe2),
    (28, 0xfffffe3),
    (28, 0xfffffe4),
    (28, 0xfffffe5),
    (28, 0xfffffe6),
    (28, 0xfffffe7),
    (28, 0xfffffe8),
    (24, 0xffffea),
    (30, 0x3ffffffc),
    (28, 0xfffffe9),
    (28, 0xfffffea),
    (30, 0x3ffffffd),
    (28, 0xfffffeb),
    (28, 0xfffffec),
    (28, 0xfffffed),
    (28, 0xfffffee),
    (28, 0xfffffef),
    (28, 0xffffff0),
    (28, 0xffffff1),
    (28, 0xffffff2),
    (30, 0x3ffffffe),
    (28, 0xffffff3),
    (28, 0xffffff4),
    (28, 0xffffff5),
    (28, 0xffffff6),
    (28, 0xffffff7),
    (28, 0xffffff8),
    (28, 0xffffff9),
    (28, 0xffffffa),
    (28, 0xffffffb),
    (6, 0x14),
    (10, 0x3f8),
    (10, 0x3f9),
    (12, 0xffa),
    (13, 0x1ff9),
    (6, 0x15),
    (8, 0xf8),
    (11, 0x7fa),
    (10, 0x3fa),
    (10, 0x3fb),
    (8, 0xf9),
    (11, 0x7fb),
    (8, 0xfa),
    (6, 0x16),
    (6, 0x17),
    (6, 0x18),
    (5, 0x0),
    (5, 0x1),
    (5, 0x2),
    (6, 0x19),
    (6, 0x1a),
    (6, 0x1b),
    (6, 0x1c),
    (6, 0x1d),
    (6, 0x1e),
    (6, 0x1f),
    (7, 0x5c),
    (8, 0xfb),
    (15, 0x7ffc),
    (6, 0x20),
    (12, 0xffb),
    (10, 0x3fc),
    (13, 0x1ffa),
    (6, 0x21),
    (7, 0x5d),
    (7, 0x5e),
    (7, 0x5f),
    (7, 0x60),
    (7, 0x61),
    (7, 0x62),
    (7, 0x63),
    (7, 0x64),
    (7, 0x65),
    (7, 0x66),
    (7, 0x67),
    (7, 0x68),
    (7, 0x69),
    (7, 0x6a),
    (7, 0x6b),
    (7, 0x6c),
    (7, 0x6d),
    (7, 0x6e),
    (7, 0x6f),
    (7, 0x70),
    (7, 0x71),
    (7, 0x72),
    (8, 0xfc),
    (7, 0x73),
    (8, 0xfd),
    (13, 0x1ffb),
    (19, 0x7fff0),
    (13, 0x1ffc),
    (14, 0x3ffc),
    (6, 0x22),
    (15, 0x7ffd),
    (5, 0x3),
    (6, 0x23),
    (5, 0x4),
    (6, 0x24),
    (5, 0x5),
    (6, 0x25),
    (6, 0x26),
    (6, 0x27),
    (5, 0x6),
    (7, 0x74),
    (7, 0x75),
    (6, 0x28),
    (6, 0x29),
    (6, 0x2a),
    (5, 0x7),
    (6, 0x2b),
    (7, 0x76),
    (6, 0x2c),
    (5, 0x8),
    (5, 0x9),
    (6, 0x2d),
    (7, 0x77),
    (7, 0x78),
    (7, 0x79),
    (7, 0x7a),
    (7, 0x7b),
    (15, 0x7ffe),
    (11, 0x7fc),
    (14, 0x3ffd),
    (13, 0x1ffd),
    (28, 0xffffffc),
    (20, 0xfffe6),
    (22, 0x3fffd2),
    (20, 0xfffe7),
    (20, 0xfffe8),
    (22, 0x3fffd3),
    (22, 0x3fffd4),
    (22, 0x3fffd5),
    (23, 0x7fffd9),
    (22, 0x3fffd6),
    (23, 0x7fffda),
    (23, 0x7fffdb),
    (23, 0x7fffdc),
    (23, 0x7fffdd),
    (23, 0x7fffde),
    (24, 0xffffeb),
    (23, 0x7fffdf),
    (24, 0xffffec),
    (24, 0xffffed),
    (22, 0x3fffd7),
    (23, 0x7fffe0),
    (24, 0xffffee),
    (23, 0x7fffe1),
    (23, 0x7fffe2),
    (23, 0x7fffe3),
    (23, 0x7fffe4),
    (21, 0x1fffdc),
    (22, 0x3fffd8),
    (23, 0x7fffe5),
    (22, 0x3fffd9),
    (23, 0x7fffe6),
    (23, 0x7fffe7),
    (24, 0xffffef),
    (22, 0x3fffda),
    (21, 0x1fffdd),
    (20, 0xfffe9),
    (22, 0x3fffdb),
    (22, 0x3fffdc),
    (23, 0x7fffe8),
    (23, 0x7fffe9),
    (21, 0x1fffde),
    (23, 0x7fffea),
    (22, 0x3fffdd),
    (22, 0x3fffde),
    (24, 0xfffff0),
    (21, 0x1fffdf),
    (22, 0x3fffdf),
    (23, 0x7fffeb),
    (23, 0x7fffec),
    (21, 0x1fffe0),
    (21, 0x1fffe1),
    (22, 0x3fffe0),
    (21, 0x1fffe2),
    (23, 0x7fffed),
    (22, 0x3fffe1),
    (23, 0x7fffee),
    (23, 0x7fffef),
    (20, 0xfffea),
    (22, 0x3fffe2),
    (22, 0x3fffe3),
    (22, 0x3fffe4),
    (23, 0x7ffff0),
    (22, 0x3fffe5),
    (22, 0x3fffe6),
    (23, 0x7ffff1),
    (26, 0x3ffffe0),
    (26, 0x3ffffe1),
    (20, 0xfffeb),
    (19, 0x7fff1),
    (22, 0x3fffe7),
    (23, 0x7ffff2),
    (22, 0x3fffe8),
    (25, 0x1ffffec),
    (26, 0x3ffffe2),
    (26, 0x3ffffe3),
    (26, 0x3ffffe4),
    (27, 0x7ffffde),
    (27, 0x7ffffdf),
    (26, 0x3ffffe5),
    (24, 0xfffff1),
    (25, 0x1ffffed),
    (19, 0x7fff2),
    (21, 0x1fffe3),
    (26, 0x3ffffe6),
    (27, 0x7ffffe0),
    (27, 0x7ffffe1),
    (26, 0x3ffffe7),
    (27, 0x7ffffe2),
    (24, 0xfffff2),
    (21, 0x1fffe4),
    (21, 0x1fffe5),
    (26, 0x3ffffe8),
    (26, 0x3ffffe9),
    (28, 0xffffffd),
    (27, 0x7ffffe3),
    (27, 0x7ffffe4),
    (27, 0x7ffffe5),
    (20, 0xfffec),
    (24, 0xfffff3),
    (20, 0xfffed),
    (21, 0x1fffe6),
    (22, 0x3fffe9),
    (21, 0x1fffe7),
    (21, 0x1fffe8),
    (23, 0x7ffff3),
    (22, 0x3fffea),
    (22, 0x3fffeb),
    (25, 0x1ffffee),
    (25, 0x1ffffef),
    (24, 0xfffff4),
    (24, 0xfffff5),
    (26, 0x3ffffea),
    (23, 0x7ffff4),
    (26, 0x3ffffeb),
    (27, 0x7ffffe6),
    (26, 0x3ffffec),
    (26, 0x3ffffed),
    (27, 0x7ffffe7),
    (27, 0x7ffffe8),
    (27, 0x7ffffe9),
    (27, 0x7ffffea),
    (27, 0x7ffffeb),
    (28, 0xffffffe),
    (27, 0x7ffffec),
    (27, 0x7ffffed),
    (27, 0x7ffffee),
    (27, 0x7ffffef),
    (27, 0x7fffff0),
    (26, 0x3ffffee),
    (30, 0x3fffffff),
];

fn huffman_decode(b: &[u8]) -> Option<Vec<u8>> {
    static MAP: OnceLock<HashMap<(u8, u32), u16>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        HUFFMAN
            .iter()
            .enumerate()
            .map(|(i, &(n, c))| ((n, c), i as u16))
            .collect()
    });
    let (mut code, mut len) = (0u32, 0u8);
    let mut out = Vec::new();
    for &byte in b {
        for i in (0..8).rev() {
            code = code << 1 | u32::from(byte >> i & 1);
            len += 1;
            if let Some(&s) = map.get(&(len, code)) {
                if s == 256 {
                    return None;
                }
                out.push(s as u8);
                code = 0;
                len = 0;
            } else if len > 30 {
                return None;
            }
        }
    }
    // Padding: at most 7 bits, all ones.
    (len <= 7 && code == (1u32 << len) - 1).then_some(out)
}

/// The static table's names (RFC 7541 appendix A); only `:path`'s values
/// are ever read.
const STATIC_NAMES: [&str; 61] = [
    ":authority",
    ":method",
    ":method",
    ":path",
    ":path",
    ":scheme",
    ":scheme",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    ":status",
    "accept-charset",
    "accept-encoding",
    "accept-language",
    "accept-ranges",
    "accept",
    "access-control-allow-origin",
    "age",
    "allow",
    "authorization",
    "cache-control",
    "content-disposition",
    "content-encoding",
    "content-language",
    "content-length",
    "content-location",
    "content-range",
    "content-type",
    "cookie",
    "date",
    "etag",
    "expect",
    "expires",
    "from",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "last-modified",
    "link",
    "location",
    "max-forwards",
    "proxy-authenticate",
    "proxy-authorization",
    "range",
    "referer",
    "refresh",
    "retry-after",
    "server",
    "set-cookie",
    "strict-transport-security",
    "transfer-encoding",
    "user-agent",
    "vary",
    "via",
    "www-authenticate",
];

#[derive(Default)]
struct Hpack {
    /// Newest first.
    dynamic: Vec<(Vec<u8>, Vec<u8>)>,
    size: usize,
    max: usize,
}

impl Hpack {
    fn new() -> Self {
        Hpack {
            dynamic: Vec::new(),
            size: 0,
            max: 4096,
        }
    }

    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) {
        self.size += name.len() + value.len() + 32;
        self.dynamic.insert(0, (name, value));
        self.evict();
    }

    fn evict(&mut self) {
        while self.size > self.max {
            match self.dynamic.pop() {
                Some((n, v)) => self.size -= n.len() + v.len() + 32,
                None => {
                    self.size = 0;
                    break;
                }
            }
        }
    }

    fn entry(&self, i: usize) -> Option<(Vec<u8>, Vec<u8>)> {
        match i {
            0 => None,
            1..=61 => {
                let v: &[u8] = match i {
                    4 => b"/",
                    5 => b"/index.html",
                    _ => b"",
                };
                Some((STATIC_NAMES[i - 1].as_bytes().to_vec(), v.to_vec()))
            }
            _ => self.dynamic.get(i - 62).cloned(),
        }
    }

    fn int(b: &[u8], p: &mut usize, bits: u32) -> Option<usize> {
        let mask = (1usize << bits) - 1;
        let mut v = usize::from(*b.get(*p)?) & mask;
        *p += 1;
        if v < mask {
            return Some(v);
        }
        let mut shift = 0u32;
        loop {
            let x = *b.get(*p)?;
            *p += 1;
            v = v.checked_add(usize::from(x & 0x7f).checked_shl(shift)?)?;
            if x < 0x80 {
                return Some(v);
            }
            shift += 7;
            if shift > 28 {
                return None;
            }
        }
    }

    fn string(b: &[u8], p: &mut usize) -> Option<Vec<u8>> {
        let huff = *b.get(*p)? & 0x80 != 0;
        let n = Self::int(b, p, 7)?;
        let end = p.checked_add(n).filter(|e| *e <= b.len())?;
        let raw = &b[*p..end];
        *p = end;
        if huff {
            huffman_decode(raw)
        } else {
            Some(raw.to_vec())
        }
    }

    /// Decode one header block; the `:path` value if it carried one.
    fn decode(&mut self, b: &[u8]) -> Option<Option<Vec<u8>>> {
        let mut p = 0;
        let mut path = None;
        while p < b.len() {
            let first = b[p];
            let (name, value) = if first & 0x80 != 0 {
                let i = Self::int(b, &mut p, 7)?;
                self.entry(i)?
            } else if first & 0xe0 == 0x20 {
                // Dynamic table size update.
                self.max = Self::int(b, &mut p, 5)?;
                self.evict();
                continue;
            } else {
                let (bits, index) = if first & 0x40 != 0 {
                    (6, true)
                } else {
                    (4, false)
                };
                let i = Self::int(b, &mut p, bits)?;
                let name = if i == 0 {
                    Self::string(b, &mut p)?
                } else {
                    self.entry(i)?.0
                };
                let value = Self::string(b, &mut p)?;
                if index {
                    self.insert(name.clone(), value.clone());
                }
                (name, value)
            };
            if name == b":path" {
                path = Some(value);
            }
        }
        Some(path)
    }
}

// ---- HTTP/2 ------------------------------------------------------------------

const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
/// RFC 9113 7 error codes used here.
const PROTOCOL_ERROR: u32 = 0x1;
const FLOW_CONTROL_ERROR: u32 = 0x3;
const ENHANCE_YOUR_CALM: u32 = 0xb;
/// A held queue gives back its sent prefix at once past this many bytes.
const RECLAIM: usize = 64 << 10;
/// `SETTINGS_MAX_CONCURRENT_STREAMS` we advertise, and refuse past (RFC 9113
/// 5.1.2). [A] Falsified if the real client opens more than 100 streams at once
/// on one gateway connection.
const MAX_STREAMS: usize = 100;
const REFUSED_STREAM: u32 = 0x7;
/// The largest flow-control window (RFC 9113 6.9.1).
const MAX_WINDOW: i64 = (1 << 31) - 1;
/// Reply bytes held back for one stream whose peer window is shut. [A]
/// Falsified if the real client legitimately leaves more than 1 MiB of
/// replies unread on one stream.
const MAX_HELD_STREAM: usize = 1 << 20;
/// Reply bytes held back on one connection, all streams together. [A]
/// Falsified if the real client legitimately leaves more than 8 MiB unread.
const MAX_HELD_CONN: usize = 8 << 20;
const CONTINUATION: u8 = 9;
const END_STREAM: u8 = 1;
const ACK: u8 = 1;
const END_HEADERS: u8 = 4;
const PADDED: u8 = 8;
const PRIORITY: u8 = 0x20;

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut f = vec![(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags];
    f.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// A literal header with no indexing and a new name, nothing Huffman coded:
/// our side of HPACK needs no state.
fn literal(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(0);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

fn response_headers() -> Vec<u8> {
    let mut h = vec![0x88]; // :status 200, indexed
    literal(&mut h, "content-type", "application/grpc");
    h
}

fn trailers(status: u8) -> Vec<u8> {
    let mut h = Vec::new();
    literal(&mut h, "grpc-status", &status.to_string());
    h
}

fn grpc_message(m: &[u8]) -> Vec<u8> {
    let mut f = vec![0];
    f.extend_from_slice(&(m.len() as u32).to_be_bytes());
    f.extend_from_slice(m);
    f
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Send,
    Consume,
    Publish,
    Ping,
}

struct Stream {
    kind: Kind,
    buf: Vec<u8>,
}

fn route(path: &[u8]) -> Option<Kind> {
    Some(match path {
        b"/eve_public.gateway.Requests/Send" => Kind::Send,
        b"/eve_public.gateway.Notices/Consume" => Kind::Consume,
        b"/eve_public.gateway.Events/Publish" => Kind::Publish,
        b"/eve_public.gateway.Requests/Ping"
        | b"/eve_public.gateway.Notices/Ping"
        | b"/eve_public.gateway.Events/Ping" => Kind::Ping,
        _ => return None,
    })
}

fn read_exact_or_eof(s: &mut TcpStream, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match s.read(&mut buf[got..]) {
            Ok(0) if got == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

// ---- notices ----------------------------------------------------------------

/// The write side of one connection. Everything that writes to the socket,
/// the serve loop and [`publish`] alike, goes through the mutex around this,
/// so frames never interleave.
struct Conn {
    sock: TcpStream,
    /// The pilot, learned from the latest served `Send` on the connection (0
    /// until), and the game session that vouched for it (0: none, a local
    /// ground). A notice reaches the pilot only while that session lives.
    character: i64,
    bound_client: i64,
    /// The client process (`application_instance_uuid`) of the served
    /// request that bound `character`: a pin that passes to another process
    /// unbinds this connection (`NoticeHub::unbind`).
    instance: Vec<u8>,
    /// Every client process served on this connection, counted open in
    /// `gateway_auth` until the connection ends.
    processes: Vec<Vec<u8>>,
    /// What the client has left us to send on the connection, h2 flow control.
    /// What the client has left us to send on the connection, h2 flow control.
    window: i64,
    /// `SETTINGS_INITIAL_WINDOW_SIZE` from the client (default 65535).
    initial: i64,
    /// `SETTINGS_MAX_FRAME_SIZE` from the client (default 16384, RFC 9113
    /// 6.5.2): no DATA frame we send is longer.
    max_frame: usize,
    /// Open `Consume` streams and the window left on each.
    consume: HashMap<u32, i64>,
    /// Open request/reply streams: the stream window, and what is queued for
    /// it until the windows open.
    replies: HashMap<u32, OutStream>,
    /// Reply bytes queued across all of `replies`, not yet on the wire.
    held: usize,
    /// The most that may be held for one stream, and for the connection.
    cap_stream: usize,
    cap_conn: usize,
    /// The highest client stream id we have processed (not refused): the
    /// last-stream-id of a GOAWAY (RFC 9113 6.8).
    last: u32,
    /// The connection cap was hit: a GOAWAY is out, end the connection.
    closing: bool,
    dead: bool,
    /// The peer's address, for the one log line when it is dropped.
    peer: String,
    /// The queue the connection's writer thread drains to the socket.
    out: Arc<Outbox>,
}

/// What waits to be written to one connection, and the state of the writer.
struct OutQ {
    frames: VecDeque<Vec<u8>>,
    /// Bytes queued or being written (a chunk counts until it is sent).
    bytes: usize,
    /// No more frames will come: send what is queued, then close.
    closed: bool,
    /// When `closed`: the one deadline for sending the rest of the queue.
    drain_by: Option<std::time::Instant>,
    /// A write failed or the queue was full: nothing more is sent.
    dead: bool,
    /// Why it died: the queue passed its cap, or a write failed or timed out.
    cause: Cause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    Alive,
    QueueFull,
    WriteFailed,
}

/// The bounded write queue of one connection (#233). Frames are appended
/// whole, so frames on the wire never interleave and keep the order in which
/// they were appended.
struct Outbox {
    q: Mutex<OutQ>,
    ready: Condvar,
    cap: usize,
}

impl Outbox {
    fn new(cap: usize) -> Arc<Outbox> {
        Arc::new(Outbox {
            q: Mutex::new(OutQ {
                frames: VecDeque::new(),
                bytes: 0,
                closed: false,
                drain_by: None,
                dead: false,
                cause: Cause::Alive,
            }),
            ready: Condvar::new(),
            cap,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutQ> {
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append `bytes`. Refused once the connection is dead or closed. A queue
    /// that would pass its cap marks the connection dead (one log line,
    /// naming `peer`); the first chunk into an empty queue is always taken.
    fn push(&self, bytes: Vec<u8>, peer: &str) -> io::Result<()> {
        let mut q = self.lock();
        if q.dead || q.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection is dead",
            ));
        }
        if q.bytes > 0 && q.bytes + bytes.len() > self.cap {
            crate::outln!(
                "[gateway] write queue to {peer} is full ({} bytes); dropping it",
                self.cap
            );
            q.dead = true;
            q.cause = Cause::QueueFull;
            q.frames.clear();
            q.bytes = 0;
            self.ready.notify_all();
            return Err(io::Error::other("write queue full"));
        }
        q.bytes += bytes.len();
        q.frames.push_back(bytes);
        self.ready.notify_one();
        Ok(())
    }

    /// No more frames: the writer sends what is queued, all within `drain`
    /// from now, and closes.
    fn close(&self, drain: Duration) {
        let mut q = self.lock();
        q.closed = true;
        q.drain_by = Some(crate::game_clock::clock::wall() + drain);
        drop(q);
        self.ready.notify_all();
    }

    fn is_dead(&self) -> bool {
        self.lock().dead
    }
}

/// `write_all` with one deadline for the whole of `bytes`. The socket's send
/// timeout only bounds a single `send`, and `write_all` loops while each send
/// moves a byte, so a peer that reads a trickle would hold the writer for
/// ever; here each send gets what is left of the deadline.
fn write_by_deadline(sock: &mut TcpStream, bytes: &[u8], timeout: Duration) -> io::Result<()> {
    let deadline = crate::game_clock::clock::wall() + timeout;
    let mut off = 0;
    while off < bytes.len() {
        let left = deadline.saturating_duration_since(crate::game_clock::clock::wall());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "write timed out"));
        }
        sock.set_write_timeout(Some(left))?;
        match sock.write(&bytes[off..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// The connection's writer: sends the queue's frames in order. A write error,
/// which includes the write timeout, leaves the stream part-written and so
/// unusable: the connection is marked dead and shut down. After `close` it
/// sends what is left and shuts the socket down.
fn write_loop(mut sock: TcpStream, out: &Outbox, timeout: Duration, peer: &str) {
    loop {
        let (chunk, limit) = {
            let mut q = out.lock();
            loop {
                if q.dead {
                    return;
                }
                if let Some(f) = q.frames.pop_front() {
                    // After close the rest of the queue shares one deadline.
                    let limit = q.drain_by.map_or(timeout, |d| {
                        d.saturating_duration_since(crate::game_clock::clock::wall())
                            .min(timeout)
                    });
                    break (f, limit);
                }
                if q.closed {
                    drop(q);
                    let _ = sock.shutdown(std::net::Shutdown::Both);
                    return;
                }
                q = out.ready.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        match write_by_deadline(&mut sock, &chunk, limit) {
            Ok(()) => {
                let mut q = out.lock();
                q.bytes = q.bytes.saturating_sub(chunk.len());
            }
            Err(e) => {
                let mut q = out.lock();
                if !q.dead {
                    crate::outln!("[gateway] write to {peer} failed ({e}); dropping it");
                    q.cause = Cause::WriteFailed;
                }
                q.dead = true;
                q.frames.clear();
                q.bytes = 0;
                drop(q);
                let _ = sock.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
    }
}

impl Conn {
    /// Queue `bytes` for the writer. Refused when the connection is dead; a
    /// queue past its cap kills the connection and shuts its socket down, so
    /// the serve loop's read ends too.
    fn write(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.dead {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection is dead",
            ));
        }
        let r = self.out.push(bytes, &self.peer);
        if r.is_err() {
            self.dead = true;
            let _ = self.sock.shutdown(std::net::Shutdown::Both);
        }
        r
    }
}

/// The send side of one request/reply stream (RFC 9113 5.2, 6.9).
struct OutStream {
    /// What the client has left us to send on this stream.
    win: i64,
    /// Message bytes not yet on the wire, from `pos`.
    buf: Vec<u8>,
    pos: usize,
    /// The trailers frame, held until `buf` has gone out (END_STREAM last).
    trailers: Option<Vec<u8>>,
}

/// A GOAWAY frame naming the highest stream we processed, with `code`.
fn goaway(last: u32, code: u32) -> Vec<u8> {
    let mut p = last.to_be_bytes().to_vec();
    p.extend_from_slice(&code.to_be_bytes());
    frame(GOAWAY, 0, 0, &p)
}

/// Append a frame to `out` without a temporary allocation.
fn put_frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    let n = payload.len();
    out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags]);
    out.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
    out.extend_from_slice(payload);
}

impl Conn {
    /// A new reply stream, with the window the client's settings give it.
    fn open_reply(&mut self, id: u32) {
        self.replies.insert(
            id,
            OutStream {
                win: self.initial,
                buf: Vec::new(),
                pos: 0,
                trailers: None,
            },
        );
    }

    /// Put as much of stream `id`'s queue on `out` as the stream and
    /// connection windows and the peer's frame size allow, then its
    /// trailers if the queue is empty. Drops the stream once they are out.
    fn drain(&mut self, id: u32, out: &mut Vec<u8>) {
        let Some(r) = self.replies.get_mut(&id) else {
            return;
        };
        while r.pos < r.buf.len() {
            let room = self.window.min(r.win);
            if room <= 0 {
                break;
            }
            let n = (r.buf.len() - r.pos).min(self.max_frame).min(room as usize);
            put_frame(out, DATA, 0, id, &r.buf[r.pos..r.pos + n]);
            r.pos += n;
            r.win -= n as i64;
            self.window -= n as i64;
            self.held -= n;
        }
        if r.pos < r.buf.len() {
            // Give back the sent prefix once it is at least what is left (or
            // large), so a stream of small window updates costs O(1) each.
            if r.pos > 0 && (r.pos >= r.buf.len() - r.pos || r.pos >= RECLAIM) {
                r.buf.drain(..r.pos);
                r.pos = 0;
            }
            return;
        }
        r.buf.clear();
        r.pos = 0;
        if let Some(t) = r.trailers.take() {
            out.extend_from_slice(&t);
            self.replies.remove(&id);
        }
    }

    /// Queue one gRPC message on stream `id` and send what fits.
    /// A stream or connection that would hold more than its cap is reset
    /// (`ENHANCE_YOUR_CALM`), its queue dropped; at the connection cap the
    /// connection is closed too. True when the stream was reset: the caller
    /// forgets it and ignores the rest of it.
    fn send(&mut self, id: u32, msg: &[u8], out: &mut Vec<u8>) -> bool {
        if !self.replies.contains_key(&id) {
            self.open_reply(id);
        }
        if let Some(r) = self.replies.get_mut(&id) {
            r.buf.extend_from_slice(msg);
            self.held += msg.len();
        }
        self.drain(id, out);
        let on_stream = self.replies.get(&id).map_or(0, |r| r.buf.len() - r.pos);
        if on_stream > self.cap_stream || self.held > self.cap_conn {
            let conn_wide = self.held > self.cap_conn;
            self.remove_reply(id);
            out.extend(frame(RST_STREAM, 0, id, &ENHANCE_YOUR_CALM.to_be_bytes()));
            if conn_wide {
                out.extend(goaway(self.last, ENHANCE_YOUR_CALM));
                self.closing = true;
            }
            return true;
        }
        false
    }

    /// Forget stream `id`'s reply state, and what it held.
    fn remove_reply(&mut self, id: u32) {
        if let Some(r) = self.replies.remove(&id) {
            self.held -= r.buf.len() - r.pos;
        }
    }

    /// End stream `id` with `trailers` (a whole frame), after its data.
    fn end(&mut self, id: u32, trailers: Vec<u8>, out: &mut Vec<u8>) {
        match self.replies.get_mut(&id) {
            Some(r) => r.trailers = Some(trailers),
            None => return out.extend_from_slice(&trailers),
        }
        self.drain(id, out);
    }

    /// A window opened: send what was held back, on every stream.
    fn flush(&mut self, out: &mut Vec<u8>) {
        let ids: Vec<u32> = self
            .replies
            .iter()
            .filter(|(_, r)| r.pos < r.buf.len())
            .map(|(&i, _)| i)
            .collect();
        for id in ids {
            if self.window <= 0 {
                break;
            }
            self.drain(id, out);
        }
    }
}

type Shared = Arc<Mutex<Conn>>;

/// Write `out` to the connection under its lock. Its DATA frames were
/// counted against the windows when they were queued ([`Conn::send`]).
fn write_out(c: &Shared, out: &[u8]) -> io::Result<()> {
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    g.write(out.to_vec())
}

struct Sub {
    conn: Shared,
    stream: u32,
}

/// Every open `Notices/Consume` stream. Cloning shares it.
#[derive(Clone, Default)]
pub struct NoticeHub(Arc<Mutex<Vec<Sub>>>, Arc<Mutex<Option<HashSet<i64>>>>);

impl NoticeHub {
    /// The game sessions alive now, as the world last said (`None`: never
    /// said, nothing is held back). A connection bound through a session not
    /// in it gets no notices. Set under the world lock by `gateway_out::take`.
    pub(crate) fn set_live(&self, live: HashSet<i64>) {
        *self.1.lock().unwrap_or_else(|e| e.into_inner()) = Some(live);
    }

    /// Unbind the connections bound to `pilot` through client process
    /// `instance`: they get none of the pilot's notices until a served request
    /// binds them again.
    pub(crate) fn unbind(&self, pilot: i64, instance: &[u8]) {
        let subs: Vec<Shared> = {
            let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
            g.iter().map(|s| Arc::clone(&s.conn)).collect()
        };
        for conn in subs {
            let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
            if c.character == pilot && c.instance == instance {
                c.character = 0;
                c.bound_client = 0;
            }
        }
    }

    /// Drop every stream of `conn` from the hub.
    fn forget(&self, conn: &Shared) {
        let mut subs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|s| !Arc::ptr_eq(&s.conn, conn));
    }
}

/// Who a notice is for. The group variants carry the member pilots, which the
/// caller resolves from the world BEFORE publishing, so the hub never holds a
/// world lock; the id only goes into the envelope's `target_group`.
pub enum Target {
    Character(i64),
    Corporation { id: i64, members: Vec<i64> },
    Alliance { id: i64, members: Vec<i64> },
    SolarSystem { id: i64, members: Vec<i64> },
}

impl Target {
    /// (`target_group` field number, the id, the pilots it reaches).
    fn parts(&self) -> (u64, i64, Vec<i64>) {
        match self {
            Target::Character(c) => (4, *c, vec![*c]),
            Target::Corporation { id, members } => (5, *id, members.clone()),
            Target::Alliance { id, members } => (6, *id, members.clone()),
            Target::SolarSystem { id, members } => (2, *id, members.clone()),
        }
    }
}

/// The `eve_public.Notice` envelope **[V: gw-protos.txt]**: dispatched 1, uuid 2,
/// payload 5 (Any), target_group 6. `TargetGroup` is a oneof of **scalars**:
/// `solar_system` 2, `character` 4, `corporation` 5 (uint32) and `alliance` 6
/// (int32), each a varint. (It was written as a nested message once; the
/// client's protobuf reader then saw no group at all, and
/// `noticeConsumer._validate_target_group_against_session` passed it as
/// `None == None`, so every notice was dispatched to whoever received it.
/// With the scalar the client checks the id against its own session.)
fn notice(ty: &str, payload: &[u8], group: u64, id: i64) -> Vec<u8> {
    static N: AtomicU64 = AtomicU64::new(1);
    let nanos = crate::game_clock::clock::wall_system()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&nanos.to_le_bytes());
    uuid[8..].copy_from_slice(&N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &timestamp(now_secs()));
    put_bytes(&mut out, 2, &uuid);
    let mut any = Vec::new();
    put_bytes(&mut any, 1, format!("type.googleapis.com/{ty}").as_bytes());
    put_bytes(&mut any, 2, payload);
    put_bytes(&mut out, 5, &any);
    let mut tg = Vec::new();
    put_uint(&mut tg, group, id as u64);
    put_bytes(&mut out, 6, &tg);
    out
}

/// Send one notice to every `Consume` stream whose pilot `target` reaches.
/// The notice is queued for each connection's writer thread (#233), never
/// written on the caller. A stream whose queue is full, or whose connection
/// or stream is gone, is dropped; a stream whose flow-control window cannot take the notice is
/// skipped (the notice is lost to it, not queued). Returns how many streams
/// got it.
pub fn publish(hub: &NoticeHub, ty: &str, payload: &[u8], target: Target) -> usize {
    let (group, id, members) = target.parts();
    let msg = grpc_message(&notice(ty, payload, group, id));
    let mut sent = 0;
    let live = hub.1.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // The hub lock is held only to copy the handles.
    let subs: Vec<(Shared, u32)> = {
        let g = hub.0.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().map(|s| (Arc::clone(&s.conn), s.stream)).collect()
    };
    let mut gone: Vec<(Shared, u32)> = Vec::new();
    for (conn, stream) in subs {
        let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
        if c.dead || c.out.is_dead() {
            drop(c);
            gone.push((conn, stream));
            continue;
        }
        let Some(&room) = c.consume.get(&stream) else {
            drop(c);
            gone.push((conn, stream));
            continue;
        };
        if c.character == 0 || !members.contains(&c.character) {
            continue;
        }
        if c.bound_client != 0 && live.as_ref().is_some_and(|l| !l.contains(&c.bound_client)) {
            // Its session ended: the binding is void, a later served request
            // makes a new one.
            c.character = 0;
            c.bound_client = 0;
            continue;
        }
        let n = msg.len() as i64;
        if room < n || c.window < n {
            continue;
        }
        let mut wire = Vec::new();
        for chunk in msg.chunks(c.max_frame) {
            put_frame(&mut wire, DATA, 0, stream, chunk);
        }
        if c.write(wire).is_err() {
            drop(c);
            gone.push((conn, stream));
            continue;
        }
        c.window -= n;
        if let Some(r) = c.consume.get_mut(&stream) {
            *r -= n;
        }
        sent += 1;
    }
    if !gone.is_empty() {
        let mut g = hub.0.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|s| {
            !gone
                .iter()
                .any(|(c, st)| Arc::ptr_eq(c, &s.conn) && *st == s.stream)
        });
    }
    sent
}

/// Serve one connection until it ends. `log` gets one line per request class
/// the first time it is seen (the inventory of what the client asks).
pub fn serve(
    s: TcpStream,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
) -> io::Result<()> {
    serve_capped(
        s,
        instance,
        seen,
        ground,
        hub,
        (MAX_HELD_STREAM, MAX_HELD_CONN),
        (MAX_STREAMS, WRITE_TIMEOUT),
    )
}

/// [`serve`] with the held-reply caps (per stream, per connection) and the
/// concurrent-stream limit given.
fn serve_capped(
    s: TcpStream,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
    caps: (usize, usize),
    limits: (usize, Duration),
) -> io::Result<()> {
    serve_queued(s, instance, seen, ground, hub, caps, limits, MAX_QUEUED)
}

/// [`serve_capped`] with the write queue's byte cap given.
#[allow(clippy::too_many_arguments)]
fn serve_queued(
    s: TcpStream,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
    caps: (usize, usize),
    limits: (usize, Duration),
    queue_cap: usize,
) -> io::Result<()> {
    let (max_streams, write_timeout) = limits;
    s.set_nodelay(true)?;
    let addr = s.peer_addr().ok().map(|a| a.ip());
    let peer = s
        .peer_addr()
        .map_or_else(|_| "an unknown peer".to_owned(), |a| a.to_string());
    let out = Outbox::new(queue_cap);
    // Both clones before the writer starts: a failure here leaves no thread.
    let (wsock, csock) = (s.try_clone()?, s.try_clone()?);
    let writer = {
        let (wout, wpeer) = (Arc::clone(&out), peer.clone());
        std::thread::Builder::new()
            .name("gateway-write".to_owned())
            .spawn(move || write_loop(wsock, &wout, write_timeout, &wpeer))?
    };
    let conn: Shared = Arc::new(Mutex::new(Conn {
        sock: csock,
        character: 0,
        bound_client: 0,
        instance: Vec::new(),
        processes: Vec::new(),
        window: 65535,
        initial: 65535,
        max_frame: 16384,
        consume: HashMap::new(),
        replies: HashMap::new(),
        held: 0,
        cap_stream: caps.0,
        cap_conn: caps.1,
        last: 0,
        closing: false,
        dead: false,
        peer,
        out: Arc::clone(&out),
    }));
    let counted = Counted {
        ground,
        conn: &conn,
    };
    let r = run(s, &conn, instance, seen, ground, hub, max_streams, addr);
    drop(counted);
    conn.lock().unwrap_or_else(|e| e.into_inner()).dead = true;
    hub.forget(&conn);
    // The writer sends what is queued (a GOAWAY) within one write timeout and
    // is joined, so a connection's slot is free only when its thread is gone.
    out.close(write_timeout);
    let _ = writer.join();
    r
}

/// Takes the client processes a connection served off the open count when it
/// ends, on every path, a panic included.
struct Counted<'a> {
    ground: &'a Ground,
    conn: &'a Shared,
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        let done = std::mem::take(
            &mut self
                .conn
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .processes,
        );
        for p in done {
            crate::gateway_auth::connection_closed(self.ground, &p);
        }
    }
}

/// Serve the frames of one connection; `addr` is the peer's address, for
/// `gateway_auth::check`.
#[allow(clippy::too_many_arguments)]
fn run(
    mut s: TcpStream,
    conn: &Shared,
    instance: [u8; 16],
    seen: &mut Vec<String>,
    ground: &Ground,
    hub: &NoticeHub,
    max_streams: usize,
    addr: Option<IpAddr>,
) -> io::Result<()> {
    s.set_read_timeout(Some(IDLE))?;
    let mut pre = [0u8; 24];
    if !read_exact_or_eof(&mut s, &mut pre)? || pre != PREFACE {
        return Ok(());
    }
    // SETTINGS_MAX_CONCURRENT_STREAMS (3).
    let mut set = vec![0, 3];
    set.extend_from_slice(&u32::try_from(max_streams).unwrap_or(u32::MAX).to_be_bytes());
    write_out(conn, &frame(SETTINGS, 0, 0, &set))?;
    // The highest stream id the client has opened, processed or not, and
    // the highest we processed.
    let mut top = 0u32;
    let mut last = 0u32;
    let mut hpack = Hpack::new();
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    // A header block split over CONTINUATION frames.
    let mut pending: Option<(u32, u8, Vec<u8>)> = None;
    loop {
        let mut h = [0u8; 9];
        if !read_exact_or_eof(&mut s, &mut h)? {
            return Ok(());
        }
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        if len > MAX_FRAME {
            return Ok(());
        }
        let (kind, flags) = (h[3], h[4]);
        let sid = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
        let mut body = vec![0u8; len];
        if len > 0 && !read_exact_or_eof(&mut s, &mut body)? {
            return Ok(());
        }
        let mut out: Vec<u8> = Vec::new();
        // Consume streams opened in this round, registered once their
        // headers are on the wire.
        let mut opened: Vec<u32> = Vec::new();
        match kind {
            SETTINGS if flags & ACK == 0 => {
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                for e in body.chunks_exact(6) {
                    let v = u32::from_be_bytes([e[2], e[3], e[4], e[5]]);
                    match u16::from_be_bytes([e[0], e[1]]) {
                        4 => {
                            let new = i64::from(v);
                            let delta = new - c.initial;
                            // RFC 9113 6.5.2, 6.9.2: too big is a flow-control error.
                            if new > MAX_WINDOW
                                || c.consume.values().any(|w| w + delta > MAX_WINDOW)
                                || c.replies.values().any(|r| r.win + delta > MAX_WINDOW)
                            {
                                drop(c);
                                out.extend(goaway(last, FLOW_CONTROL_ERROR));
                                write_out(conn, &out)?;
                                return Ok(());
                            }
                            c.initial = new;
                            for w in c.consume.values_mut() {
                                *w += delta;
                            }
                            for r in c.replies.values_mut() {
                                r.win += delta;
                            }
                        }
                        // Only 16384..=16777215 is legal (RFC 9113 6.5.2).
                        5 if (16384..1 << 24).contains(&v) => c.max_frame = v as usize,
                        _ => {}
                    }
                }
                c.flush(&mut out);
                drop(c);
                out.extend(frame(SETTINGS, ACK, 0, &[]));
            }
            WINDOW_UPDATE if body.len() == 4 => {
                let inc = i64::from(
                    u32::from_be_bytes([body[0], body[1], body[2], body[3]]) & 0x7fff_ffff,
                );
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                if sid == 0 {
                    if inc == 0 || c.window + inc > MAX_WINDOW {
                        drop(c);
                        let code = if inc == 0 {
                            PROTOCOL_ERROR
                        } else {
                            FLOW_CONTROL_ERROR
                        };
                        out.extend(goaway(last, code));
                        write_out(conn, &out)?;
                        return Ok(());
                    }
                    c.window += inc;
                    c.flush(&mut out);
                } else {
                    // RFC 9113 6.9: zero is a stream error PROTOCOL_ERROR, too
                    // big a stream error FLOW_CONTROL_ERROR.
                    let cur = c
                        .consume
                        .get(&sid)
                        .copied()
                        .or_else(|| c.replies.get(&sid).map(|r| r.win));
                    let open = cur.is_some() || streams.contains_key(&sid);
                    if !open && sid > top {
                        // An idle stream (RFC 9113 5.1, 6.9): a connection error.
                        drop(c);
                        out.extend(goaway(last, PROTOCOL_ERROR));
                        write_out(conn, &out)?;
                        return Ok(());
                    }
                    // A closed stream: ignore the update.
                    let bad = if !open {
                        None
                    } else if inc == 0 {
                        Some(PROTOCOL_ERROR)
                    } else if cur.is_some_and(|w| w + inc > MAX_WINDOW) {
                        Some(FLOW_CONTROL_ERROR)
                    } else {
                        None
                    };
                    if let Some(code) = bad {
                        streams.remove(&sid);
                        c.consume.remove(&sid);
                        c.remove_reply(sid);
                        out.extend(frame(RST_STREAM, 0, sid, &code.to_be_bytes()));
                    } else if let Some(w) = c.consume.get_mut(&sid) {
                        *w += inc;
                    } else if let Some(r) = c.replies.get_mut(&sid) {
                        r.win += inc;
                        c.drain(sid, &mut out);
                    }
                }
            }
            PING if flags & ACK == 0 => out.extend(frame(PING, ACK, 0, &body)),
            GOAWAY => return Ok(()),
            RST_STREAM => {
                streams.remove(&sid);
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                c.consume.remove(&sid);
                c.remove_reply(sid);
            }
            HEADERS | CONTINUATION => {
                let mut b: &[u8] = &body;
                let mut fl = flags;
                if kind == HEADERS {
                    if fl & PADDED != 0 {
                        let Some((&pad, rest)) = b.split_first() else {
                            return Ok(());
                        };
                        let Some(keep) = rest.len().checked_sub(usize::from(pad)) else {
                            return Ok(());
                        };
                        b = &rest[..keep];
                    }
                    if fl & PRIORITY != 0 {
                        b = b.get(5..).unwrap_or(&[]);
                    }
                    pending = Some((sid, fl, b.to_vec()));
                } else if let Some((_, _, blk)) = pending.as_mut() {
                    blk.extend_from_slice(b);
                    fl |= flags & END_HEADERS;
                }
                if fl & END_HEADERS != 0
                    && let Some((id, first_flags, blk)) = pending.take()
                {
                    let Some(path) = hpack.decode(&blk) else {
                        return Ok(()); // a compression error ends the connection
                    };
                    let known = streams.contains_key(&id);
                    if !known && id <= top {
                        // A closed stream (reset, or already answered): ignore.
                    } else if known {
                        // Trailers from the client: it is done sending.
                        if first_flags & END_STREAM != 0 {
                            finish(&mut streams, conn, id, &mut out);
                        }
                    } else if let Some(k) = path.as_deref().and_then(route) {
                        top = id;
                        let busy = {
                            let c = conn.lock().unwrap_or_else(|e| e.into_inner());
                            streams.len()
                                + c.replies
                                    .keys()
                                    .filter(|k| !streams.contains_key(k))
                                    .count()
                        };
                        if busy >= max_streams {
                            // RFC 9113 5.1.2: over our limit, refused unprocessed.
                            out.extend(frame(RST_STREAM, 0, id, &REFUSED_STREAM.to_be_bytes()));
                            write_out(conn, &out)?;
                            continue;
                        }
                        last = id;
                        conn.lock().unwrap_or_else(|e| e.into_inner()).last = id;
                        streams.insert(
                            id,
                            Stream {
                                kind: k,
                                buf: Vec::new(),
                            },
                        );
                        out.extend(frame(HEADERS, END_HEADERS, id, &response_headers()));
                        if k == Kind::Consume {
                            opened.push(id);
                        } else {
                            let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                            c.open_reply(id);
                        }
                        if first_flags & END_STREAM != 0 {
                            finish(&mut streams, conn, id, &mut out);
                        }
                    } else {
                        // Unknown method: trailers-only, UNIMPLEMENTED.
                        top = top.max(id);
                        last = last.max(id);
                        conn.lock().unwrap_or_else(|e| e.into_inner()).last = last;
                        let mut t = response_headers();
                        literal(&mut t, "grpc-status", "12");
                        out.extend(frame(HEADERS, END_HEADERS | END_STREAM, id, &t));
                    }
                }
            }
            DATA => {
                let mut b: &[u8] = &body;
                if flags & PADDED != 0 {
                    let Some((&pad, rest)) = b.split_first() else {
                        return Ok(());
                    };
                    let Some(keep) = rest.len().checked_sub(usize::from(pad)) else {
                        return Ok(());
                    };
                    b = &rest[..keep];
                }
                if len > 0 {
                    // Keep both windows open: a long-lived Send stream
                    // carries far more than the initial 65535 bytes.
                    let inc = (len as u32).to_be_bytes();
                    out.extend(frame(WINDOW_UPDATE, 0, 0, &inc));
                    if streams.contains_key(&sid) && flags & END_STREAM == 0 {
                        out.extend(frame(WINDOW_UPDATE, 0, sid, &inc));
                    }
                }
                let mut reset = false;
                if let Some(st) = streams.get_mut(&sid) {
                    st.buf.extend_from_slice(b);
                    while st.buf.len() >= 5 {
                        let n = u32::from_be_bytes([st.buf[1], st.buf[2], st.buf[3], st.buf[4]])
                            as usize;
                        if n > MAX_FRAME {
                            return Ok(());
                        }
                        if st.buf.len() < 5 + n {
                            break;
                        }
                        let msg: Vec<u8> = st.buf[5..5 + n].to_vec();
                        let compressed = st.buf[0] != 0;
                        st.buf.drain(..5 + n);
                        match st.kind {
                            Kind::Send if !compressed => {
                                let served = crate::gateway_auth::check(ground, &msg, addr);
                                if served.is_some() {
                                    let process = request_instance(&msg);
                                    let new = {
                                        let c = conn.lock().unwrap_or_else(|e| e.into_inner());
                                        !c.processes.contains(&process)
                                    };
                                    if new {
                                        // Count it open before the connection
                                        // remembers it, so the close never
                                        // takes off more than was put on.
                                        crate::gateway_auth::connection_opened(ground, &process);
                                        conn.lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .processes
                                            .push(process.clone());
                                    }
                                    if let Some((who, client)) =
                                        served.as_ref().and_then(|s| s.bind)
                                    {
                                        let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                                        c.character = who;
                                        c.bound_client = client;
                                        c.instance = process;
                                    }
                                }
                                let (resp, ty, status) =
                                    respond_as(&msg, &instance, ground, served.as_ref());
                                if !seen.contains(&ty) {
                                    crate::outln!("[gateway] request class {ty} -> {status}");
                                    seen.push(ty);
                                }
                                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                                reset = c.send(sid, &grpc_message(&resp), &mut out);
                            }
                            Kind::Publish | Kind::Ping => {
                                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                                reset = c.send(sid, &grpc_message(&[]), &mut out);
                            }
                            _ => {}
                        }
                        if reset {
                            break;
                        }
                    }
                }
                if reset {
                    // Reset by us: the rest of the stream, trailers included, is ignored.
                    streams.remove(&sid);
                }
                if flags & END_STREAM != 0 {
                    finish(&mut streams, conn, sid, &mut out);
                }
            }
            _ => {}
        }
        if !out.is_empty() {
            write_out(conn, &out)?;
        }
        if conn.lock().unwrap_or_else(|e| e.into_inner()).closing {
            return Ok(());
        }
        for id in opened {
            let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
            let w = c.initial;
            c.consume.insert(id, w);
            drop(c);
            hub.0.lock().unwrap_or_else(|e| e.into_inner()).push(Sub {
                conn: Arc::clone(conn),
                stream: id,
            });
        }
    }
}

/// The client has half-closed `id`: end every stream but `Consume`, which
/// stays open for notices we never send.
/// The trailers wait behind any reply data still held for the windows.
fn finish(streams: &mut HashMap<u32, Stream>, conn: &Shared, id: u32, out: &mut Vec<u8>) {
    if streams.get(&id).is_some_and(|s| s.kind != Kind::Consume) {
        streams.remove(&id);
        let t = frame(HEADERS, END_HEADERS | END_STREAM, id, &trailers(0));
        conn.lock()
            .unwrap_or_else(|e| e.into_inner())
            .end(id, t, out);
    }
}

/// Bind `addr` and serve in the background. The bound address.
pub fn spawn(addr: SocketAddr, ground: Ground) -> io::Result<SocketAddr> {
    spawn_with(addr, ground, NoticeHub::default())
}

/// [`spawn`] with the caller's [`NoticeHub`], so it can [`publish`].
pub fn spawn_with(addr: SocketAddr, ground: Ground, hub: NoticeHub) -> io::Result<SocketAddr> {
    let socket = TcpListener::bind(addr)?;
    let bound = socket.local_addr()?;
    let live = Arc::new(AtomicUsize::new(0));
    let instance = {
        // Any 16 bytes will do; vary them by start time.
        let t = crate::game_clock::clock::wall_system()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        t.to_le_bytes()
    };
    std::thread::Builder::new()
        .name("gateway-accept".to_owned())
        .spawn(move || {
            for conn in socket.incoming() {
                let Ok(stream) = conn else {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if live.fetch_add(1, Ordering::SeqCst) >= CAP {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let live = Arc::clone(&live);
                let ground = ground.clone();
                let hub = hub.clone();
                let _ = std::thread::Builder::new()
                    .name("gateway-conn".to_owned())
                    .spawn(move || {
                        let mut seen = Vec::new();
                        let _ = serve(stream, instance, &mut seen, &ground, &hub);
                        live.fetch_sub(1, Ordering::SeqCst);
                    });
            }
        })?;
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn huffman_matches_the_rfc_examples() {
        // RFC 7541 C.4.1, C.4.2, C.6.1.
        assert_eq!(
            huffman_decode(&hex("f1e3c2e5f23a6ba0ab90f4ff")).unwrap(),
            b"www.example.com"
        );
        assert_eq!(huffman_decode(&hex("a8eb10649cbf")).unwrap(), b"no-cache");
        assert_eq!(
            huffman_decode(&hex("25a849e95ba97d7f")).unwrap(),
            b"custom-key"
        );
        // Bad padding is refused.
        assert!(huffman_decode(&hex("f1e3c2e5f23a6ba0ab90f4fe")).is_none());
    }

    #[test]
    fn hpack_indexes_into_its_dynamic_table() {
        let mut h = Hpack::new();
        // Literal with incremental indexing, name from static index 4.
        let mut b = vec![0x44, 3];
        b.extend_from_slice(b"/ab");
        assert_eq!(h.decode(&b).unwrap().unwrap(), b"/ab");
        // Index 62 is now that entry.
        assert_eq!(h.decode(&[0xbe]).unwrap().unwrap(), b"/ab");
        // A size update to 0 empties it, and the index is then invalid.
        assert!(h.decode(&[0x20]).unwrap().is_none());
        assert!(h.decode(&[0xbe]).is_none());
    }

    fn request(ty: &str, corr: &[u8]) -> Vec<u8> {
        request_for(ty, &[], corr, 0)
    }

    /// A request from pilot `character` (0: no context) with a payload.
    fn request_for(ty: &str, value: &[u8], corr: &[u8], character: u64) -> Vec<u8> {
        let mut any = Vec::new();
        put_bytes(&mut any, 1, format!("type.googleapis.com/{ty}").as_bytes());
        if !value.is_empty() {
            put_bytes(&mut any, 2, value);
        }
        let mut r = Vec::new();
        put_bytes(&mut r, 2, corr);
        put_bytes(&mut r, 3, &any);
        if character != 0 {
            let mut id = Vec::new();
            put_uint(&mut id, 1, character);
            let mut who = Vec::new();
            put_bytes(&mut who, 1, &id);
            let mut ctx = Vec::new();
            put_bytes(&mut ctx, 10, &who);
            put_bytes(&mut r, 6, &ctx);
        }
        r
    }

    fn decoded(resp: &[u8]) -> (u64, Vec<u8>, Option<String>) {
        let f = fields(resp);
        let get = |n| f.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
        let status = u64::from_le_bytes(get(3).unwrap().try_into().unwrap());
        let ty = get(5).and_then(|any| {
            fields(&any)
                .into_iter()
                .find(|(k, _)| *k == 1)
                .map(|(_, v)| String::from_utf8(v).unwrap())
        });
        (status, get(2).unwrap_or_default(), ty)
    }

    /// The `Any.value` of a response.
    fn payload(resp: &[u8]) -> Vec<u8> {
        get_bytes(&get_bytes(resp, 5).unwrap_or_default(), 2).unwrap_or_default()
    }

    /// Ask `ty` as pilot `who` and return `(status, payload)`.
    fn ask(g: &Ground, ty: &str, value: &[u8], who: u64) -> (u64, Vec<u8>) {
        let (r, _, _) = respond(&request_for(ty, value, b"x", who), &[0; 16], g);
        (decoded(&r).0, payload(&r))
    }

    /// Two pilots of two accounts, each in a live session (clients 1 and 2).
    fn two_live_pilots() -> (Ground, Arc<RwLock<Registry>>) {
        let mut reg = crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: 1_001,
                owner_user_id: Some(11),
                ..Default::default()
            };
            let mut b = a.clone();
            b.character_id = 1_002;
            b.ship_id = a.ship_id + 1;
            b.owner_user_id = Some(12);
            cfg.characters = vec![a, b];
            cfg.catch_all = None;
        });
        for (client_id, user_id, who) in [(1, 11, 1_001), (2, 12, 1_002)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id,
                session_id: client_id,
                user_id,
                character_id: Some(who),
            });
        }
        let w = Arc::new(RwLock::new(reg));
        (Ground::Server(Arc::clone(&w)), w)
    }

    #[test]
    fn a_server_ground_serves_a_live_pilot_and_refuses_one_with_no_session() {
        let (g, w) = two_live_pilots();
        let ask = |who: i64, proc_: &[u8]| {
            let (r, _, status) = respond(
                &crate::gateway_auth::test_request(who, None, proc_),
                &[0; 16],
                &g,
            );
            (status, decoded(&r))
        };
        // Served: a typed 200 (the empty-message arm for an unknown class).
        let (status, d) = ask(1_001, b"process-one-0001");
        assert_eq!(status, 200);
        assert_eq!(d.0, 200);
        assert!(d.2.unwrap().ends_with("thing.api.GetResponse"));
        // Another process for that pilot, a pilot of nobody's session, none.
        assert_eq!(ask(1_001, b"process-two-0002").0, 403);
        w.write().unwrap().session_closed(2);
        let (status, d) = ask(1_002, b"process-two-0002");
        assert_eq!((status, d), (403, (403, b"c1".to_vec(), None)));
    }

    /// Send `m` on stream 1 and read until its reply arrives.
    fn send_and_read(s: &mut TcpStream, m: &[u8]) -> u64 {
        s.write_all(&frame(DATA, 0, 1, &grpc_message(m))).unwrap();
        loop {
            let (kind, _, sid, body) = read_frame(s);
            if kind == DATA && sid == 1 {
                return decoded(&body[5..]).0;
            }
        }
    }

    /// A connection to a server-ground gateway with a `Send` stream (1) and a
    /// `Consume` stream (3) open.
    fn connected(g: &Ground, hub: &NoticeHub) -> TcpStream {
        let addr = spawn_with("127.0.0.1:0".parse().unwrap(), g.clone(), hub.clone()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 3, true));
        s.write_all(&out).unwrap();
        s
    }

    fn live_clients(w: &Arc<RwLock<Registry>>) -> HashSet<i64> {
        let r = w.read().unwrap();
        r.sessions().entries().iter().map(|e| e.client_id).collect()
    }

    #[test]
    fn a_pilot_who_is_not_live_is_not_bound_for_notices() {
        let (g, _w) = two_live_pilots();
        let hub = NoticeHub::default();
        let mut s = connected(&g, &hub);
        // Pilot 1_003 has no session: refused, and the connection stays unbound.
        let m = crate::gateway_auth::test_request(1_003, None, b"process-one-0001");
        assert_eq!(send_and_read(&mut s, &m), 403);
        let ty = "eve_public.invented.notice.Thing";
        assert_eq!(publish(&hub, ty, b"pp", Target::Character(1_003)), 0);
        // A live pilot's request binds it.
        let m = crate::gateway_auth::test_request(1_001, None, b"process-one-0001");
        assert_eq!(send_and_read(&mut s, &m), 200);
        assert_eq!(publish(&hub, ty, b"pp", Target::Character(1_001)), 1);
    }

    #[test]
    fn notices_stop_at_logout_and_a_character_switch_moves_the_binding() {
        let (g, w) = two_live_pilots();
        let hub = NoticeHub::default();
        let mut s = connected(&g, &hub);
        let ty = "eve_public.invented.notice.Thing";
        let m = crate::gateway_auth::test_request(1_001, None, b"process-one-0001");
        assert_eq!(send_and_read(&mut s, &m), 200);
        assert_eq!(publish(&hub, ty, b"a", Target::Character(1_001)), 1);
        // The same connection now serves a second pilot (a second window of
        // one client process): the binding moves.
        let m = crate::gateway_auth::test_request(1_002, None, b"process-one-0001");
        assert_eq!(send_and_read(&mut s, &m), 200);
        assert_eq!(publish(&hub, ty, b"a", Target::Character(1_001)), 0);
        assert_eq!(publish(&hub, ty, b"b", Target::Character(1_002)), 1);
        // The world tells the hub who is live (`gateway_out::take` does, under
        // the world lock); once 1_002's session ends, nothing reaches it.
        hub.set_live(live_clients(&w));
        assert_eq!(publish(&hub, ty, b"b", Target::Character(1_002)), 1);
        w.write().unwrap().session_closed(2);
        hub.set_live(live_clients(&w));
        assert_eq!(publish(&hub, ty, b"b", Target::Character(1_002)), 0);
        // And it stays unbound: a re-login of that pilot does not revive it.
        {
            let mut r = w.write().unwrap();
            r.session_opened(crate::session_table::SessionEntry {
                client_id: 3,
                session_id: 3,
                user_id: 12,
                character_id: Some(1_002),
            });
        }
        hub.set_live(live_clients(&w));
        assert_eq!(publish(&hub, ty, b"b", Target::Character(1_002)), 0);
    }

    #[test]
    fn answers_by_request_class() {
        let g = Ground::local();
        let (r, ty, status) = respond(
            &request("eve_public.cosmetic.ship.api.GetRequest", b"c1"),
            &[0; 16],
            &g,
        );
        assert_eq!(ty, "eve_public.cosmetic.ship.api.GetRequest");
        assert_eq!(status, 404);
        assert_eq!(decoded(&r), (404, b"c1".to_vec(), None));
        let (r, _, _) = respond(
            &request(
                "eve_public.sovereignty.mercenaryden.activity.api.GetAllRequest",
                b"c2",
            ),
            &[0; 16],
            &g,
        );
        assert_eq!(
            decoded(&r),
            (
                200,
                b"c2".to_vec(),
                Some(
                    "type.googleapis.com/eve_public.sovereignty.mercenaryden.activity.api.GetAllResponse"
                        .to_owned()
                )
            )
        );
        let (r, _, _) = respond(
            &request("eve_public.invented.thing.api.GetRequest", b"c3"),
            &[0; 16],
            &g,
        );
        assert_eq!(
            decoded(&r),
            (
                200,
                b"c3".to_vec(),
                Some("type.googleapis.com/eve_public.invented.thing.api.GetResponse".to_owned())
            )
        );
        // Not a *Request name: payload unset.
        let (r, _, _) = respond(&request("eve_public.invented.Thing", b"c4"), &[0; 16], &g);
        assert_eq!(decoded(&r), (200, b"c4".to_vec(), None));
        // Corruption: total_progress = Fraction { denominator: 1 }.
        let (_, value) = ask(
            &g,
            "eve_public.pirate.corruption.api.GetSystemInfoRequest",
            &[],
            0,
        );
        let progress = get_bytes(&value, 1).unwrap();
        assert_eq!(progress, vec![0x10, 1]);
    }

    #[test]
    fn workforce_hub_answers_set_a_mode() {
        let g = Ground::local();
        for ty in [
            "eve_public.sovereignty.hub.workforce.api.GetConfigurationRequest",
            "eve_public.sovereignty.hub.workforce.api.GetStateRequest",
        ] {
            let (status, value) = ask(&g, ty, &[], 0);
            assert_eq!(status, 200, "{ty}");
            let mode = get_bytes(&value, 1).expect(ty);
            // Exactly one of the oneof fields is set; which one is [A].
            let set = (1..=4)
                .filter(|f| get_uint(&mode, *f).is_some() || get_bytes(&mode, *f).is_some())
                .count();
            assert_eq!(set, 1, "{ty}");
        }
    }

    #[test]
    fn single_records_are_not_found_and_content_checkpoints_too() {
        let g = Ground::local();
        for ty in [
            "eve_public.cosmetic.corporation.palette.api.GetRequest",
            "eve_public.cosmetic.corporation.palette.api.GetOwnRequest",
            "eve_public.cosmetic.structure.paintwork.api.GetRequest",
            "eve_public.stargate.lock.api.GetRequest",
            "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest",
            "eve_public.invented.content.api.cdn.GetLatestRequest",
            "eve_public.invented.title.definition.api.GetLatestRequest",
            "eve_public.invented.space.api.cdn.GetUpdateRequest",
        ] {
            assert_eq!(ask(&g, ty, &[], 7), (404, vec![]), "{ty}");
        }
        // A list of the same thing is an empty list, not a 404.
        assert_eq!(
            ask(
                &g,
                "eve_public.stargate.lock.api.GetRestrictedSystemsRequest",
                &[],
                7
            ),
            (200, vec![])
        );
        assert_eq!(
            ask(
                &g,
                "eve_public.cosmetic.corporation.palette.api.SetRequest",
                &[],
                7
            )
            .0,
            403
        );
    }

    #[test]
    fn small_classes_have_their_own_answers() {
        let g = Ground::local();
        // The omega licence runs for years; any other licence is not found.
        let mut kind = Vec::new();
        put_bytes(&mut kind, 1, b"eve_clonestate_omega");
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &kind);
        let (status, v) = ask(&g, "eve_public.user.license.api.GetRequest", &req, 7);
        assert_eq!(status, 200);
        let attrs = get_bytes(&v, 1).unwrap();
        let expiry = get_uint(&get_bytes(&attrs, 2).unwrap(), 1).unwrap();
        assert!(expiry > now_secs() + 365 * 86_400);
        assert!(get_bytes(&attrs, 3).is_some());
        let mut other = Vec::new();
        put_bytes(&mut other, 1, b"invented_license");
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &other);
        assert_eq!(
            ask(&g, "eve_public.user.license.api.GetRequest", &req, 7).0,
            404
        );
        // Title: the no_title branch.
        let (_, v) = ask(
            &g,
            "eve_public.cosmetic.character.title.api.GetEquippedForCharacterRequest",
            &[],
            7,
        );
        assert_eq!(get_uint(&v, 2), Some(1));
        assert_eq!(get_bytes(&v, 1), None);
        // PLEX vault: a balance message, zero.
        let (_, v) = ask(&g, "eve_public.plex.vault.api.BalanceRequest", &[], 7);
        assert_eq!(
            get_uint(&get_bytes(&v, 1).unwrap(), 1),
            Some(PLEX_BALANCE_CENTS)
        );
        // Faction scores echo the asked system.
        let mut sys = Vec::new();
        put_uint(&mut sys, 1, 1234);
        let mut req = Vec::new();
        put_bytes(&mut req, 1, &sys);
        let (_, v) = ask(
            &g,
            "eve_public.faction.activity.GetSolarSystemScoresRequest",
            &req,
            7,
        );
        assert_eq!(get_bytes(&v, 1), Some(sys));
    }

    fn read_frame(s: &mut TcpStream) -> (u8, u8, u32, Vec<u8>) {
        let mut h = [0u8; 9];
        s.read_exact(&mut h).unwrap();
        let len = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
        let mut b = vec![0u8; len];
        s.read_exact(&mut b).unwrap();
        (h[3], h[4], u32::from_be_bytes([h[5], h[6], h[7], h[8]]), b)
    }

    fn open_stream(path: &str, id: u32, index: bool) -> Vec<u8> {
        let mut blk = Vec::new();
        if index {
            blk.push(0x44);
            blk.push(path.len() as u8);
            blk.extend_from_slice(path.as_bytes());
        } else {
            blk.push(0xbe); // the entry the first stream added
        }
        frame(HEADERS, END_HEADERS, id, &blk)
    }

    #[test]
    fn serves_send_over_h2c_and_leaves_consume_open() {
        let hub = NoticeHub::default();
        let addr =
            spawn_with("127.0.0.1:0".parse().unwrap(), Ground::local(), hub.clone()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        out.extend(frame(
            DATA,
            0,
            1,
            &grpc_message(&request("eve_public.cosmetic.ship.api.GetRequest", b"k9")),
        ));
        // A path that is not routed ends in UNIMPLEMENTED, trailers only.
        out.extend(open_stream("/eve_public.gateway.Nothing/Here", 3, true));
        // The dynamic entry from stream 1 (now the newest one is the other
        // path, index 62; Send is 63) routes a second Send stream.
        out.extend(frame(HEADERS, END_HEADERS, 5, &[0xbf]));
        out.extend(frame(
            DATA,
            END_STREAM,
            5,
            &grpc_message(&request("eve_public.invented.thing.api.GetRequest", b"k8")),
        ));
        // A Send naming invented pilot 9000001 teaches the connection who it
        // is; a Consume stream then registers with the hub.
        let m = request_for(
            "eve_public.invented.thing.api.GetRequest",
            b"",
            b"k7",
            9000001,
        );
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 7, true));
        s.write_all(&out).unwrap();
        let mut got_data = None;
        let mut unimplemented = false;
        let mut headers_on_1 = false;
        let mut done5 = false;
        let mut headers_on_7 = false;
        for _ in 0..16 {
            let (kind, flags, sid, body) = read_frame(&mut s);
            match (kind, sid) {
                (HEADERS, 1) => headers_on_1 = true,
                (HEADERS, 7) => headers_on_7 = true,
                (DATA, 1) if got_data.is_some() => {}
                (DATA, 1) => got_data = Some(body),
                (HEADERS, 3) if flags & END_STREAM != 0 => unimplemented = true,
                // Trailers after the one response: the half-close was seen.
                (HEADERS, 5) if flags & END_STREAM != 0 => done5 = true,
                _ => {}
            }
            if got_data.is_some() && unimplemented && done5 && headers_on_7 {
                break;
            }
        }
        assert!(headers_on_1, "response headers precede the data");
        let body = got_data.expect("a response");
        assert!(done5);
        assert_eq!(body[0], 0);
        let (status, corr, ty) = decoded(&body[5..]);
        assert_eq!((status, corr, ty), (404, b"k9".to_vec(), None));
        assert!(unimplemented);
        assert!(headers_on_7);

        // Publish: the right pilot gets it, another gets nothing. The Send
        // above may still be in flight, so retry for the one that must land.
        let ty = "eve_public.invented.notice.Thing";
        let mut sent = 0;
        for _ in 0..200 {
            sent = publish(&hub, ty, b"pp", Target::Character(9000001));
            if sent > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sent, 1);
        assert_eq!(publish(&hub, ty, b"pp", Target::Character(9000002)), 0);
        let members = vec![9000001];
        assert_eq!(
            publish(&hub, ty, b"qq", Target::Corporation { id: 5, members }),
            1
        );
        let mut notices = Vec::new();
        while notices.len() < 2 {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 7 {
                notices.push(body);
            }
        }
        let env = &notices[0][5..];
        let any = get_bytes(env, 5).unwrap();
        assert!(type_name(&get_bytes(&any, 1).unwrap()).ends_with("Thing"));
        assert_eq!(get_bytes(&any, 2).unwrap(), b"pp");
        assert_eq!(get_uint(&get_bytes(env, 6).unwrap(), 4), Some(9000001));
        assert_eq!(get_bytes(env, 2).unwrap().len(), 16);
        let env = &notices[1][5..];
        assert_eq!(get_uint(&get_bytes(env, 6).unwrap(), 5), Some(5));
    }

    /// `Notice.TargetGroup` is a oneof of varint scalars (gw-protos.txt:
    /// `solar_system` 2, `character` 4, `corporation` 5, `alliance` 6): the
    /// id is the field's own value, never a nested message.
    #[test]
    fn the_target_group_is_a_scalar_the_client_can_read() {
        for (target, field, id) in [
            (Target::Character(7), 4u64, 7i64),
            (
                Target::SolarSystem {
                    id: 8,
                    members: vec![],
                },
                2,
                8,
            ),
            (
                Target::Corporation {
                    id: 9,
                    members: vec![],
                },
                5,
                9,
            ),
            (
                Target::Alliance {
                    id: 10,
                    members: vec![],
                },
                6,
                10,
            ),
        ] {
            let (group, gid, _) = target.parts();
            let env = notice("a.B", b"p", group, gid);
            let tg = get_bytes(&env, 6).expect("a target_group");
            assert_eq!(get_uint(&tg, field), Some(id as u64));
            assert!(
                get_bytes(&tg, field).is_none(),
                "not a length-delimited field"
            );
        }
    }

    /// GW-OUT: a notice queued on the world's outbox reaches a subscribed
    /// `Consume` stream once the drained batch is published, and only the
    /// pilot it names gets it (`gateway_out`).
    #[test]
    fn an_outbox_entry_reaches_a_subscribed_stream_after_a_drain() {
        let hub = NoticeHub::default();
        let addr =
            spawn_with("127.0.0.1:0".parse().unwrap(), Ground::local(), hub.clone()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let m = request_for("eve_public.invented.thing.api.GetRequest", b"", b"k1", 7001);
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 3, true));
        s.write_all(&out).unwrap();
        let ty = "eve_public.invented.notice.Echo";
        let mut w = crate::service::Registry::new(crate::model::SandboxConfig::default());
        crate::gateway_out::queue(&mut w, ty, b"hello".to_vec(), Target::Character(7001));
        crate::gateway_out::queue(&mut w, ty, b"other".to_vec(), Target::Character(7002));
        let batch = crate::gateway_out::take(&mut w);
        assert_eq!(batch.len(), 2);
        assert!(crate::gateway_out::take(&mut w).is_empty());
        // The guard is gone (the registry is a plain value here); the Send
        // above may still be in flight, so retry until the stream is known.
        let mut sent = 0;
        for _ in 0..200 {
            let again = batch
                .iter()
                .map(|(t, p, tg)| {
                    let tg = match tg {
                        Target::Character(c) => Target::Character(*c),
                        _ => unreachable!("pilots only"),
                    };
                    (t.clone(), p.clone(), tg)
                })
                .collect();
            sent = crate::gateway_out::publish_all(&hub, again);
            if sent > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sent, 1, "7001 got it; 7002 has no stream");
        loop {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 3 {
                let any = get_bytes(&body[5..], 5).unwrap();
                assert_eq!(get_bytes(&any, 2).unwrap(), b"hello");
                break;
            }
        }
    }

    /// A skill plan made and read back over a real h2c socket, as one pilot,
    /// on one long-lived `Send` stream: the same stream carries both calls.
    #[test]
    fn a_skill_plan_round_trips_over_h2c() {
        let addr = spawn("127.0.0.1:0".parse().unwrap(), Ground::local()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut attrs = Vec::new();
        put_bytes(&mut attrs, 1, b"Over the wire");
        let mut create = Vec::new();
        put_bytes(&mut create, 1, &attrs);
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &[]));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let ty = "eve_public.character.skill.plan";
        for (name, body, corr) in [
            ("CreateRequest", create, b"a".as_slice()),
            ("GetAllRequest", Vec::new(), b"b".as_slice()),
        ] {
            let m = request_for(&format!("{ty}.{name}"), &body, corr, 42);
            out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        }
        s.write_all(&out).unwrap();
        let mut replies: Vec<Vec<u8>> = Vec::new();
        while replies.len() < 2 {
            let (kind, _, sid, body) = read_frame(&mut s);
            if kind == DATA && sid == 1 {
                replies.push(body[5..].to_vec());
            }
        }
        let (status, corr, _) = decoded(&replies[0]);
        assert_eq!((status, corr), (200, b"a".to_vec()));
        let made = get_bytes(&payload(&replies[0]), 1).unwrap();
        let (status, corr, kind) = decoded(&replies[1]);
        assert_eq!((status, corr), (200, b"b".to_vec()));
        assert!(
            kind.unwrap()
                .ends_with("character.skill.plan.GetAllResponse")
        );
        assert_eq!(field(&payload(&replies[1]), 1), vec![made]);
    }

    // ---- frame size and flow control on replies (RFC 9113 4.2, 5.2, 6.9) ----

    /// A `Conn` over a loopback socket, with the given windows.
    fn conn_with(max_frame: usize, window: i64, stream_win: i64) -> (Conn, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (sock, _) = l.accept().unwrap();
        let mut c = Conn {
            sock,
            character: 0,
            bound_client: 0,
            instance: Vec::new(),
            processes: Vec::new(),
            window,
            initial: stream_win,
            max_frame,
            consume: HashMap::new(),
            replies: HashMap::new(),
            held: 0,
            cap_stream: usize::MAX,
            cap_conn: usize::MAX,
            last: 0,
            closing: false,
            dead: false,
            peer: String::new(),
            out: Outbox::new(MAX_QUEUED),
        };
        c.open_reply(1);
        (c, peer)
    }

    /// The (kind, flags, stream, payload length) of every frame in `b`.
    fn frames_in(b: &[u8]) -> Vec<(u8, u8, u32, usize)> {
        let mut v = Vec::new();
        let mut p = 0;
        while p + 9 <= b.len() {
            let n = usize::from(b[p]) << 16 | usize::from(b[p + 1]) << 8 | usize::from(b[p + 2]);
            let sid = u32::from_be_bytes([b[p + 5], b[p + 6], b[p + 7], b[p + 8]]);
            v.push((b[p + 3], b[p + 4], sid, n));
            p += 9 + n;
        }
        assert_eq!(p, b.len(), "whole frames only");
        v
    }

    #[test]
    fn a_reply_over_the_frame_size_arrives_as_several_data_frames() {
        let (mut c, _peer) = conn_with(16384, 1 << 20, 1 << 20);
        let msg = grpc_message(&vec![7u8; 40_000]);
        let mut out = Vec::new();
        c.send(1, &msg, &mut out);
        let f = frames_in(&out);
        assert!(f.len() >= 3, "{f:?}");
        assert!(
            f.iter()
                .all(|&(k, _, s, n)| k == DATA && s == 1 && n <= 16384)
        );
        assert_eq!(f.iter().map(|x| x.3).sum::<usize>(), msg.len());
        // The trailers come last, after the data.
        c.end(
            1,
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &trailers(0)),
            &mut out,
        );
        let f = frames_in(&out);
        assert_eq!(f.last().unwrap().0, HEADERS);
        assert!(c.replies.is_empty());
    }

    #[test]
    fn a_peer_with_a_larger_frame_size_gets_frames_of_that_size() {
        let (mut c, _peer) = conn_with(20_000, 1 << 20, 1 << 20);
        let msg = grpc_message(&vec![7u8; 50_000]);
        let mut out = Vec::new();
        c.send(1, &msg, &mut out);
        let f = frames_in(&out);
        assert_eq!(f[0].3, 20_000);
        assert!(f.iter().all(|x| x.3 <= 20_000));
    }

    #[test]
    fn a_small_window_holds_the_rest_until_a_window_update() {
        // Stream window 10_000, connection window 25_000, frames of 4_000.
        let (mut c, _peer) = conn_with(4_000, 25_000, 10_000);
        let msg = grpc_message(&vec![1u8; 30_000]);
        let mut out = Vec::new();
        c.send(1, &msg, &mut out);
        c.end(
            1,
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &trailers(0)),
            &mut out,
        );
        let sent: usize = frames_in(&out).iter().map(|x| x.3).sum();
        assert_eq!(sent, 10_000, "held at the stream window");
        assert!(frames_in(&out).iter().all(|x| x.0 == DATA && x.3 <= 4_000));
        // The stream window opens by 20_000, but the connection has 15_000.
        out.clear();
        c.replies.get_mut(&1).unwrap().win += 20_000;
        c.flush(&mut out);
        assert_eq!(frames_in(&out).iter().map(|x| x.3).sum::<usize>(), 15_000);
        assert!(frames_in(&out).iter().all(|x| x.0 == DATA));
        // The connection opens, but the stream has 30_000 in all: 5_000 more.
        out.clear();
        c.window += 100_000;
        c.flush(&mut out);
        assert_eq!(frames_in(&out).iter().map(|x| x.3).sum::<usize>(), 5_000);
        // The stream opens too: the last 5 bytes, then the trailers.
        out.clear();
        c.replies.get_mut(&1).unwrap().win += 100;
        c.flush(&mut out);
        let f = frames_in(&out);
        let data: usize = f.iter().filter(|x| x.0 == DATA).map(|x| x.3).sum();
        assert_eq!(data, msg.len() - 30_000);
        assert_eq!(f.last().unwrap().0, HEADERS);
        assert!(f[..f.len() - 1].iter().all(|x| x.0 == DATA));
        assert!(c.replies.is_empty());
    }

    #[test]
    fn a_zero_stream_window_over_h2c_holds_the_reply_until_the_update() {
        let addr = spawn("127.0.0.1:0".parse().unwrap(), Ground::local()).unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(400)))
            .unwrap();
        // SETTINGS_INITIAL_WINDOW_SIZE (4) = 0.
        let mut set = vec![0, 4];
        set.extend_from_slice(&0u32.to_be_bytes());
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &set));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let m = request_for(
            "eve_public.character.skill.plan.GetAllRequest",
            &[],
            b"z",
            42,
        );
        out.extend(frame(DATA, END_STREAM, 1, &grpc_message(&m)));
        s.write_all(&out).unwrap();
        // Everything but DATA and the trailers arrives; those are held.
        let mut seen = Vec::new();
        loop {
            let mut h = [0u8; 9];
            if s.read_exact(&mut h).is_err() {
                break;
            }
            let n = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
            let mut b = vec![0u8; n];
            s.read_exact(&mut b).unwrap();
            seen.push((h[3], u32::from_be_bytes([h[5], h[6], h[7], h[8]])));
        }
        assert!(
            !seen.iter().any(|&(k, sid)| sid == 1 && k == DATA),
            "{seen:?}"
        );
        assert!(seen.contains(&(HEADERS, 1)), "response headers go out");
        assert_eq!(
            seen.iter().filter(|x| x.0 == HEADERS).count(),
            1,
            "no trailers yet"
        );
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(&frame(WINDOW_UPDATE, 0, 1, &65535u32.to_be_bytes()))
            .unwrap();
        let mut got = Vec::new();
        loop {
            let (kind, flags, sid, body) = read_frame(&mut s);
            if sid != 1 {
                continue;
            }
            got.push(kind);
            if kind == HEADERS && flags & END_STREAM != 0 {
                break;
            }
            assert_eq!(kind, DATA);
            let (status, corr, _) = decoded(&body[5..]);
            assert_eq!((status, corr), (200, b"z".to_vec()));
        }
        assert_eq!(got, vec![DATA, HEADERS]);
    }

    // ---- caps, window errors and settings, by Conn and over h2c ----

    /// A message that is `n` bytes on the wire.
    fn msg_of(n: usize) -> Vec<u8> {
        grpc_message(&vec![3u8; n - 5])
    }

    #[test]
    fn a_stream_past_its_cap_is_reset_and_its_queue_dropped() {
        // Invented caps; the window is shut, so every byte is held.
        let (mut c, _peer) = conn_with(16384, 0, 0);
        c.cap_stream = 100;
        c.cap_conn = 10_000;
        let mut out = Vec::new();
        assert!(!c.send(1, &msg_of(60), &mut out));
        assert!(out.is_empty());
        assert_eq!(c.held, 60);
        assert!(c.send(1, &msg_of(60), &mut out), "reports the reset");
        assert_eq!(
            frames_in(&out),
            vec![(RST_STREAM, 0, 1, 4)],
            "reset, no GOAWAY"
        );
        assert_eq!(&out[9..], &ENHANCE_YOUR_CALM.to_be_bytes());
        assert!(c.replies.is_empty());
        assert_eq!(c.held, 0);
        assert!(!c.closing);
    }

    #[test]
    fn a_connection_past_its_cap_is_reset_and_closed() {
        let (mut c, _peer) = conn_with(16384, 0, 0);
        c.cap_stream = 150;
        c.cap_conn = 250;
        let mut out = Vec::new();
        for id in [1, 3, 5] {
            c.send(id, &msg_of(100), &mut out);
            assert!(c.held <= c.cap_conn, "held {} stays under the cap", c.held);
        }
        let f = frames_in(&out);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!((f[0].0, f[0].2), (RST_STREAM, 5));
        assert_eq!(f[1].0, GOAWAY);
        assert!(c.closing);
        assert_eq!(c.held, 200, "the two kept streams");
        let sum: usize = c.replies.values().map(|r| r.buf.len() - r.pos).sum();
        assert_eq!(sum, c.held, "the total tracks the queues");
    }

    #[test]
    fn held_data_gives_back_its_sent_prefix() {
        let (mut c, _peer) = conn_with(1_000, 1 << 20, 1_500);
        let mut out = Vec::new();
        c.send(1, &msg_of(4_000), &mut out);
        let r = &c.replies[&1];
        assert_eq!(r.pos, 1_500, "less sent than left: not moved yet");
        assert_eq!(c.held, 4_000 - 1_500);
        // More goes out; now the sent part is at least what is left.
        c.replies.get_mut(&1).unwrap().win += 1_500;
        c.flush(&mut out);
        let r = &c.replies[&1];
        assert_eq!((r.pos, r.buf.len()), (0, 1_000), "reclaimed");
        assert_eq!(c.held, 1_000);
        // A big sent prefix is reclaimed even when more is left.
        let (mut c, _peer) = conn_with(16384, 1 << 20, 70_000);
        c.send(1, &msg_of(200_000), &mut out);
        let r = &c.replies[&1];
        assert_eq!((r.pos, r.buf.len()), (0, 130_000));
    }

    /// Read frames until the server is quiet for 300 ms.
    fn drain_frames(s: &mut TcpStream) -> Vec<(u8, u8, u32, Vec<u8>)> {
        s.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut v = Vec::new();
        loop {
            let mut h = [0u8; 9];
            if s.read_exact(&mut h).is_err() {
                break;
            }
            let n = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
            let mut b = vec![0u8; n];
            if s.read_exact(&mut b).is_err() {
                break;
            }
            v.push((h[3], h[4], u32::from_be_bytes([h[5], h[6], h[7], h[8]]), b));
        }
        v
    }

    /// A server over loopback with the given caps, and a client that has
    /// sent the preface and `settings`.
    fn h2c(g: Ground, settings: &[(u16, u32)], caps: (usize, usize)) -> TcpStream {
        h2c_with(g, settings, caps, 1_000)
    }

    /// [`h2c`] with the concurrent-stream limit given.
    fn h2c_with(
        g: Ground,
        settings: &[(u16, u32)],
        caps: (usize, usize),
        max_streams: usize,
    ) -> TcpStream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((sock, _)) = l.accept() {
                let _ = serve_capped(
                    sock,
                    [0; 16],
                    &mut Vec::new(),
                    &g,
                    &NoticeHub::default(),
                    caps,
                    (max_streams, WRITE_TIMEOUT),
                );
            }
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let mut set = Vec::new();
        for &(id, v) in settings {
            set.extend_from_slice(&id.to_be_bytes());
            set.extend_from_slice(&v.to_be_bytes());
        }
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &set));
        s.write_all(&out).unwrap();
        s
    }

    /// A ground whose pilot 42 has a plan with `n` requirements (invented
    /// rows, written past the handler's own limit), and the plan's id.
    fn big_plan(n: usize) -> (Ground, Vec<u8>) {
        let g = Ground::local();
        let id = [9u8; 16];
        let hexid: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let reqs = vec!["1:0"; n].join(";");
        g.write(|d| {
            d.put(
                "skillplan",
                vec!["42".into(), "p".into(), hexid],
                vec!["n".into(), "d".into(), reqs],
            )
        });
        let mut ident = Vec::new();
        put_bytes(&mut ident, 1, &id);
        let mut body = Vec::new();
        put_bytes(&mut body, 1, &ident);
        (g, body)
    }

    /// Open stream `id` (the first one indexes the path) with the plan
    /// request and half-close it.
    fn plan_get(body: &[u8], id: u32) -> Vec<u8> {
        let m = request_for("eve_public.character.skill.plan.GetRequest", body, b"q", 42);
        let mut out = open_stream("/eve_public.gateway.Requests/Send", id, id == 1);
        out.extend(frame(DATA, END_STREAM, id, &grpc_message(&m)));
        out
    }

    fn data_sizes(f: &[(u8, u8, u32, Vec<u8>)], sid: u32) -> Vec<usize> {
        f.iter()
            .filter(|x| x.0 == DATA && x.2 == sid)
            .map(|x| x.3.len())
            .collect()
    }

    #[test]
    fn a_peer_max_frame_size_of_20000_gives_a_40k_reply_in_frames_of_20000() {
        let (g, body) = big_plan(5_000);
        let mut s = h2c(g, &[(5, 20_000)], (usize::MAX, usize::MAX));
        // Room for the whole reply.
        let mut out = frame(WINDOW_UPDATE, 0, 0, &(1u32 << 20).to_be_bytes());
        out.extend(plan_get(&body, 1));
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        let sizes = data_sizes(&f, 1);
        assert!(sizes.iter().sum::<usize>() > 40_000, "{sizes:?}");
        assert!(sizes.iter().all(|&n| n <= 20_000), "{sizes:?}");
        assert_eq!(sizes[0], 20_000, "the larger size is used");
    }

    #[test]
    fn a_peer_max_frame_size_of_1000_is_ignored() {
        let (g, body) = big_plan(5_000);
        let mut s = h2c(g, &[(5, 1_000)], (usize::MAX, usize::MAX));
        let mut out = frame(WINDOW_UPDATE, 0, 0, &(1u32 << 20).to_be_bytes());
        out.extend(plan_get(&body, 1));
        s.write_all(&out).unwrap();
        let sizes = data_sizes(&drain_frames(&mut s), 1);
        assert_eq!(sizes[0], 16384, "{sizes:?}");
        assert!(sizes.iter().all(|&n| n <= 16384));
    }

    #[test]
    fn a_reset_stream_gets_nothing_more_even_after_window_updates() {
        let (g, body) = big_plan(100);
        let mut s = h2c(g, &[(4, 0)], (usize::MAX, usize::MAX));
        s.write_all(&plan_get(&body, 1)).unwrap();
        let f = drain_frames(&mut s);
        assert!(data_sizes(&f, 1).is_empty(), "held behind the zero window");
        let mut out = frame(RST_STREAM, 0, 1, &8u32.to_be_bytes());
        out.extend(frame(WINDOW_UPDATE, 0, 0, &65535u32.to_be_bytes()));
        out.extend(frame(WINDOW_UPDATE, 0, 1, &65535u32.to_be_bytes()));
        out.extend(frame(PING, 0, 0, &[1; 8]));
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        assert!(f.iter().any(|x| x.0 == PING && x.1 & ACK != 0), "alive");
        assert!(!f.iter().any(|x| x.2 == 1), "nothing for stream 1: {f:?}");
    }

    #[test]
    fn a_window_size_setting_raised_after_open_releases_held_data() {
        let (g, body) = big_plan(100);
        let mut s = h2c(g, &[(4, 0)], (usize::MAX, usize::MAX));
        s.write_all(&plan_get(&body, 1)).unwrap();
        assert!(data_sizes(&drain_frames(&mut s), 1).is_empty());
        let mut set = vec![0, 4];
        set.extend_from_slice(&65535u32.to_be_bytes());
        s.write_all(&frame(SETTINGS, 0, 0, &set)).unwrap();
        let f = drain_frames(&mut s);
        assert!(!data_sizes(&f, 1).is_empty(), "released: {f:?}");
        let last = f.iter().rfind(|x| x.2 == 1).unwrap();
        assert_eq!((last.0, last.1 & END_STREAM), (HEADERS, END_STREAM));
    }

    #[test]
    fn a_zero_window_and_many_requests_end_in_enhance_your_calm() {
        let (g, body) = big_plan(1_000);
        // Invented caps: about 9 KB a reply, three of them pass 25 KB.
        let mut s = h2c(g, &[(4, 0)], (1 << 20, 25_000));
        let mut out = Vec::new();
        for id in (1..=15).step_by(2) {
            out.extend(plan_get(&body, id));
        }
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        assert!(
            f.iter()
                .any(|x| x.0 == RST_STREAM && x.3 == ENHANCE_YOUR_CALM.to_be_bytes()),
            "{:?}",
            f.iter().map(|x| (x.0, x.2)).collect::<Vec<_>>()
        );
        assert!(
            f.iter()
                .any(|x| x.0 == GOAWAY && x.3[4..] == ENHANCE_YOUR_CALM.to_be_bytes())
        );
        assert!(data_sizes(&f, 1).is_empty(), "nothing was sent");
    }

    fn code_of(f: &[(u8, u8, u32, Vec<u8>)], kind: u8) -> Option<u32> {
        let x = f.iter().find(|x| x.0 == kind)?;
        let t = &x.3[x.3.len() - 4..];
        Some(u32::from_be_bytes([t[0], t[1], t[2], t[3]]))
    }

    #[test]
    fn window_errors_follow_rfc_9113() {
        let big = 0x7fff_ffffu32.to_be_bytes();
        // A connection window past 2^31-1: GOAWAY FLOW_CONTROL_ERROR.
        let mut s = h2c(Ground::local(), &[], (1, 1));
        s.write_all(&frame(WINDOW_UPDATE, 0, 0, &big)).unwrap();
        assert_eq!(
            code_of(&drain_frames(&mut s), GOAWAY),
            Some(FLOW_CONTROL_ERROR)
        );
        // A zero increment on the connection: GOAWAY PROTOCOL_ERROR.
        let mut s = h2c(Ground::local(), &[], (1, 1));
        s.write_all(&frame(WINDOW_UPDATE, 0, 0, &0u32.to_be_bytes()))
            .unwrap();
        assert_eq!(code_of(&drain_frames(&mut s), GOAWAY), Some(PROTOCOL_ERROR));
        // An initial window past 2^31-1: GOAWAY FLOW_CONTROL_ERROR.
        let mut s = h2c(Ground::local(), &[(4, 0x8000_0000)], (1, 1));
        assert_eq!(
            code_of(&drain_frames(&mut s), GOAWAY),
            Some(FLOW_CONTROL_ERROR)
        );
        // On a stream: zero is RST PROTOCOL_ERROR, overflow RST FLOW_CONTROL_ERROR.
        let (g, body) = big_plan(10);
        let mut s = h2c(g, &[(4, 0)], (usize::MAX, usize::MAX));
        s.write_all(&plan_get(&body, 1)).unwrap();
        drain_frames(&mut s);
        s.write_all(&frame(WINDOW_UPDATE, 0, 1, &0u32.to_be_bytes()))
            .unwrap();
        assert_eq!(
            code_of(&drain_frames(&mut s), RST_STREAM),
            Some(PROTOCOL_ERROR)
        );
        let (g, body) = big_plan(10);
        let mut s = h2c(g, &[(4, 0)], (usize::MAX, usize::MAX));
        // Left open, so the stream outlives its reply.
        let m = request_for(
            "eve_public.character.skill.plan.GetRequest",
            &body,
            b"q",
            42,
        );
        let mut out = open_stream("/eve_public.gateway.Requests/Send", 1, true);
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(frame(WINDOW_UPDATE, 0, 1, &big));
        s.write_all(&out).unwrap();
        drain_frames(&mut s);
        s.write_all(&frame(WINDOW_UPDATE, 0, 1, &big)).unwrap();
        assert_eq!(
            code_of(&drain_frames(&mut s), RST_STREAM),
            Some(FLOW_CONTROL_ERROR)
        );
    }

    #[test]
    fn a_window_update_on_an_idle_stream_is_a_connection_error() {
        let (g, body) = big_plan(10);
        let mut s = h2c(g, &[], (usize::MAX, usize::MAX));
        // Stream 1 is answered and closed; an update for it is ignored.
        let mut out = plan_get(&body, 1);
        out.extend(frame(WINDOW_UPDATE, 0, 1, &0u32.to_be_bytes()));
        out.extend(frame(PING, 0, 0, &[2; 8]));
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        assert!(f.iter().any(|x| x.0 == PING && x.1 & ACK != 0), "alive");
        assert!(!f.iter().any(|x| x.0 == GOAWAY || x.0 == RST_STREAM));
        // Stream 7 was never opened: idle, so GOAWAY PROTOCOL_ERROR.
        s.write_all(&frame(WINDOW_UPDATE, 0, 7, &0u32.to_be_bytes()))
            .unwrap();
        let f = drain_frames(&mut s);
        assert_eq!(code_of(&f, GOAWAY), Some(PROTOCOL_ERROR));
        // The connection is over: a ping is not answered.
        let _ = s.write_all(&frame(PING, 0, 0, &[3; 8]));
        assert!(drain_frames(&mut s).iter().all(|x| x.0 != PING));
    }

    #[test]
    fn goaway_names_the_highest_processed_stream() {
        let (g, body) = big_plan(10);
        let mut s = h2c(g, &[], (usize::MAX, usize::MAX));
        let mut out = plan_get(&body, 1);
        out.extend(plan_get(&body, 3));
        out.extend(frame(WINDOW_UPDATE, 0, 0, &0u32.to_be_bytes()));
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        let g = f.iter().find(|x| x.0 == GOAWAY).expect("a GOAWAY");
        assert_eq!(&g.3[..4], &3u32.to_be_bytes(), "last stream id");
    }

    #[test]
    fn many_messages_on_one_stream_past_its_cap_end_in_one_reset() {
        let (g, body) = big_plan(1_000);
        // Invented caps: about 9 KB a reply, three pass 25 KB on one stream.
        let mut s = h2c(g, &[(4, 0)], (25_000, 1 << 30));
        let m = request_for(
            "eve_public.character.skill.plan.GetRequest",
            &body,
            b"q",
            42,
        );
        let mut out = open_stream("/eve_public.gateway.Requests/Send", 1, true);
        for _ in 0..10 {
            out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        }
        out.extend(frame(DATA, END_STREAM, 1, &[]));
        out.extend(frame(PING, 0, 0, &[4; 8]));
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        let rsts: Vec<usize> = (0..f.len()).filter(|&i| f[i].0 == RST_STREAM).collect();
        assert_eq!(rsts.len(), 1, "one reset, not one per message");
        assert_eq!(f[rsts[0]].3, ENHANCE_YOUR_CALM.to_be_bytes());
        assert!(
            f[rsts[0] + 1..].iter().all(|x| x.2 != 1),
            "nothing for the stream after its reset"
        );
        assert!(!f.iter().any(|x| x.0 == GOAWAY));
        assert!(f.iter().any(|x| x.0 == PING && x.1 & ACK != 0), "alive");
    }

    #[test]
    fn streams_past_the_concurrent_limit_are_refused() {
        let (g, body) = big_plan(10);
        let mut s = h2c_with(g, &[], (usize::MAX, usize::MAX), 2);
        // Three streams left open.
        let m = request_for(
            "eve_public.character.skill.plan.GetRequest",
            &body,
            b"q",
            42,
        );
        let mut out = Vec::new();
        for id in [1u32, 3, 5] {
            out.extend(open_stream(
                "/eve_public.gateway.Requests/Send",
                id,
                id == 1,
            ));
            out.extend(frame(DATA, 0, id, &grpc_message(&m)));
        }
        s.write_all(&out).unwrap();
        let f = drain_frames(&mut s);
        let st = f
            .iter()
            .find(|x| x.0 == SETTINGS && x.1 & ACK == 0)
            .unwrap();
        assert_eq!(st.3, [0, 3, 0, 0, 0, 2], "the limit is advertised");
        let r = f.iter().find(|x| x.0 == RST_STREAM).expect("a refusal");
        assert_eq!(
            (r.2, r.3.clone()),
            (5, REFUSED_STREAM.to_be_bytes().to_vec())
        );
        assert!(!f.iter().any(|x| x.2 == 5 && x.0 != RST_STREAM));
        assert!(f.iter().any(|x| x.2 == 3 && x.0 == DATA), "3 was served");
    }

    // ---- a peer that stops reading must not stall the server (#229) ----

    /// A server over loopback that serves every connection against `hub`,
    /// with the given write timeout.
    fn serve_hub(hub: &NoticeHub, timeout: Duration) -> SocketAddr {
        serve_hub_queued(hub, timeout, MAX_QUEUED)
    }

    /// [`serve_hub`] with the write queue's byte cap given.
    fn serve_hub_queued(hub: &NoticeHub, timeout: Duration, queue_cap: usize) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let hub = hub.clone();
        std::thread::spawn(move || {
            for sock in l.incoming().flatten() {
                let hub = hub.clone();
                std::thread::spawn(move || {
                    let _ = serve_queued(
                        sock,
                        [0; 16],
                        &mut Vec::new(),
                        &Ground::local(),
                        &hub,
                        (MAX_HELD_STREAM, MAX_HELD_CONN),
                        (MAX_STREAMS, timeout),
                        queue_cap,
                    );
                });
            }
        });
        addr
    }

    /// A client that opens the largest windows, makes itself pilot `who`
    /// with one `Send`, and opens a `Consume` stream.
    fn big_window_consumer(addr: SocketAddr, who: u64) -> TcpStream {
        let mut s = TcpStream::connect(addr).unwrap();
        let mut set = Vec::new();
        set.extend_from_slice(&4u16.to_be_bytes());
        set.extend_from_slice(&(MAX_WINDOW as u32).to_be_bytes());
        let mut out = PREFACE.to_vec();
        out.extend(frame(SETTINGS, 0, 0, &set));
        out.extend(frame(
            WINDOW_UPDATE,
            0,
            0,
            &((MAX_WINDOW - 65535) as u32).to_be_bytes(),
        ));
        out.extend(open_stream("/eve_public.gateway.Requests/Send", 1, true));
        let m = request_for("eve_public.invented.thing.api.GetRequest", b"", b"k1", who);
        out.extend(frame(DATA, 0, 1, &grpc_message(&m)));
        out.extend(open_stream("/eve_public.gateway.Notices/Consume", 3, true));
        s.write_all(&out).unwrap();
        s
    }

    /// How many streams the hub holds.
    fn hub_len(hub: &NoticeHub) -> usize {
        hub.0.lock().unwrap().len()
    }

    #[test]
    fn a_peer_that_stops_reading_is_dropped_and_the_others_still_get_notices() {
        let hub = NoticeHub::default();
        // An invented queue cap far past anything the flood sends: it is the
        // 3 s write timeout that must drop the stuck peer here.
        let addr = serve_hub_queued(&hub, Duration::from_millis(3_000), 1 << 40);
        let (stuck_who, healthy_who) = (7101u64, 7102u64);
        // The stuck peer connects first, so the fan-out reaches it first. It
        // never reads again.
        let _stuck = big_window_consumer(addr, stuck_who);
        let mut healthy = big_window_consumer(addr, healthy_who);
        let known = |hub: &NoticeHub| {
            let g = hub.0.lock().unwrap();
            g.len() == 2 && g.iter().all(|s| s.conn.lock().unwrap().character != 0)
        };
        let start = std::time::Instant::now();
        while !known(&hub) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "peers never set up"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // The healthy peer reads everything and reports whether the final
        // notice came.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            healthy
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            loop {
                let mut h = [0u8; 9];
                if healthy.read_exact(&mut h).is_err() {
                    return;
                }
                let n = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
                let mut b = vec![0u8; n];
                if healthy.read_exact(&mut b).is_err() {
                    return;
                }
                if h[3] == DATA && b.windows(14).any(|w| w == b"final-invented") {
                    let _ = tx.send(());
                    return;
                }
            }
        });
        let ty = "eve_public.invented.notice.Echo";
        let both = move || Target::Corporation {
            id: 5,
            members: vec![stuck_who as i64, healthy_who as i64],
        };
        // Flood the stuck peer alone, paced, until its socket buffers are
        // full, its write times out, and it is gone from the hub.
        let flood = {
            let hub = hub.clone();
            std::thread::spawn(move || {
                let big = vec![7u8; 256 << 10];
                let start = std::time::Instant::now();
                while hub_len(&hub) > 1 {
                    assert!(start.elapsed() < Duration::from_secs(20), "never dropped");
                    publish(&hub, ty, &big, Target::Character(stuck_who as i64));
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        // While that goes on the hub lock must stay free: taking it never
        // waits on a blocked write.
        let mut slowest = Duration::ZERO;
        while !flood.is_finished() {
            let t = std::time::Instant::now();
            while hub.0.try_lock().is_err() {
                assert!(t.elapsed() < Duration::from_secs(5), "hub lock held");
                std::thread::sleep(Duration::from_millis(1));
            }
            slowest = slowest.max(t.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        flood.join().unwrap();
        assert!(
            slowest < Duration::from_millis(2_000),
            "the hub lock was held for {slowest:?} while a write was stuck"
        );
        assert_eq!(hub_len(&hub), 1, "only the healthy peer is left");
        let sent = publish(&hub, ty, b"final-invented", both());
        assert_eq!(sent, 1);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the healthy peer got the final notice");
    }

    #[test]
    fn a_peer_that_reads_a_trickle_is_still_dropped_within_the_timeout() {
        let hub = NoticeHub::default();
        let timeout = Duration::from_millis(1_000);
        // An invented cap far past what is sent: the drop must come from the
        // write deadline, not the queue.
        let addr = serve_hub_queued(&hub, timeout, 1 << 40);
        let mut slow = big_window_consumer(addr, 7201);
        let start = std::time::Instant::now();
        while hub_len(&hub) != 1 {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "peer never set up"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // 1 KB every 50 ms: each read is well inside the timeout, so each
        // send makes progress, but the notice never fits.
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut b = [0u8; 1024];
            while !stop.load(Ordering::Relaxed) {
                if slow.read(&mut b).unwrap_or(0) == 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        wait_for_pilots(&hub, 1);
        let out = Arc::clone(&hub.0.lock().unwrap()[0].conn.lock().unwrap().out);
        let big = vec![7u8; 256 << 10];
        let mut longest = Duration::ZERO;
        let begin = std::time::Instant::now();
        while hub_len(&hub) > 0 {
            assert!(begin.elapsed() < Duration::from_secs(60), "never dropped");
            std::thread::sleep(Duration::from_millis(10));
            let t = std::time::Instant::now();
            publish(
                &hub,
                "eve_public.invented.notice.Echo",
                &big,
                Target::Character(7201),
            );
            longest = longest.max(t.elapsed());
        }
        done.store(true, Ordering::Relaxed);
        assert_eq!(out.lock().cause, Cause::WriteFailed, "dropped by the queue");
        assert!(
            begin.elapsed() < timeout * 6,
            "the deadline took {:?} against a {timeout:?} timeout",
            begin.elapsed()
        );
        assert!(
            longest < timeout * 2,
            "one publish took {longest:?} against a {timeout:?} timeout"
        );
    }

    // ---- publish only queues; a writer thread sends (#233) ----

    /// Wait until the hub holds `n` streams whose pilots are known.
    fn wait_for_pilots(hub: &NoticeHub, n: usize) {
        let start = std::time::Instant::now();
        loop {
            let ready = {
                let g = hub.0.lock().unwrap();
                g.len() == n && g.iter().all(|s| s.conn.lock().unwrap().character != 0)
            };
            if ready {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "peers never set up"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A thread that reads frames from `s` and sends each DATA payload down
    /// the channel, until the socket ends.
    fn data_reader(mut s: TcpStream) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            loop {
                let mut h = [0u8; 9];
                if s.read_exact(&mut h).is_err() {
                    return;
                }
                let n = usize::from(h[0]) << 16 | usize::from(h[1]) << 8 | usize::from(h[2]);
                let mut b = vec![0u8; n];
                if s.read_exact(&mut b).is_err() {
                    return;
                }
                if h[3] == DATA && tx.send(b).is_err() {
                    return;
                }
            }
        });
        rx
    }

    #[test]
    fn publish_to_a_stuck_peer_returns_at_once_and_the_healthy_peer_gets_the_notice() {
        let hub = NoticeHub::default();
        // A long timeout: the stuck peer's writer is blocked for all of it, so
        // an inline write would stall the publisher for seconds. An invented
        // queue cap of 64 MiB, well past the ~10 MiB sent here.
        let addr = serve_hub_queued(&hub, Duration::from_secs(20), 64 << 20);
        let (stuck_who, healthy_who) = (7301u64, 7302u64);
        let _stuck = big_window_consumer(addr, stuck_who);
        let healthy = big_window_consumer(addr, healthy_who);
        wait_for_pilots(&hub, 2);
        let rx = data_reader(healthy);
        let ty = "eve_public.invented.notice.Echo";
        let both = move || Target::Corporation {
            id: 5,
            members: vec![stuck_who as i64, healthy_who as i64],
        };
        // Fill the stuck peer's socket buffers, then keep publishing while its
        // writer is blocked; the queue takes the rest.
        let big = vec![7u8; 256 << 10];
        let mut slowest = Duration::ZERO;
        let mut blocked_for = Duration::ZERO;
        let begin = std::time::Instant::now();
        for _ in 0..40 {
            let t = std::time::Instant::now();
            publish(&hub, ty, &big, both());
            slowest = slowest.max(t.elapsed());
            blocked_for = begin.elapsed();
            while rx.try_recv().is_ok() {}
        }
        assert_eq!(hub_len(&hub), 2, "neither queue is full");
        assert!(
            slowest < Duration::from_millis(500),
            "one publish took {slowest:?} (40 publishes in {blocked_for:?})"
        );
        let sent = publish(&hub, ty, b"final-invented", both());
        assert_eq!(sent, 2);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let b = rx.recv_timeout(left).expect("the healthy peer got it");
            if b.windows(14).any(|w| w == b"final-invented") {
                break;
            }
        }
    }

    #[test]
    fn a_full_write_queue_drops_the_connection_without_waiting_for_the_timeout() {
        let hub = NoticeHub::default();
        // A one-hour write timeout: only the queue cap can drop the peer.
        let addr = serve_hub_queued(&hub, Duration::from_secs(3600), 512 << 10);
        let _stuck = big_window_consumer(addr, 7401);
        wait_for_pilots(&hub, 1);
        let big = vec![7u8; 256 << 10];
        let begin = std::time::Instant::now();
        while hub_len(&hub) > 0 {
            assert!(begin.elapsed() < Duration::from_secs(30), "never dropped");
            publish(
                &hub,
                "eve_public.invented.notice.Echo",
                &big,
                Target::Character(7401),
            );
        }
        // And once dropped, a publish reaches nobody.
        assert_eq!(
            publish(
                &hub,
                "eve_public.invented.notice.Echo",
                b"x",
                Target::Character(7401)
            ),
            0
        );
    }

    #[test]
    fn frames_on_one_connection_keep_their_order_across_two_publishing_threads() {
        let hub = NoticeHub::default();
        let addr = serve_hub(&hub, Duration::from_secs(20));
        let who = 7501u64;
        let rx = data_reader(big_window_consumer(addr, who));
        wait_for_pilots(&hub, 1);
        let per = 300usize;
        let threads: Vec<_> = (0..2)
            .map(|t| {
                let hub = hub.clone();
                std::thread::spawn(move || {
                    for i in 0..per {
                        let body = format!("seq-t{t}-{i:05}");
                        let n = publish(
                            &hub,
                            "eve_public.invented.notice.Echo",
                            body.as_bytes(),
                            Target::Character(who as i64),
                        );
                        assert_eq!(n, 1);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut next = [0usize; 2];
        let mut seen = 0;
        while seen < 2 * per {
            let b = rx.recv_timeout(Duration::from_secs(10)).expect("a notice");
            let text = String::from_utf8_lossy(&b).into_owned();
            // The reply to the pilot's first request is not a notice.
            let Some(at) = text.find("seq-t") else {
                continue;
            };
            let t = usize::from(text.as_bytes()[at + 5] - b'0');
            let i: usize = text[at + 7..at + 12].parse().unwrap();
            assert_eq!(i, next[t], "thread {t} arrived out of order");
            next[t] += 1;
            seen += 1;
        }
    }

    #[test]
    fn a_closing_connection_whose_peer_reads_a_trickle_frees_its_slot_within_the_timeout() {
        let hub = NoticeHub::default();
        let timeout = Duration::from_millis(1_000);
        let live = Arc::new(AtomicUsize::new(0));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        {
            let (hub, live) = (hub.clone(), Arc::clone(&live));
            std::thread::spawn(move || {
                for sock in l.incoming().flatten() {
                    let (hub, live) = (hub.clone(), Arc::clone(&live));
                    live.fetch_add(1, Ordering::SeqCst);
                    std::thread::spawn(move || {
                        let _ = serve_queued(
                            sock,
                            [0; 16],
                            &mut Vec::new(),
                            &Ground::local(),
                            &hub,
                            (MAX_HELD_STREAM, MAX_HELD_CONN),
                            (MAX_STREAMS, timeout),
                            1 << 40,
                        );
                        live.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            });
        }
        let mut peer = big_window_consumer(addr, 7601);
        wait_for_pilots(&hub, 1);
        let out = Arc::clone(&hub.0.lock().unwrap()[0].conn.lock().unwrap().out);
        // Queue far more than the socket buffers hold, so the writer is
        // blocked with frames pending.
        let big = vec![7u8; 256 << 10];
        for _ in 0..60 {
            publish(
                &hub,
                "eve_public.invented.notice.Echo",
                &big,
                Target::Character(7601),
            );
        }
        assert!(out.lock().bytes > 0, "precondition: frames are pending");
        // The peer ends the connection, then reads a trickle.
        let mut reader = peer.try_clone().unwrap();
        peer.write_all(&frame(GOAWAY, 0, 0, &[0; 8])).unwrap();
        let begin = std::time::Instant::now();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut b = [0u8; 1024];
            while !stop.load(Ordering::Relaxed) {
                if reader.read(&mut b).unwrap_or(0) == 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        while live.load(Ordering::SeqCst) > 0 {
            assert!(
                begin.elapsed() < timeout * 5,
                "the slot was not freed in {:?}",
                begin.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // The slot is free only once the writer thread is gone: it held the
        // last other reference to the queue.
        let gone_at = begin.elapsed();
        done.store(true, Ordering::Relaxed);
        assert!(gone_at < timeout * 3, "freed after {gone_at:?}");
        assert_eq!(Arc::strong_count(&out), 1, "the writer outlived its slot");
    }

    #[test]
    fn a_pin_passed_to_another_process_unbinds_the_old_connection() {
        let (g, _w) = two_live_pilots();
        let hub = NoticeHub::default();
        let mut s = connected(&g, &hub);
        let ty = "eve_public.invented.notice.Thing";
        let m = crate::gateway_auth::test_request(1_001, None, b"process-one-0001");
        assert_eq!(send_and_read(&mut s, &m), 200);
        assert_eq!(publish(&hub, ty, b"a", Target::Character(1_001)), 1);
        hub.unbind(1_001, b"process-two-0002");
        assert_eq!(
            publish(&hub, ty, b"a", Target::Character(1_001)),
            1,
            "another process"
        );
        hub.unbind(1_002, b"process-one-0001");
        assert_eq!(
            publish(&hub, ty, b"a", Target::Character(1_001)),
            1,
            "another pilot"
        );
        hub.unbind(1_001, b"process-one-0001");
        assert_eq!(publish(&hub, ty, b"a", Target::Character(1_001)), 0);
    }
}
