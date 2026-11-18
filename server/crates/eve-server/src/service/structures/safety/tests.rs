//! The wire of `structureAssetSafety`, on the same invented world as
//! [`crate::asset_safety`]'s tests.

use crate::asset_safety::tests::*;
use crate::asset_safety::{self as safety, SECOND};
use crate::pyutil::pairs_get;
use crate::service::{Request, ServiceError};
use eve_marshal::PyValue;

fn call(
    reg: &mut crate::service::Registry,
    who: i64,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    call_args(reg, who, "structureAssetSafety", method, args)
}

fn rows(v: PyValue) -> Vec<Vec<(PyValue, PyValue)>> {
    let PyValue::List(l) = v else {
        panic!("a list expected: {v:?}")
    };
    l.into_iter()
        .map(|r| {
            let PyValue::Dict(d) = r else {
                panic!("a dict row expected")
            };
            d
        })
        .collect()
}

fn field<'a>(row: &'a [(PyValue, PyValue)], name: &str) -> &'a PyValue {
    pairs_get(row, name.as_bytes()).unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn the_assets_tab_lists_a_persons_wraps_and_the_corp_tab_the_corporations() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let mine = rows(call(&mut reg, BOSS, "GetItemsInSafetyForCharacter", &[]).unwrap());
    assert_eq!(mine.len(), 1);
    let w = wrap_of(&reg, BOSS);
    let r = &mine[0];
    assert_eq!(field(r, "assetWrapID"), &PyValue::Int(w.id));
    assert_eq!(field(r, "solarSystemID"), &PyValue::Int(SYSTEM));
    assert_eq!(field(r, "ejectTime"), &PyValue::Long(w.created));
    assert_eq!(
        field(r, "wrapName"),
        &PyValue::Unicode("Invented Home Asset Safety".to_owned())
    );
    // The two waits are in days; the invented ones are not whole days.
    for k in ["daysUntilCanDeliver", "daysUntilAutoMove"] {
        assert!(matches!(field(r, k), PyValue::Real(_)), "{k}");
    }
    let PyValue::Dict(near) = field(r, "defaultDestinationInfo") else {
        panic!("a dict")
    };
    assert_eq!(field(near, "itemID"), &PyValue::Int(HARBOUR));
    assert!(matches!(field(near, "typeID"), PyValue::Int(_)));
    assert_eq!(field(r, "structureTypeID"), &PyValue::Int(w.origin_type));
    assert_ne!(w.origin_type, 0);

    // The corporation's wrap is on the corp tab, for a director only.
    assert_eq!(
        rows(call(&mut reg, BOSS, "GetItemsInSafetyForCorp", &[]).unwrap()).len(),
        1
    );
    assert!(rows(call(&mut reg, PAL, "GetItemsInSafetyForCorp", &[]).unwrap()).is_empty());
    assert!(rows(call(&mut reg, FOE, "GetItemsInSafetyForCorp", &[]).unwrap()).is_empty());
    assert_eq!(
        rows(call(&mut reg, FOE, "GetItemsInSafetyForCharacter", &[]).unwrap()).len(),
        1
    );
    assert!(rows(call(&mut reg, PAL, "GetItemsInSafetyForCharacter", &[]).unwrap()).is_empty());
}

#[test]
fn a_whole_number_of_days_is_an_integer() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    reg.rules_mut().safety_manual_secs = 2 * 86_400;
    reg.rules_mut().safety_auto_secs = 7 * 86_400;
    let mine = rows(call(&mut reg, BOSS, "GetItemsInSafetyForCharacter", &[]).unwrap());
    assert_eq!(field(&mine[0], "daysUntilCanDeliver"), &PyValue::Int(2));
    assert_eq!(field(&mine[0], "daysUntilAutoMove"), &PyValue::Int(7));
}

#[test]
fn the_delivery_window_answers_one_flat_list_tagged_with_systems() {
    let (mut reg, ..) = world();
    let places = rows(
        call(
            &mut reg,
            BOSS,
            "GetStructuresICanDeliverTo",
            &[PyValue::Int(SYSTEM), PyValue::Int(1_234)],
        )
        .unwrap(),
    );
    assert!(places.iter().any(|p| {
        field(p, "itemID") == &PyValue::Int(HARBOUR)
            && field(p, "solarSystemID") == &PyValue::Int(SYSTEM)
    }));
    assert!(
        places
            .iter()
            .all(|p| field(p, "itemID") != &PyValue::Int(FAR))
    );
}

#[test]
fn the_default_destination_answers_the_origins_station() {
    let (mut reg, ..) = world();
    let PyValue::Dict(d) = call(
        &mut reg,
        BOSS,
        "GetDefaultAssetSafetyDestination",
        &[PyValue::Int(SYSTEM), PyValue::Int(1_234)],
    )
    .unwrap() else {
        panic!("a dict")
    };
    assert_eq!(field(&d, "solarSystemID"), &PyValue::Int(SYSTEM));
    assert_eq!(field(&d, "itemID"), &PyValue::Int(HARBOUR));
}

