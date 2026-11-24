//! `moonExtractions` (issue #53, lane MOON-SCHEDULE): scheduling a moon
//! extraction from the controls of a drilling platform. The state and the rules
//! are [`crate::moon_extract`]; this is the arguments and the answers.
//!
//! The six methods and their arities are the client's own **[V]**
//! (`ui/moonmining/schedulingSvc.py`, `findings/client-calls.tsv`): every call
//! but `StartNewExtraction(structureID, durationSeconds, addToCalendar)` takes no
//! argument and means the structure the pilot has the controls of.
//!
//! - `GetExtractionAndEventIDFromStructureID()` answers `(keyval | None,
//!   eventID | None)`, which the client unpacks into two names.
//! - `GetExtractionsForCorp()` answers the owner corporation's extractions to a
//!   pilot with the station manager role, `[]` to anyone else **[I]** (the client
//!   asks only with the role, `schedulingSvc.py:38-40`).
//! - `GetMoonResourcesForStructure()` answers `{oreTypeID: fraction}` for the
//!   moon whose beacon is in range, or a refusal with the client's own key.
//! - `FractureChunkWithStructure()` is #54's: until then it refuses with
//!   `ExtractionNotAvailableForFracture` **[V key]**.

use super::super::args::Args;
use super::*;
use crate::moon_extract::{self as mx, Denied};
use crate::structure_dock::control;

pub(super) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetExtractionAndEventIDFromStructureID" => current(reg, req),
        "GetExtractionsForCorp" => for_corp(reg, req),
        "GetMoonResourcesForStructure" => resources(reg, req),
        "StartNewExtraction" => start(reg, req),
        "CancelExtraction" => cancel(reg, req),
        // #54 (MOON-FRACTURE) answers this; until then the chunk cannot be fired at.
        "FractureChunkWithStructure" => Err(Denied::NotYetFracturable.into_error()),
        other => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: other.to_owned(),
        }),
    }
}

fn current(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let found = req
        .actor
        .and_then(|a| control::controlling(reg, a))
        .and_then(|sid| mx::get(reg.deltas(), sid));
    Ok(match found {
        Some(e) => PyValue::Tuple(vec![
            mx::to_keyval(&e),
            e.calendar_event.map_or(PyValue::None, PyValue::Int),
        ]),
        None => PyValue::Tuple(vec![PyValue::None, PyValue::None]),
    })
}

fn for_corp(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (Some(actor), Some(corp)) = (req.actor, my_corp(reg, req.actor)) else {
        return Ok(PyValue::List(Vec::new()));
    };
    if !crate::service::sov_hub::may_launch(reg, actor) {
        return Ok(PyValue::List(Vec::new()));
    }
    Ok(PyValue::List(
        mx::of_corp(reg.deltas(), corp)
            .iter()
            .map(mx::to_keyval)
            .collect(),
    ))
}

fn resources(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let sid = req
        .actor
        .and_then(|a| control::controlling(reg, a))
        .ok_or_else(|| Denied::NotControlling.into_error())?;
    let s = structure::get(reg.deltas(), sid)
        .ok_or_else(|| Denied::NotDrillingPlatform.into_error())?;
    if !crate::structure_services::online_with(reg, sid, mx::SERVICE_MOONMINING) {
        return Err(Denied::ResourcesNeedDrill.into_error());
    }
    let moon = crate::moon_beacon::moon_in_range(reg, s.system, s.pos)
        .ok_or_else(|| Denied::ResourcesNeedBeacon.into_error())?;
    Ok(crate::moon_scan::results(
        &crate::moon_scan::makeup(reg, moon).unwrap_or_default(),
    ))
}

fn start(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    const SHAPE: &str = "StartNewExtraction(structureID, durationSeconds, addToCalendar)";
    let a = Args::new(req);
    let (Some(actor), Some(sid), Some(duration)) = (req.actor, a.int(0), a.int_real(1)) else {
        return Err(a.bad(SHAPE));
    };
    mx::start(
        reg,
        actor,
        sid,
        duration,
        a.flag_lax(2),
        crate::pyutil::filetime_now(),
    )
    .map_err(Denied::into_error)?;
    Ok(PyValue::None)
}

fn cancel(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(Args::new(req).bad("CancelExtraction()"));
    };
    mx::cancel(reg, actor, crate::pyutil::filetime_now()).map_err(Denied::into_error)?;
    Ok(PyValue::None)
}
