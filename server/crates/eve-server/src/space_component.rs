//! `beyonce.CallComponentFromClient`: one seam for every space-component
//! call the client sends (issue #217, COMP-CALL).
//!
//! The client reaches a ball's component with
//! `remoteBallpark.CallComponentFromClient(itemID, componentName, methodName,
//! *args, **kwargs)` [V]: `spacecomponents/client/components/orbitalSkyhook.py:240,246`,
//! `mercenaryDen.py:103`, `linkWithShip.py:235,240`, `microJumpDriver.py:92`,
//! `phaseStabilizer.py:147`, `towGameObjective.py:125,130`,
//! `jumpbridge/client/jumpBridge.py:134`, `itemTraderNew.py:30`,
//! `itemtrader_legacy.py:34` and
//! `eve/client/script/ui/services/menuSvcExtras/movementFunctions.py:507`
//! (15 call sites). The component names are the strings of
//! `spacecomponents/common/componentConst.py` (read there when a lane
//! registers one; never copied into a test).
//!
//! **Keyword arguments.** Three callers pass them: `underConstruction.py:169`,
//! `spacecomponents/client/components/itemTraderNew.py:28-33` and
//! `itemtrader_legacy.py:28-34` [V]. A handler gets them in
//! [`CompCall::kwargs`] (the request's keyword pairs, as received).
//!
//! **Routing.** [`CALLS`] maps `component -> (locate, [(method, handler)])`.
//! It starts empty; the skyhook, merc-den and link lanes each export a
//! handler and a locate fn, and the coordinator adds one row, like `CHAIN`.
//!
//! **Checks the router makes before any handler runs** (so a handler cannot
//! forget them): the actor is in space; and the item is a ball in the
//! *caller's* system, by the component's [`Locate`] fn (only the component's
//! own state knows which system its item is in). Either failure is
//! [`not_in_space`].
//!
//! **Refusals.**
//! - An unknown `(component, method)` is `ServiceError::NoSuchMethod`. The
//!   dispatcher (`dispatch.rs`, `answer_call`) answers such an error with
//!   **`None`**, not a failure, and logs `UNANSWERED ...` with the error
//!   text, which names the pair (`CallComponentFromClient:comp.method`). The
//!   per-key UNANSWERED count is keyed by `Registry::describe`
//!   (`service.method/argc as actor`), not by pair. Nothing panics.
//! - **Rule for handler lanes: every failure must be `ServiceError::Raise`,
//!   never `BadArguments` or `NoSuchMethod`.** Those two are answered `None`,
//!   and a `None` makes the merc-den and item-trader windows throw
//!   (`uiController.py:392-393`, mercenaryden `cargo.py:199`) and the skyhook
//!   extract show a false success [V].
//! - A caller who is not in space, or an item not in the caller's system,
//!   gets `CustomNotify` with a notify text ([`not_in_space`]). `TargetNotInSpace`
//!   is not a client message key (`findings/user-errors.tsv` has no row; the
//!   client would show "message not found"), so the existing `CustomNotify`
//!   pattern of `dispatch.rs` `refuse_call` is used. **[A]** the wording and
//!   that a real notify-type label is not wanted instead; no test pins it.

use crate::service::args::{as_int, as_text};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// One decoded call, as a handler sees it.
pub struct CompCall<'a> {
    /// The calling character.
    pub actor: i64,
    /// The caller's solar system (the item has been located in it).
    pub system: i64,
    /// The ball's item id (the first argument).
    pub item_id: i64,
    /// The arguments after the method name.
    pub args: &'a [PyValue],
    /// The keyword arguments of the call.
    pub kwargs: &'a [(PyValue, PyValue)],
}

/// A component method: the registry and the decoded call, to the value the
/// client's call returns.
pub type CompFn = fn(&mut Registry, &CompCall<'_>) -> Result<PyValue, ServiceError>;

/// The solar system an item of a component is in, or `None` when it is not a
/// live ball of that component.
pub type Locate = fn(&Registry, i64) -> Option<i64>;

/// One component's row: its name, its locate fn and its methods.
pub type Component<'a> = (&'a str, Locate, &'a [(&'a str, CompFn)]);

