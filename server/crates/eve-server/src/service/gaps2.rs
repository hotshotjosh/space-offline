//! **The wire arms of GAPS-2** (X-GAPS items 1-6; lane note
//! `docs/lanes/gaps-2.md`): calls a player hits that went unanswered, each
//! delegated here from the fall-through of the service that owns the name.
//!
//! One function per service. Each returns `None` for a method that is not
//! this lane's, so the caller's own `no_method` stays the answer for
//! everything else. The models live in crate-root modules
//! ([`crate::item_names`], [`crate::jettison`], [`crate::notes`],
//! [`crate::contact_labels`], [`crate::lp_transfer`],
//! [`crate::ship_clone`], [`crate::fleet_finder`]); this file only parses
//! the request and builds the `PyValue`.

use super::args::{self, Args};
use super::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

type Answer = Option<Result<PyValue, ServiceError>>;

/// Our words, for a refusal that is about this server rather than EVE.
fn custom_notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// An id or a list of ids, as `AssignLabels` sends a selection.
fn id_list(v: Option<&PyValue>) -> Vec<i64> {
    match args::as_int(v) {
        Some(n) => vec![n],
        None => args::as_int_list(v),
    }
}

/// `charMgr`: the notes, the notepad and the contact labels.
pub(crate) fn char_mgr(reg: &mut Registry, req: &Request<'_>) -> Answer {
    // Every call here is about the caller's own book: no character, no
    // answer worth giving.
    let known = [
        "GetNote",
        "SetNote",
        "GetOwnerNoteLabels",
        "GetOwnerNote",
        "AddOwnerNote",
        "EditOwnerNote",
        "RemoveOwnerNote",
        "GetLabels",
        "CreateLabel",
        "EditLabel",
        "DeleteLabel",
        "AssignLabels",
        "RemoveLabels",
    ];
    if !known.contains(&req.method) {
        return None;
    }
    let Some(me) = req.actor else {
        return Some(Err(Args::new(req).bad("no character")));
    };
    Some(match req.method {
        // `GetNote(ownerID)` -> the text (`panelNotes.Load:40`).
        "GetNote" => match Args::new(req).int(0) {
            Some(owner) => Ok(PyValue::Unicode(crate::notes::owner_note(reg, me, owner))),
            None => Err(Args::new(req).bad("(ownerID,)")),
        },
        "SetNote" => match (Args::new(req).int(0), Args::new(req).text(1)) {
            (Some(owner), Some(text)) => {
                crate::notes::set_owner_note(reg, me, owner, &text);
                Ok(PyValue::None)
            }
            _ => Err(Args::new(req).bad("(ownerID, text)")),
        },
        "GetOwnerNoteLabels" => Ok(crate::notes::labels_value(reg, me)),
        "GetOwnerNote" => match Args::new(req).int(0) {
            Some(id) => Ok(crate::notes::note_value(reg, me, id)),
            None => Err(Args::new(req).bad("(noteID,)")),
        },
        "AddOwnerNote" => match (Args::new(req).text(0), Args::new(req).text(1)) {
            (Some(label), Some(text)) => crate::notes::add(reg, me, &label, &text)
                .map(PyValue::Int)
                .ok_or_else(|| custom_notify("You have too many notes.")),
            _ => Err(Args::new(req).bad("(label, text)")),
        },
        // `EditOwnerNote(noteID, label[, text])`.
        "EditOwnerNote" => match (Args::new(req).int(0), Args::new(req).text(1)) {
            (Some(id), Some(label)) => {
                let text = Args::new(req).text(2);
                crate::notes::edit(reg, me, id, &label, text.as_deref());
                Ok(PyValue::None)
            }
            _ => Err(Args::new(req).bad("(noteID, label[, text])")),
        },
        "RemoveOwnerNote" => match Args::new(req).int(0) {
            Some(id) => {
                crate::notes::remove(reg, me, id);
                Ok(PyValue::None)
            }
            None => Err(Args::new(req).bad("(noteID,)")),
        },
        "GetLabels" => Ok(crate::contact_labels::labels_value(reg, me)),
        "CreateLabel" => match Args::new(req).text(0) {
            Some(name) => crate::contact_labels::create(reg, me, &name, Args::new(req).int(1))
                .map(PyValue::Int)
                .ok_or_else(|| custom_notify("You cannot make that label.")),
            None => Err(Args::new(req).bad("(name, color)")),
        },
        "EditLabel" => match Args::new(req).int(0) {
            Some(id) => {
                let name = Args::new(req).text(1);
                crate::contact_labels::edit(reg, me, id, name.as_deref(), Args::new(req).int(2));
                Ok(PyValue::None)
            }
            None => Err(Args::new(req).bad("(labelID, name, color)")),
        },
        "DeleteLabel" => match Args::new(req).int(0) {
            Some(id) => {
                crate::contact_labels::delete(reg, me, id);
                Ok(PyValue::None)
            }
            None => Err(Args::new(req).bad("(labelID,)")),
        },
        // `AssignLabels(contactIDs, labelID)`, `RemoveLabels(...)`.
        _ => match Args::new(req).int(1) {
            Some(label) => {
                let ids = id_list(Args::new(req).get(0));
                crate::contact_labels::assign(reg, me, &ids, label, req.method == "AssignLabels");
                Ok(PyValue::None)
            }
            None => Err(Args::new(req).bad("(contactIDs, labelID)")),
        },
    })
}

