//! The `crimewatch` bound object's calls beyond the state and the security
//! status (which `service/space.rs` answers): the safety setting and duels.
//! Reached from `space.rs`'s `crimewatch` arm (coordinator seam), so the
//! service keeps one route.
//!
//! - `SetSafetyLevel(safetyLevel)` -- `crimewatchSvc.SetSafetyLevel`, the
//!   HUD's safety button; the reply is discarded and the client sets its
//!   own copy at once **[V]**. Stored ([`super::set_safety`]) and answered
//!   back by every `GetClientStates`.
//! - `StartDuelChallenge(charID)` and `RespondToDuelChallenge(fromCharID,
//!   expiryTime, accept)` -- [`super::duel`].

use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The methods this module answers.
pub const METHODS: &[&str] = &[
    "SetSafetyLevel",
    "StartDuelChallenge",
    "RespondToDuelChallenge",
];

fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    Args::new(req).bad(why)
}

/// `crimewatch.<method>` for the methods in [`METHODS`].
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let a = Args::new(req);
    match req.method {
        "SetSafetyLevel" => {
            let level = a
                .int_or_kw(0, "safetyLevel")
                .and_then(super::Safety::from_wire)
                .ok_or_else(|| bad(req, "safetyLevel is not 0, 1 or 2"))?;
            if let Some(me) = req.actor {
                super::set_safety(reg, me, level);
                crate::outln!("[crimewatch] {me} sets safety to {level:?}");
            }
            Ok(PyValue::None)
        }
        "StartDuelChallenge" => {
            let target = a
                .int_or_kw(0, "charID")
                .filter(|n| *n != 0)
                .ok_or_else(|| bad(req, "charID is not an id"))?;
            super::duel::start(reg, req.actor, target, now)
        }
        "RespondToDuelChallenge" => {
            let from = a
                .int_or_kw(0, "fromCharID")
                .filter(|n| *n != 0)
                .ok_or_else(|| bad(req, "fromCharID is not an id"))?;
            let expiry = a.int_or_kw(1, "expiryTime");
            let accept = a.flag_lax_or_kw(2, "accept");
            super::duel::respond(reg, req.actor, from, expiry, accept, now)
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}