/// `component -> (locate, methods)`. Coordinator-only: later lanes export
/// their fns and the coordinator adds the rows.
pub const CALLS: &[Component<'static>] = &[
    // SKY-1 (#81): the skyhook window's authorisation and the take.
    (
        crate::skyhook::ORBITAL_SKYHOOK,
        crate::skyhook::locate,
        &[
            (
                "IsCharacterAuthorised",
                crate::skyhook::is_character_authorised,
            ),
            ("ExtractReagentsToShip", crate::skyhook::extract),
        ],
    ),
    // SKY-2 (#82): the reagent silo's link (theft, or an owner securing stock).
    (
        crate::skyhook_theft::LINK_WITH_SHIP,
        crate::skyhook_theft::locate,
        &[
            ("InitiateLink", crate::skyhook_theft::initiate_link),
            (
                "InitiateLinkQuick",
                crate::skyhook_theft::initiate_link_quick,
            ),
        ],
    ),
    // MERC-1 (#83): the den window's cargo take.
    (
        crate::merc_den::MERCENARY_DEN,
        crate::merc_den::locate,
        &[("ExtractInfomorphsToShip", crate::merc_den::extract)],
    ),
    // ANSI-2a (#86): the Ansiblex window's capacitor and access answer.
    (
        crate::jump_bridge::COMPONENT,
        crate::jump_bridge::locate,
        &[("GetJumpBridgeInfo", crate::jump_bridge::info)],
    ),
];

/// The notify text the client is given when the caller or the item is not in
/// space. **[A]**
pub const NOT_IN_SPACE_TEXT: &str = "The target is not in space here.";

/// The refusal for a caller or item that is not in space.
pub fn not_in_space() -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(NOT_IN_SPACE_TEXT))]),
    ))
}

/// The system the caller's ship is in, or `None` when docked.
pub fn actor_system(reg: &Registry, actor: i64) -> Option<i64> {
    reg.roster().get(actor).map(|p| p.solar_system_id)
}

/// The call: route through [`CALLS`].
pub fn call(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    call_in(CALLS, reg, req)
}