/// `jumpCloneSvc`: installing a clone in a ship.
pub(crate) fn jump_clone(reg: &mut Registry, req: &Request<'_>) -> Answer {
    use crate::ship_clone as sc;
    Some(match req.method {
        // `OfferShipCloneInstallation(charID)` (`clonejumpsvc.py:129`).
        "OfferShipCloneInstallation" => match Args::new(req).int(0) {
            Some(target) => sc::offer(reg, req.actor, target).map(|()| PyValue::None),
            None => Err(Args::new(req).bad("(charID,)")),
        },
        "AcceptShipCloneInstallation" => sc::accept(reg, req.actor).map(|()| PyValue::None),
        "CancelShipCloneInstallation" => {
            sc::cancel(reg, req.actor);
            Ok(PyValue::None)
        }
        _ => return None,
    })
}

/// `DestroyInstalledClone(cloneID)` when the clone is one kept in a ship;
/// `None` for a station clone (the caller's own rule).
pub(crate) fn destroy_ship_clone(reg: &mut Registry, req: &Request<'_>) -> Answer {
    let id = Args::new(req).int(0)?;
    crate::ship_clone::destroy(reg, req.actor, id).then_some(Ok(PyValue::None))
}

/// `fleetProxy`: the fleet finder's adverts and applying to join.
pub(crate) fn fleet_proxy(reg: &mut Registry, req: &Request<'_>) -> Answer {
    use crate::fleet_finder as ff;
    let own = |a: Option<ff::Advert>| Ok(a.map_or(PyValue::None, |a| a.to_value(true)));
    Some(match req.method {
        "AddFleetFinderAdvert" => match Args::new(req).get(0) {
            Some(data) => ff::add(reg, req.actor, data).map(|a| a.to_value(true)),
            None => Err(Args::new(req).bad("(adInfoData,)")),
        },
        "RemoveFleetFinderAdvert" => own(ff::remove(reg, req.actor)),
        "GetMyFleetFinderAdvert" => own(ff::mine(reg, req.actor)),
        // `UpdateAdvertInfo(numMembers, KeyVal(allowedDiff))`.
        "UpdateAdvertInfo" => own(ff::update(
            reg,
            req.actor,
            Args::new(req).int(0),
            Args::new(req).get(1),
            false,
        )),
        "UpdateAdvertAllowedEntities" => own(ff::update(
            reg,
            req.actor,
            None,
            Args::new(req).get(0),
            false,
        )),
        "UpdateFleetAdvertWithNewLeader" => own(ff::update(
            reg,
            req.actor,
            None,
            Args::new(req).get(0),
            true,
        )),
        // `ApplyToJoinFleet(fleetID, autoAccept)`.
        "ApplyToJoinFleet" => match Args::new(req).int(0) {
            Some(id) => ff::apply(reg, req.actor, id).map(PyValue::Bool),
            None => Err(Args::new(req).bad("(fleetID, autoAccept)")),
        },
        _ => return None,
    })
}

