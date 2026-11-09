//! `crateService`: opening a crate (lane l6a-geodes, issue #32).
//!
//! The client's crate window (`eve/client/script/ui/crate/controller.py`)
//! makes these calls [V]:
//!
//! - `GetCrateLocation(crateID)` -> `(destinationID, destinationFlagID)`,
//!   where the prizes are claimed to;
//! - `OpenCrate(crateID)` -> `[(itemID, typeID, quantity), ...]`, which
//!   `LoadLootFromTypes` turns into the reveal (the loot is shuffled client
//!   side); each prize is then claimed with `ClaimLoot(itemID, destinationID,
//!   destinationFlagID)` when the window closes;
//! - `ClaimLootFromCrateStack(crateID, qty)` -> `(opened, wanted)`, a whole
//!   stack at once (`MAX_MULTI_OPEN` 30);
//! - `PeekInsideFixedCrate(crateID)` -> `[(typeID, quantity), ...]`;
//! - `ClaimSkillBundle` and `ClaimFitting` for the special loot of a fixed
//!   crate.
//!
//! **What the core decides** is CCP's: a crate opens only while the pilot is
//! docked (the prizes go to the station hangar, and a pilot in space gets a
//! `CustomNotify`), a crate not the pilot's own is refused, and one open
//! consumes exactly one crate, and only when the row made a prize: a row that
//! returns no loot leaves the crate where it is. **What is inside** is a module's
//! ([`crate::crate_hooks`]); the core has no rows, so every crate is
//! "not a crate" here and these calls then answer as an unknown item does.
//!
//! **The prizes are real items in the hangar already when `OpenCrate`
//! answers** [A], so `ClaimLoot` has nothing to do and answers `None` (a
//! prize paid to the vault has item id 0). The reveal still plays from the
//! list. Falsifier: a client that raises on `None` from `ClaimLoot`, or a
//! prize that shows twice in the hangar.
//!
//! `GetCrateLocation` fails before `OpenCrate` is sent, so a crate opened in
//! space costs nothing.

use super::*;
use crate::crate_hooks::{Loot, OpenEvent, Row};
use crate::service::args::Args;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["crateService"];

/// A refusal the player reads (`CustomNotify`) **[A]**. Falsified by an empty
/// or garbled dialog.
fn notify(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(msg.to_owned()))]),
    ))
}

/// The row that owns crate type `type_id`, if any of `rows` claims it.
fn row_of(reg: &Registry, rows: &[Row], type_id: i64) -> Option<Row> {
    rows.iter().find(|r| (r.is_crate)(reg, type_id)).copied()
}

/// A crate the actor owns, and the row that opens it.
fn own_crate(
    reg: &Registry,
    rows: &[Row],
    req: &Request<'_>,
) -> Result<(crate::item::Item, Row), ServiceError> {
    let (Some(actor), Some(id)) = (req.actor, Args::new(req).int(0)) else {
        return Err(notify("There is no crate to open."));
    };
    let item = reg
        .store()
        .get(id)
        .filter(|i| i.owner_id == actor)
        .cloned()
        .ok_or_else(|| notify("There is no crate to open."))?;
    let row = row_of(reg, rows, item.type_id).ok_or_else(|| notify("That is not a crate."))?;
    Ok((item, row))
}

/// The station a crate may be opened in: the one the pilot is docked in.
fn dock(reg: &Registry, actor: i64) -> Result<i64, ServiceError> {
    reg.docked_station(actor)
        .ok_or_else(|| notify("A crate can only be opened while docked in a station."))
}

fn tuple_of(l: &Loot) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(l.item_id),
        PyValue::Int(l.type_id),
        PyValue::Int(l.quantity),
    ])
}

