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
//! # The charge, the zone cost and the access rule (issue #86, ANSI-2a)
//!
//! **A jump burns no fuel: it draws on the gate's own capacitor** **[V]**
//! (`spacecomponents/client/ui/jumpbridge/jumpBridgeContController.py:28-29,
//! 49-63,159-186`; the manual text `CapacitorUsageText`). The gate's capacity
//! and recharge time are its type's attributes 482 and 55 (A7); the ship's
//! cost is its hull's attribute 6364 times the multiplier of the **zone of
//! the destination system**, by its distance from the pilot alliance's
//! capital (`zone_modifier_util.py`, [`zone_index`]). The charge recharges on
//! the ship capacitor's curve ([`charge_at`]); a new gate starts at 0.
//! [`take_charge`] settles it, refuses when the cost is more than it holds
//! and otherwise stores the rest. The window asks
//! `GetJumpBridgeInfo` (`jumpBridge.py:131-134`), answered by [`info`] with
//! `jumpbridge.common.jump_bridge_authorized_info.JumpBridgeAuthorizedInfo`, a
//! new-style object ([`info_at`]); for a gate not in the caller's
//! system it is `None` ([`quiet_info`]).
//! The `jbcap` row (`structure_id -> charge_gj settled_at counter`) holds the
//! settled charge; the counter is the slim's `component_jumpBridge` etag, so
//! a change makes the bracket ask again (`jumpBridge.py:56-73`).
//!
//! **Access** ([`may_use`], `JumpRestrictionsText`): the pilot's alliance
//! holds the sovereignty of the gate's system, the structure profile gives
//! `SETTING_ACTIVATE` (a profile with no activation row lets its owner group
//! use it, the interim rule), and the sov upgrade rule holds ([`upgraded`]).
//! The hull list is the gate type's `restrictionTypeList` from
//! `deploy/jump-bridges.tsv` ([`Bridges`], written by ANSI-2b), resolved
//! through `typelists.tsv`; the same file carries the five zone multipliers.
//! Without the file every multiplier is 1.0 and no hull is listed.
//!
//! Not here: the Stargate Engine service module (ANSI-2c, after #41/#42) and
//! jump fees.
//!
//! # `[A]`s
//!
//! - A corporation outside an alliance is its own owner group. Falsified by a
//!   corp-owned pair that Tranquility refuses to link without an alliance.
//! - Surface distance to the gate is what the client's `DistanceBetween`
//!   compares with 2,500 m; the server adds [`RANGE_SLACK`]. Falsified by an
//!   honest jump refused with `NotCloseEnoughToJump` (the `[jbridge] out of
//!   range` line gives the distance).
//! - `activationState` is `None`: no activation phase exists yet. Falsified
//!   by a gate bracket that logs on it.
//! - Without `deploy/jump-bridges.tsv` every zone multiplier is 1.0 and no
//!   hull is restricted. Falsified by the extracted row (ANSI-2b).
//! - A gate whose profile holds no activation row (the default profile every
//!   anchored gate gets holds docking rows only) lets its owner group use it
//!   (today's interim), so no live gate locks its owners out. Falsified by a
//!   TQ gate with the default profile refusing its own alliance.
//! - A hull with no base cost (attribute 6364 absent or 0) is refused as
//!   restricted. Falsified by a TQ hull with no cost jumping.
//! - A refusal for an empty capacitor is `CustomNotify` "The Ansiblex does not
//!   have enough charge": the client has no such key (the autopilot's
//!   `JumpNotEnoughCharge3` is the old ozone text). Falsified by a TQ capture
//!   naming a key.
//! - `_sim_timestamp` is the wall FILETIME the server's clock runs on
//!   (`pyutil::filetime_now`, which the client's clock is set from), so the
//!   bracket's extrapolated charge follows ours. Falsified by the bracket's
//!   charge creeping off the server's by a clock skew. At `--time-scale` above
//!   1 the recharge runs on game time ([`crate::game_clock`]) while the
//!   client extrapolates on wall time, so the bar reads low until the next
//!   ask (O-86b's default).
//! - Capacity and recharge time absent from the type (no bundle): capacity 0,
//!   so the gate never holds charge. Falsified by a bundle without them.
//! - A zone for a destination or capital the centres file does not hold is
//!   zone 5, as no capital is (`zone_modifier_util.py`). Falsified by a TQ
//!   system pair the client puts in a lower zone.
//! - The departure effect is the stargate's (`effects.JumpOut`, `targetID`
//!   the gate). Falsified by a jump tunnel that never starts, or a client log
//!   line about the gate ball.
//! - `GetNearbyJumpBridges` returns every gate of the pilot's group, not only
//!   those within 5 ly: the client looks its own near systems up in the
//!   answer (`structureDeployment._GetNearbyJumpBridges:239` is a dict by
//!   system). Falsified by a client that iterates the whole answer.
//! - A gate removed from space leaves a stale [`KIND`] row; ids are never
//!   reused, and every reader joins the row to a live structure.

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::delta::Deltas;
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::structure::{self, Structure};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

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