/// `fleetMgr`: the watchlist and `ForceLeaveFleet`.
pub(crate) fn fleet_mgr(reg: &mut Registry, req: &Request<'_>) -> Answer {
    use crate::fleet_finder as ff;
    let known = ["AddToWatchlist", "RemoveFromWatchlist", "ForceLeaveFleet"];
    if !known.contains(&req.method) {
        return None;
    }
    let Some(me) = req.actor else {
        return Some(Ok(PyValue::None));
    };
    Some(match req.method {
        // `AddToWatchlist(newCharsAdded, currentWatchlist)` (`:764`).
        "AddToWatchlist" => {
            ff::watch(reg, me, &id_list(Args::new(req).get(0)));
            Ok(PyValue::None)
        }
        // `RemoveFromWatchlist(charID, currentWatchlist)` (`:814`).
        "RemoveFromWatchlist" => {
            if let Some(gone) = Args::new(req).int(0) {
                ff::unwatch(reg, me, gone);
            }
            Ok(PyValue::None)
        }
        _ => {
            ff::force_leave(reg, me);
            Ok(PyValue::None)
        }
    })
}

/// `LPSvc`: LP given to a player corporation.
pub(crate) fn lp_svc(reg: &mut Registry, req: &Request<'_>) -> Answer {
    let own = match req.method {
        "TransferLPFromMyWalletToOtherCorp" => true,
        "TransferLPFromMyCorpWalletToOtherCorp" => false,
        _ => return None,
    };
    // `(receiverCorpID, issuerCorpID, lpAmount)`
    // (`loyaltyPointsWalletSvc.py:114`, `:123`), the answer dropped.
    let (Some(receiver), Some(issuer)) = (Args::new(req).int(0), Args::new(req).int(1)) else {
        return Some(Err(
            Args::new(req).bad("(receiverCorpID, issuerCorpID, lpAmount)")
        ));
    };
    let amount = Args::new(req).int(2);
    let done = if own {
        crate::lp_transfer::from_player(reg, req.actor, receiver, issuer, amount)
    } else {
        crate::lp_transfer::from_corp(reg, req.actor, receiver, issuer, amount)
    };
    Some(done.map(|()| PyValue::None))
}

/// `ship` (the ship access object, `GetShipAccess()`): jettison and scoop.
pub(crate) fn ship(reg: &mut Registry, req: &Request<'_>) -> Answer {
    match req.method {
        // `Jettison(itemIDs)` -> `(jettisonedToCanIDs, jettisonItemList)`
        // (`invItemFunctions.Jettison:64`).
        "Jettison" => Some((|| {
            let ids = id_list(Args::new(req).get(0));
            let (moved, launched) = crate::jettison::jettison_items(reg, req.actor, &ids)?;
            Ok(crate::jettison::jettison_result(moved, launched))
        })()),
        // `Scoop(objectID[, password])`, the answer dropped
        // (`menusvc.RealScoop:3741-3743`).
        "Scoop" => Some((|| {
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(objectID[, password])"));
            };
            let password = Args::new(req).text(1);
            crate::jettison::scoop_with(
                reg,
                req.actor,
                id,
                password.as_deref(),
                crate::fitting::FLAG_CARGO,
            )?;
            Ok(PyValue::None)
        })()),
        // `ScoopToMobileDepotHold(objectID)`: the same, into flag 183
        // (`menusvc.ScoopToMobileDepotHold`).
        "ScoopToMobileDepotHold" => Some((|| {
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(objectID)"));
            };
            crate::jettison::scoop_into(reg, req.actor, id, crate::mtu::FLAG_MOBILE_DEPOT_HOLD)?;
            Ok(PyValue::None)
        })()),
        // `ScoopToFleetHangar(objectID)`: the same can, into flag 155
        // (`menusvc.RealScoopToFleetHangar`).
        "ScoopToFleetHangar" => Some((|| {
            let Some(id) = Args::new(req).int(0) else {
                return Err(Args::new(req).bad("(objectID)"));
            };
            crate::jettison::scoop_to_fleet_hangar(reg, req.actor, id)?;
            Ok(PyValue::None)
        })()),
        _ => None,
    }
}

