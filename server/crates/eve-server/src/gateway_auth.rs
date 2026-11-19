//! Who the public gateway may serve a request for.
//!
//! A gateway request names its pilot itself (`authoritative_context.identity
//! .character`, see `gateway::request_character`), so the name is a claim, not
//! proof. The gateway therefore serves a pilot only while that pilot is
//! **selected in a live game session** on this server whose account owns the
//! character (without accounts, any live session of the pilot), and only to
//! the one client process that asked first. Anything else is a plain 403.
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
//! **What this does not do [A]:** the process pin is as strong as that uuid is
//! secret, and the uuid is sent in the clear on every request, so a peer who
//! can read the link (the gateway has no encryption) or who sends before the
//! real client does is still served. Anyone who can reach the gateway port and
//! knows an online pilot's id before the pilot's own client has asked is
//! served. Falsified by: the gateway carrying a credential the game login also
//! issues (then bind it here). See `docs/guides/remote-clients.md`.

use crate::gateway::{Ground, request_character, request_instance, request_user};

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

/// May the gateway answer `request`, and as whom? `None` is a 403.
///
/// - Pilot request (`character > 0`), server ground: a live session that may
///   play the pilot, and the client process pinned to it (below). Every
///   request on a server ground must name its process (`application_instance
///   _uuid`).
/// - Pilot below 0: refused.
/// - Request naming no pilot (the PLEX vault is a per-user request): the
///   account it claims must have a live session; the vault is then read for
///   that account, never for the sandbox's.
/// - A local ground (tests, tools) has no sessions and serves as asked.
///
/// **The pin [I]:** the first served request for a pilot fixes the client
/// process (`application_instance_uuid`, a uuid4 per client process) for as
/// long as the session that vouched for it lives; another process asking for
/// that pilot is refused. The real client sends right after it selects the
/// pilot, so it wins. Falsified by: a client that sends nothing for the pilot
/// before a peer does, or one that changes its uuid in a run.
pub fn check(g: &Ground, request: &[u8]) -> Option<Served> {
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
    if character == 0 {
        let user = request_user(request)?;
        let reg = crate::service::world_read_timed(w, "gateway_auth");
        let live = reg.sessions().entries().iter().any(|e| e.user_id == user);
        return live.then_some(Served {
            character: -user,
            bind: None,
        });
    }
    let mut reg = crate::service::world_write_timed(w, "gateway_auth");
    let reg = &mut *reg;
    let c = reg.character(character)?;
    let client = reg
        .sessions()
        .of_character(character)
        .iter()
        .filter(|e| {
            let caller = crate::service::Caller::Session {
                user_id: e.user_id,
                role: 0,
            };
            reg.may_play(caller, c)
        })
        .map(|e| e.client_id)
        .collect::<Vec<_>>();
    if client.is_empty() {
        return None;
    }
    let live = |reg: &crate::service::Registry, client: i64| {
        reg.sessions()
            .entries()
            .iter()
            .any(|e| e.client_id == client && e.character_id == Some(character))
    };
    let pin = reg
        .ext::<crate::gateway_out::GatewayOutbox>()
        .pins
        .get(&character)
        .cloned();
    match pin {
        Some((pc, pi)) if live(reg, pc) => {
            if pi != instance {
                return None;
            }
            Some(Served {
                character,
                bind: Some((character, pc)),
            })
        }
        _ => {
            let first = client[0];
            reg.ext_mut::<crate::gateway_out::GatewayOutbox>()
                .pins
                .insert(character, (first, instance));
            Some(Served {
                character,
                bind: Some((character, first)),
            })
        }
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
        check(g, &test_request(character, None, instance)).is_some()
    }

    #[test]
    fn served_for_its_own_live_character() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        let s = check(&g, &test_request(ALICE, None, P1)).unwrap();
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
        let s = check(&g, &test_request(ALICE, None, P2)).unwrap();
        assert_eq!(s.bind, Some((ALICE, 2)));
        assert!(!served(&g, ALICE, P1));
    }

    #[test]
    fn a_request_naming_no_pilot_is_served_as_its_live_account_only() {
        let mut reg = world();
        reg.session_opened(entry(1, ALICE_USER, Some(ALICE)));
        let (g, _) = ground(reg);
        let s = check(&g, &test_request(0, Some(ALICE_USER), P1)).unwrap();
        assert_eq!((s.character, s.bind), (-ALICE_USER, None));
        // An account with no live session, an unclaimed account, and a
        // pilotless request that claims no account at all.
        assert!(check(&g, &test_request(0, Some(BOB_USER), P1)).is_none());
        assert!(check(&g, &test_request(0, None, P1)).is_none());
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
        let s = check(&Ground::local(), &test_request(ALICE, None, b"")).unwrap();
        assert_eq!((s.character, s.bind), (ALICE, Some((ALICE, 0))));
    }
}
