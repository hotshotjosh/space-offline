//! **Ansiblex jump gates** (issue #85, ANSI-1): two Upwell jump gates, one in
//! each of two systems, each pointing at the other's system, and a jump
//! through one that lands the ship at the other.
//!
//! # What the client does **[V]** (decompiled 3662168; line numbers are its)
//!
//! - **Deploying.** `structureDeployment.ConfirmDeployment:160-167` puts the
//!   chosen system into the `extraConfig` of `Anchor` as
//!   `destinationSolarsystemID`; a gate is anchored *aimed at* a system, and
//!   its name carries it (`structures/deployment.GetStructureNamePrefix:151`,
//!   `"<here> <link char> <there> - "`). One gate per system
//!   (`MAX_GROUP_PER_SOLARSYSTEM`, `deployment.py:45`), 100,000 km from a
//!   stargate (`:40`), nullsec only (`GROUPS_REQUIRING_CONQUERABLE_SYSTEM`,
//!   `:42`). The window lists systems within `MAX_JUMPBRIDGE_RANGE_LIGHTYEARS`
//!   = 5 (`structures/const.py:192`) and marks a system whose gate already
//!   points here (`stargateDeploymentCont.LoadListAndSetFilter:41-45`).
//! - **The link rule.** A gate A in system X aimed at Y *links to* the gate B
//!   in Y aimed back at X, when both belong to the same owner (the alliance;
//!   a corporation outside any alliance is its own group here [A]).
//!   [`partner`] is that rule; nothing is stored for the pair itself, only
//!   each gate's destination ([`KIND`]).
//! - **Calls on `structureJumpBridgeMgr`**: `GetLinkedStructure(itemID)` →
//!   `(destinationStructureID, destinationSolarsystemID)` or falsy
//!   (`infosvc.py:800-806`); `GetJbStructureDestination(structureID)` → the
//!   system id the gate is aimed at, falsy for none (`structureDirectory.py:
//!   169`, read by `menuFunctions._SetStructureName:351-356`, which raises
//!   `JumpGateHasNoDestination` for a falsy answer);
//!   `CmdJumpThroughStructureStargate(structureID)` inside
//!   `sessionMgr.PerformSessionChange` (`menusvc.py:2095`, `autopilot.py:
//!   338`), whose truthy answer is `didJumpThrough`.
//! - **On `structureDirectory`**: `GetJumpBridgesWithMyAccess()` →
//!   `(pairs, hasAccessTo, hasNoAccessTo)`, pairs of `KeyVal(solarSystemID,
//!   structureID)` (`mapView.LoadAllJumpBridges:1374-1394`;
//!   `fakeDataUtil.GetJumpBridgeData:148-161`); `GetNearbyJumpBridges()` →
//!   rows with `solarSystemID`, `structureID`, `typeID`, `structureName`,
//!   `ownerID`, `alignedToCurrentSystem`
//!   (`structureDeployment._GetNearbyJumpBridges:237-239`,
//!   `stargateSystemEntry.LoadUI:38-52`).
//! - **The slim.** The gate bracket is the `JumpBridge` space component
//!   (`jumpbridge/client/jumpBridge.py`): `_OnSlimItemUpdate:56-66` reads
//!   `component_jumpBridge` (an etag) and `activationState`, both as plain
//!   attributes, and `jumpBridgeContController.get_target_system_name:43`
//!   reads `targetSolarsystemID` (falsy → "unlinked"). The menu entry
//!   "Jump using bridge" is offered when the type is an Upwell stargate with
//!   the component, `targetSolarsystemID` is set and the structure is not
//!   offline (`celestialCheckers.OfferActivateJumpBridge:320-334`,
//!   `IsConnectedJumpBridgeStructure:1798-1806`). [`slim_keys`] sends all
//!   three, appended to the structure's slim by `structure_space::extras_of`.
//! - **The client's own refusals** (`jumpBridge.ValidateJumpAttempt:93-98`,
//!   run before the call): `JumpBridgeRestrictedShip{shipName}` for a hull in
//!   the type's restriction list, `NotCloseEnoughToJump{maxdist}` beyond
//!   `maxStargateJumpingDistance` = 2,500 m. [`jump_through`] repeats the
//!   range check (a modified client could skip it); the hull list is a seam
//!   ([`restricted_hull`]).
//!
//! # The jump
//!
//! The same sequence as a stargate's and a wormhole's (`crate::jump`,
//! `crate::wormhole_jump`): the departure FX to the jumper and everyone whose
//! park holds the ship, locks, roster and park left, the departure removal on
//! its clock, and [`crate::jump::arm_at`] for the session change after
//! `JUMP_DELAY` and the bind that lands the ship beside the partner gate.
//! Nothing here moves a ship by itself.
//!
//! # Not here (issue #86)
//!
//! **Fuel**, the **sov-upgrade gate** and the **access profile setting**
//! (`SETTING_JUMP_BRIDGE_ACTIVATION`, 34) are #86's. Their seams are
//! [`may_use`] (today: a member of the gate owner's group, the interim rule),
//! [`take_fuel`] (today: free) and [`restricted_hull`] (today: none).
//!
//! # `[A]`s
//!
//! - A corporation outside an alliance is its own owner group. Falsified by a
//!   corp-owned pair that Tranquility refuses to link without an alliance.
//! - Surface distance to the gate is what the client's `DistanceBetween`
//!   compares with 2,500 m; the server adds [`RANGE_SLACK`]. Falsified by an
//!   honest jump refused with `NotCloseEnoughToJump` (the `[jbridge] out of
//!   range` line gives the distance).
//! - `component_jumpBridge` is 0 and `activationState` is `None`: no charge
//!   or activation phase exists yet. Falsified by a gate bracket that draws
//!   nothing or logs on a missing etag.
//! - The departure effect is the stargate's (`effects.JumpOut`, `targetID`
//!   the gate). Falsified by a jump tunnel that never starts, or a client log
//!   line about the gate ball.
//! - `GetNearbyJumpBridges` returns every gate of the pilot's group, not only
//!   those within 5 ly: the client looks its own near systems up in the
//!   answer (`structureDeployment._GetNearbyJumpBridges:239` is a dict by
//!   system). Falsified by a client that iterates the whole answer.
//! - A gate removed from space leaves a stale [`KIND`] row; ids are never
//!   reused, and every reader joins the row to a live structure.