/// The jump bridge's type attributes **[V]**: the gate's `capacitorCapacity`
/// and `rechargeRate` (`jumpBridgeContController.py:28-29`), the hull's
/// `attributeBaseJumpBridgeCapacitorCost` (`dogma/const.py:1425`).
pub const ATTR_CAPACITY: i64 = 482;
pub const ATTR_RECHARGE: i64 = 55;
pub const ATTR_BASE_COST: i64 = 6364;

/// The `jbcap` delta kind: `structure_id -> charge_gj settled_at counter`.
/// `settled_at` is a FILETIME, `counter` the slim's etag. Written only here.
pub const KIND_CHARGE: &str = "jbcap";

/// The component the client's calls name (`jumpBridge.py:134`).
pub const COMPONENT: &str = "structureJumpBridge";

/// The notify text for an empty capacitor **[A]** (module header).
pub const NO_CHARGE: &str = "The Ansiblex does not have enough charge";

/// The zone bounds in light years, the last being open (`ZONE_DISTANCES`
/// **[V]**, `zone_modifier_util.py`).
pub const ZONE_BOUNDS: [f64; 4] = [5.0, 10.0, 15.0, 20.0];

// -- the zone table ------------------------------------------------------------------

/// The file `tools/content_deploy.py` writes (ANSI-2b), in the bundle.
pub const TABLE_FILE: &str = "deploy/jump-bridges.tsv";
/// Its header line.
pub const TABLE_HEADER: &str = "#type_id\trestriction_list\tzone1\tzone2\tzone3\tzone4\tzone5";

/// One gate type's component attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRow {
    /// `restrictionTypeList`: the typelist of excluded hulls, if any.
    pub restriction_list: Option<i64>,
    /// `zone1Modifier..zone5Modifier`.
    pub zones: [f64; 5],
}

/// The table: gate type -> its row. Empty when the file is absent **[A]**.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bridges {
    rows: BTreeMap<i64, GateRow>,
}

impl Bridges {
    /// A table of these rows.
    pub fn new(rows: impl IntoIterator<Item = (i64, GateRow)>) -> Self {
        Bridges {
            rows: rows.into_iter().collect(),
        }
    }

    /// The type's row.
    pub fn get(&self, type_id: i64) -> Option<&GateRow> {
        self.rows.get(&type_id)
    }

