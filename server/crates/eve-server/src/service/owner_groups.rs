//! **`ownerGroupManager`** -- the access groups behind the Access Lists
//! window and the bookmark folder window (lane GAP-ACCESS; the model is
//! [`crate::owner_groups`], the wire is verified from
//! `accessGroupsController.py` in the spec's B7 table).
//!
//! Lists come back as lists, groups as `KeyVal`s (the client assigns to
//! `.name`), failures as `(memberID, reason)` tuples.

use super::args::{Args, as_id_or_ids, as_int};
use super::*;
use crate::owner_groups as og;

pub(crate) const SERVICES: &[&str] = &["ownerGroupManager"];

fn refuse(r: og::Refusal) -> ServiceError {
    let words = match r {
        og::Refusal::NoSuchGroup => "That access list does not exist.",
        og::Refusal::NotAllowed => "You are not allowed to do that to this access list.",
        og::Refusal::BadName => "An access list needs a name.",
        og::Refusal::TooMany => "You manage too many access lists.",
    };
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(words))]),
    ))
}

fn failures(f: Vec<og::Failure>) -> PyValue {
    PyValue::List(
        f.into_iter()
            .map(|(m, why)| PyValue::Tuple(vec![PyValue::Int(m), PyValue::Unicode(why.to_owned())]))
            .collect(),
    )
}