use crate::delta::Deltas;
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::structure::{self, Structure};
use eve_marshal::PyValue;

/// `structure_id -> destination_system`: where a gate is aimed. Written only
/// here (`delta_kinds.rs`, coordinator).
pub const KIND: &str = "jbridge";

/// The services this module answers (`service::DOMAINS`, coordinator-only).
pub const SERVICES: &[&str] = &["structureJumpBridgeMgr"];

/// The Ansiblex **[V]**: `structure::size_of_type` lists it as a flex
/// structure, and the content bundle's component table gives it (and no other
/// flex type) the `structureJumpBridge` component.
pub const TYPE_ANSIBLEX: i64 = 35_841;

/// `const.maxStargateJumpingDistance`, metres **[V]** (`appConst.py:1468`).
pub const MAX_JUMP_DISTANCE: f64 = crate::jump::MAX_JUMP_DISTANCE;

/// What the server adds to [`MAX_JUMP_DISTANCE`] before it refuses, metres:
/// the same lag allowance as `wormhole_jump::ENTER_RANGE_SLACK` **[A]**.
pub const RANGE_SLACK: f64 = 1_500.0;

/// `SETTING_JUMP_BRIDGE_ACTIVATION` **[V]** (`structures/settings.py:141`),
/// whose denial key is `ActivateJumpBridgeDenied`.
pub const SETTING_ACTIVATE: i64 = 34;

/// `NotCloseEnoughToJump` **[V key]** (`jumpBridge.py:97`), kwarg `maxdist`.
pub const TOO_FAR: &str = "NotCloseEnoughToJump";
/// `JumpBridgeRestrictedShip` **[V key]** (`jumpBridge.py:94`), kwarg
/// `shipName`.
pub const RESTRICTED_SHIP: &str = "JumpBridgeRestrictedShip";

// -- the model ----------------------------------------------------------------

/// Whether a structure type is an Ansiblex.
pub fn is_gate(type_id: i64) -> bool {
    type_id == TYPE_ANSIBLEX
}

fn row_key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