/// Open one crate of the stack `item_id`: let the module's row make the
/// prizes, then take the one crate. The stack is read afresh here (never a
/// copy from an earlier pass), and the crate is taken **only if the row made
/// something**: a row that returns no loot is saying "this cannot pay", the
/// crate stays exactly where it is and the answer is an empty list. `None`
/// when the stack is gone or could not give one up.
fn open_one(
    reg: &mut Registry,
    row: &Row,
    item_id: i64,
    station: i64,
    nth: u64,
) -> Option<Vec<Loot>> {
    let item = reg.store().get(item_id).cloned()?;
    let ev = OpenEvent {
        character: item.owner_id,
        crate_type: item.type_id,
        crate_item: item.item_id,
        station,
        nth,
        when: crate::pyutil::filetime_now(),
    };
    let loot = (row.open)(reg, &ev);
    if loot.is_empty() {
        return Some(loot);
    }
    let take = if item.quantity > 1 && !item.singleton {
        crate::item::Edit::Consume {
            item_id: item.item_id,
            quantity: 1,
        }
    } else {
        crate::item::Edit::Destroy {
            item_id: item.item_id,
        }
    };
    if reg.apply_items(&[take]).is_err() {
        // The row already made its prizes; the crate could not be taken, so
        // the pilot still holds it and can open it again.
        crate::outln!(
            "[crate] LOST: crate {} (type {}) of character {} could not be taken after {} prize(s) were made",
            item.item_id,
            item.type_id,
            item.owner_id,
            loot.len()
        );
        return None;
    }
    Some(loot)
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    dispatch_with(reg, req, &crate::module::plan().crates)
}

