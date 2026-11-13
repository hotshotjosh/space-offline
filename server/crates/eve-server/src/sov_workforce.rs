//! Workforce transfer between sov hubs (SOV-WF, #219, lane L12 of
//! `eve-offline-lanes/specs/parity-sov.md`; the wire is `docs/re/sov-gateway.md`
//! §2.4 and §2.8).
//!
//! **What it is.** A hub is in one of four modes (`workforce.proto`
//! `Configuration.mode`): `inactive`, `import` (from a set of source systems),
//! `export` (an amount to one destination system, or to none yet) and
//! `transit`. An exporting hub whose destination imports from it, in the same
//! holding alliance, hands over its amount; the importer's workforce budget
//! rises by it, the exporter's falls by it. The budget is
//! [`crate::sov_resources::harvested`]'s workforce, which is what the upgrade
//! rule ([`crate::sov_upgrade`]) and `GetResources` start from.
//!
//! **State** (delta kind [`KIND`], `delta_kinds::KINDS` row `sovwf`):
//!
//! ```text
//! sovwf <hubID> -> inactive | import src... | export amount dest|- | transit
//! ```
//!
//! A hub with no row is `inactive`. Sources and destinations are solar
//! system ids (the client's own words, `data_types.py:46-55`).
//!
//! **The client's own ordering** is why an exporter may be configured with no
//! destination: the window offers an importer only the hubs that already
//! export (`workforceController.py:_FilterOutHub`, `:190-208` **[V]**), and an
//! exporter only the hubs that already import; so the first step is an export
//! to nowhere, then an import from it, then the export to the importer. An
//! export is *connected* when its destination imports from it
//! ([`State::Export`]), else *disconnected* and delivers nothing.
//!
//! **What the rule is not.** It is server design, not client data **[A]**:
//!
//! - **[A]** a hub is *networkable* when it is another hub of the same holding
//!   alliance within [`WF_JUMPS`] gates. Falsifier: an owner ruling, or a
//!   retail hub that lists a hub further away or in another alliance.
//! - **[A]** an export above the exporter's own local harvest is clamped to
//!   it. Falsifier: a retail hub that accepts and shows the larger figure.
//! - **[A]** a disconnected export keeps its workforce at home (the client's
//!   word is `local_reserve`, `workforce.proto` `ExportState.Disconnected`).
//!   Falsifier: a retail exporter whose own budget falls with no importer.
//! - **[I]** `transit` passes workforce through and changes no budget here;
//!   there is no multi-hop routing.
//! - The exporter's amount comes from its **own** harvest only, never from
//!   what it imports, so a chain can never loop.
//!
//! **Notices.** `ConfiguredNotice` and `StateChangedNotice` are under the
//! package `eve_public.sovereignty.resource.transfer.workforce.api`, not the
//! request package `…hub.workforce.api` **[V]** (descriptor of
//! `hub/workforce/api/notices.proto`, `docs/re/sov-gateway.md` §2.8). A change
//! that moves a hub's workforce budget re-runs that hub's upgrades
//! ([`crate::sov_upgrade::resimulate`]) and tells its window.
//!
//! **Do not rely on `StateChangedNotice` for a window refresh.** The client's
//! `sovHubSvc._OnHubWorkforceStateChangedNotice` scatters
//! `OnHubWorkforceStateChanged(solarSystemID)` with one argument
//! (`sovHubSvc.py:72-73`), but `hubController.OnHubWorkforceStateChanged` takes
//! four (`hubController.py:637`) **[V]**; the call most likely dies in a
//! swallowed `TypeError` **[I]**. The window refreshes from
//! `ResourcesSimulatedNotice`, which every re-run also sends. Falsifier: a
//! client run in which the notice alone changes the window.
//!
//! **Registration (coordinator).** [`gateway`] in `gateway_ext::CHAIN` above
//! `sov_gw_stub::gateway` (which refuses `ConfigureRequest` and answers an
//! unconfigured `Get*` as `inactive`: this arm takes over for a known hub and
//! passes everything else down), `pub mod sov_workforce;` in `lib.rs` and
//! `("sovwf", "sov_workforce")` in `delta_kinds::KINDS`.
//!
//! **A change elsewhere.** A source's skyhook going on or off, a corp changing
//! alliance and a change of holder move other hubs' budgets: [`budgets_changed`]
//! (called from `skyhook::changed`/`remove` and
//! `sov_upgrade::corp_membership_changed`) and [`claim_changed`] (from
//! `sov_hub::claim_changed` and `lapse`) re-run them. A row remembers the
//! alliance that set it (`@<alliance>`, last cell); under another holder it
//! reads inactive and `claim_changed` drops it.
//!
//! **[A] open:** the `WF_JUMPS` rule. Transit likely routes along system links
//! (`hub/workforce/api/events_pb2.py` `SystemLinkCommanded`/`Unlink`), which
//! would make the reach a path, not a jump count. Falsifier: those events.

use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, field, get_bytes, get_uint, put_bytes};
use crate::service::Registry;
use crate::sov;
use crate::sov_hub::{self, Hub};
use crate::sov_proto::{id, put_bool, put_uint, read_id, status, typed};
use std::collections::{BTreeMap, BTreeSet};

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovwf";

/// How many gates apart two hubs may be and still network **[A]**.
pub const WF_JUMPS: u32 = 6;

/// How many sources a hub may import from **[I]** (`editSections.py:268-274`,
/// `workforceController.py:204-206`).
pub const WF_MAX_SOURCES: usize = 3;

const PKG: &str = "eve_public.sovereignty.hub.workforce.api.";
/// The notice package: **not** [`PKG`] **[V]**.
const NOTICE_PKG: &str = "eve_public.sovereignty.resource.transfer.workforce.api.";

// ---------------------------------------------------------------------------
// The configuration
// ---------------------------------------------------------------------------

/// What a hub is set to do (`workforce.proto` `Configuration.mode`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Inactive,
    /// Source systems.
    Import(BTreeSet<i64>),
    /// An amount to a destination system, `None` while there is none yet.
    Export {
        amount: u64,
        dest: Option<i64>,
    },
    Transit,
}

fn key(hub: i64) -> Vec<String> {
    vec![hub.to_string()]
}

fn row_of(m: &Mode) -> Vec<String> {
    match m {
        Mode::Inactive => vec!["inactive".to_owned()],
        Mode::Import(s) => std::iter::once("import".to_owned())
            .chain(s.iter().map(i64::to_string))
            .collect(),
        Mode::Export { amount, dest } => vec![
            "export".to_owned(),
            amount.to_string(),
            dest.map_or_else(|| "-".to_owned(), |d| d.to_string()),
        ],
        Mode::Transit => vec!["transit".to_owned()],
    }
}

fn mode_of(row: &[String]) -> Option<Mode> {
    let (head, rest) = row.split_first()?;
    Some(match head.as_str() {
        "inactive" => Mode::Inactive,
        "transit" => Mode::Transit,
        "import" => Mode::Import(
            rest.iter()
                .map(|s| s.parse().ok())
                .collect::<Option<BTreeSet<i64>>>()?,
        ),
        "export" => Mode::Export {
            amount: rest.first()?.parse().ok()?,
            dest: match rest.get(1)?.as_str() {
                "-" => None,
                s => Some(s.parse().ok()?),
            },
        },
        _ => return None,
    })
}

/// A row's last cell `@<alliance>` names the alliance that held the hub's
/// system when it was stored; the rest is the mode.
fn split_holder(row: &[String]) -> (&[String], Option<i64>) {
    match row.split_last() {
        Some((last, rest)) if last.starts_with('@') => (rest, last[1..].parse().ok()),
        _ => (row, None),
    }
}

/// The hub's system's holder alliance now.
fn holder_of_hub(d: &Deltas, hub: i64) -> Option<i64> {
    sov_hub::by_id(d, hub).and_then(|h| sov::holder(d, h.system_id))
}