/// The system gate `id` is aimed at, if one was set.
pub fn destination(d: &Deltas, id: i64) -> Option<i64> {
    d.of(KIND)
        .get(&row_key(id))
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|s| *s != 0)
}

/// Aim gate `id` at `system`.
pub fn set_destination(d: &mut Deltas, id: i64, system: i64) {
    d.put(KIND, row_key(id), vec![system.to_string()]);
}

/// Forget where gate `id` was aimed.
pub fn clear_destination(d: &mut Deltas, id: i64) -> bool {
    d.del(KIND, &row_key(id)).is_some()
}

/// Whether two owner corporations are one owner: the same alliance, else the
/// same corporation **[A]** (module header).
pub fn same_owner(d: &Deltas, a: i64, b: i64) -> bool {
    if a == b {
        return true;
    }
    match (
        crate::alliance::of_corp(d, a),
        crate::alliance::of_corp(d, b),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The gate `gate` links to: in the system it is aimed at, an Ansiblex of
/// the same owner aimed back at `gate`'s system. The lowest id wins when a
/// hand-made world holds two. Online or not: a link is a fact of the two
/// rows, [`usable`] is the state.
pub fn partner(d: &Deltas, gate: &Structure) -> Option<Structure> {
    if !is_gate(gate.type_id) {
        return None;
    }
    let there = destination(d, gate.id)?;
    structure::in_system(d, there)
        .into_iter()
        .filter(|b| {
            b.id != gate.id
                && is_gate(b.type_id)
                && destination(d, b.id) == Some(gate.system)
                && same_owner(d, gate.owner_corp, b.owner_corp)
        })
        .min_by_key(|b| b.id)
}

/// A gate that can be jumped through or to: online, not disabled.
pub fn usable(s: &Structure) -> bool {
    s.is_online() && !s.is_disabled()
}

/// Every linked pair once, lower id first.
pub fn links(d: &Deltas) -> Vec<(Structure, Structure)> {
    let mut out = Vec::new();
    for g in structure::all(d).into_iter().filter(|s| is_gate(s.type_id)) {
        if let Some(p) = partner(d, &g) {
            // Mutual: the partner's own rule must name `g` back, else the
            // two rows disagree (two gates aimed at one, one aimed back).
            let back = partner(d, &p).is_some_and(|q| q.id == g.id);
            if back && g.id < p.id {
                out.push((g, p));
            }
        }
    }
    out
}

/// The owner group's key for one corporation: its alliance, else itself.
fn group_of(d: &Deltas, corp: i64) -> (bool, i64) {
    crate::alliance::of_corp(d, corp).map_or((false, corp), |a| (true, a))
}

// -- the seams of #86 --------------------------------------------------------------

/// **OWNER SWITCH (SOV-UPG-2, #80), shipped `false`.** `true`: a gate
/// works only while the sov hub of **both** its ends' systems has an Online
/// upgrade that enables `jump_bridge`, so every linked gate stops working until
/// its systems have one. `false`: the old rule, no upgrade needed (a level-3
/// upgrade needs 35 days of held claim, so `true` would stop every live gate on
/// deploy). **[A]** the gate does not check that the structure's owner is the
/// system's holder (only the holder may anchor on TQ); falsifier: an Ansiblex
/// of a non-holder working off the holder's upgrade on TQ.
pub const ANSIBLEX_NEEDS_UPGRADE: bool = false;

/// Whether the sov upgrade rule lets `gate` (and the partner it is linked to,
/// when it has one) work. `needs` names [`ANSIBLEX_NEEDS_UPGRADE`]; `t` is
/// `sov_upgrade::table(reg.content())`. An unlinked gate is asked about its own
/// system only.
pub fn upgraded_with(
    d: &Deltas,
    t: &crate::sov_upgrade::UpgradeTable,
    gate: &Structure,
    needs: bool,
) -> bool {
    use crate::sov_upgrade::{Want, upgrade_online};
    if !needs {
        return true;
    }
    let ends = std::iter::once(gate.system).chain(partner(d, gate).map(|p| p.system));
    ends.into_iter()
        .all(|s| upgrade_online(d, t, s, Want::Service("jump_bridge")))
}

#[cfg(test)]
thread_local! {
    /// A test turns the gate on for its own thread, whatever the shipped switch says.
    pub(crate) static FORCE_GATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`ANSIBLEX_NEEDS_UPGRADE`] (in a test, or'ed with the thread's [`FORCE_GATE`]).
fn needs_upgrade() -> bool {
    #[cfg(test)]
    if FORCE_GATE.with(std::cell::Cell::get) {
        return true;
    }
    ANSIBLEX_NEEDS_UPGRADE
}

/// [`upgraded_with`] on the registry's table and the named switch.
pub fn upgraded(reg: &Registry, gate: &Structure) -> bool {
    upgraded_with(
        reg.deltas(),
        crate::sov_upgrade::table(reg.content()),
        gate,
        needs_upgrade(),
    )
}

/// Whether `actor` may use `gate`. **#86 replaces this** with the structure
/// profile's `SETTING_ACTIVATE` (`structure_profile::has_access`). Today: a
/// pilot of the gate owner's group, and (SOV-UPG-2) the sov-upgrade gate at
/// both ends ([`upgraded`]).
pub fn may_use(reg: &Registry, actor: i64, gate: &Structure) -> bool {
    reg.character(actor)
        .is_some_and(|c| same_owner(reg.deltas(), c.corporation_id, gate.owner_corp))
        && upgraded(reg, gate)
}

/// Take the jump's fuel. **#86 fills this in**; free today.
pub fn take_fuel(_reg: &mut Registry, _actor: i64, _gate: &Structure) -> Result<(), ServiceError> {
    Ok(())
}

/// Whether the hull may not use the bridge (the type's `restrictionTypeList`,
/// `jumpBridge.py:54` / `:93`). The list is not in the content bundle, so no
/// hull is restricted yet; **#86** names it.
pub fn restricted_hull(_reg: &Registry, _ship_type: i64) -> bool {
    false
}

// -- the slim -----------------------------------------------------------------------

/// The keys a gate's slim carries beyond a structure's (module header):
/// `targetSolarsystemID` is the linked partner's system, `None` while
/// unlinked; `jumps` is the stargate's one-row list. Empty for any other structure. Called from
/// `structure_space::extras_of`.
pub fn slim_keys(reg: &Registry, s: &Structure) -> Vec<(&'static str, PyValue)> {
    if !is_gate(s.type_id) {
        return Vec::new();
    }
    let partner = partner(reg.deltas(), s);
    let target = partner
        .as_ref()
        .map_or(PyValue::None, |p| PyValue::Int(p.system));
    // `spaceMgr.OnSpecialFX:478-481` (the `effects.JumpOut` of our own
    // departure) reads `slimItem.jumps[0]` with no guard, and the autopilot
    // walks `destItem.jumps` for any Upwell gate (`autopilot.py:309`) **[V]**:
    // a linked gate carries the stargate's row, an unlinked one an empty list.
    let jumps = partner.as_ref().map_or(Vec::new(), |p| {
        vec![crate::destiny::jumps_row(p.id, p.system)]
    });
    vec![
        ("jumps", PyValue::List(jumps)),
        ("targetSolarsystemID", target),
        ("component_jumpBridge", PyValue::Int(0)),
        ("activationState", PyValue::None),
    ]
}

/// Tell the system's pilots the slim of every gate in `systems` as it now
/// reads. Called after a link changes.
fn refresh_slims(reg: &mut Registry, systems: &[i64]) {
    for system in systems {
        for g in structure::in_system(reg.deltas(), *system)
            .into_iter()
            .filter(|s| is_gate(s.type_id))
        {
            crate::structure_space::announce_slim(reg, &g);
        }
    }
}

/// `Anchor`'s hook (one line in `service/structures/deploy.rs`): a gate
/// anchored with `extraConfig.destinationSolarsystemID` is aimed there
/// **[V]** (`structureDeployment.ConfirmDeployment:163-167`).
pub fn on_anchored(reg: &mut Registry, s: &Structure, extra: Option<&PyValue>) {
    if !is_gate(s.type_id) {
        return;
    }
    let dest = match extra {
        Some(PyValue::Instance { state, .. }) => {
            crate::pyutil::dict_get(state, b"destinationSolarsystemID")
        }
        Some(d @ PyValue::Dict(_)) => crate::pyutil::dict_get(d, b"destinationSolarsystemID"),
        _ => None,
    };
    let Some(PyValue::Int(dest)) = dest else {
        return;
    };
    set_destination(reg.deltas_mut(), s.id, *dest);
    reg.mark_dirty();
    refresh_slims(reg, &[s.system, *dest]);
}

// -- the directory ------------------------------------------------------------------

fn entry(s: &Structure) -> PyValue {
    crate::pyutil::keyval(vec![
        ("solarSystemID", PyValue::Int(s.system)),
        ("structureID", PyValue::Int(s.id)),
    ])
}

/// `structureDirectory.GetJumpBridgesWithMyAccess()`: every linked pair, and
/// which gates the pilot has access to (the ones [`may_use`] lets them use).
/// Lists, because our marshal writer has no set form; element 2 is never
/// read **[V]** (`mapView`).
pub fn with_my_access(reg: &Registry, actor: Option<i64>) -> PyValue {
    let mut pairs = Vec::new();
    let mut yes = Vec::new();
    let mut no = Vec::new();
    for (a, b) in links(reg.deltas()) {
        pairs.push(PyValue::Tuple(vec![entry(&a), entry(&b)]));
        for g in [&a, &b] {
            let mine = actor.is_some_and(|me| may_use(reg, me, g));
            (if mine { &mut yes } else { &mut no }).push(PyValue::Int(g.id));
        }
    }
    PyValue::Tuple(vec![
        PyValue::List(pairs),
        PyValue::List(yes),
        PyValue::List(no),
    ])
}

/// `structureDirectory.GetNearbyJumpBridges()`: one row per gate of the
/// pilot's owner group, outside the pilot's system or in it **[A]** (module
/// header). `alignedToCurrentSystem` is "this gate is aimed at where the pilot
/// is" (`stargateDeploymentCont.py:42`).
pub fn nearby(reg: &Registry, actor: Option<i64>) -> PyValue {
    let (Some(me), Some(here)) = (actor, actor.and_then(|a| reg.session_system(a))) else {
        return PyValue::List(Vec::new());
    };
    let Some(corp) = reg.character(me).map(|c| c.corporation_id) else {
        return PyValue::List(Vec::new());
    };
    let mine = group_of(reg.deltas(), corp);
    let rows = structure::all(reg.deltas())
        .into_iter()
        .filter(|s| is_gate(s.type_id) && group_of(reg.deltas(), s.owner_corp) == mine)
        .map(|s| {
            crate::pyutil::keyval(vec![
                ("solarSystemID", PyValue::Int(s.system)),
                ("structureID", PyValue::Int(s.id)),
                ("typeID", PyValue::Int(s.type_id)),
                ("structureName", PyValue::Unicode(s.name.clone())),
                ("ownerID", PyValue::Int(s.owner_corp)),
                (
                    "alignedToCurrentSystem",
                    PyValue::Bool(destination(reg.deltas(), s.id) == Some(here)),
                ),
            ])
        })
        .collect();
    PyValue::List(rows)
}

// -- structureJumpBridgeMgr -----------------------------------------------------------

/// The service's methods.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // `(destinationStructureID, destinationSolarsystemID)`, or `None`
        // (`infosvc.py:800`): the partner, linked and nothing else.
        "GetLinkedStructure" => {
            let Some(gate) = Args::new(req)
                .int(0)
                .and_then(|id| structure::get(reg.deltas(), id))
            else {
                return Ok(PyValue::None);
            };
            Ok(partner(reg.deltas(), &gate).map_or(PyValue::None, |p| {
                PyValue::Tuple(vec![PyValue::Int(p.id), PyValue::Int(p.system)])
            }))
        }
        // The system the gate is aimed at: the configured destination, even
        // while the other end is not built (`menuFunctions.py:352`).
        "GetJbStructureDestination" => {
            let dest = Args::new(req).int(0).and_then(|id| {
                structure::get(reg.deltas(), id)
                    .filter(|s| is_gate(s.type_id))
                    .and_then(|s| destination(reg.deltas(), s.id))
            });
            Ok(dest.map_or(PyValue::None, PyValue::Int))
        }
        "CmdJumpThroughStructureStargate" => jump_through(reg, req),
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

fn user_error(key: &str, kw: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(kw)))
}

