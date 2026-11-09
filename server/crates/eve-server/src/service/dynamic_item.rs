//! `dynamicItemService`: describing a mutated item (#69 MUTA-1).
//!
//! The client makes two calls [V `dogma/dynamicItemSvc.py:20-41`]:
//! `GetDynamicItemInfo(itemID)`, answered from the item's `dynitem` row
//! ([`crate::dynamic_item`]), and `CreateDynamicItem(mutatorItemID,
//! sourceItemID)`, which applies a mutaplasmid ([`crate::dynamic_item::create`],
//! #70 MUTA-2).

use super::*;
use crate::service::args::Args;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["dynamicItemService"];

/// A refusal the player reads (`CustomNotify`) **[A]**, the shape
/// `crateService` uses. Falsified by an empty or garbled dialog.
fn notify(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(msg.to_owned()))]),
    ))
}

const NOT_MUTATED: &str = "That item is not a mutated item.";

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // Any owner may ask: contracts show other pilots' escrowed items.
        "GetDynamicItemInfo" => {
            let item = Args::new(req)
                .int(0)
                .and_then(|id| reg.store().get(id))
                .filter(|i| {
                    crate::dynamic_item::row(reg, i.item_id).is_some()
                        || crate::dynamic_item::is_dynamic_item(reg, i)
                })
                .cloned()
                .ok_or_else(|| notify(NOT_MUTATED))?;
            if let Some(row) = crate::dynamic_item::row(reg, item.item_id) {
                return Ok(crate::dynamic_item::info_value(
                    reg.content(),
                    &row,
                    item.type_id,
                ));
            }
            // Never `None`: the client caches it and every caller then raises.
            crate::outln!("dynitem: no row for item {}", item.item_id);
            crate::dynamic_item::fallback_info(reg.content(), item.item_id, item.type_id)
                .ok_or_else(|| notify(NOT_MUTATED))
        }
        "CreateDynamicItem" => {
            let a = Args::new(req);
            let (Some(mutator), Some(source)) = (a.int(0), a.int(1)) else {
                return Err(a.expected("(mutatorItemID, sourceItemID)"));
            };
            crate::dynamic_item::create(reg, req.actor, mutator, source)
                .map(PyValue::Int)
                .map_err(|e| {
                    if let crate::dynamic_item::CreateError::Store(s) = &e {
                        crate::outln!("dynitem: create refused by the store: {s}");
                    }
                    notify(e.text())
                })
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}
