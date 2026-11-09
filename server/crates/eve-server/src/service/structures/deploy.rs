//! `structureDeployment`: `Anchor`, `Unanchor`, `CancelUnanchor` and
//! `RenameStructure` (lane U3, issue #63). The logic is
//! [`crate::structure_space`]; this is the arguments and the role.
//!
//! The roles: anchoring and renaming need the station-manager role (the
//! client's own `Deploy` tests `corprole & corpRoleStationManager`; a
//! director or the CEO passes too, `super::manager_corp`), and so does a
//! decommission. The browser's menu shows Decommission to a director only
//! (`structureEntryController.CanUnanchor`) **[V]**; the server is the
//! looser rule, which only ever matters to a hand-made call. **[A]**
//! falsifier: a station manager who is refused in a retail capture.
//!
//! The replies are all `None` **[V]** (`structureDeployment.ConfirmDeployment`
//! and the menu callbacks drop them); a refusal is a `CustomNotify`.

use super::super::args::Args;
use super::*;
use crate::structure_space as space;

pub(super) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "Anchor" => anchor(reg, req),
        "Unanchor" => {
            let id = Args::new(req)
                .int(0)
                .ok_or_else(|| Args::new(req).bad("Unanchor(structureID)"))?;
            let corp = manager_corp(reg, req.actor)?;
            space::unanchor(reg, corp, id).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        "CancelUnanchor" => {
            let id = Args::new(req)
                .int(0)
                .ok_or_else(|| Args::new(req).bad("CancelUnanchor(structureID)"))?;
            let corp = manager_corp(reg, req.actor)?;
            space::cancel_unanchor(reg, corp, id).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        "RenameStructure" => {
            let (Some(id), Some(name)) = (Args::new(req).int(0), Args::new(req).text(1)) else {
                return Err(Args::new(req).bad("RenameStructure(structureID, name)"));
            };
            let corp = manager_corp(reg, req.actor)?;
            space::rename(reg, corp, id, &name).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        other => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: other.to_owned(),
        }),
    }
}

