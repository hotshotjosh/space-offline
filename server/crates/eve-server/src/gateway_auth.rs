//! Who the public gateway may serve a request for.
//!
//! A gateway request names its pilot itself (`authoritative_context.identity
//! .character`, see `gateway::request_character`), so the name is a claim, not
//! proof. The gateway therefore serves a pilot only while that pilot is
//! **selected in a live game session** on this server whose account owns the
//! character (without accounts, any live session of the pilot) and is the
//! account the request claims, only to a peer at the address that session's
//! game connection came from, and only to the one client process that asked
//! first. Anything else is a plain 403.
//!
//! **What the client sends [V]** (the current build's `out/pyc`, read with
//! `tools/pyc27.py`; the older text decompile `out/research/_w/src` agrees):
//! `requestBroker.pyc` `new_eve_public_request` (60-82) puts on every request
//! its `application_instance_uuid` (66), the claimed account
//! (`authenticated_user.sequential`, 74) and the selected pilot
//! (`identity.character.sequential`, 80). That uuid is a uuid4 made once per
//! client process (`publicGatewaySvc.pyc`, class body of `PublicGatewaySvc`;
//! handed to the broker in `_initialize_public_gateway`). The connection also
//! carries a JWT given to the native module (`publicGatewaySvc.pyc` `Run`
//! 47-59, `set_auth_token` at 56; `update_active_identity` 263-282). When the
//! launcher gave none, the client fabricates it (`setup_local_dev_token`,
//! 285-287): HS-signed with the literal key `dev`, subject `USER:EVE:<id>`
//! (`jwt/__init__.pyc` `fabricate_local_dev_user_token`, 401-417), so anyone
//! can write one and it ties nothing. This server never issues a token at
//! game login, and how the native module puts it on the wire is not in the
//! Python **[I]**, so nothing here depends on it.
//!
//! **The address [I]:** a pilot is also served only to a peer at the address
//! its game connection came from (recorded at session open, [`note_peer`]).
//! Two loopback addresses count as the same, so local play is unchanged, and
//! a global IPv6 address counts by its /64 (one host may use many addresses
//! in it), as an IPv4 NAT counts as one. Through the jail both connections
//! come from the jail side. **A tunnelled client must tunnel game and gateway
//! together:** a tunnel's connections reach the server from wherever the
//! tunnel ends (loopback for a host-run server; the container bridge's
//! gateway for a port that Docker publishes on loopback), so a game port
//! tunnelled and a gateway reached directly arrive from two addresses and
//! the gateway refuses the pilot. Two friends behind one NAT, or on one
//! tunnel, share an address, so the address alone does not tell them apart:
//! the account check and the process pin do. Falsified by: a setup where the
//! game and gateway connections of one client arrive from different
//! addresses (the `[gateway] peer mismatch` log line shows it).
//!
//! **What this does not do [A]:** the process pin is as strong as that uuid is
//! secret, and the uuid is sent in the clear on every request, so a peer at
//! the pilot's own address (the same NAT, the tunnel, the same host) who can
//! read the link (the gateway has no encryption) or who sends before the real
//! client does is still served. Falsified by: the gateway carrying a
//! credential the game login also issues (then bind it here). See
//! `docs/guides/remote-clients.md`.

use crate::gateway::{Ground, request_character, request_instance, request_user};
use crate::service::Registry;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long a pinned client process must have sent nothing, with no gateway
/// connection open, before another process may take the pin over. The new
/// one must pass every other check and claim the pilot's account. Without it,
/// a process that won the pin first and went away keeps the pilot's real
/// client refused for the whole session; a process that keeps a connection
/// open is never displaced. **[A]** 30 s. Falsified by: a real client whose
/// gateway connection drops and that is refused when it dials again because
/// another process at its address took its pilot in between (the real
/// client's own process never changes its uuid, so it is served again).
pub const PIN_IDLE: Duration = Duration::from_secs(30);

/// A pilot's pin: the game session that vouched for it, the client process
/// served, and when that process last asked (ms since the Unix epoch, the
/// wall clock). `last` is atomic so a served request under the read lock can
/// move it on.
#[derive(Debug)]
pub struct Pin {
    pub client: i64,
    pub instance: Vec<u8>,
    pub last: AtomicU64,
}