/// `invbroker` (the unbound `GetInventoryMgr()` object).
pub(crate) fn invbroker(reg: &mut Registry, req: &Request<'_>) -> Answer {
    match req.method {
        // `GetInventoryMgr().SetLabel(itemID, name)`, answer dropped:
        // `menuFunctions.SetName:402`, `fittingSvc._RenameShip:586` [V].
        "SetLabel" => Some((|| {
            let (Some(item), Some(name)) = (Args::new(req).int(0), Args::new(req).text(1)) else {
                return Err(Args::new(req).bad("(itemID, name)"));
            };
            crate::item_names::set_label(reg, req.actor, item, &name)?;
            Ok(PyValue::None)
        })()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn id_list_takes_an_int_or_the_ints_of_a_list() {
        use super::id_list;
        assert_eq!(id_list(Some(&PyValue::Int(7))), vec![7]);
        let mixed = vec![
            PyValue::Int(1),
            PyValue::Bool(true),
            PyValue::Long(2),
            PyValue::Unicode("3".into()),
            PyValue::Int(4),
        ];
        assert_eq!(id_list(Some(&PyValue::List(mixed.clone()))), vec![1, 4]);
        assert_eq!(id_list(Some(&PyValue::Tuple(mixed))), vec![1, 4]);
        for none in [
            Some(&PyValue::Bool(true)),
            Some(&PyValue::Long(5)),
            Some(&PyValue::None),
            None,
        ] {
            assert!(id_list(none).is_empty(), "{none:?}");
        }
    }

    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;

    fn call(
        reg: &mut Registry,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    }

    fn world() -> Registry {
        Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..Character::default()
            }],
            ..SandboxConfig::default()
        })
    }

    /// `SetLabel` through the dispatcher: the answer is `None`, the name is
    /// read back by `GetMultiLocationsEx`.
    #[test]
    fn set_label_names_a_ship_and_locations_ex_reads_it_back() {
        let mut reg = world();
        let got = call(
            &mut reg,
            "invbroker",
            "SetLabel",
            &[
                PyValue::Int(ALICE_SHIP),
                PyValue::Unicode("Wanderer".to_owned()),
            ],
        )
        .expect("answered");
        assert_eq!(got, PyValue::None);
        let rows = call(
            &mut reg,
            "config",
            "GetMultiLocationsEx",
            &[PyValue::List(vec![PyValue::Int(ALICE_SHIP)])],
        )
        .expect("answered");
        assert!(format!("{rows:?}").contains("Wanderer"), "{rows:?}");
    }

    // ---- LP transfers ------------------------------------------------------

    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;
    /// An invented issuing corporation.
    const ISSUER: i64 = 1_000_990;

    fn call_as(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    /// Alice founds a corporation (CEO), Bob joins it with no roles; Carol
    /// founds her own.
    fn corp_world() -> (Registry, i64, i64) {
        let pilot = |id: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 1,
            balance: 5_000_000.0,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ],
            ..SandboxConfig::default()
        });
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let found = |reg: &mut Registry, who: i64, name: &str, ticker: &str| {
            let args = vec![
                u(name),
                u(ticker),
                u("We mine."),
                u("http://"),
                PyValue::Real(0.0),
                PyValue::Int(419),
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::Bool(true),
                PyValue::Bool(false),
                PyValue::Real(0.0),
            ];
            call_as(reg, who, "corpRegistry", "AddCorporation", &args).expect("founded");
            reg.character(who).expect("a pilot").corporation_id
        };
        let mine = found(&mut reg, ALICE, "Gaps Mining", "GAPM");
        let theirs = found(&mut reg, CAROL, "Gaps Other", "GAPO");
        reg.corporations.enrol_for_test(BOB, mine, 0);
        let _ = reg.take_pending();
        (reg, mine, theirs)
    }

    fn balances(reg: &mut Registry, who: i64, method: &str) -> Vec<(i64, i64)> {
        let got = call_as(reg, who, "LPSvc", method, &[]).expect("answered");
        let PyValue::List(rows) = got else {
            panic!("a list")
        };
        rows.iter()
            .map(|r| match r {
                PyValue::Tuple(t) => match (&t[0], &t[1]) {
                    (PyValue::Int(a), PyValue::Int(b)) => (*a, *b),
                    other => panic!("(int, int): {other:?}"),
                },
                other => panic!("a pair: {other:?}"),
            })
            .collect()
    }

    fn refused(e: &ServiceError) -> bool {
        matches!(e, ServiceError::Raise(_))
    }

    /// A pilot gives LP to a corporation: their wallet drops, the
    /// corporation's rises, `OnLPChange` for the pilot and `OnCorpLPChange`
    /// for the members; an overdraft, a stranger and a zero change nothing.
    #[test]
    fn a_pilot_donates_lp_to_a_corporation() {
        let (mut reg, mine, theirs) = corp_world();
        crate::loyalty::adjust(&mut reg, BOB, ISSUER, 1_000, crate::loyalty::CHANGE_MISSION);
        let _ = reg.take_pending();
        let args = |recv: i64, n: i64| [PyValue::Int(recv), PyValue::Int(ISSUER), PyValue::Int(n)];
        let a = args(mine, 400);
        call_as(
            &mut reg,
            BOB,
            "LPSvc",
            "TransferLPFromMyWalletToOtherCorp",
            &a,
        )
        .expect("donated");
        assert_eq!(
            balances(&mut reg, BOB, "GetAllMyCharacterWalletLPBalances"),
            vec![(ISSUER, 600)]
        );
        assert_eq!(
            balances(&mut reg, ALICE, "GetAllMyCorporationWalletLPBalances"),
            vec![(ISSUER, 400)]
        );
        assert_eq!(
            balances(&mut reg, BOB, "GetAllMyCorporationWalletLPBalances"),
            vec![(ISSUER, 400)],
            "every member sees it"
        );
        assert!(balances(&mut reg, CAROL, "GetAllMyCorporationWalletLPBalances").is_empty());
        let pushed: Vec<(&'static str, Option<Vec<i64>>)> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| (p.to_push().0, p.audience()))
            .collect();
        assert!(
            pushed
                .iter()
                .any(|(n, a)| *n == "OnLPChange" && *a == Some(vec![BOB])),
            "{pushed:?}"
        );
        assert!(
            pushed.iter().any(|(n, a)| *n == "OnCorpLPChange"
                && a.as_ref()
                    .is_some_and(|m| m.contains(&ALICE) && m.contains(&BOB))),
            "{pushed:?}"
        );
        for bad in [
            args(mine, 601),
            args(mine, 0),
            args(mine, -5),
            args(ISSUER, 10),
            args(9_999_999, 10),
        ] {
            let e = call_as(
                &mut reg,
                BOB,
                "LPSvc",
                "TransferLPFromMyWalletToOtherCorp",
                &bad,
            )
            .expect_err("refused");
            assert!(refused(&e), "{bad:?}");
        }
        assert_eq!(
            balances(&mut reg, BOB, "GetAllMyCharacterWalletLPBalances"),
            vec![(ISSUER, 600)]
        );
        let _ = theirs;
    }

    /// A corporation's LP moves to another corporation under a director or
    /// an accountant, and under nobody else.
    #[test]
    fn a_corporation_donates_lp_under_the_right_role() {
        let (mut reg, mine, theirs) = corp_world();
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            ISSUER,
            1_000,
            crate::loyalty::CHANGE_MISSION,
        );
        call_as(
            &mut reg,
            ALICE,
            "LPSvc",
            "TransferLPFromMyWalletToOtherCorp",
            &[
                PyValue::Int(mine),
                PyValue::Int(ISSUER),
                PyValue::Int(1_000),
            ],
        )
        .expect("funded");
        let to_other = [
            PyValue::Int(theirs),
            PyValue::Int(ISSUER),
            PyValue::Int(300),
        ];
        // Bob has no role.
        let e = call_as(
            &mut reg,
            BOB,
            "LPSvc",
            "TransferLPFromMyCorpWalletToOtherCorp",
            &to_other,
        )
        .expect_err("no role");
        assert!(refused(&e));
        // Alice is the CEO.
        call_as(
            &mut reg,
            ALICE,
            "LPSvc",
            "TransferLPFromMyCorpWalletToOtherCorp",
            &to_other,
        )
        .expect("the CEO may");
        assert_eq!(
            balances(&mut reg, ALICE, "GetAllMyCorporationWalletLPBalances"),
            vec![(ISSUER, 700)]
        );
        assert_eq!(
            balances(&mut reg, CAROL, "GetAllMyCorporationWalletLPBalances"),
            vec![(ISSUER, 300)]
        );
        // An accountant may too; the own corporation and an overdraft may not.
        reg.corporations
            .enrol_for_test(BOB, mine, crate::corp_wallet::ROLE_ACCOUNTANT);
        call_as(
            &mut reg,
            BOB,
            "LPSvc",
            "TransferLPFromMyCorpWalletToOtherCorp",
            &to_other,
        )
        .expect("an accountant may");
        let own = [PyValue::Int(mine), PyValue::Int(ISSUER), PyValue::Int(1)];
        assert!(
            call_as(
                &mut reg,
                ALICE,
                "LPSvc",
                "TransferLPFromMyCorpWalletToOtherCorp",
                &own
            )
            .is_err()
        );
        let over = [
            PyValue::Int(theirs),
            PyValue::Int(ISSUER),
            PyValue::Int(5_000),
        ];
        assert!(
            call_as(
                &mut reg,
                ALICE,
                "LPSvc",
                "TransferLPFromMyCorpWalletToOtherCorp",
                &over
            )
            .is_err()
        );
        assert_eq!(
            balances(&mut reg, ALICE, "GetAllMyCorporationWalletLPBalances"),
            vec![(ISSUER, 400)]
        );
    }

    /// **Everything here survives a save and a load** of the world file,
    /// text with tabs, newlines and percent signs included: a ship's name,
    /// a note on a character, a notepad note, a contact label and the bit it
    /// puts on a contact, a corporation's LP, a clone kept in a ship.
    #[test]
    fn what_a_pilot_keeps_survives_a_restart() {
        let (mut reg, corp, _) = corp_world();
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let ship = reg.character(ALICE).expect("Alice").ship_id;
        call_as(
            &mut reg,
            ALICE,
            "invbroker",
            "SetLabel",
            &[PyValue::Int(ship), u("Tab\there 100%")],
        )
        .expect("renamed");
        call_as(
            &mut reg,
            ALICE,
            "charMgr",
            "SetNote",
            &[PyValue::Int(BOB), u("a\tb\nc%d")],
        )
        .expect("noted");
        let note = call_as(
            &mut reg,
            ALICE,
            "charMgr",
            "AddOwnerNote",
            &[u("N:x%y"), u("<br>\nz\t")],
        )
        .expect("added");
        reg.contacts.put(ALICE, BOB, 5.0, false);
        let label = call_as(
            &mut reg,
            ALICE,
            "charMgr",
            "CreateLabel",
            &[u("Fr\tiends"), PyValue::Int(2)],
        )
        .expect("label");
        let PyValue::Int(label_id) = label else {
            panic!("an id")
        };
        call_as(
            &mut reg,
            ALICE,
            "charMgr",
            "AssignLabels",
            &[
                PyValue::List(vec![PyValue::Int(BOB)]),
                PyValue::Int(label_id),
            ],
        )
        .expect("assigned");
        crate::loyalty::adjust(&mut reg, ALICE, ISSUER, 700, crate::loyalty::CHANGE_MISSION);
        call_as(
            &mut reg,
            ALICE,
            "LPSvc",
            "TransferLPFromMyWalletToOtherCorp",
            &[PyValue::Int(corp), PyValue::Int(ISSUER), PyValue::Int(300)],
        )
        .expect("donated");

        let dir = std::env::temp_dir().join(format!(
            "eve-gaps2-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);

        let (mut fresh, _, _) = corp_world();
        assert!(
            crate::item_names::name_of(&fresh, ship).is_none(),
            "or this proves nothing"
        );
        fresh.apply_snapshot(snap);
        assert_eq!(
            crate::item_names::name_of(&fresh, ship).as_deref(),
            Some("Tab here 100%"),
            "a tab in a name is a space"
        );
        assert_eq!(crate::notes::owner_note(&fresh, ALICE, BOB), "a\tb\nc%d");
        let PyValue::Int(note_id) = note else {
            panic!("an id")
        };
        let n = crate::notes::note(&fresh, ALICE, note_id).expect("the note");
        assert_eq!((n.label.as_str(), n.text.as_str()), ("N:x%y", "<br>\nz\t"));
        let labels = crate::contact_labels::labels(&fresh, ALICE);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].name, "Fr\tiends");
        assert_eq!(fresh.contacts.book(ALICE)[0].1.label_mask, label_id);
        assert_eq!(crate::loyalty::balance(&fresh, ALICE, ISSUER), 400);
        assert_eq!(crate::lp_transfer::corp_balance(&fresh, corp, ISSUER), 300);
    }

    /// `/givelp` credits the caller's wallet; a bad line says its usage.
    #[test]
    fn the_gm_grants_lp() {
        let (mut reg, _, _) = corp_world();
        let said = reg
            .slash(Some(ALICE), &format!("/givelp {ISSUER} 250"))
            .expect("answered");
        assert!(format!("{said:?}").len() > 4);
        assert_eq!(crate::loyalty::balance(&reg, ALICE, ISSUER), 250);
        reg.slash(Some(ALICE), &format!("/givelp {ISSUER} -1000"))
            .expect("answered");
        assert_eq!(
            crate::loyalty::balance(&reg, ALICE, ISSUER),
            250,
            "never below zero"
        );
        reg.slash(Some(ALICE), "/givelp nonsense")
            .expect("answered");
    }

    // ---- notes and labels ---------------------------------------------------

    fn attr(v: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal: {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("no .{name}"))
            .clone()
    }

    /// The notepad's calls, in the order the client makes them.
    #[test]
    fn the_notepad_and_the_note_on_a_character_answer() {
        let mut reg = world();
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let id = call(
            &mut reg,
            "charMgr",
            "AddOwnerNote",
            &[u("N:Plans"), u("<br>")],
        )
        .expect("added");
        let PyValue::Int(id) = id else {
            panic!("an id")
        };
        call(
            &mut reg,
            "charMgr",
            "EditOwnerNote",
            &[PyValue::Int(id), u("N:Plans"), u("<br>go")],
        )
        .expect("edited");
        let PyValue::List(rows) =
            call(&mut reg, "charMgr", "GetOwnerNote", &[PyValue::Int(id)]).unwrap()
        else {
            panic!("a list")
        };
        assert_eq!(attr(&rows[0], "note"), u("<br>go"));
        assert_eq!(attr(&rows[0], "label"), u("N:Plans"));
        let PyValue::List(all) = call(&mut reg, "charMgr", "GetOwnerNoteLabels", &[]).unwrap()
        else {
            panic!("a list")
        };
        assert_eq!(all.len(), 1);
        call(
            &mut reg,
            "charMgr",
            "SetNote",
            &[PyValue::Int(BOB), u("slow to pay")],
        )
        .expect("set");
        assert_eq!(
            call(&mut reg, "charMgr", "GetNote", &[PyValue::Int(BOB)]).unwrap(),
            u("slow to pay")
        );
        assert_eq!(
            call(&mut reg, "charMgr", "GetNote", &[PyValue::Int(CAROL)]).unwrap(),
            u("")
        );
        call(&mut reg, "charMgr", "RemoveOwnerNote", &[PyValue::Int(id)]).expect("removed");
        let PyValue::List(none) = call(&mut reg, "charMgr", "GetOwnerNoteLabels", &[]).unwrap()
        else {
            panic!("a list")
        };
        assert!(none.is_empty());
    }

    /// The contact labels' calls, through `GetContacts`' own `labelMask`.
    #[test]
    fn contact_labels_come_back_on_the_contacts() {
        let mut reg = world();
        reg.contacts.put(ALICE, BOB, 5.0, false);
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let PyValue::Int(label) = call(
            &mut reg,
            "charMgr",
            "CreateLabel",
            &[u("Friends"), PyValue::Int(2)],
        )
        .unwrap() else {
            panic!("an id")
        };
        assert_eq!(label, 1);
        let PyValue::Dict(d) = call(&mut reg, "charMgr", "GetLabels", &[]).unwrap() else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1);
        assert_eq!(attr(&d[0].0, "name"), u("Friends"));
        call(
            &mut reg,
            "charMgr",
            "EditLabel",
            &[PyValue::Int(label), u("Pals"), PyValue::None],
        )
        .unwrap();
        call(
            &mut reg,
            "charMgr",
            "AssignLabels",
            &[PyValue::List(vec![PyValue::Int(BOB)]), PyValue::Int(label)],
        )
        .unwrap();
        let book = reg.contacts.book(ALICE);
        assert_eq!(book[0].1.label_mask, label);
        call(
            &mut reg,
            "charMgr",
            "RemoveLabels",
            &[PyValue::List(vec![PyValue::Int(BOB)]), PyValue::Int(label)],
        )
        .unwrap();
        assert_eq!(reg.contacts.book(ALICE)[0].1.label_mask, 0);
        call(&mut reg, "charMgr", "DeleteLabel", &[PyValue::Int(label)]).unwrap();
        let PyValue::Dict(d) = call(&mut reg, "charMgr", "GetLabels", &[]).unwrap() else {
            panic!("a dict")
        };
        assert!(d.is_empty());
    }
}