/// Members of a group are shown to its own members only.
fn visible_members(reg: &Registry, gid: i64, me: i64) -> std::collections::BTreeMap<i64, i64> {
    let mem = og::members(reg, gid);
    if og::effective_in(reg, &mem, me) >= og::MEMBER {
        mem
    } else {
        Default::default()
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let a = Args::new(req);
    let Some(me) = req.actor else {
        return Err(a.bad("no character"));
    };
    let gid = a.int(0);
    match req.method {
        "GetMyGroups" => Ok(og::mine_value(reg, me)),
        // The audit log is not kept [A: nothing in the window needs rows].
        "GetGroupLogs" => Ok(PyValue::List(vec![])),
        "GetGroup" => Ok(gid
            .and_then(|g| og::group(reg, g))
            .map_or(PyValue::None, |g| og::group_value_for(reg, &g, me))),
        "GetGroupsMany" => Ok(PyValue::List(
            as_id_or_ids(a.get(0))
                .into_iter()
                .filter_map(|g| og::group(reg, g))
                .map(|g| og::group_value_for(reg, &g, me))
                .collect(),
        )),
        "GetMembers" => match gid {
            Some(g) => Ok(og::members_value(&visible_members(reg, g, me))),
            None => Err(a.bad("(groupID,)")),
        },
        // `{groupID: [KeyVal]}` (`_GetUncachedMembers` iterates `iteritems()`).
        "GetMembersForMultipleGroups" => Ok(crate::pyutil::dict_kv(
            as_id_or_ids(a.get(0)).into_iter().map(|g| {
                (
                    PyValue::Int(g),
                    og::members_value(&visible_members(reg, g, me)),
                )
            }),
        )),
        "Create" => match a.text(0) {
            Some(name) => {
                let desc = a.text(1).unwrap_or_default();
                og::create(reg, me, &name, &desc)
                    .map(|g| og::group_value_for(reg, &g, me))
                    .map_err(refuse)
            }
            None => Err(a.bad("(name, description)")),
        },
        "Delete" => match gid {
            Some(g) => og::delete(reg, me, g)
                .map(|()| PyValue::None)
                .map_err(refuse),
            None => Err(a.bad("(groupID,)")),
        },
        "UpdateName" | "UpdateDescription" => match (gid, a.text(1)) {
            (Some(g), Some(s)) => og::rename(reg, me, g, &s, req.method == "UpdateDescription")
                .map(|()| PyValue::None)
                .map_err(refuse),
            _ => Err(a.bad("(groupID, text)")),
        },
        "AddMembers" => match (gid, a.get(1)) {
            (Some(g), Some(PyValue::Dict(d))) => {
                // `{memberID: type}`: a marshal pair is `(value, key)`.
                let wanted: Vec<(i64, i64)> = d
                    .iter()
                    .filter_map(|(v, k)| Some((as_int(Some(k))?, as_int(Some(v))?)))
                    .collect();
                og::add_members(reg, me, g, &wanted)
                    .map(failures)
                    .map_err(refuse)
            }
            _ => Err(a.bad("(groupID, {memberID: type})")),
        },
        "UpdateMembershipTypes" => match (gid, a.int(2)) {
            (Some(g), Some(t)) => og::retype(reg, me, g, &as_id_or_ids(a.get(1)), t)
                .map(failures)
                .map_err(refuse),
            _ => Err(a.bad("(groupID, [ids], type)")),
        },
        "DeleteMembers" => match gid {
            Some(g) => og::remove_members(reg, me, g, &as_id_or_ids(a.get(1)))
                .map(failures)
                .map_err(refuse),
            None => Err(a.bad("(groupID, [ids])")),
        },
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    fn world() -> Registry {
        let ch = |id: i64| Character {
            character_id: id,
            name: format!("P{id}"),
            ship_id: 1_000 + id,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![ch(7), ch(8)],
            ..SandboxConfig::default()
        })
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "ownerGroupManager",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(who),
            },
        )
    }

    #[test]
    fn the_stub_answers_an_empty_world() {
        let mut reg = world();
        assert_eq!(
            call(&mut reg, 7, "GetMyGroups", &[]),
            Ok(PyValue::List(vec![]))
        );
        assert_eq!(
            call(&mut reg, 7, "GetGroupLogs", &[PyValue::Int(1)]),
            Ok(PyValue::List(vec![]))
        );
        assert_eq!(
            call(&mut reg, 7, "GetGroup", &[PyValue::Int(1)]),
            Ok(PyValue::None)
        );
    }

    /// `{groupID: [KeyVal]}`: the group id is the dict's KEY, which a marshal
    /// pair holds second. Built `(key, value)` the client got `[KeyVal]` as the
    /// key and an int as the members.
    #[test]
    fn members_for_multiple_groups_is_keyed_by_group_id() {
        let mut reg = world();
        let g = call(
            &mut reg,
            7,
            "Create",
            &[
                PyValue::Unicode("ops".into()),
                PyValue::Unicode(String::new()),
            ],
        );
        assert!(g.is_ok());
        let gid = og::all(&reg)[0].group_id;
        let answer = call(
            &mut reg,
            7,
            "GetMembersForMultipleGroups",
            &[PyValue::List(vec![PyValue::Int(gid)])],
        )
        .unwrap();
        let kv = answer.dict_as_kv().expect("a dict");
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].0, &PyValue::Int(gid), "the key is the group id");
        assert!(
            matches!(kv[0].1, PyValue::List(_)),
            "the value is the member list"
        );
    }

    #[test]
    fn the_spec_scenario_over_the_wire() {
        let mut reg = world();
        let g = call(
            &mut reg,
            7,
            "Create",
            &[
                PyValue::Unicode("ops".into()),
                PyValue::Unicode(String::new()),
            ],
        )
        .unwrap();
        assert!(format!("{g:?}").contains("ops"));
        let gid = og::all(&reg)[0].group_id;
        let _ = reg.take_pending();
        let add = crate::pyutil::dict_kv([(PyValue::Int(8), PyValue::Int(0))]);
        assert_eq!(
            call(&mut reg, 7, "AddMembers", &[PyValue::Int(gid), add]),
            Ok(PyValue::List(vec![]))
        );
        let PyValue::List(l) = call(&mut reg, 8, "GetMyGroups", &[]).unwrap() else {
            panic!()
        };
        assert_eq!(l.len(), 1);
        assert!(
            call(
                &mut reg,
                8,
                "UpdateName",
                &[PyValue::Int(gid), PyValue::Unicode("x".into())]
            )
            .is_err()
        );
        let _ = reg.take_pending();
        assert_eq!(
            call(
                &mut reg,
                7,
                "DeleteMembers",
                &[PyValue::Int(gid), PyValue::List(vec![PyValue::Int(8)])]
            ),
            Ok(PyValue::List(vec![]))
        );
        assert_eq!(
            call(&mut reg, 8, "GetMyGroups", &[]),
            Ok(PyValue::List(vec![]))
        );
        let pushes: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push().0, p.audience()))
            .collect();
        assert_eq!(pushes, vec![("OnOwnerGroupsUpdated", Some(vec![8]))]);
    }

    /// `GetGroupsMany` reads a bare id or a list of ids and skips a `Bool`
    /// entry (pins the shared id reader).
    #[test]
    fn group_ids_take_a_bare_id_and_skip_a_bool() {
        let mut reg = world();
        let made = call(
            &mut reg,
            7,
            "Create",
            &[
                PyValue::Unicode("ops".into()),
                PyValue::Unicode(String::new()),
            ],
        );
        assert!(made.is_ok());
        let gid = og::all(&reg)[0].group_id;
        let n = |reg: &mut Registry, arg: PyValue| match call(reg, 7, "GetGroupsMany", &[arg]) {
            Ok(PyValue::List(l)) => l.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(n(&mut reg, PyValue::Int(gid)), 1);
        assert_eq!(
            n(
                &mut reg,
                PyValue::List(vec![PyValue::Bool(true), PyValue::Int(gid)])
            ),
            1
        );
        assert_eq!(n(&mut reg, PyValue::Bool(true)), 0);
    }
}
