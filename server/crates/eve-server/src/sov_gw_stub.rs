//! SOV-GW-0: honest sovereignty gateway stubs. Writes nothing can honour
//! are refused (403) instead of answered with an empty 200 that the client
//! reads as success; skyhook `GetAllLocal` names the session system so the
//! client's local cache primes. Placed last of the sov arms in
//! `gateway_ext::CHAIN`: a later sov lane's arm sits above and takes its
//! types over. Contract: `docs/re/sov-gateway.md` §2, §8.

use crate::gateway::{Ground, Reply, put_bytes};
use crate::sov_proto::{id, status, typed};

const PKG: &str = "eve_public.sovereignty.";

/// Request types refused with 403, by name after [`PKG`].
const REFUSED: &[&str] = &[
    // The reagent bay "not allowed" path [V]: the 403 raises
    // `SovFuelAddRequestForbiddenError` (`fuel/client/external_messenger.py:43-44`),
    // the controller swallows it (`reagentBay/controllers.py:99-105`) and the
    // window stays open with the stack unmoved: no dialog, no hang
    // (`reagentBayWnd.py:190-209`).
    "hub.fuel.api.AddRequest",
    // The window's `SovHubChangeNotSaved` dialog [V].
    "hub.upgrade.api.ProcessConfigurationRequest",
    "hub.upgrade.api.UninstallRequest",
    // The client applies the transport visual only on success
    // (`sovHubSvc.py:99-103`) [V]. The 403 is not caught by
    // `quasarCallWrapper.py:66-73` (it catches the sibling
    // `SovWorkforceGetConfigError`): an uncaught traceback, no dialog, no false
    // state [V]. `ProcessConfiguration` shows `SovHubChangeNotSaved`, then
    // re-raises (`hubController.py:354-356`) [V]. A later scenario that runs
    // `no_new_tracebacks` around these saves must allow those tracebacks.
    "hub.workforce.api.ConfigureRequest",
    // The skyhook window's 403 branch [V].
    "skyhook.api.ActivateRequest",
    "skyhook.api.DeactivateRequest",
];

/// Admin (QA) namespaces, refused whole [V]: the client has no sender.
/// `admin_modify_reagents` (`skyhook/client/messenger.py:139`) has no caller
/// and there is no den admin messenger.
const ADMIN: &[&str] = &["skyhook.api.admin.", "mercenaryden.api.admin."];

/// `skyhook.api.GetAllLocalResponse` for a pilot in `system`: field 1 is the
/// system id, no skyhooks. The client primes its local cache only when
/// field 1 equals the session system (`data_sources.py:64`) [V].
pub(crate) fn get_all_local(system: u64) -> Reply {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &id(system));
    typed("eve_public.sovereignty.skyhook.api.GetAllLocalRequest", out)
}

/// The gateway arm. `None` for any type it does not own.
pub fn gateway(ty: &str, _body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    if REFUSED.contains(&rest) || ADMIN.iter().any(|p| rest.starts_with(p)) {
        return Some(status(403));
    }
    if rest == "skyhook.api.GetAllLocalRequest" {
        let system = match g {
            Ground::Server(w) => crate::service::world_read_timed(w, "sov_gw_stub")
                .session_system(character)
                .unwrap_or(0),
            Ground::Local(_) => 0,
        };
        return Some(get_all_local(system.max(0) as u64));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sov_proto::read_id;

    const PILOT: i64 = 7_001;
    const SYSTEM: u64 = 30_900_001;

    fn ask(ty: &str) -> Option<Reply> {
        gateway(ty, &[], PILOT, &Ground::local())
    }

    #[test]
    fn writes_nothing_can_honour_are_refused() {
        for rest in [
            "hub.fuel.api.AddRequest",
            "hub.upgrade.api.ProcessConfigurationRequest",
            "hub.upgrade.api.UninstallRequest",
            "hub.workforce.api.ConfigureRequest",
            "skyhook.api.ActivateRequest",
            "skyhook.api.DeactivateRequest",
            "skyhook.api.admin.ModifyReagentsRequest",
            "mercenaryden.api.admin.ModifyAnarchyLevelRequest",
        ] {
            let r = ask(&format!("{PKG}{rest}")).expect(rest);
            assert_eq!((r.status, r.payload), (403, None), "{rest}");
        }
    }

    #[test]
    fn the_chain_routes_a_refusal_to_this_arm() {
        let r = crate::gateway::answer(
            "eve_public.sovereignty.hub.fuel.api.AddRequest",
            &[],
            PILOT,
            &Ground::local(),
        );
        assert_eq!(r.status, 403);
    }

    #[test]
    fn get_all_local_reads_the_session_system_from_the_world() {
        let mut reg = crate::testkit::registry();
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                PILOT,
                1,
                SYSTEM as i64,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let g = Ground::Server(std::sync::Arc::new(std::sync::RwLock::new(reg)));
        let r = gateway(
            "eve_public.sovereignty.skyhook.api.GetAllLocalRequest",
            &[],
            PILOT,
            &g,
        )
        .unwrap();
        assert_eq!(read_id(&r.payload.unwrap().1, 1), Some(SYSTEM));
    }

    #[test]
    fn get_all_local_names_the_system_and_lists_nothing() {
        let r = get_all_local(SYSTEM);
        let (name, body) = r.payload.expect("payload");
        assert_eq!(r.status, 200);
        assert_eq!(
            name,
            "eve_public.sovereignty.skyhook.api.GetAllLocalResponse"
        );
        assert_eq!(read_id(&body, 1), Some(SYSTEM));
        assert!(crate::gateway::field(&body, 2).is_empty());
    }

    #[test]
    fn get_all_local_on_a_local_ground_answers_system_zero() {
        let r = ask("eve_public.sovereignty.skyhook.api.GetAllLocalRequest").unwrap();
        assert_eq!(read_id(&r.payload.unwrap().1, 1), Some(0));
    }

    #[test]
    fn other_types_fall_through() {
        assert!(ask("eve_public.sovereignty.hub.fuel.api.GetRequest").is_none());
        assert!(ask("eve_public.sovereignty.skyhook.api.GetRequest").is_none());
        assert!(ask("eve_public.sovereignty.mercenaryden.activity.api.GetAllRequest").is_none());
        assert!(ask("eve_public.cosmetic.ship.api.GetRequest").is_none());
    }
}