/// A request the gateway will answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// What `gateway::answer` is told the pilot is: the pilot, or for a
    /// request that names none, minus the account it claims (`<= 0` is "no
    /// pilot" to every arm; only the PLEX vault reads the account, by this).
    pub character: i64,
    /// The pilot and the game session that vouched for it, for binding the
    /// connection to the pilot's notices. `None` for a request naming no pilot
    /// (a local ground binds with session 0: no session to end).
    pub bind: Option<(i64, i64)>,
}

/// The address class a peer counts as: the address itself (IPv4-mapped
/// addresses as IPv4), or for a non-loopback IPv6 address its /64.
pub(crate) fn peer_class(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) if !v6.is_loopback() => IpAddr::V6(std::net::Ipv6Addr::from(
            u128::from(v6) & !u128::from(u64::MAX),
        )),
        other => other,
    }
}

/// Count a gateway connection of client process `instance` as open; see
/// [`connection_closed`]. A connection counts each process it served once.
pub(crate) fn connection_opened(g: &Ground, instance: &[u8]) {
    if let Ground::Server(w) = g {
        let reg = crate::service::world_read_timed(w, "gateway_auth");
        let out = reg.ext::<crate::gateway_out::GatewayOutbox>();
        let mut open = out.open.lock().unwrap_or_else(|e| e.into_inner());
        *open.entry(instance.to_vec()).or_insert(0) += 1;
    }
}

/// One gateway connection that served `instance` has ended.
pub(crate) fn connection_closed(g: &Ground, instance: &[u8]) {
    if let Ground::Server(w) = g {
        let reg = crate::service::world_read_timed(w, "gateway_auth");
        let out = reg.ext::<crate::gateway_out::GatewayOutbox>();
        let mut open = out.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = open.get_mut(instance) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(instance);
            }
        }
    }
}

/// Record where game session `client_id` connected from. Under the world
/// write lock the session open already holds. Drops the records of sessions
/// that have ended on the way, so the map holds live sessions only (client
/// ids are never reused, so a stale record could not be matched anyway).
pub fn note_peer(reg: &mut Registry, client_id: i64, peer: IpAddr) {
    let live: std::collections::HashSet<i64> = reg
        .sessions()
        .entries()
        .iter()
        .map(|e| e.client_id)
        .collect();
    let peers = &mut reg.ext_mut::<crate::gateway_out::GatewayOutbox>().peers;
    peers.retain(|c, _| live.contains(c));
    peers.insert(client_id, peer_class(peer));
}

/// Does a gateway peer at `peer` match a game session recorded at
/// `recorded`? No record (a session opened without one: tests, tools) keeps
/// the rule without addresses. A record but no gateway peer is refused. Two
/// loopback addresses match each other.
fn same_peer(recorded: Option<IpAddr>, peer: Option<IpAddr>) -> bool {
    match (recorded, peer) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => {
            let (a, b) = (peer_class(a), peer_class(b));
            a == b || (a.is_loopback() && b.is_loopback())
        }
    }
}