/// `Anchor(itemID, x, z, yaw, profileID, name, bio, reinforceWeekday,
/// reinforceHour, extraConfig)` **[V]**.
fn anchor(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    const SHAPE: &str = "Anchor(itemID, x, z, yaw, profileID, name, bio, weekday, hour, extra)";
    let a = Args::new(req);
    let (Some(item_id), Some(x), Some(z), Some(yaw)) =
        (a.int(0), a.float(1), a.float(2), a.float(3))
    else {
        return Err(a.bad(SHAPE));
    };
    let (Some(weekday), Some(hour)) = (a.int(7), a.int(8)) else {
        return Err(a.bad(SHAPE));
    };
    let args = space::AnchorArgs {
        item_id,
        x,
        z,
        yaw,
        profile: a.int(4),
        name: a.text(5).unwrap_or_default(),
        bio: a.text(6).unwrap_or_default(),
        reinforce_weekday: weekday,
        reinforce_hour: hour,
    };
    let corp = manager_corp(reg, req.actor)?;
    let actor = req.actor.unwrap_or_default();
    // `structureDeployment.py:96-97,117` **[V]**: a Triglavian system takes no
    // structure, and the dialog names the type and the faction (plain ints,
    // as the client passes them).
    if let Some(system) = reg.present_system(actor)
        && crate::pochven::in_pochven(reg, system)
        && let Some(item) = reg.store().get(item_id)
    {
        return Err(crate::modules::user_error(
            "CannotBeDeployedInSystemOwnedByFaction",
            vec![
                ("deployTypeID", PyValue::Int(item.type_id)),
                (
                    "factionID",
                    PyValue::Int(crate::pochven::FACTION_TRIGLAVIAN),
                ),
            ],
        ));
    }
    let s = space::anchor(reg, actor, corp, &args).map_err(|e| notify(&e))?;
    // ANSI-1 (#85): a jump gate is aimed by `extraConfig`.
    crate::jump_bridge::on_anchored(reg, &s, a.get(9));
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::space_state::{Motion, Presence};

    const BOSS: i64 = 7;
    const CORP: i64 = 11;
    const SHIP: i64 = 1_007;
    const ITEM: i64 = 100_700_001;
    /// Invented systems: one of the Triglavian faction, one of none.
    const POCHVEN: i64 = 39_070_001;
    const PLAIN: i64 = 39_070_002;

    /// BOSS, a director, in space in `system`, with a structure in his hold.
    fn world(system: i64) -> Registry {
        let rows = [POCHVEN]
            .iter()
            .map(|id| {
                format!(
                    "{id}\t19070001\t29070001\t{}\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t25\t0\t\t\tInvented",
                    crate::pochven::FACTION_TRIGLAVIAN
                )
            })
            .collect::<Vec<_>>();
        let mut c = crate::content::Content::fixture();
        c.insert(
            "universe",
            crate::celestial::Universe::parse(&rows, &[], &[]).expect("invented rows parse"),
        );
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: BOSS,
                corporation_id: CORP,
                solar_system_id: Some(system),
                ship_id: SHIP,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(c));
        let mut items = reg.store().items().to_vec();
        items.push(Item {
            item_id: ITEM,
            type_id: space::ASTRAHUS,
            owner_id: BOSS,
            location_id: SHIP,
            flag_id: crate::sov_hub::FLAG_CARGO,
            quantity: 1,
            group_id: 1657,
            category_id: 65,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_701_000,
            ..crate::store::Snapshot::default()
        });
        reg.corporations_mut()
            .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
        reg.roster_mut().arrive(Presence::arriving(
            BOSS,
            SHIP,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        reg
    }

    fn anchor_call(reg: &mut Registry) -> Result<PyValue, ServiceError> {
        let args = vec![
            PyValue::Int(ITEM),
            PyValue::Real(5_000.0),
            PyValue::Real(0.0),
            PyValue::Real(0.5),
            PyValue::None,
            PyValue::Unicode("Invented Keep".to_owned()),
            PyValue::Unicode(String::new()),
            PyValue::Int(3),
            PyValue::Int(12),
            PyValue::None,
        ];
        dispatch(
            reg,
            &Request {
                service: "structureDeployment",
                method: "Anchor",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(BOSS),
            },
        )
    }

    /// **PV-1d.** Anchoring in a Triglavian system is refused with the
    /// client's key, naming the type and the faction; the same call in a
    /// system of no faction anchors.
    #[test]
    fn anchoring_a_structure_in_pochven_names_the_type_and_the_faction() {
        let mut reg = world(POCHVEN);
        let e = anchor_call(&mut reg).expect_err("refused");
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = &e else {
            panic!("a user error, got {e:?}");
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("header");
        };
        let PyValue::Tuple(a) = &h[1] else {
            panic!("args");
        };
        assert_eq!(
            a[0],
            PyValue::Bytes(b"CannotBeDeployedInSystemOwnedByFaction".to_vec())
        );
        let PyValue::Dict(d) = &a[1] else {
            panic!("dict");
        };
        // `pypairs` writes `{value: key}`.
        let val = |k: &[u8]| {
            d.iter().find_map(|(v, key)| match key {
                PyValue::Bytes(b) if b.as_slice() == k => Some(v.clone()),
                _ => None,
            })
        };
        assert_eq!(val(b"deployTypeID"), Some(PyValue::Int(space::ASTRAHUS)));
        assert_eq!(
            val(b"factionID"),
            Some(PyValue::Int(crate::pochven::FACTION_TRIGLAVIAN))
        );
        assert!(crate::structure::all(reg.deltas()).is_empty());
        let mut reg = world(PLAIN);
        assert_eq!(anchor_call(&mut reg), Ok(PyValue::None));
        assert_eq!(crate::structure::all(reg.deltas()).len(), 1);
    }
}
