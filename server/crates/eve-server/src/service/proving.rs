//! `pvpFilamentMgr`: the Proving Grounds calendar and its empty boards (#64,
//! PG-1b). The wire arms only; the rules are `crate::proving`.
//!
//! `JoinPVPQueue` refuses with `PVPJumpError` code 33 until PG-1c puts the queue
//! behind it **[V]** `pvpFilamentController.JoinQueue` (it reads `PVPJumpError`
//! from the raised `UserError`). `LeavePVPQueue` answers `None`: `LeaveQueue`
//! does not catch an error, and `Close()` calls it **[V]**.

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
                proving::push_leaderboard(reg, now, m, s);
            } else {
                proving::push_statistics(reg, now, m, s);
            }
            Ok(PyValue::None)
        }
        "JoinPVPQueue" => Err(proving::refusal(proving::EVENT_NOT_ACTIVE)),
        "LeavePVPQueue" => Ok(PyValue::None),
        _ => Err(reg.no_method(req)),
    }
}