/// `hub`'s stored configuration whoever holds the system now.
fn config_raw(d: &Deltas, hub: i64) -> Mode {
    d.of(KIND)
        .get(&key(hub))
        .and_then(|r| mode_of(split_holder(r).0))
        .unwrap_or_default()
}

/// Whether `hub` has a row stored under a holder other than the current one.
fn is_stale(d: &Deltas, hub: i64) -> bool {
    d.of(KIND)
        .get(&key(hub))
        .and_then(|r| split_holder(r).1)
        .is_some_and(|h| Some(h) != holder_of_hub(d, hub))
}

/// `hub`'s stored configuration; a hub with no row (or a row that does not
/// parse, or one stored under an earlier holder) is inactive: a new holder
/// does not inherit the old one's network.
pub fn config(d: &Deltas, hub: i64) -> Mode {
    if is_stale(d, hub) {
        return Mode::Inactive;
    }
    config_raw(d, hub)
}

/// Store `hub`'s configuration (with the holder it was set under); inactive
/// removes the row.
pub fn put_config(d: &mut Deltas, hub: i64, m: &Mode) {
    if *m == Mode::Inactive {
        d.del(KIND, &key(hub));
    } else {
        let mut row = row_of(m);
        if let Some(h) = holder_of_hub(d, hub) {
            row.push(format!("@{h}"));
        }
        d.put(KIND, key(hub), row);
    }
}

// ---------------------------------------------------------------------------
// The state (pure over the world)
// ---------------------------------------------------------------------------

/// An export as it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    /// The destination imports from here: this much arrives.
    Connected { dest: i64, quantity: u64 },
    /// Nobody takes it: this much stays at home **[A]**.
    Disconnected { reserve: u64 },
}

/// What a hub does now (`workforce.proto` `State.mode`): never unset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Inactive,
    /// Per connected source, what arrives from it.
    Import(Vec<(i64, u64)>),
    Export(Export),
    Transit,
}

/// The workforce `system`'s active skyhooks give (before any transfer).
fn local_workforce(reg: &Registry, system: i64) -> u64 {
    crate::skyhook::counted(reg, system).1
}

/// Whether `a` and `b` are held by one alliance.
fn same_holder(d: &Deltas, a: i64, b: i64) -> bool {
    sov::holder(d, a).is_some() && sov::holder(d, a) == sov::holder(d, b)
}

/// What `from` hands `to`: the exporter's amount, clamped to its own harvest
/// **[A]**, when `from` exports to `to`, `to` imports from `from`, both have a
/// hub and one alliance holds both. Else `None` (not connected); `Some(0)` is
/// connected but the clamp left nothing.
fn connected_from(reg: &Registry, from: i64, to: i64) -> Option<u64> {
    let d = reg.deltas();
    let (Some(a), Some(b)) = (sov_hub::hub(d, from), sov_hub::hub(d, to)) else {
        return None;
    };
    let Mode::Export {
        amount,
        dest: Some(dest),
    } = config(d, a.hub_id)
    else {
        return None;
    };
    let Mode::Import(sources) = config(d, b.hub_id) else {
        return None;
    };
    if dest != to || !sources.contains(&from) || !same_holder(d, from, to) {
        return None;
    }
    Some(amount.min(local_workforce(reg, from)))
}

/// [`state`] when the hub's own local harvest is already known.
fn state_with(reg: &Registry, hub: &Hub, local: u64) -> State {
    let d = reg.deltas();
    match config(d, hub.hub_id) {
        Mode::Inactive => State::Inactive,
        Mode::Transit => State::Transit,
        // Only the connected sources are listed; a connected one whose clamp
        // left nothing shows 0 **[V]** (`workforceController.py:125-144`, `editSections.py:107-115`).
        Mode::Import(sources) => State::Import(
            sources
                .into_iter()
                .filter_map(|s| connected_from(reg, s, hub.system_id).map(|n| (s, n)))
                .collect(),
        ),
        Mode::Export { amount, dest } => {
            let quantity = amount.min(local);
            match dest {
                Some(dest) if connected(reg, hub, dest) => {
                    State::Export(Export::Connected { dest, quantity })
                }
                _ => State::Export(Export::Disconnected { reserve: quantity }),
            }
        }
    }
}

/// Whether `hub`'s export to `dest` is taken: `dest` has a hub that imports
/// from `hub`'s system, in one alliance.
fn connected(reg: &Registry, hub: &Hub, dest: i64) -> bool {
    let d = reg.deltas();
    let Some(b) = sov_hub::hub(d, dest) else {
        return false;
    };
    matches!(config(d, b.hub_id), Mode::Import(s) if s.contains(&hub.system_id))
        && same_holder(d, hub.system_id, dest)
}

/// `hub`'s state now.
pub fn state(reg: &Registry, hub: &Hub) -> State {
    state_with(reg, hub, local_workforce(reg, hub.system_id))
}

/// The workforce budget of `system` after transfers, from its `local` harvest:
/// plus what its sources hand it, minus what its connected export takes. The
/// one place `sov_resources::harvested` asks. A system with no hub, or an
/// idle one, keeps `local`.
pub fn net_workforce(reg: &Registry, system: i64, local: u64) -> u64 {
    let Some(hub) = sov_hub::hub(reg.deltas(), system) else {
        return local;
    };
    match state_with(reg, &hub, local) {
        State::Import(v) => local.saturating_add(v.iter().map(|(_, n)| n).sum()),
        State::Export(Export::Connected { quantity, .. }) => local.saturating_sub(quantity),
        _ => local,
    }
}

/// The hubs that import from `system`: the ones whose budget moves when
/// `system`'s local harvest does.
pub fn importers_of(reg: &Registry, system: i64) -> Vec<i64> {
    let d = reg.deltas();
    sov_hub::all(d)
        .into_iter()
        .filter(|h| matches!(config(d, h.hub_id), Mode::Import(s) if s.contains(&system)))
        .map(|h| h.hub_id)
        .collect()
}

// ---------------------------------------------------------------------------
// When something other than a configure moves a budget
// ---------------------------------------------------------------------------

/// The hubs whose workforce budget moves with `system`'s: the ones that import
/// from it and the destination of its own export; with `links`, also the
/// sources of its own import (a connection changed, not only a harvest).
fn peers(reg: &Registry, system: i64, links: bool) -> Vec<i64> {
    let d = reg.deltas();
    let mut out = importers_of(reg, system);
    if let Some(own) = sov_hub::hub(d, system) {
        let systems: Vec<i64> = match config_raw(d, own.hub_id) {
            Mode::Export { dest: Some(x), .. } => vec![x],
            Mode::Import(s) if links => s.into_iter().collect(),
            _ => Vec::new(),
        };
        out.extend(
            systems
                .into_iter()
                .filter_map(|s| sov_hub::hub(d, s).map(|h| h.hub_id)),
        );
    }
    out.retain(|h| sov_hub::hub(d, system).is_none_or(|o| o.hub_id != *h));
    out.sort_unstable();
    out.dedup();
    out
}

fn rerun(reg: &mut Registry, hubs: &[i64]) {
    for h in hubs {
        crate::sov_upgrade::resimulate(reg, *h);
        crate::sov_resources::announce(reg, *h);
    }
}

/// `system`'s own harvest or membership changed (a skyhook switched, removed
/// or handed over, a corp moved alliance): re-run the upgrades of the hubs
/// that depend on its workforce and tell their windows. The system's own hub
/// is the caller's.
pub fn budgets_changed(reg: &mut Registry, system: i64) {
    let hubs = peers(reg, system, false);
    rerun(reg, &hubs);
}

/// The peers' states, taken **before** a change that moves `system`'s
/// workforce (the caller's mutation): once it is made a stale row already
/// reads inactive and the harvest is the new one, so a before/after diff is
/// possible only from a snapshot taken first.
#[derive(Debug, Default)]
pub struct Before(Vec<(i64, State)>);