    /// One data line.
    pub fn parse_line(line: &str) -> Result<(i64, GateRow), String> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            return Err(format!("expected 7 columns, found {}", f.len()));
        }
        let type_id = f[0]
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|t| *t > 0)
            .ok_or("type_id is not a positive type id")?;
        let list = f[1].trim();
        let restriction_list = if list.is_empty() || list == "-" || list == "0" {
            None
        } else {
            Some(
                list.parse::<i64>()
                    .ok()
                    .filter(|l| *l > 0)
                    .ok_or("restriction_list is not a list id")?,
            )
        };
        let mut zones = [1.0; 5];
        for (i, z) in zones.iter_mut().enumerate() {
            *z = f[2 + i]
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| format!("zone{} is not a multiplier", i + 1))?;
        }
        Ok((
            type_id,
            GateRow {
                restriction_list,
                zones,
            },
        ))
    }
}

fn load_table(src: &Sources) -> Result<Boxed, String> {
    let Some(bundle) = src.bundle else {
        return Ok(Box::new(Bridges::default()));
    };
    src.note(Origin::Bundle);
    let mut t = Bridges::default();
    for line in bundle.data_lines(TABLE_FILE, TABLE_HEADER)? {
        let (id, row) = Bridges::parse_line(&line).map_err(|e| format!("{TABLE_FILE}: {e}"))?;
        if t.rows.insert(id, row).is_some() {
            return Err(format!("{TABLE_FILE}: type {id} twice"));
        }
    }
    Ok(Box::new(t))
}

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
/// An absent file is an empty table (no restriction, multipliers 1.0 **[A]**).
pub const TABLE: TableSpec = TableSpec {
    name: "jump bridges",
    load: load_table,
    fixture: || Box::new(Bridges::default()) as Boxed,
};

static NO_BRIDGES: Bridges = Bridges {
    rows: BTreeMap::new(),
};

/// The table in `content`; empty when none is registered.
pub fn bridges(content: &Content) -> &Bridges {
    content.try_get::<Bridges>().unwrap_or(&NO_BRIDGES)
}

// -- the capacitor -------------------------------------------------------------------

/// The gate's charge `now` after holding `c0` GJ at `t0` (FILETIMEs), in a
/// capacitor of `cap` GJ whose type has recharge time `recharge_ms`: the ship
/// capacitor's curve **[V]** (`jumpBridgeContController.py:159-173`),
/// `tau = recharge / 5`, `C = (1 + (sqrt(c0/cap) - 1) * exp(-dt/tau))^2 * cap`
/// with `dt` in milliseconds (`dgmTauConstant` = 10,000 FILETIME ticks per ms).
/// `c0` is clamped to `[0, cap]`; a time before `t0` counts as none.
pub fn charge_at(cap: f64, recharge_ms: f64, c0: f64, t0: i64, now: i64) -> f64 {
    if cap.is_nan() || cap <= 0.0 || cap.is_infinite() {
        return 0.0;
    }
    let c0 = if c0.is_finite() {
        c0.clamp(0.0, cap)
    } else {
        0.0
    };
    let tau = recharge_ms / 5.0;
    if tau.is_nan() || tau <= 0.0 {
        return cap;
    }
    let dt = now.saturating_sub(t0).max(0) as f64 / 10_000.0;
    let sq = (c0 / cap).sqrt();
    let c = (1.0 + (sq - 1.0) * (-dt / tau).exp()).powi(2) * cap;
    c.clamp(0.0, cap)
}

/// The zone (0 to 4) of a destination `ly` light years from the capital;
/// `None` (no capital, or a distance unknown) is the last zone, and NaN is the
/// first as in the client's `lightYears > distance` loop **[V]**
/// (`zone_modifier_util._GetZoneIndexFromLYDistance`).
pub fn zone_index(ly: Option<f64>) -> usize {
    let Some(ly) = ly else {
        return ZONE_BOUNDS.len();
    };
    ZONE_BOUNDS
        .iter()
        .position(|bound| ly.partial_cmp(bound) != Some(std::cmp::Ordering::Greater))
        .unwrap_or(ZONE_BOUNDS.len())
}

/// A ship's cost: its base cost times the multiplier of zone `idx`.
pub fn zone_cost(base: f64, mults: [f64; 5], idx: usize) -> f64 {
    base * mults[idx.min(4)]
}

