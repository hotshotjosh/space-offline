//! **`superWeaponMgr`** (NW-9a; the model is [`crate::superweapon`]).
//!
//! `positionalControl._StartTargetedEffect` calls
//! `ActivateSinglePointTargetedModule(moduleID, effectID, destination)` and
//! `_StartSlasherEffect` calls `ActivateSlashModule(moduleID, p0, p1)` [V];
//! both discard the answer (`findings/client-calls.tsv`). Only the first
//! reaches the Lance (a cylinder); the second is the Reaper's slice (9b).

use super::*;

pub(crate) const SERVICES: &[&str] = &["superWeaponMgr"];

/// A point as the client marshals it **[A]**: a 3-tuple or list of numbers.
fn point(v: Option<&PyValue>) -> Option<[f64; 3]> {
    let n = |x: &PyValue| args::as_float(Some(x));
    match args::as_list(v)? {
        [x, y, z] => Some([n(x)?, n(y)?, n(z)?]),
        _ => None,
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "ActivateSinglePointTargetedModule" => {
            let a = args::Args::new(req);
            let (Some(module), Some(dest)) = (a.int(0), point(a.get(2))) else {
                return Err(bad_args(req, "(moduleID, effectID, (x, y, z))"));
            };
            crate::superweapon::fire_lance(
                reg,
                req.actor,
                module,
                dest,
                crate::pyutil::filetime_now(),
            )?;
            Ok(PyValue::None)
        }
        "ActivateSlashModule" => {
            let a = args::Args::new(req);
            let (Some(module), Some(p0), Some(p1)) = (a.int(0), point(a.get(1)), point(a.get(2)))
            else {
                return Err(bad_args(req, "(moduleID, (x, y, z), (x, y, z))"));
            };
            crate::superweapon::fire_slash(
                reg,
                req.actor,
                module,
                p0,
                p1,
                crate::pyutil::filetime_now(),
            )?;
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superweapon::tests::{AIM, ALICE, ALICE_LANCE, ship, world};

    #[test]
    fn the_service_decodes_the_clients_call() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        let args = [
            PyValue::Int(ALICE_LANCE),
            PyValue::Int(0),
            PyValue::Tuple(vec![
                PyValue::Real(AIM[0]),
                PyValue::Real(AIM[1]),
                PyValue::Int(0),
            ]),
        ];
        let out = dispatch(
            &mut reg,
            &Request {
                service: "superWeaponMgr",
                method: "ActivateSinglePointTargetedModule",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(out, Ok(PyValue::None));
        assert!(crate::superweapon::immobile_at(&reg, ship(ALICE), now));
    }

    /// fci_doomsday's target client asked whether the beam reaches it: the
    /// Lance fired through the client's call pushes `OnSpecialFX` start and
    /// stop under a race guid the client has a row for, and both go to the
    /// firer and to every other pilot in the system (the target among them),
    /// not to the firer alone.
    #[test]
    fn the_beam_fx_reaches_every_pilot_in_the_system_start_and_stop() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        let args = [
            PyValue::Int(ALICE_LANCE),
            PyValue::Int(0),
            PyValue::Tuple(AIM.iter().map(|x| PyValue::Real(*x)).collect()),
        ];
        let out = dispatch(
            &mut reg,
            &Request {
                service: "superWeaponMgr",
                method: "ActivateSinglePointTargetedModule",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(out, Ok(PyValue::None));
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let here: Vec<i64> = reg
            .roster()
            .in_system(system)
            .into_iter()
            .map(|p| p.character_id)
            .collect();
        assert!(here.len() > 1, "the fixture has others in the system");
        let _ = reg.take_pending();
        // Far past the warning and the burn: the start and the stop both fall
        // in this one tick, in that order.
        crate::superweapon::tick(
            &mut reg,
            std::time::Instant::now(),
            now + 3_600 * 1_000 * crate::modules::MSEC,
        );
        let fx: Vec<_> = reg
            .take_due(std::time::Instant::now() + std::time::Duration::from_secs(3_600))
            .into_iter()
            .map(|p| (p.to_push(), p.audience()))
            .filter(|((n, _), _)| *n == "OnSpecialFX")
            .collect();
        assert_eq!(fx.len(), 2, "one start and one stop");
        for (i, ((_, a), to)) in fx.iter().enumerate() {
            let PyValue::Bytes(guid) = &a[5] else {
                panic!("the guid is a string: {:?}", a[5]);
            };
            assert!(
                crate::superweapon::GUIDS_BEAM
                    .iter()
                    .any(|g| g.as_bytes() == guid.as_slice()),
                "a race guid, never the bare one"
            );
            assert_eq!(a[0], PyValue::Int(ship(ALICE)), "on the firer's ship");
            assert_eq!(a[7], PyValue::Int(i64::from(i == 0)), "start, then stop");
            let to = to.as_ref().expect("an audience");
            for c in &here {
                assert!(to.contains(c), "pilot {c} hears the beam");
            }
        }
    }

    #[test]
    fn the_service_decodes_the_slash_call() {
        let mut reg = world();
        let args = [
            PyValue::Int(ALICE_LANCE),
            PyValue::Tuple(vec![PyValue::Real(1.0), PyValue::Real(2.0)]),
            PyValue::Int(0),
        ];
        let out = dispatch(
            &mut reg,
            &Request {
                service: "superWeaponMgr",
                method: "ActivateSlashModule",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert!(matches!(out, Err(ServiceError::BadArguments { .. })));
    }

    /// #139: a point is three numbers in a tuple or a list, nothing else.
    #[test]
    fn a_point_is_three_numbers_in_a_tuple_or_list() {
        let n = vec![PyValue::Int(1), PyValue::Real(2.5), PyValue::Int(-3)];
        assert_eq!(
            point(Some(&PyValue::Tuple(n.clone()))),
            Some([1.0, 2.5, -3.0])
        );
        assert_eq!(
            point(Some(&PyValue::List(n.clone()))),
            Some([1.0, 2.5, -3.0])
        );
        let bad = vec![PyValue::Int(1), PyValue::Bool(true), PyValue::Int(3)];
        assert_eq!(point(Some(&PyValue::Tuple(bad))), None);
        assert_eq!(point(Some(&PyValue::Tuple(n[..2].to_vec()))), None);
        assert_eq!(point(Some(&PyValue::Int(1))), None);
        assert_eq!(point(None), None);
    }
}