/// Our own words, for refusals about this server (as `jump.rs`'s).
fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// The surface distance from the pilot's ship to the gate, if both are balls.
fn surface_distance(reg: &Registry, ship_id: i64, gate: &Structure) -> Option<f64> {
    let me = reg.ball_target(ship_id)?;
    let r = crate::structure_space::radius(reg.content(), gate.type_id);
    Some(crate::space_state::distance(me.position, gate.pos) - me.radius - r)
}

/// Where the `index`-th ship to arrive lands beside a gate at `at` of
/// `radius`: [`crate::jump::ARRIVAL_CLEARANCE`] outside its surface on the
/// star's side, stepped per arriver as at a stargate **[A]** (as
/// `jump::arrival_position`).
pub fn arrival_position(at: [f64; 3], radius: f64, index: usize) -> [f64; 3] {
    let inward = crate::space_state::unit([-at[0], -at[1], -at[2]]).unwrap_or([1.0, 0.0, 0.0]);
    let out = radius + crate::jump::ARRIVAL_CLEARANCE;
    let spot = [
        at[0] + inward[0] * out,
        at[1] + inward[1] * out,
        at[2] + inward[2] * out,
    ];
    crate::destiny::berth(spot, at, index)
}

/// `CmdJumpThroughStructureStargate(structureID)`: answered `True`
/// (`didJumpThrough`, `menusvc.py:2096`); the effect is the sequence in the
/// module header. Refused, with nothing changed, when the caller is not in
/// space or already jumping, names no gate in their system, a guard says no
/// (aggression timer, CONCORD, `movement_guard`), the gate has no online
/// partner, [`may_use`] says no, the hull is restricted, or the ship is out of
/// range.
pub fn jump_through(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(gate_id) = Args::new(req).int(0) else {
        return Err(notify(
            "CmdJumpThroughStructureStargate needs (structureID,).",
        ));
    };
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    if reg.jumps().contains(actor) {
        return Err(notify("You are already jumping."));
    }
    let origin = here.solar_system_id;
    let Some(gate) =
        structure::get(reg.deltas(), gate_id).filter(|g| is_gate(g.type_id) && g.system == origin)
    else {
        return Err(notify("That jump gate is not in this system."));
    };
    // The same refusals as a stargate's (`jump::cmd_stargate_jump`).
    crate::concord::refuse_jump(reg, Some(actor), crate::pyutil::filetime_now())?;
    crate::crimewatch::refuse_jump(reg, Some(actor), crate::pyutil::filetime_now())?;
    if let Some(refusal) =
        crate::movement_guard::check(&crate::module::plan().jump_guards, reg, actor)
    {
        return Err(refusal.into_error());
    }
    let Some(there) = partner(reg.deltas(), &gate) else {
        return Err(notify("This jump gate is not linked to another."));
    };
    if !usable(&gate) || !usable(&there) {
        return Err(notify(
            "This jump gate, or the one it links to, is offline.",
        ));
    }
    if !upgraded(reg, &gate) {
        return Err(notify(
            "This jump gate, or the one it links to, is in a system without the jump bridge sovereignty upgrade.",
        ));
    }
    if !may_use(reg, actor, &gate) {
        return Err(user_error(
            crate::structure_profile::access_error(SETTING_ACTIVATE),
            vec![],
        ));
    }
    let ship_type = reg.character(actor).map_or(0, |c| c.ship_type_id);
    if restricted_hull(reg, ship_type) {
        // `shipName` is the hull's name on the client; the server has no type
        // names, so the id stands in until #86 names the list **[A]**.
        return Err(user_error(
            RESTRICTED_SHIP,
            vec![("shipName", crate::pyutil::bstr(&ship_type.to_string()))],
        ));
    }
    // The client's range gate, repeated: a modified client could skip it.
    if let Some(d) =
        surface_distance(reg, here.ship_id, &gate).filter(|d| *d > MAX_JUMP_DISTANCE + RANGE_SLACK)
    {
        crate::outln!("[jbridge] out of range: {actor} is {d:.0} m from gate {gate_id}");
        return Err(user_error(
            TOO_FAR,
            vec![("maxdist", PyValue::Int(MAX_JUMP_DISTANCE as i64))],
        ));
    }
    take_fuel(reg, actor, &gate)?;

    let dest_system = there.system;
    let index = reg
        .roster()
        .others_in_system(dest_system, actor)
        .into_iter()
        .filter(|id| reg.character(*id).is_some())
        .count();
    let radius = crate::structure_space::radius(reg.content(), there.type_id);
    let arrival = arrival_position(there.pos, radius, index);

    // From here on, `jump::cmd_stargate_jump`'s sequence in its order.
    let mut audience = vec![actor];
    audience.extend(
        reg.roster()
            .others_in_system(origin, actor)
            .into_iter()
            .filter(|id| reg.character(*id).is_some()),
    );
    let stamp = reg.next_destiny_stamp();
    reg.push(crate::jump::JumpFx {
        to: audience.clone(),
        stamp,
        ship: here.ship_id,
        gate: gate.id,
        dest_system,
    });
    crate::targeting::left_space(
        reg,
        actor,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::WithAnswer,
    );
    let held = crate::jump_drive::park_held(reg, actor, here.ship_id);
    reg.roster_mut().leave(actor);
    crate::grid::forget(reg, actor);
    crate::jump_drive::schedule_departure_removal(reg, stamp, here.ship_id, held, audience);
    crate::jump::arm_at(reg, actor, origin, dest_system, arrival);
    crate::outln!(
        "[jbridge] character {actor} ship {}: gate {} in {origin} -> gate {} in {dest_system}",
        here.ship_id,
        gate.id,
        there.id
    );
    Ok(PyValue::Bool(true))
}

