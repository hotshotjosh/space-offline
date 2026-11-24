//! `pvpFilamentMgr`: the Proving Grounds calendar and its empty boards (#64,
//! PG-1b). The wire arms only; the rules are `crate::proving`.
//!
//! `JoinPVPQueue` answers the estimate 0 ("unknown"), or raises `PVPJumpError`
//! with every reason `proving::queue::join_errors` finds **[V]**
//! `pvpFilamentController.JoinQueue`. `LeavePVPQueue` answers `None` and never
//! raises: `LeaveQueue` does not catch an error, and `Close()` calls it
//! **[V]**.

use super::*;
use crate::proving::{self, Catalogue};

fn cat(reg: &Registry) -> &Catalogue {
    reg.content().get::<Catalogue>()
}

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = proving::SERVICES;

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let ar = args::Args::new(req);
    match req.method {
        "GetAllEvents" => Ok(proving::all_events(cat(reg), now)),
        "GetActiveEvents" => Ok(proving::active_events(cat(reg), now)),
        "GetMostRecentEvent" => Ok(proving::most_recent_event(cat(reg), now)),
        "GetNextEventDate" => Ok(proving::next_event_date(cat(reg), now)),
        // The answer is ignored by the client; it waits for the push.
        "GetLeaderboard" | "GetCharacterStatistics" => {
            let (Some(m), Some(s)) = (ar.int(0), ar.int(1)) else {
                return Err(ar.expected("(matchTypeID, scheduleID)"));
            };
            if req.method == "GetLeaderboard" {
                proving::push_leaderboard(reg, now, m, s, req.actor);
            } else {
                proving::push_statistics(reg, now, m, s, req.actor);
            }
            Ok(PyValue::None)
        }
        "JoinPVPQueue" => {
            let (Some(item), Some(type_id)) = (ar.int(0), ar.int(1)) else {
                return Err(ar.expected("(itemID, typeID)"));
            };
            let Some(me) = req.actor else {
                return Err(ar.expected("a character"));
            };
            match proving::queue::join_slot(reg, me, item, type_id, now) {
                Ok(estimate) => Ok(PyValue::Int(estimate)),
                Err(errors) => Err(proving::queue::raise(&errors)),
            }
        }
        "LeavePVPQueue" => {
            if let Some(me) = req.actor {
                proving::queue::leave_slot(reg, me);
            }
            Ok(PyValue::None)
        }
        // #65: the conduit of a decided match sends its pilot home.
        "AbyssalPVPEndGateActivation" => proving::arena::end_gate(reg, req),
        _ => Err(reg.no_method(req)),
    }
}