/// The gate type's capacity and recharge time, from the type data; `(0, 0)`
/// when it has neither **[A]**.
fn gate_stats(content: &Content) -> (f64, f64) {
    let row = crate::typedata::installed_in(content)
        .table()
        .get(TYPE_ANSIBLEX);
    let at = |a| row.and_then(|r| r.attribute(a)).unwrap_or(0.0);
    (at(ATTR_CAPACITY), at(ATTR_RECHARGE))
}

/// A hull's base cost on the bridge (attribute 6364), when it has one.
pub fn base_cost(content: &Content, ship_type: i64) -> Option<f64> {
    crate::typedata::installed_in(content)
        .table()
        .get(ship_type)
        .and_then(|r| r.attribute(ATTR_BASE_COST))
        .filter(|c| *c > 0.0 && c.is_finite())
}

fn cap_key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

/// The stored row: `(charge, settled_at, counter)`.
fn cap_row(d: &Deltas, id: i64) -> Option<(f64, i64, i64)> {
    let v = d.of(KIND_CHARGE).get(&cap_key(id))?;
    Some((
        v.first()?.parse().ok()?,
        v.get(1)?.parse().ok()?,
        v.get(2).and_then(|c| c.parse().ok()).unwrap_or(0),
    ))
}

fn put_cap(d: &mut Deltas, id: i64, charge: f64, at: i64, counter: i64) {
    d.put(
        KIND_CHARGE,
        cap_key(id),
        vec![charge.to_string(), at.to_string(), counter.to_string()],
    );
}

/// When a gate with no row started at 0: its anchoring's end **[A]**.
fn anchored_at(gate: &Structure) -> Option<i64> {
    gate.deploy_end.or(gate.deploy_start)
}

/// The gate's charge at `now`, and its etag. A gate with no row has 0 charge
/// from its anchoring (`CapacitorUsageText`), or from `now` when it has no
/// anchoring time.
pub fn charge_of(reg: &Registry, gate: &Structure, now: i64) -> (f64, i64) {
    let (cap, recharge) = gate_stats(reg.content());
    let (c0, t0, counter) =
        cap_row(reg.deltas(), gate.id).unwrap_or((0.0, anchored_at(gate).unwrap_or(now), 0));
    let wall = now.saturating_sub(t0).max(0);
    let game = wall.saturating_mul(i64::from(crate::game_clock::of(reg).scale()));
    (
        charge_at(cap, recharge, c0, t0, t0.saturating_add(game)),
        counter,
    )
}

/// Store `charge` for the gate at `now`, bump its etag and tell the system.
fn store_charge(reg: &mut Registry, gate: &Structure, charge: f64, now: i64) {
    let counter = cap_row(reg.deltas(), gate.id).map_or(0, |r| r.2) + 1;
    put_cap(reg.deltas_mut(), gate.id, charge, now, counter);
    reg.mark_dirty();
    crate::structure_space::announce_slim(reg, gate);
}