/// [`call`] against an explicit table (the tests use their own).
pub fn call_in(
    table: &[Component<'_>],
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let item = as_int(req.args.first());
    let component = as_text(req.args.get(1));
    let method = as_text(req.args.get(2));
    let (Some(item), Some(component), Some(method)) = (item, component, method) else {
        return Err(ServiceError::BadArguments {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
            why: "expected (itemID, componentName, methodName, ...)".to_owned(),
        });
    };
    let found = table
        .iter()
        .find(|(c, _, _)| *c == component)
        .and_then(|(_, locate, methods)| {
            methods
                .iter()
                .find(|(m, _)| *m == method)
                .map(|(_, f)| (*locate, *f))
        });
    let Some((locate, handler)) = found else {
        return Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: format!("{}:{component}.{method}", req.method),
        });
    };
    let Some(actor) = req.actor else {
        return Err(not_in_space());
    };
    if crate::jump_bridge::quiet_in_transit(reg, actor, item, &component, &method) {
        return Ok(PyValue::None);
    }
    let Some(system) = actor_system(reg, actor) else {
        return Err(not_in_space());
    };
    if locate(reg, item) != Some(system) {
        return Err(not_in_space());
    }
    handler(
        reg,
        &CompCall {
            actor,
            system,
            item_id: item,
            args: &req.args[3..],
            kwargs: req.kwargs,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;
    use crate::space_state::{Motion, Presence};

    const PILOT: i64 = 7_000_001;
    const SHIP: i64 = 7_000_002;
    const SYSTEM: i64 = 7_000_003;
    const ITEM: i64 = 7_000_004;

    /// Echo the decoded call so the test can see what the handler got.
    fn echo(_: &mut Registry, c: &CompCall<'_>) -> Result<PyValue, ServiceError> {
        Ok(PyValue::Tuple(vec![
            PyValue::Int(c.actor),
            PyValue::Int(c.item_id),
            PyValue::List(c.args.to_vec()),
            PyValue::Int(c.system),
            PyValue::Int(c.kwargs.len() as i64),
        ]))
    }

    const FAR_ITEM: i64 = 7_000_005;
    const OTHER_SYSTEM: i64 = 7_000_006;

    /// The test component knows `ITEM` in `SYSTEM` and `FAR_ITEM` elsewhere.
    fn locate(_: &Registry, item: i64) -> Option<i64> {
        match item {
            ITEM => Some(SYSTEM),
            FAR_ITEM => Some(OTHER_SYSTEM),
            _ => None,
        }
    }

    const TABLE: &[Component<'static>] = &[("testComp", locate, &[("Ping", echo)])];

    fn world() -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.roster_mut().arrive(Presence::arriving(
            PILOT,
            SHIP,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        reg
    }

    fn run(
        reg: &mut Registry,
        actor: Option<i64>,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        run_kw(reg, actor, args, &[])
    }

    fn run_kw(
        reg: &mut Registry,
        actor: Option<i64>,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "beyonce",
            method: "CallComponentFromClient",
            args,
            kwargs,
            bind_params: None,
            actor,
        };
        call_in(TABLE, reg, &req)
    }

    fn ping_args(component: &str, method: &str) -> Vec<PyValue> {
        vec![
            PyValue::Int(ITEM),
            PyValue::Bytes(component.as_bytes().to_vec()),
            PyValue::Unicode(method.to_owned()),
            PyValue::Int(11),
            PyValue::Int(22),
        ]
    }

    #[test]
    fn a_registered_handler_gets_the_decoded_call() {
        let mut reg = world();
        let got = run(&mut reg, Some(PILOT), &ping_args("testComp", "Ping")).expect("routed");
        assert_eq!(
            got,
            PyValue::Tuple(vec![
                PyValue::Int(PILOT),
                PyValue::Int(ITEM),
                PyValue::List(vec![PyValue::Int(11), PyValue::Int(22)]),
                PyValue::Int(SYSTEM),
                PyValue::Int(0),
            ])
        );
    }

    #[test]
    fn keyword_arguments_reach_the_handler() {
        let mut reg = world();
        let kw = [(PyValue::Unicode("typeAndQty".to_owned()), PyValue::Int(3))];
        let got =
            run_kw(&mut reg, Some(PILOT), &ping_args("testComp", "Ping"), &kw).expect("routed");
        let PyValue::Tuple(t) = got else {
            panic!("tuple")
        };
        assert_eq!(t[4], PyValue::Int(1));
    }

    #[test]
    fn an_item_in_another_system_is_refused_before_the_handler() {
        let mut reg = world();
        let mut args = ping_args("testComp", "Ping");
        args[0] = PyValue::Int(FAR_ITEM);
        assert!(matches!(
            run(&mut reg, Some(PILOT), &args),
            Err(ServiceError::Raise(_))
        ));
        // An item the component does not know at all is refused the same way.
        args[0] = PyValue::Int(ITEM + 100);
        assert!(matches!(
            run(&mut reg, Some(PILOT), &args),
            Err(ServiceError::Raise(_))
        ));
    }

    #[test]
    fn an_unknown_component_or_method_is_refused_by_name() {
        let mut reg = world();
        for (c, m) in [("noSuchComp", "Ping"), ("testComp", "NoSuchMethod")] {
            match run(&mut reg, Some(PILOT), &ping_args(c, m)) {
                Err(ServiceError::NoSuchMethod { service, method }) => {
                    assert_eq!(service, "beyonce");
                    assert!(method.contains(c) && method.contains(m), "{method}");
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_pilot_not_in_space_is_refused_before_the_handler() {
        let mut reg = world();
        let docked = 7_000_099;
        assert!(matches!(
            run(&mut reg, Some(docked), &ping_args("testComp", "Ping")),
            Err(ServiceError::Raise(_))
        ));
        assert!(matches!(
            run(&mut reg, None, &ping_args("testComp", "Ping")),
            Err(ServiceError::Raise(_))
        ));
    }

    /// The jump-bridge exception (`jump_bridge::quiet_in_transit`) is that
    /// component's alone: any other component's call from a pilot mid-jump is
    /// still refused, about an item in the system the session still names.
    #[test]
    fn a_pilot_mid_jump_is_still_refused_by_any_other_component() {
        let mut reg = world();
        reg.roster_mut().leave(PILOT);
        crate::jump::arm_at(&mut reg, PILOT, SYSTEM, OTHER_SYSTEM, [0.0; 3]);
        assert_eq!(reg.jumps().session_system(PILOT), Some(SYSTEM));
        assert!(matches!(
            run(&mut reg, Some(PILOT), &ping_args("testComp", "Ping")),
            Err(ServiceError::Raise(_))
        ));
    }

    #[test]
    fn short_or_mistyped_arguments_are_a_bad_call_not_a_panic() {
        let mut reg = world();
        assert!(matches!(
            run(&mut reg, Some(PILOT), &[PyValue::Int(ITEM)]),
            Err(ServiceError::BadArguments { .. })
        ));
        assert!(matches!(
            run(&mut reg, Some(PILOT), &[]),
            Err(ServiceError::BadArguments { .. })
        ));
    }

    #[test]
    fn the_shipped_table_does_not_answer_the_test_component() {
        let mut reg = world();
        let args = ping_args("testComp", "Ping");
        let req = Request {
            service: "beyonce",
            method: "CallComponentFromClient",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(PILOT),
        };
        assert!(matches!(
            call(&mut reg, &req),
            Err(ServiceError::NoSuchMethod { .. })
        ));
    }

    /// Through the registry's own dispatcher: the unknown pair reaches the
    /// seam and comes back as `NoSuchMethod` naming the pair (the dispatcher
    /// then answers `None` and logs it).
    #[test]
    fn an_unknown_pair_through_the_registry_names_the_pair() {
        let mut reg = world();
        let args = ping_args("noSuchComp", "NoSuchMethod");
        let req = Request {
            service: "beyonce",
            method: "CallComponentFromClient",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(PILOT),
        };
        match reg.dispatch(&req) {
            Err(ServiceError::NoSuchMethod { service, method }) => {
                assert_eq!(service, "beyonce");
                assert!(method.contains("noSuchComp.NoSuchMethod"), "{method}");
            }
            other => panic!("expected NoSuchMethod, got {other:?}"),
        }
    }
}