/// [`dispatch`] over `rows`, which a test chooses (the plan is process-wide).
fn dispatch_with(
    reg: &mut Registry,
    req: &Request<'_>,
    rows: &[Row],
) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetCrateLocation" => {
            let (item, _) = own_crate(reg, rows, req)?;
            let station = dock(reg, item.owner_id)?;
            Ok(PyValue::Tuple(vec![
                PyValue::Int(station),
                PyValue::Int(crate::model::FLAG_HANGAR),
            ]))
        }
        "OpenCrate" => {
            let (item, row) = own_crate(reg, rows, req)?;
            let station = dock(reg, item.owner_id)?;
            let loot = open_one(reg, &row, item.item_id, station, 0)
                .ok_or_else(|| notify("That crate could not be opened."))?;
            // The client lowers its stack count when it sends OpenCrate and
            // restores it only on an exception, so a kept crate must raise.
            if loot.is_empty() {
                return Err(notify("Nothing in it can be paid out right now."));
            }
            Ok(PyValue::List(loot.iter().map(tuple_of).collect()))
        }
        "ClaimLootFromCrateStack" => {
            let (item, row) = own_crate(reg, rows, req)?;
            let station = dock(reg, item.owner_id)?;
            let want = Args::new(req)
                .int(1)
                .unwrap_or(1)
                .clamp(1, item.quantity.max(1))
                .min(30);
            let mut opened = 0;
            for nth in 0..want {
                // An open that pays nothing leaves its crate; the rest of the
                // stack would draw from the same empty pools, so stop.
                match open_one(reg, &row, item.item_id, station, nth as u64) {
                    Some(loot) if !loot.is_empty() => opened += 1,
                    _ => break,
                }
            }
            Ok(PyValue::Tuple(vec![
                PyValue::Int(opened),
                PyValue::Int(want),
            ]))
        }
        // A fixed crate's contents are shown before it opens; ours are
        // random, so there is nothing to show.
        "PeekInsideFixedCrate" => Ok(PyValue::List(Vec::new())),
        // The prizes are in the hangar already; special loot is not used.
        "ClaimLoot" | "ClaimSkillBundle" | "ClaimFitting" => Ok(PyValue::None),
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Edit, Slot};

    const ALICE: i64 = 1_000_001;

    /// Two stackable types the item table knows, whatever they are.
    fn types() -> (i64, i64) {
        let mut it = crate::item::SPAWNABLE.iter().filter(|t| !t.singleton);
        (
            it.next().expect("a stackable type").type_id,
            it.next().expect("another").type_id,
        )
    }

    fn is_crate(_: &Registry, t: i64) -> bool {
        t == types().0
    }

    /// Opens to two of the other type, at the station's hangar.
    fn open(reg: &mut Registry, ev: &OpenEvent) -> Vec<Loot> {
        let prize = types().1;
        let made = reg
            .apply_items(&[Edit::Create {
                type_id: prize,
                quantity: 2 + ev.nth as i64,
                to: Slot::new(ev.character, ev.station, crate::model::FLAG_HANGAR),
            }])
            .expect("created");
        let crate::item::Change::Created { item_id, .. } = made[0] else {
            panic!("{made:?}")
        };
        vec![Loot {
            item_id,
            type_id: prize,
            quantity: 2 + ev.nth as i64,
        }]
    }

    const ROW: Row = Row {
        name: "test_crates",
        is_crate,
        open,
    };

    fn world(stack: i64) -> (Registry, i64, i64) {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        });
        let station = reg.docked_station(ALICE).expect("docked");
        let made = reg
            .apply_items(&[Edit::Create {
                type_id: types().0,
                quantity: stack,
                to: Slot::new(ALICE, station, crate::model::FLAG_HANGAR),
            }])
            .expect("crate");
        let crate::item::Change::Created { item_id, .. } = made[0] else {
            panic!("{made:?}")
        };
        (reg, station, item_id)
    }

    fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "crateService",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        dispatch_with(reg, &req, &[ROW])
    }

    fn count(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ALICE && i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    #[test]
    fn a_docked_pilot_is_told_the_hangar_to_claim_to() {
        let (mut reg, station, id) = world(3);
        let r = call(&mut reg, "GetCrateLocation", &[PyValue::Int(id)]).expect("answers");
        assert_eq!(
            r,
            PyValue::Tuple(vec![
                PyValue::Int(station),
                PyValue::Int(crate::model::FLAG_HANGAR)
            ])
        );
    }

    #[test]
    fn a_crate_opens_only_docked_and_costs_nothing_in_space() {
        let (mut reg, _, id) = world(3);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        assert!(reg.docked_station(ALICE).is_none(), "in space");
        for m in ["GetCrateLocation", "OpenCrate"] {
            assert!(call(&mut reg, m, &[PyValue::Int(id)]).is_err(), "{m}");
        }
        assert_eq!(count(&reg, types().0), 3, "no crate was taken");
        assert_eq!(count(&reg, types().1), 0, "no prize was made");
    }

    #[test]
    fn opening_takes_one_crate_and_lists_the_prize() {
        let (mut reg, _, id) = world(3);
        let r = call(&mut reg, "OpenCrate", &[PyValue::Int(id)]).expect("opens");
        let PyValue::List(l) = r else { panic!("{r:?}") };
        assert_eq!(l.len(), 1);
        assert_eq!(count(&reg, types().0), 2);
        assert_eq!(count(&reg, types().1), 2);
        // The prize is already in the hangar: the claim has nothing to do.
        assert_eq!(
            call(&mut reg, "ClaimLoot", &[PyValue::Int(0)]).unwrap(),
            PyValue::None
        );
    }

    #[test]
    fn the_last_crate_leaves_no_stack_and_a_stranger_is_refused() {
        let (mut reg, _, id) = world(1);
        call(&mut reg, "OpenCrate", &[PyValue::Int(id)]).expect("opens");
        assert_eq!(count(&reg, types().0), 0);
        assert!(call(&mut reg, "OpenCrate", &[PyValue::Int(id)]).is_err());
        assert!(call(&mut reg, "OpenCrate", &[PyValue::Int(7)]).is_err());
    }

    #[test]
    fn a_stack_opens_at_once_up_to_the_stack() {
        let (mut reg, _, id) = world(4);
        let r = call(
            &mut reg,
            "ClaimLootFromCrateStack",
            &[PyValue::Int(id), PyValue::Int(30)],
        )
        .expect("opens");
        assert_eq!(r, PyValue::Tuple(vec![PyValue::Int(4), PyValue::Int(4)]));
        assert_eq!(count(&reg, types().0), 0);
        // Four opens, nth 0 to 3: 2 + 3 + 4 + 5.
        assert_eq!(count(&reg, types().1), 14);
    }

    /// A row that pays on even `nth` only.
    fn open_even(reg: &mut Registry, ev: &OpenEvent) -> Vec<Loot> {
        if ev.nth.is_multiple_of(2) {
            open(reg, ev)
        } else {
            Vec::new()
        }
    }

    fn call_row(reg: &mut Registry, row: Row, method: &str, args: &[PyValue]) -> PyValue {
        let req = Request {
            service: "crateService",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        dispatch_with(reg, &req, &[row]).expect("answers")
    }

    fn row_with(open: fn(&mut Registry, &OpenEvent) -> Vec<Loot>) -> Row {
        Row {
            name: "test_crates",
            is_crate,
            open,
        }
    }

    #[test]
    fn a_stack_whose_row_cannot_pay_is_left_whole() {
        fn none(_: &mut Registry, _: &OpenEvent) -> Vec<Loot> {
            Vec::new()
        }
        let (mut reg, _, id) = world(3);
        let r = call_row(
            &mut reg,
            row_with(none),
            "ClaimLootFromCrateStack",
            &[PyValue::Int(id), PyValue::Int(30)],
        );
        assert_eq!(r, PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(3)]));
        assert_eq!(count(&reg, types().0), 3);
        // A single open raises, so the client puts its stack count back.
        let args = [PyValue::Int(id)];
        let req = Request {
            service: "crateService",
            method: "OpenCrate",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        let err = dispatch_with(&mut reg, &req, &[row_with(none)]).expect_err("raises");
        assert!(matches!(err, ServiceError::Raise(_)), "{err:?}");
        assert_eq!(count(&reg, types().0), 3);
    }

    /// A misbehaving row that pays and also removes its own crate, so the
    /// core's take finds nothing: the open fails (and is logged) rather than
    /// reporting a clean open.
    #[test]
    fn a_take_that_fails_after_the_prizes_is_an_error() {
        fn eats(reg: &mut Registry, ev: &OpenEvent) -> Vec<Loot> {
            let loot = open(reg, ev);
            reg.apply_items(&[Edit::Destroy {
                item_id: ev.crate_item,
            }])
            .expect("destroyed");
            loot
        }
        let (mut reg, _, id) = world(1);
        let args = [PyValue::Int(id)];
        let req = Request {
            service: "crateService",
            method: "OpenCrate",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert!(dispatch_with(&mut reg, &req, &[row_with(eats)]).is_err());
        assert_eq!(count(&reg, types().1), 2, "the prizes were made");
    }

    #[test]
    fn a_stack_that_pays_then_cannot_keeps_the_rest() {
        // nth 0 pays (2 prizes), nth 1 cannot: one crate used, two remain.
        let (mut reg, _, id) = world(3);
        let r = call_row(
            &mut reg,
            row_with(open_even),
            "ClaimLootFromCrateStack",
            &[PyValue::Int(id), PyValue::Int(30)],
        );
        assert_eq!(r, PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(3)]));
        assert_eq!(count(&reg, types().0), 2);
        assert_eq!(count(&reg, types().1), 2);
    }

    #[test]
    fn with_no_module_nothing_is_a_crate() {
        let (mut reg, _, id) = world(1);
        let args = [PyValue::Int(id)];
        let req = Request {
            service: "crateService",
            method: "OpenCrate",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert!(dispatch(&mut reg, &req).is_err());
        assert_eq!(count(&reg, types().0), 1);
    }
}