/// Now, in ms since the Unix epoch, on the wall clock (the idle limit is
/// real time, not game time).
fn now_ms() -> u64 {
    crate::game_clock::clock::wall_system()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// May the gateway answer `request` from `peer`, and as whom? `None` is a
/// 403. `peer` is the gateway connection's remote address; `None` when it is
/// not known (`gateway::respond`, for tests and tools).
///
/// - Pilot request (`character > 0`), server ground: a live session that may
///   play the pilot, whose account is the one the request claims (when it
///   claims one), whose game connection came from `peer`'s address, and the
///   client process pinned to it (below). Every request on a server ground
///   must name its process (`application_instance_uuid`).
/// - Pilot below 0: refused.
/// - Request naming no pilot (the PLEX vault is a per-user request): it must
///   come from the process pinned to a pilot of the account it claims, whose
///   session is live and whose game address matches; the vault is then read
///   for that account, never for the sandbox's.
/// - A local ground (tests, tools) has no sessions and serves as asked.
///
/// **The pin [I]:** the first served request for a pilot fixes the client
/// process (`application_instance_uuid`, a uuid4 per client process) for as
/// long as the session that vouched for it lives; another process asking for
/// that pilot is refused, unless the pinned one has been silent for
/// [`PIN_IDLE`] and the new one passes every other check. The real client
/// sends right after it selects the pilot, so it wins. Falsified by: a client
/// that sends nothing for the pilot before a peer does, or one that changes
/// its uuid in a run.
///
/// Every refusal is decided under the world **read** lock; the write lock is
/// taken only to set a pin, and the decision is made again under it.
pub fn check(g: &Ground, request: &[u8], peer: Option<IpAddr>) -> Option<Served> {
    check_at(g, request, peer, now_ms())
}

/// Under the write lock: pin `instance` to `character`. A pin it replaces had
/// another process; the connections still bound to that pilot through it
/// lose the binding, so the pilot's notices stop reaching them.
fn set_pin(reg: &mut Registry, character: i64, client: i64, instance: Vec<u8>, now: u64) {
    let pins = &mut reg.ext_mut::<crate::gateway_out::GatewayOutbox>().pins;
    let old = pins.insert(
        character,
        Pin {
            client,
            instance: instance.clone(),
            last: AtomicU64::new(now),
        },
    );
    if let Some(old) = old
        && old.instance != instance
    {
        crate::gateway_out::unbind(character, &old.instance);
    }
}

/// What [`decide`] found.
enum Decision {
    Refuse,
    Serve(Served),
    /// Serve after pinning `instance` to the pilot through session `client`.
    Pin {
        client: i64,
        served: Served,
    },
}

/// [`check`] at wall time `now` (ms since the Unix epoch).
pub(crate) fn check_at(
    g: &Ground,
    request: &[u8],
    peer: Option<IpAddr>,
    now: u64,
) -> Option<Served> {
    let character = request_character(request);
    let Ground::Server(w) = g else {
        return Some(Served {
            character,
            bind: (character > 0).then_some((character, 0)),
        });
    };
    if character < 0 {
        return None;
    }
    let instance = request_instance(request);
    if instance.is_empty() {
        return None;
    }
    let user = request_user(request);
    {
        let reg = crate::service::world_read_timed(w, "gateway_auth");
        match decide(&reg, character, user, &instance, peer, now, true) {
            Decision::Refuse => return None,
            Decision::Serve(s) => return Some(s),
            Decision::Pin { .. } => {}
        }
    }
    let mut reg = crate::service::world_write_timed(w, "gateway_auth");
    match decide(&reg, character, user, &instance, peer, now, false) {
        Decision::Refuse => None,
        Decision::Serve(s) => Some(s),
        Decision::Pin { client, served } => {
            set_pin(&mut reg, character, client, instance, now);
            Some(served)
        }
    }
}

/// The checks of [`check`], read only. A served request moves its pin's
/// `last` on. `log` says whether to log a refusal (the first pass only, so a
/// recheck under the write lock does not log twice).
fn decide(
    reg: &Registry,
    character: i64,
    user: Option<i64>,
    instance: &[u8],
    peer: Option<IpAddr>,
    now: u64,
    log: bool,
) -> Decision {
    let out = reg.ext::<crate::gateway_out::GatewayOutbox>();
    let peer_of = |client: i64| out.peers.get(&client).copied();
    let live = |client: i64, pilot: i64| {
        reg.sessions()
            .entries()
            .iter()
            .find(|e| e.client_id == client && e.character_id == Some(pilot))
    };
    if character == 0 {
        let Some(user) = user else {
            return Decision::Refuse;
        };
        let pinned = out.pins.iter().find(|(pilot, pin)| {
            pin.instance == instance
                && live(pin.client, **pilot).is_some_and(|e| e.user_id == user)
                && same_peer(peer_of(pin.client), peer)
        });
        return match pinned {
            Some((_, pin)) => {
                pin.last.fetch_max(now, Ordering::Relaxed);
                Decision::Serve(Served {
                    character: -user,
                    bind: None,
                })
            }
            None => Decision::Refuse,
        };
    }
    let Some(c) = reg.character(character) else {
        return Decision::Refuse;
    };
    let playing = reg
        .sessions()
        .of_character(character)
        .iter()
        .filter(|e| {
            let caller = crate::service::Caller::Session {
                user_id: e.user_id,
                role: 0,
            };
            reg.may_play(caller, c) && user.is_none_or(|u| u == e.user_id)
        })
        .map(|e| e.client_id)
        .collect::<Vec<_>>();
    let client = playing
        .iter()
        .copied()
        .filter(|&e| same_peer(peer_of(e), peer))
        .collect::<Vec<_>>();
    if client.is_empty() {
        if log && !playing.is_empty() {
            log_mismatch(character, now);
        }
        return Decision::Refuse;
    }
    let served = |client: i64| Served {
        character,
        bind: Some((character, client)),
    };
    match out.pins.get(&character) {
        Some(pin) if live(pin.client, character).is_some() => {
            if pin.instance == instance {
                if !client.contains(&pin.client) {
                    return Decision::Refuse;
                }
                pin.last.fetch_max(now, Ordering::Relaxed);
                return Decision::Serve(served(pin.client));
            }
            // Another process. It may take the pin only when it claims the
            // pilot's account, and the pinned process has no gateway
            // connection open and has been silent for `PIN_IDLE`.
            let idle = now.saturating_sub(pin.last.load(Ordering::Relaxed));
            let open = out
                .open
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&pin.instance)
                .is_some_and(|&n| n > 0);
            if user.is_none() || open || u128::from(idle) < PIN_IDLE.as_millis() {
                return Decision::Refuse;
            }
            let at = if client.contains(&pin.client) {
                pin.client
            } else {
                client[0]
            };
            Decision::Pin {
                client: at,
                served: served(at),
            }
        }
        _ => Decision::Pin {
            client: client[0],
            served: served(client[0]),
        },
    }
}