/// Snapshot the hubs a change at `system` can move (`links`: also the
/// sources of its own import, for a change of holder).
pub fn before(reg: &Registry, system: i64, links: bool) -> Before {
    before_systems(reg, &[system], links)
}

/// [`before`] for a change that moves several systems at once (a corp
/// changing alliance moves every system where it has a skyhook).
pub fn before_systems(reg: &Registry, systems: &[i64], links: bool) -> Before {
    let mut hubs = Vec::new();
    for system in systems {
        // `peers` leaves out the system's own hub; its state moves too.
        hubs.extend(peers(reg, *system, links));
        hubs.extend(sov_hub::hub(reg.deltas(), *system).map(|h| h.hub_id));
    }
    hubs.sort_unstable();
    hubs.dedup();
    Before(
        hubs.into_iter()
            .filter_map(|h| sov_hub::by_id(reg.deltas(), h))
            .map(|h| (h.hub_id, state(reg, &h)))
            .collect(),
    )
}

/// One `StateChangedNotice` for each hub of `before` whose state differs now.
pub fn tell_changed(reg: &mut Registry, before: &Before) {
    for (h, old) in &before.0 {
        let Some(hub) = sov_hub::by_id(reg.deltas(), *h) else {
            continue;
        };
        let new = state(reg, &hub);
        if *old != new {
            state_changed(reg, *h, old, &new);
        }
    }
}

/// [`budgets_changed`], and the `StateChangedNotice`s of the peers whose state
/// moved since `before` (taken by the caller ahead of its mutation).
pub fn budgets_changed_since(reg: &mut Registry, system: i64, before: &Before) {
    budgets_changed(reg, system);
    tell_changed(reg, before);
}

/// [`claim_changed`], and the `StateChangedNotice`s of the peers whose state
/// moved since `before` (taken ahead of the claim's mutation, with links).
pub fn claim_changed_since(reg: &mut Registry, system: i64, before: &Before) {
    claim_changed(reg, system);
    tell_changed(reg, before);
}

/// `system`'s holder changed (`sov_hub::claim_changed`, `lapse`): a hub's
/// network belongs to the alliance that set it, so a row stored under another
/// holder is dropped; then the hubs it connected to re-run.
pub fn claim_changed(reg: &mut Registry, system: i64) {
    // Runs on every entosis score change: do nothing unless a row is stale.
    let Some(h) = sov_hub::hub(reg.deltas(), system) else {
        return;
    };
    if !is_stale(reg.deltas(), h.hub_id) {
        return;
    }
    let hubs = peers(reg, system, true);
    reg.deltas_mut().del(KIND, &key(h.hub_id));
    rerun(reg, &hubs);
}

// ---------------------------------------------------------------------------
// Who may, and what may be networked
// ---------------------------------------------------------------------------

/// Whether `who`'s alliance holds the hub's system.
fn is_holder(reg: &Registry, hub: &Hub, who: i64) -> bool {
    let mine = reg.character(who).and_then(|c| c.alliance_id);
    mine.is_some() && mine == sov::holder(reg.deltas(), hub.system_id)
}

/// The window's rule: the holder alliance and a station manager or director
/// (the same as the upgrade window, `sov_upgrade::may_manage`).
fn may_manage(reg: &Registry, hub: &Hub, who: i64) -> bool {
    is_holder(reg, hub, who) && crate::service::sov_hub::may_launch(reg, who)
}

/// The other hubs `hub` may network with **[A]**: same holder, a hub, within
/// [`WF_JUMPS`] gates. In system order.
pub fn networkable(reg: &Registry, hub: &Hub) -> Vec<Hub> {
    let near: BTreeSet<i64> = crate::world_events::near(reg, hub.system_id, WF_JUMPS)
        .into_iter()
        .collect();
    sov_hub::all(reg.deltas())
        .into_iter()
        .filter(|h| h.system_id != hub.system_id && near.contains(&h.system_id))
        .filter(|h| same_holder(reg.deltas(), hub.system_id, h.system_id))
        .collect()
}