// -- GM: /jbridge ---------------------------------------------------------------------

/// `/jbridge link <gateID> <systemID>`, `/jbridge unlink <gateID>` and
/// `/jbridge list`: aim a gate by hand (the Upwell anchor flow does it with
/// `extraConfig`; `/upwell spawn` has none). A GM tool, not a game rule.
pub(crate) fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    const USAGE: &str =
        "/jbridge link <gateID> <systemID> | /jbridge unlink <gateID> | /jbridge list";
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let gate_of = |reg: &Registry, word: Option<&&str>| {
        word.and_then(|w| w.parse::<i64>().ok())
            .and_then(|id| structure::get(reg.deltas(), id))
            .filter(|s| is_gate(s.type_id))
    };
    match args.first().copied() {
        Some("link") => {
            let Some(gate) = gate_of(reg, args.get(1)) else {
                return say("not a jump gate; /upwell list shows the structures");
            };
            let Some(dest) = args
                .get(2)
                .and_then(|w| w.parse::<i64>().ok())
                .filter(|d| *d != gate.system && *d != 0)
            else {
                return say(USAGE);
            };
            set_destination(reg.deltas_mut(), gate.id, dest);
            reg.mark_dirty();
            refresh_slims(reg, &[gate.system, dest]);
            let linked = structure::get(reg.deltas(), gate.id)
                .and_then(|g| partner(reg.deltas(), &g))
                .map(|p| format!("linked to gate {} in {}", p.id, p.system))
                .unwrap_or_else(|| "no gate aimed back yet".to_owned());
            say(&format!("gate {} aimed at {dest}: {linked}", gate.id))
        }
        Some("unlink") => {
            let Some(gate) = gate_of(reg, args.get(1)) else {
                return say("not a jump gate");
            };
            let there = destination(reg.deltas(), gate.id);
            clear_destination(reg.deltas_mut(), gate.id);
            reg.mark_dirty();
            let mut systems = vec![gate.system];
            systems.extend(there);
            refresh_slims(reg, &systems);
            say(&format!("gate {} is aimed at nothing", gate.id))
        }
        Some("list") => {
            let lines: Vec<String> = structure::all(reg.deltas())
                .iter()
                .filter(|s| is_gate(s.type_id))
                .map(|s| {
                    format!(
                        "gate {} in {} aimed at {} partner {}",
                        s.id,
                        s.system,
                        destination(reg.deltas(), s.id).map_or("-".to_owned(), |d| d.to_string()),
                        partner(reg.deltas(), s).map_or("-".to_owned(), |p| p.id.to_string()),
                    )
                })
                .collect();
            say(&if lines.is_empty() {
                "no jump gates".to_owned()
            } else {
                lines.join("\n")
            })
        }
        _ => say(USAGE),
    }
}

#[cfg(test)]
mod tests;