#[test]
fn wrap_names_answer_by_id() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let out = call(
        &mut reg,
        BOSS,
        "GetWrapNames",
        &[PyValue::List(vec![PyValue::Int(w.id), PyValue::Int(99)])],
    )
    .unwrap();
    let PyValue::Dict(d) = out else {
        panic!("a dict")
    };
    // Stored value first, key second.
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].1, PyValue::Int(w.id));
    assert_eq!(d[0].0, PyValue::Unicode(w.name));
}

/// `GetWrapNames` reads a list or a tuple of ids; an integer or nothing is
/// `BadArguments` (pins the shared list reader).
#[test]
fn wrap_names_take_a_tuple_and_refuse_a_bare_id_or_nothing() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let out = call(
        &mut reg,
        BOSS,
        "GetWrapNames",
        &[PyValue::Tuple(vec![PyValue::Int(w.id)])],
    )
    .unwrap();
    let PyValue::Dict(d) = out else {
        panic!("a dict")
    };
    assert_eq!(d.len(), 1);
    let e = call(&mut reg, BOSS, "GetWrapNames", &[PyValue::Int(w.id)]).unwrap_err();
    assert!(matches!(e, ServiceError::BadArguments { .. }), "{e:?}");
    let e = call(&mut reg, BOSS, "GetWrapNames", &[]).unwrap_err();
    assert!(matches!(e, ServiceError::BadArguments { .. }), "{e:?}");
}

#[test]
fn the_wrap_is_delivered_by_keyword_and_early_delivery_is_refused() {
    let (mut reg, id, corp) = world();
    let (b1, ..) = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let kw = crate::pyutil::pypairs(vec![("destinationID", PyValue::Int(HARBOUR))]);
    let go = |reg: &mut crate::service::Registry| {
        reg.dispatch(&Request {
            service: "structureAssetSafety",
            method: "MoveSafetyWrapToStructure",
            args: &[PyValue::Int(w.id), PyValue::Int(SYSTEM)],
            kwargs: &kw,
            bind_params: None,
            actor: Some(BOSS),
        })
    };
    // The invented wait has not run out in real time.
    assert!(go(&mut reg).is_err());
    assert_eq!(reg.store().get(b1).unwrap().location_id, w.id);
    call(
        &mut reg,
        BOSS,
        "MoveEjectTimeGM",
        &[PyValue::Int(w.id), PyValue::Int(1)],
    )
    .unwrap();
    assert_eq!(go(&mut reg).unwrap(), PyValue::None);
    assert_eq!(reg.store().get(b1).unwrap().location_id, HARBOUR);
    assert!(rows(call(&mut reg, BOSS, "GetItemsInSafetyForCharacter", &[]).unwrap()).is_empty());
    let _ = SECOND;
    let _ = safety::KIND;
}

#[test]
fn putting_things_in_safety_by_hand() {
    let (mut reg, id, corp) = world();
    let mine = store_item(&mut reg, BOSS, id);
    let corps = store_item(&mut reg, corp, id);
    // Docked there: refused.
    let ship = reg.character(BOSS).unwrap().ship_id;
    call_args(
        &mut reg,
        BOSS,
        "structureDocking",
        "Dock",
        &[PyValue::Int(id), PyValue::Int(ship)],
    )
    .unwrap();
    let args = [PyValue::Int(SYSTEM), PyValue::Int(id)];
    assert!(call(&mut reg, BOSS, "MovePersonalAssetsToSafety", &args).is_err());
    // A member without roles may not put the corporation's things away.
    assert!(call(&mut reg, PAL, "MoveCorpAssetsToSafety", &args).is_err());
    assert_eq!(
        call(&mut reg, BOSS, "MoveCorpAssetsToSafety", &args).unwrap(),
        PyValue::None
    );
    assert_eq!(
        reg.store().get(corps).unwrap().location_id,
        wrap_of(&reg, corp).id
    );
    // PAL is not docked and has nothing there.
    assert!(call(&mut reg, PAL, "MovePersonalAssetsToSafety", &args).is_err());
    let _ = mine;
}

#[test]
fn eject_time_is_a_gm_call() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let args = [PyValue::Int(w.id), PyValue::Int(30)];
    let req = Request {
        service: "structureAssetSafety",
        method: "MoveEjectTimeGM",
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: Some(BOSS),
    };
    let player = crate::service::Caller::Session {
        user_id: 1,
        role: 0,
    };
    assert!(reg.dispatch_as(player, &req).is_err());
    assert_eq!(wrap_of(&reg, BOSS).created, w.created, "not moved");
    let gm = crate::service::Caller::Session {
        user_id: 1,
        role: crate::roles::ROLE_GML,
    };
    reg.dispatch_as(gm, &req).expect("a GM may");
    assert!(wrap_of(&reg, BOSS).created < w.created);
}

#[test]
fn a_zero_manual_wait_goes_out_as_a_positive_number_of_days() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    reg.rules_mut().safety_manual_secs = 0;
    let mine = rows(call(&mut reg, BOSS, "GetItemsInSafetyForCharacter", &[]).unwrap());
    let PyValue::Real(d) = field(&mine[0], "daysUntilCanDeliver") else {
        panic!("a real")
    };
    assert!(*d > 0.0);
}