/// A gate with no row and no anchoring time gets its row (0 at `now`), so it
/// does charge from here on.
fn ensure_row(reg: &mut Registry, gate: &Structure, now: i64) {
    if cap_row(reg.deltas(), gate.id).is_none() && anchored_at(gate).is_none() {
        put_cap(reg.deltas_mut(), gate.id, 0.0, now, 0);
        reg.mark_dirty();
    }
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

/// Whether the gate's profile (its own, else its owner's default, the rule
/// `structure_profile::has_access` judges by) holds an activation row. Every
/// gate anchored or spawned in the server has the owner's default profile, which
/// holds only docking rows, so a profile without an activation row does not
/// decide **[A]**: the owner group rule applies.
fn profile_decides(d: &Deltas, gate: &Structure) -> bool {
    crate::structure_profile::profile(d, gate.profile)
        .filter(|p| p.corp == gate.owner_corp)
        .or_else(|| crate::structure_profile::default_of(d, gate.owner_corp))
        .is_some_and(|p| {
            crate::structure_profile::settings(d, p.id)
                .iter()
                .any(|(set, _, _)| *set == SETTING_ACTIVATE)
        })
}

/// Whether `actor` may use `gate` (`JumpRestrictionsText`): a pilot of the
/// alliance that holds the sovereignty of the gate's system, whom the
/// structure profile's `SETTING_ACTIVATE` lets in (a gate with no profile lets
/// its owner group **[A]**, module header), and the sov-upgrade gate at both
/// ends ([`upgraded`]).
pub fn may_use(reg: &Registry, actor: i64, gate: &Structure) -> bool {
    let d = reg.deltas();
    let Some(corp) = reg.character(actor).map(|c| c.corporation_id) else {
        return false;
    };
    let Some(mine) = crate::alliance::of_corp(d, corp) else {
        return false;
    };
    crate::sov::holder(d, gate.system) == Some(mine)
        && if profile_decides(d, gate) {
            crate::structure_profile::has_access(reg, gate, actor, SETTING_ACTIVATE)
        } else {
            same_owner(d, corp, gate.owner_corp)
        }
        && upgraded(reg, gate)
}

/// `actor`'s alliance, if any.
fn alliance_of(reg: &Registry, actor: i64) -> Option<i64> {
    let corp = reg.character(actor)?.corporation_id;
    crate::alliance::of_corp(reg.deltas(), corp)
}

/// The zone (0 to 4) a jump to `destination` falls in for `alliance`: the
/// distance from its capital at `now`, in light years, through the centres
/// file (`zone_modifier_util.GetCapacitorZoneModifierAndIdx`).
pub fn zone_of(reg: &Registry, alliance: i64, destination: i64, now: i64) -> usize {
    let capital = crate::sov_index::capital(reg.deltas(), alliance, now).current;
    let ly = capital.and_then(|c| {
        reg.content()
            .get::<crate::celestial::Centers>()
            .ly(destination, c)
    });
    zone_index(ly)
}

/// The cost of `ship_type` jumping from `gate` for `actor`, in GJ: its base
/// cost times the zone multiplier of the partner's system. `None` for a hull
/// with no base cost.
pub fn cost_of(
    reg: &Registry,
    actor: i64,
    gate: &Structure,
    ship_type: i64,
    now: i64,
) -> Option<f64> {
    let base = base_cost(reg.content(), ship_type)?;
    let there = partner(reg.deltas(), gate)?;
    let mults = bridges(reg.content())
        .get(gate.type_id)
        .map_or([1.0; 5], |r| r.zones);
    let idx =
        alliance_of(reg, actor).map_or(ZONE_BOUNDS.len(), |a| zone_of(reg, a, there.system, now));
    Some(zone_cost(base, mults, idx))
}

/// Take the jump's cost from the gate's capacitor: settle the charge, refuse
/// when it is less than the cost, else store what is left. Returns the cost.
/// Nothing changes on a refusal.
pub fn take_charge(
    reg: &mut Registry,
    actor: i64,
    gate: &Structure,
    ship_type: i64,
    now: i64,
) -> Result<f64, ServiceError> {
    ensure_row(reg, gate, now);
    let Some(cost) = cost_of(reg, actor, gate, ship_type, now) else {
        return Err(restricted_error(reg.content(), ship_type));
    };
    let (charge, _) = charge_of(reg, gate, now);
    if cost > charge {
        return Err(notify(NO_CHARGE));
    }
    store_charge(reg, gate, charge - cost, now);
    Ok(cost)
}

/// `JumpBridgeRestrictedShip`; `shipName` is the hull's name from the type
/// data, the id when the type is unknown.
fn restricted_error(content: &Content, ship_type: i64) -> ServiceError {
    let name = crate::typedata::installed_in(content)
        .table()
        .get(ship_type)
        .map(|r| r.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| ship_type.to_string());
    user_error(
        RESTRICTED_SHIP,
        vec![("shipName", crate::pyutil::bstr(&name))],
    )
}

/// Whether the hull is in the gate type's `restrictionTypeList` (A1, A2),
/// resolved through `typelists.tsv` and the type's group and category. No list,
/// no row or an unknown hull: not restricted **[A]**.
pub fn restricted_hull(reg: &Registry, ship_type: i64) -> bool {
    let content = reg.content();
    let Some(list) = bridges(content)
        .get(TYPE_ANSIBLEX)
        .and_then(|r| r.restriction_list)
    else {
        return false;
    };
    let Some(row) = crate::typedata::installed_in(content)
        .table()
        .get(ship_type)
    else {
        return false;
    };
    content
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.typelists.get(&list))
        .is_some_and(|l| l.contains(ship_type, row.group_id, row.category_id))
}