fn validate(reg: &Registry, hub: &Hub, m: &Mode) -> Result<(), u32> {
    let ok = networkable(reg, hub);
    let all_ok = |systems: &mut dyn Iterator<Item = i64>| -> Result<(), u32> {
        for s in systems {
            if !ok.iter().any(|h| h.system_id == s) {
                return Err(400);
            }
        }
        Ok(())
    };
    match m {
        Mode::Import(s) if s.len() > WF_MAX_SOURCES => Err(400),
        Mode::Import(s) => all_ok(&mut s.iter().copied()),
        Mode::Export { dest: Some(d), .. } => all_ok(&mut std::iter::once(*d)),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Configure
// ---------------------------------------------------------------------------

/// What a change can move, per hub: the state and the workforce budget.
fn snapshot(reg: &Registry) -> BTreeMap<i64, (State, u64)> {
    sov_hub::all(reg.deltas())
        .into_iter()
        .map(|h| {
            let budget = crate::sov_resources::harvested(reg, h.system_id).1;
            (h.hub_id, (state(reg, &h), budget))
        })
        .collect()
}

/// `ConfigureRequest`: `who` sets `hub_id` to `new`. 404 for no such hub, 403
/// for a pilot who is not a manager of the holding alliance, 400 for a source
/// or destination that cannot be networked. On success: stores it, re-runs the
/// upgrades of every hub whose workforce budget moved and tells its window,
/// and queues `ConfiguredNotice` and `StateChangedNotice`s.
pub fn configure(reg: &mut Registry, hub_id: i64, who: i64, new: Mode) -> Result<(), u32> {
    let hub = sov_hub::by_id(reg.deltas(), hub_id).ok_or(404u32)?;
    if !may_manage(reg, &hub, who) {
        return Err(403);
    }
    validate(reg, &hub, &new)?;
    let old = config(reg.deltas(), hub_id);
    let before = snapshot(reg);
    put_config(reg.deltas_mut(), hub_id, &new);
    let after = snapshot(reg);
    for (h, (state_after, budget_after)) in &after {
        let Some((state_before, budget_before)) = before.get(h) else {
            continue;
        };
        if budget_before != budget_after {
            crate::sov_upgrade::resimulate(reg, *h);
            crate::sov_resources::announce(reg, *h);
        }
        if state_before != state_after {
            state_changed(reg, *h, state_before, state_after);
        }
    }
    configured(reg, &hub, &old, &new);
    Ok(())
}

fn audience(reg: &Registry, system: i64) -> Target {
    Target::SolarSystem {
        id: system,
        members: crate::service::structures::system_audience(reg, system),
    }
}

fn configured(reg: &mut Registry, hub: &Hub, old: &Mode, new: &Mode) {
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(hub.system_id as u64));
    put_bytes(&mut p, 2, &configuration_bytes(old));
    put_bytes(&mut p, 3, &configuration_bytes(new));
    put_bytes(&mut p, 4, &id(hub.hub_id as u64));
    let target = audience(reg, hub.system_id);
    crate::gateway_out::queue(reg, &format!("{NOTICE_PKG}ConfiguredNotice"), p, target);
}

fn state_changed(reg: &mut Registry, hub_id: i64, old: &State, new: &State) {
    let Some(hub) = sov_hub::by_id(reg.deltas(), hub_id) else {
        return;
    };
    let mut p = Vec::new();
    put_bytes(&mut p, 1, &id(hub.system_id as u64));
    put_bytes(&mut p, 2, &state_bytes(old));
    put_bytes(&mut p, 3, &state_bytes(new));
    put_bytes(&mut p, 4, &id(hub_id as u64));
    let target = audience(reg, hub.system_id);
    crate::gateway_out::queue(reg, &format!("{NOTICE_PKG}StateChangedNotice"), p, target);
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// `Configuration` (`workforce_pb2`, `workforce.proto`): exactly one of
/// `1 inactive`, `2 import_settings`, `3 export_settings`, `4 transit`.
pub fn configuration_bytes(m: &Mode) -> Vec<u8> {
    let mut out = Vec::new();
    match m {
        Mode::Inactive => put_bool(&mut out, 1, true),
        Mode::Import(sources) => {
            let mut imp = Vec::new();
            for s in sources {
                let mut src = Vec::new();
                put_bytes(&mut src, 1, &id(*s as u64));
                put_bytes(&mut imp, 1, &src);
            }
            put_bytes(&mut out, 2, &imp);
        }
        Mode::Export { amount, dest } => {
            let mut exp = Vec::new();
            put_uint(&mut exp, 2, *amount);
            match dest {
                Some(d) => put_bytes(&mut exp, 3, &id(*d as u64)),
                None => put_bool(&mut exp, 4, true),
            }
            put_bytes(&mut out, 3, &exp);
        }
        Mode::Transit => put_bool(&mut out, 4, true),
    }
    out
}

/// `State`: exactly one of `1 inactive`, `2 import_state`, `3 export_state`
/// (`5 connected` | `6 disconnected`), `4 transit`.
pub fn state_bytes(s: &State) -> Vec<u8> {
    let mut out = Vec::new();
    match s {
        State::Inactive => put_bool(&mut out, 1, true),
        State::Import(v) => {
            let mut imp = Vec::new();
            for (src, amount) in v {
                let mut e = Vec::new();
                put_bytes(&mut e, 1, &id(*src as u64));
                put_uint(&mut e, 2, *amount);
                put_bytes(&mut imp, 1, &e);
            }
            put_bytes(&mut out, 2, &imp);
        }
        State::Export(e) => {
            let mut exp = Vec::new();
            match e {
                Export::Connected { dest, quantity } => {
                    let mut c = Vec::new();
                    put_bytes(&mut c, 1, &id(*dest as u64));
                    put_uint(&mut c, 2, *quantity);
                    put_bytes(&mut exp, 5, &c);
                }
                Export::Disconnected { reserve } => {
                    let mut c = Vec::new();
                    put_uint(&mut c, 1, *reserve);
                    put_bytes(&mut exp, 6, &c);
                }
            }
            put_bytes(&mut out, 3, &exp);
        }
        State::Transit => put_bytes(&mut out, 4, &[]),
    }
    out
}

/// Read a `Configuration`; `None` when no mode is set (the client never sends
/// that: `_get_new_configuration` returns `None` and sends nothing) or a
/// source is not a system id.
pub fn parse_configuration(b: &[u8]) -> Option<Mode> {
    if get_uint(b, 1) == Some(1) {
        return Some(Mode::Inactive);
    }
    if let Some(imp) = get_bytes(b, 2) {
        let mut set = BTreeSet::new();
        for src in field(&imp, 1) {
            set.insert(i64::try_from(read_id(&src, 1).filter(|s| *s > 0)?).ok()?);
        }
        return Some(Mode::Import(set));
    }
    if let Some(exp) = get_bytes(b, 3) {
        let amount = get_uint(&exp, 2).unwrap_or(0);
        let dest = match read_id(&exp, 3).filter(|d| *d > 0) {
            Some(d) => Some(i64::try_from(d).ok()?),
            None if get_uint(&exp, 4) == Some(1) => None,
            None => return None,
        };
        return Some(Mode::Export { amount, dest });
    }
    if get_uint(b, 4) == Some(1) {
        return Some(Mode::Transit);
    }
    None
}

fn hub_of(body: &[u8]) -> Option<i64> {
    read_id(body, 1)
        .filter(|h| *h > 0)
        .and_then(|h| i64::try_from(h).ok())
}

/// The gateway arm (`gateway_ext::CHAIN`, above `sov_gw_stub`): the four
/// requests of `hub.workforce.api`. `None` for everything else, for a
/// [`Ground::Local`] (no world) and, for a read, for a hub this server does not
/// know (the stub answers it `inactive`).
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    if !matches!(
        rest,
        "GetConfigurationRequest"
            | "GetStateRequest"
            | "ConfigureRequest"
            | "GetNetworkableHubsRequest"
    ) {
        return None;
    }
    let Ground::Server(w) = g else {
        return None;
    };
    let hub_id = hub_of(body);
    if rest == "ConfigureRequest" {
        let (Some(hub_id), Some(cfg)) = (hub_id, get_bytes(body, 2)) else {
            return Some(status(404));
        };
        let Some(mode) = parse_configuration(&cfg) else {
            return Some(status(400));
        };
        let mut reg = crate::service::world_write_timed(w, "sov_workforce_configure");
        return Some(match configure(&mut reg, hub_id, character, mode) {
            Ok(()) => typed(ty, Vec::new()),
            Err(e) => status(e),
        });
    }
    let reg = crate::service::world_read_timed(w, "sov_workforce_get");
    let hub = hub_id.and_then(|h| sov_hub::by_id(reg.deltas(), h))?;
    if !is_holder(&reg, &hub, character) {
        return Some(status(403));
    }
    let mut out = Vec::new();
    match rest {
        "GetConfigurationRequest" => {
            put_bytes(
                &mut out,
                1,
                &configuration_bytes(&config(reg.deltas(), hub.hub_id)),
            );
        }
        "GetStateRequest" => put_bytes(&mut out, 1, &state_bytes(&state(&reg, &hub))),
        _ => {
            for h in networkable(&reg, &hub) {
                let mut e = Vec::new();
                put_bytes(&mut e, 1, &id(h.hub_id as u64));
                put_bytes(&mut e, 2, &id(h.system_id as u64));
                put_bytes(
                    &mut e,
                    3,
                    &configuration_bytes(&config(reg.deltas(), h.hub_id)),
                );
                put_bytes(&mut e, 4, &state_bytes(&state(&reg, &h)));
                put_bytes(&mut out, 1, &e);
            }
        }
    }
    Some(typed(ty, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::sov_upgrade::{Def, Installed, Power, Row, UpgradeTable};
    use std::sync::{Arc, RwLock};

    // Invented ids. Systems S0.. form a chain of gates.
    const SYS0: i64 = 39_100_000;
    const ALICE: i64 = 1_001;
    const BOB: i64 = 1_002;
    const CAROL: i64 = 1_003;
    const CORP: i64 = 98_000_001;
    const OTHER_CORP: i64 = 98_000_002;
    const OTHER_ALLIANCE: i64 = 91_000_099;
    const UPGRADE: i64 = 9_301;

    fn sys(i: i64) -> i64 {
        SYS0 + i
    }
    fn hub_id(i: i64) -> i64 {
        43_000_000 + i
    }
    fn land(i: i64) -> i64 {
        41_000_000 + i * 10 + 1
    }

    fn content() -> Content {
        let mut c = Content::fixture();
        c.insert("sov planets", crate::sov_resources::fixture_planets());
        c.insert("sov stars", crate::sov_resources::fixture_stars());
        let row = Row {
            def: Def {
                power: 0,
                workforce: 10,
                fuel: None,
                per_hour: 0,
                startup: 0,
                power_out: 0,
                workforce_out: 0,
            },
            group: "invented".to_owned(),
            services: BTreeSet::new(),
            tier: None,
            effects: Vec::new(),
        };
        c.insert("sov upgrades", UpgradeTable::new(vec![(UPGRADE, row)]).0);
        c
    }

    /// `n` systems in a chain, a hub in each of `hubs`, all held by one real
    /// alliance, an active workforce skyhook (20) over the land planet of each
    /// of `skyhooks`. Alice is CEO of the holders; Bob is a plain member;
    /// Carol is in another alliance.
    fn world(n: i64, hubs: &[i64], skyhooks: &[i64]) -> (Registry, i64) {
        let mut text = String::new();
        for i in 0..n {
            let b = 41_000_000 + i * 10;
            text += &format!("star\t{b}\t9000\t1000\t0\t0\t0\t{}\t0\t0\n", sys(i));
            text += &format!(
                "planet\t{}\t9002\t100\t2\t0\t0\t{}\t0\t0\n",
                land(i),
                sys(i)
            );
            if i + 1 < n {
                let (g, h) = (42_000_000 + i * 2, 42_000_000 + i * 2 + 1);
                text += &format!(
                    "gate\t{g}\t16\t1000\t0\t0\t0\t{}\t{h}\t{}\n",
                    sys(i),
                    sys(i + 1)
                );
                text += &format!(
                    "gate\t{h}\t16\t1000\t0\t0\t0\t{}\t{g}\t{}\n",
                    sys(i + 1),
                    sys(i)
                );
            }
        }
        let pilot = |id: i64| crate::model::Character {
            character_id: id,
            ship_id: 900_000 + id,
            solar_system_id: Some(sys(0)),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![pilot(ALICE), pilot(BOB), pilot(CAROL)],
            celestials: crate::celestial::Catalogue::parse(&text),
            ..crate::model::SandboxConfig::default()
        })
        .with_content(Arc::new(content()));
        crate::service::sov_hub::test_corp(&mut reg, CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
        crate::service::sov_hub::test_corp(&mut reg, OTHER_CORP, CAROL, &[(CAROL, 0)]);
        let mut st = crate::alliance::Alliances::load(reg.deltas());
        let a = st
            .create(CORP, ALICE, "Invented Holders", "IHD", "", "", 1)
            .expect("an alliance");
        st.save(reg.deltas_mut());
        for (who, corp, alliance) in [
            (ALICE, CORP, a),
            (BOB, CORP, a),
            (CAROL, OTHER_CORP, OTHER_ALLIANCE),
        ] {
            let c = reg.character_mut(who).expect("a pilot");
            c.corporation_id = corp;
            c.alliance_id = Some(alliance);
        }
        for &i in hubs {
            sov_hub::put(
                reg.deltas_mut(),
                &Hub {
                    system_id: sys(i),
                    hub_id: hub_id(i),
                    position: [0.0; 3],
                    launcher_id: None,
                    launch_time: 0,
                    offline_days: 0,
                },
            );
            sov::set_claim(reg.deltas_mut(), sys(i), Some((a, CORP)), 1);
        }
        for &i in skyhooks {
            crate::skyhook::put(reg.deltas_mut(), &skyhook(i, true));
        }
        (reg, a)
    }

    fn skyhook(i: i64, active: bool) -> crate::skyhook::Skyhook {
        crate::skyhook::Skyhook {
            id: 5_000 + i,
            planet: land(i),
            system: sys(i),
            corp: CORP,
            active,
            activated_at: 1,
            built_until: 1,
            secured: 0,
            unsecured: 0,
            last_cycle: 0,
            vuln_start: 0,
            vuln_end: 0,
            state: 0,
        }
    }

    fn wf(reg: &Registry, i: i64) -> u64 {
        crate::sov_resources::harvested(reg, sys(i)).1
    }

    fn export(reg: &mut Registry, from: i64, amount: u64, to: Option<i64>) {
        let m = Mode::Export {
            amount,
            dest: to.map(sys),
        };
        configure(reg, hub_id(from), ALICE, m).expect("export");
    }

    fn import(reg: &mut Registry, at: i64, from: &[i64]) {
        let m = Mode::Import(from.iter().map(|i| sys(*i)).collect());
        configure(reg, hub_id(at), ALICE, m).expect("import");
    }

    fn state_of(reg: &Registry, i: i64) -> State {
        state(reg, &sov_hub::hub(reg.deltas(), sys(i)).unwrap())
    }

    #[test]
    fn every_configuration_branch_encodes_its_field_number_and_reads_back() {
        let cases = [
            (Mode::Inactive, 1u64),
            (Mode::Import(BTreeSet::from([7, 9])), 2),
            (
                Mode::Export {
                    amount: 30,
                    dest: Some(7),
                },
                3,
            ),
            (
                Mode::Export {
                    amount: 0,
                    dest: None,
                },
                3,
            ),
            (Mode::Transit, 4),
        ];
        for (m, n) in cases {
            let b = configuration_bytes(&m);
            let present: Vec<u64> = (1..=4)
                .filter(|f| get_uint(&b, *f).is_some() || get_bytes(&b, *f).is_some())
                .collect();
            assert_eq!(present, [n], "{m:?}: exactly one mode field");
            assert_eq!(parse_configuration(&b), Some(m));
        }
        // The export destination is field 3, "no destination" field 4.
        let exp = get_bytes(
            &configuration_bytes(&Mode::Export {
                amount: 30,
                dest: Some(7),
            }),
            3,
        )
        .unwrap();
        assert_eq!(
            (get_uint(&exp, 2), read_id(&exp, 3), get_uint(&exp, 4)),
            (Some(30), Some(7), None)
        );
        let none = get_bytes(
            &configuration_bytes(&Mode::Export {
                amount: 30,
                dest: None,
            }),
            3,
        )
        .unwrap();
        assert_eq!((read_id(&none, 3), get_uint(&none, 4)), (None, Some(1)));
    }

    #[test]
    fn an_unset_mode_is_not_a_configuration_and_every_state_sets_one() {
        assert_eq!(parse_configuration(&[]), None);
        // An export that names neither a destination nor "none" says nothing.
        let mut half = Vec::new();
        put_uint(&mut half, 2, 5);
        let mut b = Vec::new();
        put_bytes(&mut b, 3, &half);
        assert_eq!(parse_configuration(&b), None);
        let states = [
            (State::Inactive, 1u64),
            (State::Import(vec![(7, 3)]), 2),
            (
                State::Export(Export::Connected {
                    dest: 7,
                    quantity: 3,
                }),
                3,
            ),
            (State::Export(Export::Disconnected { reserve: 3 }), 3),
            (State::Transit, 4),
        ];
        for (s, n) in states {
            let b = state_bytes(&s);
            let present: Vec<u64> = (1..=4)
                .filter(|f| get_uint(&b, *f).is_some() || get_bytes(&b, *f).is_some())
                .collect();
            assert_eq!(present, [n], "{s:?}");
        }
        let conn = get_bytes(
            &state_bytes(&State::Export(Export::Connected {
                dest: 7,
                quantity: 3,
            })),
            3,
        )
        .unwrap();
        let c = get_bytes(&conn, 5).unwrap();
        assert_eq!((read_id(&c, 1), get_uint(&c, 2)), (Some(7), Some(3)));
        let disc = get_bytes(
            &state_bytes(&State::Export(Export::Disconnected { reserve: 4 })),
            3,
        )
        .unwrap();
        assert_eq!(get_uint(&get_bytes(&disc, 6).unwrap(), 1), Some(4));
    }

    #[test]
    fn the_stored_row_round_trips() {
        let (mut reg, _) = world(2, &[0, 1], &[]);
        for m in [
            Mode::Import(BTreeSet::from([3, 1])),
            Mode::Export {
                amount: 9,
                dest: Some(4),
            },
            Mode::Export {
                amount: 9,
                dest: None,
            },
            Mode::Transit,
        ] {
            put_config(reg.deltas_mut(), 77, &m);
            assert_eq!(config(reg.deltas(), 77), m);
        }
        put_config(reg.deltas_mut(), 77, &Mode::Inactive);
        assert!(reg.deltas().of(KIND).is_empty());
        assert_eq!(config(reg.deltas(), 77), Mode::Inactive);
    }

    #[test]
    fn a_connected_export_moves_workforce_from_one_budget_to_the_other() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
        export(&mut reg, 0, 15, None);
        // No importer yet: it stays at home.
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
        assert_eq!(
            state_of(&reg, 0),
            State::Export(Export::Disconnected { reserve: 15 })
        );
        import(&mut reg, 1, &[0]);
        // The export has no destination: the source is not connected, not listed.
        assert_eq!(state_of(&reg, 1), State::Import(vec![]));
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
        export(&mut reg, 0, 15, Some(1));
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (5, 15));
        assert_eq!(
            state_of(&reg, 0),
            State::Export(Export::Connected {
                dest: sys(1),
                quantity: 15
            })
        );
        assert_eq!(state_of(&reg, 1), State::Import(vec![(sys(0), 15)]));
        // Switching the exporter off gives it back.
        configure(&mut reg, hub_id(0), ALICE, Mode::Inactive).unwrap();
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
    }

    #[test]
    fn an_export_the_importer_does_not_expect_delivers_nothing() {
        let (mut reg, _) = world(3, &[0, 1, 2], &[0]);
        import(&mut reg, 1, &[0]);
        // The exporter names system 2, which imports from nobody.
        export(&mut reg, 0, 15, Some(2));
        assert_eq!((wf(&reg, 0), wf(&reg, 1), wf(&reg, 2)), (20, 0, 0));
        assert!(matches!(
            state_of(&reg, 0),
            State::Export(Export::Disconnected { .. })
        ));
    }

    #[test]
    fn an_export_above_the_local_harvest_is_clamped() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        import(&mut reg, 1, &[0]);
        export(&mut reg, 0, 500, Some(1));
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (0, 20));
        assert_eq!(state_of(&reg, 1), State::Import(vec![(sys(0), 20)]));
    }

    #[test]
    fn an_import_from_two_sources_sums() {
        let (mut reg, _) = world(3, &[0, 1, 2], &[0, 2]);
        // System 1 sits between the two exporters.
        export(&mut reg, 0, 12, None);
        export(&mut reg, 2, 7, None);
        import(&mut reg, 1, &[0, 2]);
        export(&mut reg, 0, 12, Some(1));
        export(&mut reg, 2, 7, Some(1));
        assert_eq!(wf(&reg, 1), 19);
        assert_eq!(
            state_of(&reg, 1),
            State::Import(vec![(sys(0), 12), (sys(2), 7)])
        );
        assert_eq!((wf(&reg, 0), wf(&reg, 2)), (8, 13));
    }

    #[test]
    fn a_source_that_stops_producing_stops_delivering() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        import(&mut reg, 1, &[0]);
        export(&mut reg, 0, 15, Some(1));
        assert_eq!(wf(&reg, 1), 15);
        crate::skyhook::put(reg.deltas_mut(), &skyhook(0, false));
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (0, 0));
        assert_eq!(importers_of(&reg, sys(0)), [hub_id(1)]);
    }

    #[test]
    fn a_hub_the_alliance_lost_stops_taking_part() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        import(&mut reg, 1, &[0]);
        export(&mut reg, 0, 15, Some(1));
        sov::set_claim(
            reg.deltas_mut(),
            sys(1),
            Some((OTHER_ALLIANCE, OTHER_CORP)),
            2,
        );
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
    }

    #[test]
    fn who_may_configure_and_what_may_be_networked() {
        // A chain of 8: system 6 is six gates from system 0, system 7 is seven.
        let (mut reg, _) = world(8, &[0, 1, 6, 7], &[]);
        let imp = |from: i64| Mode::Import(BTreeSet::from([sys(from)]));
        assert_eq!(configure(&mut reg, hub_id(0), BOB, imp(1)), Err(403));
        assert_eq!(configure(&mut reg, hub_id(0), CAROL, imp(1)), Err(403));
        assert_eq!(configure(&mut reg, 12_345, ALICE, imp(1)), Err(404));
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, imp(7)), Err(400));
        // A system with no hub, and the hub's own system, are not sources.
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, imp(3)), Err(400));
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, imp(0)), Err(400));
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, imp(6)), Ok(()));
        let far = Mode::Export {
            amount: 1,
            dest: Some(sys(7)),
        };
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, far), Err(400));
        // None of the refusals stored anything.
        assert_eq!(config(reg.deltas(), hub_id(0)), imp(6));
        // Another alliance's hub is not networkable.
        sov::set_claim(
            reg.deltas_mut(),
            sys(1),
            Some((OTHER_ALLIANCE, OTHER_CORP)),
            2,
        );
        assert_eq!(configure(&mut reg, hub_id(0), ALICE, imp(1)), Err(400));
    }

    #[test]
    fn a_transfer_re_simulates_the_upgrades_of_both_hubs() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        let up = |state| {
            [Installed {
                type_id: UPGRADE,
                online: true,
                state,
            }]
        };
        crate::sov_upgrade::put_installed(reg.deltas_mut(), hub_id(1), &up(Power::Low));
        // System 0 runs the same upgrade on its own 20.
        crate::sov_upgrade::put_installed(reg.deltas_mut(), hub_id(0), &up(Power::Online));
        let states = |reg: &Registry| {
            (
                crate::sov_upgrade::installed(reg.deltas(), hub_id(0))[0].state,
                crate::sov_upgrade::installed(reg.deltas(), hub_id(1))[0].state,
            )
        };
        import(&mut reg, 1, &[0]);
        assert_eq!(states(&reg), (Power::Online, Power::Low));
        // Exporting 15 leaves the exporter 5 (below the upgrade's 10) and lifts
        // the importer to 15 (above it).
        export(&mut reg, 0, 15, Some(1));
        assert_eq!(states(&reg), (Power::Low, Power::Online));
        // 5 fewer: 15 left at home, 5 abroad; the importer falls short again.
        export(&mut reg, 0, 5, Some(1));
        assert_eq!(states(&reg), (Power::Online, Power::Low));
    }

    #[test]
    fn a_configure_queues_the_notices_under_the_transfer_package() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        let _ = crate::gateway_out::take(&mut reg);
        import(&mut reg, 1, &[0]);
        let sent = crate::gateway_out::take(&mut reg);
        let conf: Vec<_> = sent
            .iter()
            .filter(|(t, _, _)| t.ends_with("ConfiguredNotice"))
            .collect();
        assert_eq!(conf.len(), 1);
        assert_eq!(
            conf[0].0,
            "eve_public.sovereignty.resource.transfer.workforce.api.ConfiguredNotice"
        );
        let p = &conf[0].1;
        assert_eq!(read_id(p, 1), Some(sys(1) as u64));
        assert_eq!(read_id(p, 4), Some(hub_id(1) as u64));
        assert_eq!(
            parse_configuration(&get_bytes(p, 2).unwrap()),
            Some(Mode::Inactive)
        );
        assert_eq!(
            parse_configuration(&get_bytes(p, 3).unwrap()),
            Some(Mode::Import(BTreeSet::from([sys(0)])))
        );
        assert!(
            !sent
                .iter()
                .any(|(t, _, _)| t.contains(".hub.workforce.api.")),
            "no notice under the request package"
        );
        // The connecting export changes both hubs' states.
        export(&mut reg, 0, 15, Some(1));
        let sent = crate::gateway_out::take(&mut reg);
        let changed: Vec<i64> = sent
            .iter()
            .filter(|(t, _, _)| t.ends_with("StateChangedNotice"))
            .filter_map(|(_, p, _)| read_id(p, 4).map(|h| h as i64))
            .collect();
        assert!(changed.contains(&hub_id(0)) && changed.contains(&hub_id(1)));
    }

    /// An importer holding an upgrade that needs 10 of the 15 it is handed.
    fn with_importer_upgrade() -> Registry {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        let up = |state| {
            [Installed {
                type_id: UPGRADE,
                online: true,
                state,
            }]
        };
        crate::sov_upgrade::put_installed(reg.deltas_mut(), hub_id(1), &up(Power::Low));
        import(&mut reg, 1, &[0]);
        export(&mut reg, 0, 15, Some(1));
        assert_eq!(imp_state(&reg), Power::Online);
        let _ = crate::gateway_out::take(&mut reg);
        reg
    }

    fn imp_state(reg: &Registry) -> Power {
        crate::sov_upgrade::installed(reg.deltas(), hub_id(1))[0].state
    }

    fn told_hub_1(reg: &mut Registry) -> bool {
        crate::gateway_out::take(reg).iter().any(|(t, p, _)| {
            t.ends_with("ResourcesSimulatedNotice") && read_id(p, 1) == Some(hub_id(1) as u64)
        })
    }

    #[test]
    fn an_exporter_losing_its_skyhook_re_runs_the_importer() {
        let mut reg = with_importer_upgrade();
        let mut sk = skyhook(0, false);
        crate::skyhook::put(reg.deltas_mut(), &sk);
        crate::skyhook::changed(&mut reg, &sk, 10);
        assert_eq!(imp_state(&reg), Power::Low);
        assert!(told_hub_1(&mut reg), "the importer's window is told");
        // And back.
        sk.active = true;
        crate::skyhook::put(reg.deltas_mut(), &sk);
        crate::skyhook::changed(&mut reg, &sk, 20);
        assert_eq!(imp_state(&reg), Power::Online);
    }

    /// Hubs named by the `StateChangedNotice`s queued so far.
    fn state_notices(reg: &mut Registry) -> Vec<i64> {
        crate::gateway_out::take(reg)
            .iter()
            .filter(|(t, _, _)| t.ends_with("StateChangedNotice"))
            .filter_map(|(_, p, _)| read_id(p, 4).map(|h| h as i64))
            .collect()
    }

    #[test]
    fn a_skyhook_change_tells_each_peer_whose_state_moved() {
        let mut reg = with_importer_upgrade();
        let sk = skyhook(0, false);
        let before = before(&reg, sys(0), false);
        crate::skyhook::put(reg.deltas_mut(), &sk);
        crate::skyhook::changed_since(&mut reg, &sk, 10, &before);
        assert_eq!(
            state_notices(&mut reg),
            vec![hub_id(0), hub_id(1)],
            "the exporter's own hub and the importer"
        );
        // Nothing moved: no notice.
        let before = super::before(&reg, sys(0), false);
        crate::skyhook::changed_since(&mut reg, &sk, 20, &before);
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn a_new_holder_tells_each_peer_whose_state_moved() {
        let mut reg = with_importer_upgrade();
        let before = before(&reg, sys(0), true);
        sov::set_claim(
            reg.deltas_mut(),
            sys(0),
            Some((OTHER_ALLIANCE, OTHER_CORP)),
            5,
        );
        claim_changed_since(&mut reg, sys(0), &before);
        assert_eq!(state_notices(&mut reg), vec![hub_id(0), hub_id(1)]);
        // The holder unchanged: nobody.
        let before = super::before(&reg, sys(0), true);
        claim_changed_since(&mut reg, sys(0), &before);
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn a_neutral_claim_by_entosis_tells_its_own_hub_and_the_peers() {
        let mut reg = with_importer_upgrade();
        let a = sov::holder(reg.deltas(), sys(0)).expect("held");
        // The claim lapsed (silently here) and the same alliance takes it
        // back: the stored network is its own again, so both hubs move.
        sov::set_claim(reg.deltas_mut(), sys(0), None, 5);
        // The stand-in row went with the claim; a launched hub keeps a neutral one.
        sov::put(
            reg.deltas_mut(),
            &sov::Claim {
                system_id: sys(0),
                alliance_id: None,
                corporation_id: sov::NEUTRAL_HUB_OWNER,
                hub_id: hub_id(0),
                state: sov::HubState::Neutral,
                claim_time: None,
                state_until: None,
                rest: Vec::new(),
            },
        );
        let _ = state_notices(&mut reg);
        crate::entosis::claim_neutral_at(&mut reg, sys(0), (a, CORP), 6).expect("claimed");
        let told = state_notices(&mut reg);
        assert_eq!(told, vec![hub_id(0), hub_id(1)], "the own hub and the peer");
    }

    #[test]
    fn a_score_tick_without_a_holder_change_sends_no_notice() {
        let mut reg = with_importer_upgrade();
        // What every entosis score change calls.
        sov_hub::claim_changed(&mut reg, sys(0));
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn a_lapse_tells_the_peers_of_the_gone_hub() {
        let mut reg = with_importer_upgrade();
        let h = sov_hub::hub(reg.deltas(), sys(0)).unwrap();
        sov_hub::lapse(&mut reg, &h, 100);
        // The hub itself is gone, so only the importer is told.
        assert_eq!(state_notices(&mut reg), vec![hub_id(1)]);
    }

    #[test]
    fn a_gm_claim_change_tells_each_hub_whose_state_moved() {
        let mut reg = with_importer_upgrade();
        let line = sys(0).to_string();
        let _ = crate::service::sov::slash(&mut reg, None, &["set", &line, "none"]);
        assert_eq!(state_notices(&mut reg), vec![hub_id(0), hub_id(1)]);
        // Nothing to change the second time: nobody is told.
        let _ = crate::service::sov::slash(&mut reg, None, &["set", &line, "none"]);
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn a_corp_leaving_its_alliance_tells_each_hub_whose_budget_moved() {
        let mut reg = with_importer_upgrade();
        let before = crate::sov_upgrade::corp_membership_before(&reg, CORP);
        let mut st = crate::alliance::Alliances::load(reg.deltas());
        st.leave(CORP, 5).expect("a member");
        st.save(reg.deltas_mut());
        crate::sov_upgrade::corp_membership_changed_since(&mut reg, CORP, &before);
        assert_eq!(state_notices(&mut reg), vec![hub_id(0), hub_id(1)]);
        // Nothing moved this time: nobody is told.
        let before = crate::sov_upgrade::corp_membership_before(&reg, CORP);
        crate::sov_upgrade::corp_membership_changed_since(&mut reg, CORP, &before);
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn the_skyhook_slash_commands_tell_each_hub_whose_budget_moved() {
        let mut reg = with_importer_upgrade();
        let id = 5_000.to_string();
        crate::skyhook::put(reg.deltas_mut(), &skyhook(0, false));
        let _ = crate::gateway_out::take(&mut reg);
        // Off to on: the exporter's harvest returns.
        let _ = crate::skyhook::slash(&mut reg, None, &["activate", &id]);
        assert_eq!(state_notices(&mut reg), vec![hub_id(0), hub_id(1)]);
        // A state change that moves no harvest tells nobody.
        let _ = crate::skyhook::slash(&mut reg, None, &["state", &id, "0"]);
        assert!(state_notices(&mut reg).is_empty());
    }

    #[test]
    fn an_exporting_corp_leaving_the_alliance_re_runs_the_importer() {
        let mut reg = with_importer_upgrade();
        // The skyhook's corp no longer counts for the holder.
        let mut sk = skyhook(0, true);
        sk.corp = OTHER_CORP;
        crate::skyhook::put(reg.deltas_mut(), &sk);
        crate::sov_upgrade::corp_membership_changed(&mut reg, OTHER_CORP);
        assert_eq!(imp_state(&reg), Power::Low);
        assert!(told_hub_1(&mut reg));
    }

    #[test]
    fn a_new_holder_does_not_inherit_the_network_and_the_peers_re_run() {
        let mut reg = with_importer_upgrade();
        let a = sov::holder(reg.deltas(), sys(0)).expect("held");
        sov::set_claim(
            reg.deltas_mut(),
            sys(0),
            Some((OTHER_ALLIANCE, OTHER_CORP)),
            5,
        );
        // Read before anyone is told: the row is the old holder's.
        assert_eq!(config(reg.deltas(), hub_id(0)), Mode::Inactive);
        sov_hub::claim_changed(&mut reg, sys(0));
        assert!(reg.deltas().of(KIND).get(&key(hub_id(0))).is_none());
        assert_eq!(imp_state(&reg), Power::Low);
        assert!(told_hub_1(&mut reg));
        // The old holder takes it back: the old export is not revived.
        sov::set_claim(reg.deltas_mut(), sys(0), Some((a, CORP)), 6);
        sov_hub::claim_changed(&mut reg, sys(0));
        assert_eq!(config(reg.deltas(), hub_id(0)), Mode::Inactive);
        assert_eq!((wf(&reg, 0), wf(&reg, 1)), (20, 0));
    }

    #[test]
    fn a_score_change_with_the_holder_unchanged_notifies_nobody() {
        let mut reg = with_importer_upgrade();
        sov_hub::claim_changed(&mut reg, sys(0));
        assert!(!told_hub_1(&mut reg), "the holder did not change");
        assert_eq!(imp_state(&reg), Power::Online);
        assert_ne!(config(reg.deltas(), hub_id(0)), Mode::Inactive);
    }

    #[test]
    fn a_lapsed_hub_drops_its_network() {
        let mut reg = with_importer_upgrade();
        let h = sov_hub::hub(reg.deltas(), sys(0)).unwrap();
        sov_hub::lapse(&mut reg, &h, 100);
        assert!(reg.deltas().of(KIND).get(&key(hub_id(0))).is_none());
        assert_eq!(imp_state(&reg), Power::Low);
    }

    #[test]
    fn the_resources_window_shows_the_gross_harvest_beside_the_net_budget() {
        let (mut reg, _) = world(2, &[0, 1], &[0]);
        import(&mut reg, 1, &[0]);
        export(&mut reg, 0, 15, Some(1));
        let ex = crate::sov_resources::hub_resources(&reg, hub_id(0)).unwrap();
        assert_eq!(
            (ex.workforce.available, ex.workforce.local_harvest),
            (5, 20)
        );
        let im = crate::sov_resources::hub_resources(&reg, hub_id(1)).unwrap();
        assert_eq!(
            (im.workforce.available, im.workforce.local_harvest),
            (15, 0)
        );
    }

    #[test]
    fn an_import_lists_only_connected_sources_and_at_most_three() {
        let (mut reg, _) = world(5, &[0, 1, 2, 3, 4], &[0]);
        // Connected but clamped to nothing still lists, with 0.
        import(&mut reg, 1, &[0, 2]);
        export(&mut reg, 0, 15, Some(1));
        assert_eq!(state_of(&reg, 1), State::Import(vec![(sys(0), 15)]));
        crate::skyhook::put(reg.deltas_mut(), &skyhook(0, false));
        assert_eq!(state_of(&reg, 1), State::Import(vec![(sys(0), 0)]));
        let imp = |v: &[i64]| Mode::Import(v.iter().map(|i| sys(*i)).collect());
        assert_eq!(
            configure(&mut reg, hub_id(1), ALICE, imp(&[0, 2, 3, 4])),
            Err(400)
        );
        assert_eq!(
            configure(&mut reg, hub_id(1), ALICE, imp(&[0, 2, 3])),
            Ok(())
        );
    }

    fn server(reg: Registry) -> Ground {
        Ground::Server(Arc::new(RwLock::new(reg)))
    }

    fn request(hub: i64, cfg: Option<&Mode>) -> Vec<u8> {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &id(hub as u64));
        if let Some(m) = cfg {
            put_bytes(&mut b, 2, &configuration_bytes(m));
        }
        b
    }

    fn ask(g: &Ground, rest: &str, body: &[u8], who: i64) -> Option<Reply> {
        gateway(&format!("{PKG}{rest}Request"), body, who, g)
    }

    #[test]
    fn the_gateway_configures_reads_back_and_lists_the_hubs_in_range() {
        let (reg, _) = world(8, &[0, 1, 7], &[0]);
        let g = server(reg);
        // Unconfigured: inactive, in both answers.
        let r = ask(&g, "GetConfiguration", &request(hub_id(0), None), ALICE).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(
            get_uint(&get_bytes(&r.payload.unwrap().1, 1).unwrap(), 1),
            Some(1)
        );
        let r = ask(&g, "GetState", &request(hub_id(0), None), ALICE).unwrap();
        assert_eq!(
            get_uint(&get_bytes(&r.payload.unwrap().1, 1).unwrap(), 1),
            Some(1)
        );
        // Bob cannot write; Alice can.
        let imp = Mode::Import(BTreeSet::from([sys(0)]));
        let r = ask(&g, "Configure", &request(hub_id(1), Some(&imp)), BOB).unwrap();
        assert_eq!((r.status, r.payload), (403, None));
        let r = ask(&g, "Configure", &request(hub_id(1), Some(&imp)), ALICE).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(
            r.payload.unwrap().0,
            "eve_public.sovereignty.hub.workforce.api.ConfigureResponse"
        );
        let r = ask(&g, "GetConfiguration", &request(hub_id(1), None), ALICE).unwrap();
        let cfg = get_bytes(&r.payload.unwrap().1, 1).unwrap();
        assert_eq!(parse_configuration(&cfg), Some(imp));
        // A Configuration with no mode is refused.
        let empty = request(hub_id(1), None);
        let mut no_mode = empty.clone();
        put_bytes(&mut no_mode, 2, &[]);
        let r = ask(&g, "Configure", &no_mode, ALICE).unwrap();
        assert_eq!(r.status, 400);
        // Networkable from system 0: system 1 (one gate) but not system 7
        // (seven), and what it lists carries that hub's configuration.
        let r = ask(&g, "GetNetworkableHubs", &request(hub_id(0), None), ALICE).unwrap();
        let hubs = field(&r.payload.unwrap().1, 1);
        assert_eq!(hubs.len(), 1);
        assert_eq!(read_id(&hubs[0], 2), Some(sys(1) as u64));
        assert_eq!(read_id(&hubs[0], 1), Some(hub_id(1) as u64));
        assert_eq!(
            parse_configuration(&get_bytes(&hubs[0], 3).unwrap()),
            Some(Mode::Import(BTreeSet::from([sys(0)])))
        );
        assert!(get_bytes(&hubs[0], 4).is_some());
        // The exporter now connects, and the state says how much.
        let exp = Mode::Export {
            amount: 8,
            dest: Some(sys(1)),
        };
        assert_eq!(
            ask(&g, "Configure", &request(hub_id(0), Some(&exp)), ALICE)
                .unwrap()
                .status,
            200
        );
        let r = ask(&g, "GetState", &request(hub_id(0), None), ALICE).unwrap();
        let st = get_bytes(&r.payload.unwrap().1, 1).unwrap();
        let c = get_bytes(&get_bytes(&st, 3).unwrap(), 5).unwrap();
        assert_eq!(
            (read_id(&c, 1), get_uint(&c, 2)),
            (Some(sys(1) as u64), Some(8))
        );
        let r = ask(&g, "GetState", &request(hub_id(1), None), ALICE).unwrap();
        let st = get_bytes(&r.payload.unwrap().1, 1).unwrap();
        let src = field(&get_bytes(&st, 2).unwrap(), 1);
        assert_eq!(
            (read_id(&src[0], 1), get_uint(&src[0], 2)),
            (Some(sys(0) as u64), Some(8))
        );
        // A pilot outside the alliance cannot read.
        let r = ask(&g, "GetState", &request(hub_id(0), None), CAROL).unwrap();
        assert_eq!(r.status, 403);
    }

    #[test]
    fn the_arm_passes_down_what_it_does_not_own() {
        let (reg, _) = world(2, &[0], &[]);
        let g = server(reg);
        // Not a workforce type.
        assert_eq!(
            gateway(
                "eve_public.sovereignty.hub.api.GetResourcesRequest",
                &[],
                ALICE,
                &g
            ),
            None
        );
        // A hub this server does not know: the stub's `inactive` answers.
        assert!(ask(&g, "GetConfiguration", &request(12_345, None), ALICE).is_none());
        assert!(ask(&g, "GetState", &[], ALICE).is_none());
        // No world: down to the stub, which refuses the write honestly.
        let local = Ground::local();
        assert!(ask(&local, "GetState", &request(hub_id(0), None), ALICE).is_none());
        let r = crate::gateway::answer(
            &format!("{PKG}ConfigureRequest"),
            &request(hub_id(0), Some(&Mode::Transit)),
            ALICE,
            &local,
        );
        assert_eq!(r.status, 403);
    }
}
