//! **`raffleProxy`** -- the wire of HyperNet (#203, HN-1). Arms only: the
//! state, the rules and the wire values are [`crate::hypernet`]; the client
//! side is `raffles/client/raffle_service.py` and its callers **[V]**.

use super::args::{Args, as_int};
use super::*;
use crate::contracts::attr;
use crate::hypernet as hn;
use crate::pyutil::pydict;

pub(crate) const SERVICES: &[&str] = &["raffleProxy"];

fn raise(name: &str) -> ServiceError {
    ServiceError::Raise(hn::error_value(name, None))
}

fn integer(v: Option<&PyValue>) -> Option<i64> {
    as_int(v)
}

/// `RaffleCreationData`: a `0x23` object whose state dict holds the eleven
/// fields (read like `contracts.rs` reads `ContractStruct`).
fn creation(v: &PyValue) -> hn::Creation {
    let i = |name: &str| integer(attr(v, name)).unwrap_or(0);
    hn::Creation {
        owner_location_id: i("owner_location_id"),
        location_id: i("location_id"),
        solar_system_id: i("solar_system_id"),
        token_id: i("token_id"),
        item_id: i("item_id"),
        type_id: i("type_id"),
        ticket_count: i("ticket_count"),
        ticket_price: i("ticket_price"),
        restriction_id: integer(attr(v, "restriction_id")),
    }
}

/// `filters` and `constraints` of `FilteredGrab`; either may be `None`.
fn filter_of(filters: Option<&PyValue>, constraints: Option<&PyValue>) -> hn::Filter {
    let get = |from: Option<&PyValue>, name: &str| {
        let from = from?;
        crate::pyutil::dict_get(from, name.as_bytes()).and_then(|v| as_int(Some(v)))
    };
    hn::Filter {
        type_id: get(filters, "type_id"),
        group_id: get(filters, "group_id"),
        category_id: get(filters, "category_id"),
        solar_system_id: get(filters, "solar_system_id"),
        meta_group_id: get(filters, "meta_group_id"),
        blueprint_type: get(filters, "blueprint_type"),
        min_ticket_price: get(constraints, "min_ticket_price"),
        max_ticket_price: get(constraints, "max_ticket_price"),
        min_ticket_count: get(constraints, "min_ticket_count"),
        max_ticket_count: get(constraints, "max_ticket_count"),
    }
}

fn list(rs: &[hn::Raffle]) -> PyValue {
    PyValue::List(rs.iter().map(|r| hn::raffle_value(r, false)).collect())
}

/// The updated, primed raffle a purchase answers with.
fn after_buy(reg: &Registry, id: i64) -> Result<PyValue, ServiceError> {
    hn::get(reg, id, true)
        .map(|r| hn::raffle_value(&r, true))
        .ok_or_else(|| raise("RaffleNotFoundError"))
}

fn buy(reg: &mut Registry, me: i64, id: i64, pick: hn::Pick) -> Result<PyValue, ServiceError> {
    match hn::buy(reg, me, id, pick) {
        Ok(()) => after_buy(reg, id),
        Err(hn::BuyError::TicketUnavailable) => Err(raise("TicketUnavailableError")),
        Err(hn::BuyError::NotEnoughIsk) => Err(raise("NotEnoughISKError")),
    }
}

fn price_stats(s: &hn::Stats) -> PyValue {
    let real = |v: Option<f64>| v.map_or(PyValue::None, PyValue::Real);
    let (hmax, hmin, havg) = s
        .historic
        .map_or((None, None, None), |(a, b, c)| (Some(a), Some(b), Some(c)));
    let (amax, amin, aavg) = s
        .active
        .map_or((None, None, None), |(a, b, c)| (Some(a), Some(b), Some(c)));
    pydict(vec![
        ("historic_count", PyValue::Int(s.historic_count)),
        ("historic_max", real(hmax)),
        ("historic_min", real(hmin)),
        ("historic_average", real(havg)),
        ("active_count", PyValue::Int(s.active_count)),
        ("active_max", real(amax)),
        ("active_min", real(amin)),
        ("active_average", real(aavg)),
    ])
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    let Some(me) = req.actor else {
        return Err(a.bad("no character"));
    };
    let id = || a.int(0).ok_or_else(|| a.bad("(raffle_id,)"));
    match req.method {
        "AmIBanned" => Ok(PyValue::Bool(false)),
        "SubscribeToTickets" => {
            hn::subscribe_tickets(reg, me, true);
            Ok(PyValue::None)
        }
        "UnSubscribeFromTickets" => {
            hn::subscribe_tickets(reg, me, false);
            Ok(PyValue::None)
        }
        "SubscribeToRaffle" => {
            hn::subscribe_raffle(reg, me, id()?, true);
            Ok(PyValue::None)
        }
        "UnsubscribeFromRaffle" => {
            hn::subscribe_raffle(reg, me, id()?, false);
            Ok(PyValue::None)
        }
        "CreateRaffle" => {
            let Some(data) = a.get(0) else {
                return Err(a.bad("(creation_data,)"));
            };
            match hn::create(reg, me, &creation(data)) {
                Ok(raffle_id) => Ok(PyValue::Int(raffle_id)),
                Err(reason) => Err(ServiceError::Raise(hn::error_value(
                    "CreateError",
                    Some(reason.name()),
                ))),
            }
        }
        "GetCreatedParticipated" => {
            let s = hn::char_stats(reg, me);
            Ok(pydict(vec![
                ("raffles_participated", PyValue::Int(s.raffles_participated)),
                ("raffles_won", PyValue::Int(s.raffles_won)),
                ("finished_delivered", PyValue::Int(s.finished_delivered)),
                ("finished_undelivered", PyValue::Int(s.finished_undelivered)),
                ("finished_expired", PyValue::Int(s.finished_expired)),
                ("created_running", PyValue::Int(s.created_running)),
            ]))
        }
        "GetActiveHistoricPrices" => Ok(price_stats(&hn::stats(reg, id()?))),
        "Grab" => Ok(list(&hn::grab(reg, me, &hn::Filter::default(), 0))),
        "FilteredGrab" => {
            let size = a
                .int_kw("size")
                .and_then(|s| usize::try_from(s).ok())
                .unwrap_or(0);
            let f = filter_of(a.kw("filters"), a.kw("constraints"));
            Ok(list(&hn::grab(reg, me, &f, size)))
        }
        "GetMyRaffleHistory" => {
            let before = a.int_or_kw(0, "running_id");
            let (page, size) = hn::history(reg, me, before);
            Ok(PyValue::Tuple(vec![list(&page), PyValue::Int(size)]))
        }
        "GetMyActiveTickets" => Ok(PyValue::List(
            hn::my_active_tickets(reg, me)
                .iter()
                .map(hn::ticket_value)
                .collect(),
        )),
        "GetRaffle" => after_buy(reg, id()?),
        "BuyTicket" => {
            let (Some(rid), Some(number)) = (a.int(0), a.int(1)) else {
                return Err(a.bad("(raffle_id, ticket_number)"));
            };
            buy(reg, me, rid, hn::Pick::Number(number))
        }
        "BuyRandomTickets" => {
            let (Some(rid), Some(count)) = (a.int(0), a.int(1)) else {
                return Err(a.bad("(raffle_id, ticket_count)"));
            };
            buy(reg, me, rid, hn::Pick::Random(count))
        }
        "AwardItem" => match hn::award(reg, me, id()?) {
            Ok(()) => Ok(PyValue::None),
            Err(hn::Refused) => Err(raise("RafflesError")),
        },
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}
