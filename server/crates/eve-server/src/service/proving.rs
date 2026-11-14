//! `pvpFilamentMgr`: the Proving Grounds calendar and its empty boards (#64,
//! PG-1b). The wire arms only; the rules are `crate::proving`.
//!
//! `JoinPVPQueue` and `LeavePVPQueue` refuse with `PVPJumpError` code 33 until
//! PG-1c puts the queue behind them **[V]** `pvpFilamentSvc.RequestToJoinPVPQueue`
//! (the client reads `PVPJumpError` from the raised `UserError`).

use super::*;
use crate::proving::{self, Catalogue};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = proving::SERVICES;

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let ar = args::Args::new(req);
    let cat: Catalogue = reg.content().get::<Catalogue>().clone();
    match req.method {
        "GetAllEvents" => Ok(proving::all_events(&cat, now)),
        "GetActiveEvents" => Ok(proving::active_events(&cat, now)),
        "GetMostRecentEvent" => Ok(proving::most_recent_event(&cat, now)),
        "GetNextEventDate" => Ok(proving::next_event_date(&cat, now)),
        // The answer is ignored by the client; it waits for the push.
        "GetLeaderboard" | "GetCharacterStatistics" => {
            let (Some(m), Some(s)) = (ar.int(0), ar.int(1)) else {
                return Err(ar.expected("(matchTypeID, scheduleID)"));
            };
            if req.method == "GetLeaderboard" {
                proving::push_leaderboard(reg, &cat, now, m, s);
            } else {
                proving::push_statistics(reg, &cat, now, m, s);
            }
            Ok(PyValue::None)
        }
        "JoinPVPQueue" | "LeavePVPQueue" => Err(proving::refusal(proving::EVENT_NOT_ACTIVE)),
        _ => Err(reg.no_method(req)),
    }
}