/// A hull the gate takes: not restricted and with a base cost.
fn hull_allowed(reg: &Registry, ship_type: i64) -> bool {
    !restricted_hull(reg, ship_type) && base_cost(reg.content(), ship_type).is_some()
}

// -- GetJumpBridgeInfo -----------------------------------------------------------------

/// `space_component::Locate`: the system of a live Ansiblex.
pub fn locate(reg: &Registry, item: i64) -> Option<i64> {
    structure::get(reg.deltas(), item)
        .filter(|g| is_gate(g.type_id))
        .map(|g| g.system)
}

/// The class of `GetJumpBridgeInfo`'s answer.
pub const INFO_CLASS: &str =
    "jumpbridge.common.jump_bridge_authorized_info.JumpBridgeAuthorizedInfo";

/// The answer of `GetJumpBridgeInfo` for `actor` at `now`: `None` unless
/// [`may_use`], else a `JumpBridgeAuthorizedInfo` with the pilot's hull's
/// validity, the settled charge and the time it is settled at (A9).
///
/// **New-style, tag `0x23`** ([`crate::pyutil::new_style`]): the class is
/// `class JumpBridgeAuthorizedInfo(object)`
/// (`jumpbridge/common/jump_bridge_authorized_info.py:8`) [V]. Sent as an
/// old-style instance (`0x17`) the client refuses the whole answer with
/// `UnmarshalError: global object '...JumpBridgeAuthorizedInfo' should be of
/// class type but is of <type 'type'>` (seen in a client run) and the
/// window's `ExceptionEater` logs `JumpBridgeContController:Failed to update
/// ui` (`jumpBridgeContController.py:75-82`) [V]. The state keys are what
/// `__init__` assigns and the properties read (`:11-25`) [V].
pub fn info_at(reg: &mut Registry, actor: i64, gate: &Structure, now: i64) -> PyValue {
    if !may_use(reg, actor, gate) {
        return PyValue::None;
    }
    ensure_row(reg, gate, now);
    let ship_type = reg.character(actor).map_or(0, |c| c.ship_type_id);
    let (charge, _) = charge_of(reg, gate, now);
    crate::pyutil::new_style(
        INFO_CLASS,
        vec![
            (
                "_valid_ship_type",
                PyValue::Bool(hull_allowed(reg, ship_type)),
            ),
            ("_structure_capacitor", PyValue::Real(charge)),
            ("_sim_timestamp", PyValue::Long(now)),
        ],
    )
}

/// `CallComponentFromClient(id, 'structureJumpBridge', 'GetJumpBridgeInfo')`
/// (`jumpBridge.py:131-134`): [`info_at`] now.
pub fn info(
    reg: &mut Registry,
    call: &crate::space_component::CompCall<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(gate) = structure::get(reg.deltas(), call.item_id).filter(|g| is_gate(g.type_id))
    else {
        return Ok(PyValue::None);
    };
    Ok(info_at(
        reg,
        call.actor,
        &gate,
        crate::pyutil::filetime_now(),
    ))
}