/// One `[gateway] peer mismatch` line, ids only, at most one a second (a
/// peer that polls must not fill the log).
fn log_mismatch(character: i64, now: u64) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let sec = now / 1000;
    if LAST.swap(sec, Ordering::Relaxed) != sec {
        crate::outln!(
            "[gateway] peer mismatch: pilot {character} asked for from an address its game \
             connection did not come from; refused"
        );
    }
}

/// A request for tests: `character` (0: none), the account it claims, and its
/// client process.
#[cfg(test)]
pub(crate) fn test_request(character: i64, user: Option<i64>, instance: &[u8]) -> Vec<u8> {
    use crate::gateway::{put_bytes, put_varint};
    let uint = |field: u64, v: u64| {
        let mut o = Vec::new();
        put_varint(&mut o, field << 3);
        put_varint(&mut o, v);
        o
    };
    let mut any = Vec::new();
    put_bytes(
        &mut any,
        1,
        b"type.googleapis.com/eve_public.invented.thing.api.GetRequest",
    );
    let mut r = Vec::new();
    put_bytes(&mut r, 2, b"c1");
    put_bytes(&mut r, 3, &any);
    if !instance.is_empty() {
        put_bytes(&mut r, 5, instance);
    }
    let mut ctx = Vec::new();
    if let Some(u) = user {
        let z = ((u << 1) ^ (u >> 63)) as u64;
        put_bytes(&mut ctx, 5, &uint(1, z));
    }
    if character != 0 {
        let mut who = Vec::new();
        put_bytes(&mut who, 1, &uint(1, character as u64));
        put_bytes(&mut ctx, 10, &who);
    }
    put_bytes(&mut r, 6, &ctx);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_table::SessionEntry;
    use std::sync::{Arc, RwLock};

    const ALICE: i64 = 1_001;
    const BOB: i64 = 1_002;
    const CAROL: i64 = 1_003;
    const ALICE_USER: i64 = 11;
    const BOB_USER: i64 = 12;
    const P1: &[u8] = b"process-one-0001";
    const P2: &[u8] = b"process-two-0002";

    fn entry(client_id: i64, user_id: i64, character: Option<i64>) -> SessionEntry {
        SessionEntry {
            client_id,
            session_id: client_id,
            user_id,
            character_id: character,
        }
    }

    fn world() -> crate::service::Registry {
        crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: ALICE,
                owner_user_id: Some(ALICE_USER),
                ..Default::default()
            };
            let mut b = a.clone();
            b.character_id = BOB;
            b.ship_id = a.ship_id + 1;
            b.owner_user_id = Some(BOB_USER);
            let mut c = a.clone();
            c.character_id = CAROL;
            c.ship_id = a.ship_id + 2;
            c.owner_user_id = Some(13);
            cfg.characters = vec![a, b, c];
            cfg.catch_all = None;
        })
    }

    fn ground(reg: crate::service::Registry) -> (Ground, Arc<RwLock<crate::service::Registry>>) {
        let w = Arc::new(RwLock::new(reg));
        (Ground::Server(Arc::clone(&w)), w)
    }

    fn served(g: &Ground, character: i64, instance: &[u8]) -> bool {
        check(g, &test_request(character, None, instance), None).is_some()
    }

    /// TEST-NET-1 (RFC 5737) addresses and loopback.
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// An invented wall time, ms since the epoch.
    const T0: u64 = 5_000_000;

    fn idle() -> u64 {
        u64::try_from(PIN_IDLE.as_millis()).unwrap()
    }

    /// `check_at` for pilot `character`, claiming `user`, from `peer` at `now`.
    fn from(
        g: &Ground,
        character: i64,
        user: Option<i64>,
        instance: &[u8],
        peer: &str,
        now: u64,
    ) -> Option<Served> {
        check_at(
            g,
            &test_request(character, user, instance),
            Some(ip(peer)),
            now,
        )
    }

    #[test]
    fn served_for_its_own_live_character() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        let s = check(&g, &test_request(ALICE, None, P1), None).unwrap();
        assert_eq!((s.character, s.bind), (ALICE, Some((ALICE, 1))));
    }

    #[test]
    fn refused_for_another_accounts_character() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        reg.session_opened(entry(2, ALICE_USER, Some(BOB)));
        let (g, _) = ground(reg);
        assert!(served(&g, ALICE, P1));
        assert!(!served(&g, BOB, P1));
    }

    #[test]
    fn refused_for_a_character_with_no_live_session() {
        let mut reg = world();
        reg.session_opened(entry(1, 13, None));
        let (g, _) = ground(reg);
        assert!(!served(&g, CAROL, P1));
        assert!(!served(&g, BOB, P1));
        assert!(!served(&g, 999_999, P1));
    }

    #[test]
    fn refused_after_logout() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, w) = ground(reg);
        assert!(served(&g, ALICE, P1));
        w.write().unwrap().session_closed(1);
        assert!(!served(&g, ALICE, P1));
    }

    #[test]
    fn a_negative_pilot_and_a_missing_process_are_refused() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        assert!(!served(&g, -ALICE, P1));
        assert!(!served(&g, ALICE, b""));
    }

    #[test]
    fn a_pilot_is_pinned_to_the_first_process_until_its_session_ends() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, w) = ground(reg);
        assert!(served(&g, ALICE, P1));
        assert!(
            served(&g, ALICE, P1),
            "the pinned process keeps being served"
        );
        assert!(
            !served(&g, ALICE, P2),
            "another process for that pilot is refused"
        );
        // The session ends and the pilot logs in again: the pin is void.
        {
            let mut r = w.write().unwrap();
            r.session_closed(1);
            r.session_opened(entry(2, ALICE_USER, Some(ALICE)));
        }
        let s = check(&g, &test_request(ALICE, None, P2), None).unwrap();
        assert_eq!(s.bind, Some((ALICE, 2)));
        assert!(!served(&g, ALICE, P1));
    }

    #[test]
    fn a_request_naming_no_pilot_is_served_as_its_live_account_only() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        // The process is pinned to the account's pilot first.
        assert!(served(&g, ALICE, P1));
        let s = check(&g, &test_request(0, Some(ALICE_USER), P1), None).unwrap();
        assert_eq!((s.character, s.bind), (-ALICE_USER, None));
        // An account with no live session, an unclaimed account, and a
        // pilotless request that claims no account at all.
        assert!(check(&g, &test_request(0, Some(BOB_USER), P1), None).is_none());
        assert!(check(&g, &test_request(0, None, P1), None).is_none());
    }

    #[test]
    fn a_pilotless_request_needs_the_accounts_pinned_process() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        reg.session_opened(entry(2, BOB_USER, Some(BOB)));
        let (g, _) = ground(reg);
        let vault = |instance: &[u8], user: i64| {
            check(&g, &test_request(0, Some(user), instance), None).is_some()
        };
        assert!(!vault(P1, ALICE_USER), "no pilot request yet: no pin");
        assert!(served(&g, ALICE, P1));
        assert!(vault(P1, ALICE_USER));
        assert!(!vault(P2, ALICE_USER), "another process of the account");
        assert!(served(&g, BOB, P2));
        assert!(!vault(P2, ALICE_USER), "another account's pinned process");
        assert!(!vault(P1, BOB_USER), "another account's pinned process");
        assert!(vault(P2, BOB_USER));
    }

    #[test]
    fn a_pilotless_request_from_another_address_is_refused() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_some());
        assert!(from(&g, 0, Some(ALICE_USER), P1, "192.0.2.2", T0).is_none());
        let s = from(&g, 0, Some(ALICE_USER), P1, "192.0.2.1", T0).unwrap();
        assert_eq!((s.character, s.bind), (-ALICE_USER, None));
    }

    #[test]
    fn a_pilot_is_not_served_to_another_address() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P2, "192.0.2.2", T0).is_none());
        assert!(from(&g, ALICE, None, P2, "127.0.0.1", T0).is_none());
        // The refusal pinned nothing: the pilot's own process is served.
        let s = from(&g, ALICE, None, P1, "192.0.2.1", T0).unwrap();
        assert_eq!(s.bind, Some((ALICE, 1)));
        // The pinned process from another address is refused too.
        assert!(from(&g, ALICE, None, P1, "192.0.2.2", T0).is_none());
        // And a known record with no gateway peer to compare is refused.
        assert!(!served(&g, ALICE, P1));
    }

    #[test]
    fn loopback_peers_are_not_told_apart() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("127.0.0.1"));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "127.0.0.1", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "127.0.0.2", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "::1", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "::ffff:127.0.0.1", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_none());
    }

    #[test]
    fn a_mapped_address_is_its_ipv4_address() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("::ffff:192.0.2.1"));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_some());
    }

    #[test]
    fn no_peer_recorded_keeps_todays_rule() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "192.0.2.2", T0).is_some());
        assert!(from(&g, ALICE, None, P2, "192.0.2.2", T0).is_none());
    }

    #[test]
    fn a_pilot_request_claiming_another_account_is_refused() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        assert!(check(&g, &test_request(ALICE, Some(BOB_USER), P1), None).is_none());
        assert!(check(&g, &test_request(ALICE, Some(ALICE_USER), P1), None).is_some());
    }

    #[test]
    fn two_pilots_behind_one_address_are_told_apart() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        reg.session_opened(entry(2, BOB_USER, Some(BOB)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        note_peer(&mut reg, 2, ip("192.0.2.1"));
        let (g, _) = ground(reg);
        let s = from(&g, ALICE, Some(ALICE_USER), P1, "192.0.2.1", T0).unwrap();
        assert_eq!(s.bind, Some((ALICE, 1)));
        let s = from(&g, BOB, Some(BOB_USER), P2, "192.0.2.1", T0).unwrap();
        assert_eq!(s.bind, Some((BOB, 2)));
        // Each one's process and account stay its own.
        assert!(from(&g, BOB, Some(BOB_USER), P1, "192.0.2.1", T0).is_none());
        assert!(from(&g, BOB, Some(ALICE_USER), P2, "192.0.2.1", T0).is_none());
    }

    #[test]
    fn an_idle_pin_passes_to_the_pilots_own_address_only() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        let (g, _) = ground(reg);
        let me = Some(ALICE_USER);
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_some());
        assert!(from(&g, ALICE, me, P2, "192.0.2.1", T0 + 1).is_none());
        // P1 asks again just before the limit: the pin is busy again.
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0 + idle() - 1).is_some());
        assert!(from(&g, ALICE, me, P2, "192.0.2.1", T0 + idle()).is_none());
        // Silent for the whole limit: another address still may not take it,
        // and neither may a request that claims no account.
        let late = T0 + 2 * idle();
        assert!(from(&g, ALICE, me, P2, "192.0.2.2", late).is_none());
        assert!(from(&g, ALICE, None, P2, "192.0.2.1", late).is_none());
        // The pilot's own address, claiming its account, may; the old process
        // is then refused.
        let s = from(&g, ALICE, me, P2, "192.0.2.1", late).unwrap();
        assert_eq!(s.bind, Some((ALICE, 1)));
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", late + 1).is_none());
        assert!(from(&g, ALICE, None, P2, "192.0.2.1", late + 1).is_some());
    }

    #[test]
    fn a_process_with_a_gateway_connection_open_keeps_its_pin() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        let (g, _) = ground(reg);
        let me = Some(ALICE_USER);
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_some());
        connection_opened(&g, P1);
        connection_opened(&g, P1);
        let late = T0 + 2 * idle();
        assert!(from(&g, ALICE, me, P2, "192.0.2.1", late).is_none());
        connection_closed(&g, P1);
        assert!(
            from(&g, ALICE, me, P2, "192.0.2.1", late).is_none(),
            "one still open"
        );
        connection_closed(&g, P1);
        assert!(from(&g, ALICE, me, P2, "192.0.2.1", late).is_some());
    }

    #[test]
    fn an_older_request_does_not_move_a_pins_last_request_back() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        let req = |inst: &[u8], user: Option<i64>, now: u64| {
            check_at(&g, &test_request(ALICE, user, inst), None, now).is_some()
        };
        assert!(req(P1, None, T0 + idle()));
        assert!(req(P1, None, T0), "served; its older time is not kept");
        assert!(!req(P2, Some(ALICE_USER), T0 + 2 * idle() - 1));
        assert!(req(P2, Some(ALICE_USER), T0 + 2 * idle()));
    }

    #[test]
    fn a_global_ipv6_address_counts_by_its_64() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("2001:db8::1"));
        let (g, _) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "2001:db8::2", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "2001:db8:0:0:ffff::9", T0).is_some());
        assert!(from(&g, ALICE, None, P1, "2001:db8:0:1::1", T0).is_none());
        assert_eq!(peer_class(ip("::1")), ip("::1"));
        assert_eq!(peer_class(ip("::ffff:192.0.2.1")), ip("192.0.2.1"));
        assert_eq!(peer_class(ip("2001:db8::7")), ip("2001:db8::"));
    }

    #[test]
    fn a_peer_record_of_an_ended_session_is_dropped() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        reg.session_closed(1);
        reg.session_opened(entry(2, BOB_USER, Some(BOB)));
        note_peer(&mut reg, 2, ip("192.0.2.2"));
        let peers = &reg.ext::<crate::gateway_out::GatewayOutbox>().peers;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers.get(&2), Some(&ip("192.0.2.2")));
    }

    /// A refusal, and a request for an already pinned pilot, are decided
    /// under the read lock: with another reader holding the world, both
    /// return. If either took the write lock it would wait for the reader;
    /// the test then lets go and fails, so nothing hangs.
    #[test]
    fn a_refused_request_does_not_take_the_world_write_lock() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        note_peer(&mut reg, 1, ip("192.0.2.1"));
        let (g, w) = ground(reg);
        assert!(from(&g, ALICE, None, P1, "192.0.2.1", T0).is_some());
        let reader = w.read().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let g2 = g.clone();
        let t = std::thread::spawn(move || {
            let r = [
                from(&g2, ALICE, None, P2, "192.0.2.2", T0).is_some(),
                from(&g2, ALICE, None, P2, "192.0.2.1", T0).is_some(),
                from(&g2, BOB, None, P1, "192.0.2.1", T0).is_some(),
                from(&g2, ALICE, Some(BOB_USER), P1, "192.0.2.1", T0).is_some(),
                from(&g2, ALICE, None, P1, "192.0.2.1", T0).is_some(),
            ];
            let _ = tx.send(r);
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(10));
        drop(reader);
        t.join().unwrap();
        assert_eq!(got.ok(), Some([false, false, false, false, true]));
    }

    #[test]
    fn without_accounts_a_live_pilot_is_served_and_a_dead_one_is_not() {
        let mut reg = crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![crate::model::Character {
                character_id: ALICE,
                ..Default::default()
            }];
            cfg.catch_all = Some(1);
        });
        reg.session_opened(entry(1, 1, Some(ALICE)));
        let (g, w) = ground(reg);
        assert!(served(&g, ALICE, P1));
        w.write().unwrap().session_closed(1);
        assert!(!served(&g, ALICE, P1));
    }

    #[test]
    fn a_local_ground_serves_as_asked() {
        let s = check(&Ground::local(), &test_request(ALICE, None, b""), None).unwrap();
        assert_eq!((s.character, s.bind), (ALICE, Some((ALICE, 0))));
    }
}