/// Whether `CallComponentFromClient(item, component, method)` from `actor`
/// is a `GetJumpBridgeInfo` about a known gate that the router would refuse
/// as not in space, which it answers `None` instead (`space_component::call_in`).
///
/// **Why.** The gate window refreshes on a 2 s timer (`jumpBridgeCont.py:104-117`)
/// [V]; the call lands in an `ExceptionEater` (`jumpBridgeContController.py:75-82`)
/// as `JumpBridgeContController:Failed to update ui` [V] when the router
/// refuses. `None` is what the window reads as "no access" without a traceback
/// (`jumpBridgeContController.py:85-89`) [V]. The refusal happens in several
/// windows, all the same shape (a read-only info about a known gate that is not
/// in the caller's current system):
/// - the leg is `Departing`: the pilot is out of the roster, the session
///   still names the origin ([`jump_through`]);
/// - the leg is `Arriving`: the session names the destination and the window
///   still asks about the origin's gate;
/// - after the bind, the old window asks about the gate left behind;
/// - after a `/tr` (no `Jumps` entry) the window of a gate in another system.
///
/// So the test is: a known gate, and either the actor has a leg
/// (`jumps().contains`) or the actor's roster system is not the gate's. Only
/// this read-only method; an unknown item and another component's call keep
/// the refusal.
///
/// `[A]` TQ answers this, or refuses it, in a way the window also swallows.
/// Falsifier: a TQ capture of `GetJumpBridgeInfo` for a gate in another system
/// answering something else visible to the window.
pub fn quiet_info(reg: &Registry, actor: i64, item: i64, component: &str, method: &str) -> bool {
    component == COMPONENT
        && method == "GetJumpBridgeInfo"
        && locate(reg, item).is_some_and(|gate_system| {
            reg.jumps().contains(actor)
                || crate::space_component::actor_system(reg, actor) != Some(gate_system)
        })
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
    let etag = cap_row(reg.deltas(), s.id).map_or(0, |r| r.2);
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
        ("component_jumpBridge", PyValue::Int(etag)),
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
    if !hull_allowed(reg, ship_type) {
        return Err(restricted_error(reg.content(), ship_type));
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
    take_charge(reg, actor, &gate, ship_type, crate::pyutil::filetime_now())?;

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

/// `/jbridge link <gateID> <systemID>`, `/jbridge unlink <gateID>`,
/// `/jbridge list` and `/jbridge charge <gateID|here> <GJ|full>`: aim a gate
/// by hand (the Upwell anchor flow does it with `extraConfig`; `/upwell spawn`
/// has none) or set its charge (a test seam). A GM tool, not a game rule.
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    const USAGE: &str = "/jbridge link <gateID> <systemID> | /jbridge unlink <gateID> | /jbridge list | /jbridge charge <gateID|here> <GJ|full>";
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
        Some("charge") => {
            let gate = match args.get(1).copied() {
                Some("here") => actor
                    .and_then(|a| reg.roster().get(a).map(|p| p.solar_system_id))
                    .and_then(|sys| {
                        structure::in_system(reg.deltas(), sys)
                            .into_iter()
                            .filter(|s| is_gate(s.type_id))
                            .min_by_key(|s| s.id)
                    }),
                word => gate_of(reg, word.as_ref()),
            };
            let Some(gate) = gate else {
                return say("not a jump gate here; /jbridge list shows the gates");
            };
            let (cap, _) = gate_stats(reg.content());
            let want = match args.get(2).copied() {
                Some("full") => Some(cap),
                Some(w) => w.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0),
                None => None,
            };
            let Some(want) = want else {
                return say(USAGE);
            };
            let charge = want.min(cap);
            store_charge(reg, &gate, charge, crate::pyutil::filetime_now());
            say(&format!(
                "gate {} holds {charge:.1} of {cap:.1} GJ",
                gate.id
            ))
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
