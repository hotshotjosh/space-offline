//! Cyno jammers (JAM-1, issue #87): the jammed state of a system and what the
//! location panel's icon reads.
//!
//! **What the client reads.** The panel builds a `CynoJammerIcon` in every
//! claimable system (`infoPanelLocationInfo.py:385,414`), which asks
//! `beyonce.GetCynoJammerState()` once (`sovDashboard/cynoJammerIcon.py:45-46`)
//! **[V]**. The answer is the jammer's *online sim time* (`:48-58`):
//!
//! - `None` is "not jammed" (`GetJamStatus`, `:77-79`) **[V]**;
//! - `0`, or a time not in the future, is "on" (`:82-83`) **[V]**;
//! - a time in the future is "pending" (`:84-85`): the icon pulses and the
//!   tooltip counts down to it (`:159-186`) **[V]**.
//!
//! A change is pushed as `OnCynoJammerChanged(solarsystemID, onlineSimTime)`
//! (`infoPanelLocationInfo.py:470-474`); the handler drops it when the system
//! is not the pilot's own and otherwise runs `UpdateIconState` with the same
//! three-way value **[V]**. That the scattered event is named exactly like the
//! handler is **[I]**: the handler has no other caller in the decompiled tree.
//!
//! **What jams a system.** An Upwell structure of type [`TYPE_TENEBREX`]
//! (`structures.STRUCTURE_SIZE_BY_TYPE` lists it as a flex structure
//! **[V]**, `crate::structure::size_of_type`) that is online
//! (`State::is_offline` false) is "on". One that is still deploying, with a
//! known finish time (`deploy_end` while `DeployVulnerable`), is "pending"
//! until then. One still `Anchoring` is "off": its timer's end is when the
//! deploy *starts*, not when it comes online, so no finish time is known. Sim time is FILETIME, as in
//! `structure.rs` **[I]**. Several jammers in a system still make one jam: the
//! best state wins, the earliest finish among pending ones.
//!
//! - **[A]** (SOV-UPG-2, #80) a Tenebrex is "on" only while its system's sov
//!   hub has an Online upgrade that enables the `cyno_jammer` service
//!   (`sov_upgrade::upgrade_online`) **[I]** (the client's upgrade events,
//!   `eveProto/generated/eve/sovereignty/hub/upgrade/api/events_pb2.py:16`,
//!   carry an upgrade's online state). **OWNER:** the gate ships OFF:
//!   [`JAMMER_NEEDS_UPGRADE`] is `false` because a level-3 upgrade needs 35
//!   days of held claim and `true` would turn every live jammer Off on deploy.
//!   Flip it when worlds hold upgrades (`/sov age claim` back-dates a claim for
//!   tests). Falsified by a Tenebrex online on TQ without the upgrade.
//! - **[A]** the activation delay `attributeCynoJammerActivationDelay` (2794)
//!   is not added to the deploy time. Falsified by a type row that carries it.
//! - **[A]** the owner need not be in an alliance (a GM-anchored one counts).
//!
//! `service/sov.rs` has the wire arm; [`tick`] pushes changes.

use crate::delta::Deltas;
use crate::notify::Push;
use crate::service::Registry;
use crate::sov_upgrade::{self, UpgradeTable, Want};
use crate::structure::{self, State, Structure};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// `Tenebrex Cyno Jammer`, an Upwell flex structure **[V]**
/// (`structure::size_of_type`).
pub const TYPE_TENEBREX: i64 = 37_534;

/// **OWNER SWITCH (SOV-UPG-2, #80), shipped `false`.** `true`: a Tenebrex
/// is Off until its system's hub has the `cyno_jammer` upgrade Online, so every
/// jammer in a world that already holds some goes Off at once. `false`: the
/// old rule, a Tenebrex that is online jams, with no upgrade needed. The gate
/// code is in place either way.
pub const JAMMER_NEEDS_UPGRADE: bool = false;

/// How a system's jammer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jam {
    /// No jammer, or none that will ever come online.
    Off,
    /// Deploying; comes online at this FILETIME.
    Pending(i64),
    /// Online.
    On,
}

impl Jam {
    /// The value `GetCynoJammerState` answers and `OnCynoJammerChanged`
    /// carries **[V]** (module header).
    pub fn wire(self) -> PyValue {
        match self {
            Jam::Off => PyValue::None,
            Jam::Pending(at) => PyValue::Int(at),
            Jam::On => PyValue::Int(0),
        }
    }

    /// Ranking for "the best jammer in the system".
    fn merge(self, other: Jam) -> Jam {
        match (self, other) {
            (Jam::On, _) | (_, Jam::On) => Jam::On,
            (Jam::Pending(a), Jam::Pending(b)) => Jam::Pending(a.min(b)),
            (Jam::Pending(a), Jam::Off) | (Jam::Off, Jam::Pending(a)) => Jam::Pending(a),
            (Jam::Off, Jam::Off) => Jam::Off,
        }
    }
}

/// One structure's contribution; `Off` for anything but a Tenebrex.
fn of_structure(s: &Structure) -> Jam {
    if s.type_id != TYPE_TENEBREX {
        return Jam::Off;
    }
    if s.is_online() {
        return Jam::On;
    }
    match s.state {
        State::DeployVulnerable => s.deploy_end.map_or(Jam::Off, Jam::Pending),
        _ => Jam::Off,
    }
}

/// Whether the sov upgrade rule lets `system`'s jammer work (`needs`: the rule
/// is on, [`JAMMER_NEEDS_UPGRADE`]).
fn allowed(d: &Deltas, t: &UpgradeTable, system: i64, needs: bool) -> bool {
    !needs || sov_upgrade::upgrade_online(d, t, system, Want::Service("cyno_jammer"))
}

/// The state of `system`'s jammer, with `needs` saying whether the sov upgrade
/// rule applies. `t` is `sov_upgrade::table(reg.content())`.
pub fn state_with(d: &Deltas, t: &UpgradeTable, system: i64, needs: bool) -> Jam {
    if !allowed(d, t, system, needs) {
        return Jam::Off;
    }
    structure::in_system(d, system)
        .iter()
        .map(of_structure)
        .fold(Jam::Off, Jam::merge)
}

/// The state of `system`'s jammer.
pub fn state(d: &Deltas, t: &UpgradeTable, system: i64) -> Jam {
    state_with(d, t, system, JAMMER_NEEDS_UPGRADE)
}

/// The state of every system that has a Tenebrex, from **one** decode of the
/// structure rows (the clock stage's read: cost grows with the structures,
/// not with structures times jammers).
pub fn states(d: &Deltas, t: &UpgradeTable) -> BTreeMap<i64, Jam> {
    states_with(d, t, JAMMER_NEEDS_UPGRADE)
}

/// [`states`] with the sov upgrade rule `needs` named.
pub fn states_with(d: &Deltas, t: &UpgradeTable, needs: bool) -> BTreeMap<i64, Jam> {
    let mut out: BTreeMap<i64, Jam> = BTreeMap::new();
    let mut ok: BTreeMap<i64, bool> = BTreeMap::new();
    for s in structure::all(d)
        .iter()
        .filter(|s| s.type_id == TYPE_TENEBREX)
    {
        let e = out.entry(s.system).or_insert(Jam::Off);
        if *ok
            .entry(s.system)
            .or_insert_with(|| allowed(d, t, s.system, needs))
        {
            *e = e.merge(of_structure(s));
        }
    }
    out
}

/// Whether `system` is jammed now: a Tenebrex is online there. What a cyno
/// refusal (#88) asks.
pub fn jammed(reg: &Registry, system: i64) -> bool {
    state(reg.deltas(), sov_upgrade::table(reg.content()), system) == Jam::On
}

/// Why a Tenebrex may not be anchored in `system`, or `None`: one jammer per
/// system. For the anchor path to ask before it takes the item.
pub fn anchor_refusal(d: &Deltas, system: i64) -> Option<&'static str> {
    structure::in_system(d, system)
        .iter()
        .any(|s| s.type_id == TYPE_TENEBREX)
        .then_some("There is already a cyno jammer in this solar system.")
}

/// `beyonce.GetCynoJammerState()` for a pilot in `system` (`None`: nowhere).
pub fn wire_state(reg: &Registry, system: Option<i64>) -> PyValue {
    let t = sov_upgrade::table(reg.content());
    system.map_or(PyValue::None, |s| state(reg.deltas(), t, s).wire())
}

/// `OnCynoJammerChanged(solarsystemID, onlineSimTime)` to the pilots in the
/// system **[V]** (module header).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CynoJammerChanged {
    pub to: Vec<i64>,
    pub system: i64,
    pub jam: Jam,
}

impl Push for CynoJammerChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCynoJammerChanged",
            vec![PyValue::Int(self.system), self.jam.wire()],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// What was last told, by system. Only systems with a jammer on record are
/// kept; a restart forgets, and the first [`tick`] then repeats the news,
/// which is harmless (the client takes the same value twice).
#[derive(Debug, Default)]
pub struct Told(BTreeMap<i64, Jam>);

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "sov_jammer",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Told::default())
}

/// Systems whose state differs from what was last told, with the new state;
/// records it.
fn changes(
    d: &Deltas,
    t: &UpgradeTable,
    needs: bool,
    told: &mut BTreeMap<i64, Jam>,
) -> Vec<(i64, Jam)> {
    let now_by = states_with(d, t, needs);
    let mut systems: Vec<i64> = now_by.keys().copied().collect();
    systems.extend(told.keys().copied());
    systems.sort_unstable();
    systems.dedup();
    let mut out = Vec::new();
    for system in systems {
        let now = now_by.get(&system).copied().unwrap_or(Jam::Off);
        let before = told.get(&system).copied().unwrap_or(Jam::Off);
        if now == before {
            continue;
        }
        if now == Jam::Off {
            told.remove(&system);
        } else {
            told.insert(system, now);
        }
        out.push((system, now));
    }
    out
}

/// The clock stage: push every jammer change to the pilots in the system.
pub fn tick(reg: &mut Registry, _: std::time::Instant, _: crate::damage::BlueTime) {
    if reg.deltas().of(structure::KIND).is_empty() && reg.ext::<Told>().0.is_empty() {
        return;
    }
    let mut told = std::mem::take(&mut reg.ext_mut::<Told>().0);
    let changed = changes(
        reg.deltas(),
        sov_upgrade::table(reg.content()),
        JAMMER_NEEDS_UPGRADE,
        &mut told,
    );
    reg.ext_mut::<Told>().0 = told;
    for (system, jam) in changed {
        let to = crate::service::structures::system_audience(reg, system);
        if !to.is_empty() {
            reg.push(CynoJammerChanged { to, system, jam });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::Env;

    const CORP: i64 = 98_700_001;
    const SYS: i64 = 30_870_001;
    const NOW: i64 = 1_000_000 * structure::SECOND;

    fn put(d: &mut Deltas, id: i64, type_id: i64, system: i64, state: State) -> Structure {
        let env = Env::new(NOW);
        let mut s = Structure::anchored(id, type_id, CORP, system, [0.0; 3], 0.0, "jam", 1, &env);
        s.set_state(state, &env);
        s.take_events();
        structure::put(d, &s);
        s
    }

    /// The pre-SOV-UPG-2 rule: no upgrade needed.
    fn old(d: &Deltas, system: i64) -> Jam {
        state_with(d, &UpgradeTable::default(), system, false)
    }

    #[test]
    fn no_structure_is_not_jammed() {
        let d = Deltas::default();
        assert_eq!(old(&d, SYS), Jam::Off);
        assert_eq!(Jam::Off.wire(), PyValue::None);
    }

    #[test]
    fn an_online_tenebrex_jams_its_own_system_only() {
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        assert_eq!(old(&d, SYS), Jam::On);
        assert_eq!(old(&d, SYS + 1), Jam::Off);
        assert_eq!(Jam::On.wire(), PyValue::Int(0));
    }

    #[test]
    fn another_structure_type_does_not_jam() {
        let mut d = Deltas::default();
        // An invented type id: any type but the Tenebrex's.
        put(&mut d, 1, 990_001, SYS, State::ShieldVulnerable);
        assert_eq!(old(&d, SYS), Jam::Off);
    }

    #[test]
    fn a_deploying_tenebrex_is_pending_until_its_deploy_end() {
        let mut d = Deltas::default();
        let s = put(&mut d, 1, TYPE_TENEBREX, SYS, State::DeployVulnerable);
        let end = s.deploy_end.expect("deploy timer set");
        assert_eq!(old(&d, SYS), Jam::Pending(end));
        assert_eq!(Jam::Pending(end).wire(), PyValue::Int(end));
        assert!(end > NOW);
    }

    #[test]
    fn an_unanchored_or_offline_tenebrex_is_off() {
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::Unanchored);
        assert_eq!(old(&d, SYS), Jam::Off);
        put(&mut d, 2, TYPE_TENEBREX, SYS + 1, State::AnchorVulnerable);
        assert_eq!(old(&d, SYS + 1), Jam::Off);
    }

    #[test]
    fn the_best_of_several_wins() {
        assert_eq!(Jam::Pending(5).merge(Jam::Pending(3)), Jam::Pending(3));
        assert_eq!(Jam::Pending(5).merge(Jam::On), Jam::On);
        assert_eq!(Jam::Off.merge(Jam::Pending(7)), Jam::Pending(7));
    }

    #[test]
    fn a_second_jammer_is_refused() {
        let mut d = Deltas::default();
        assert_eq!(anchor_refusal(&d, SYS), None);
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::DeployVulnerable);
        assert!(anchor_refusal(&d, SYS).is_some());
        assert_eq!(anchor_refusal(&d, SYS + 1), None);
    }

    #[test]
    fn changes_are_reported_once_per_change() {
        let mut d = Deltas::default();
        let mut told = BTreeMap::new();
        assert!(changes(&d, &UpgradeTable::default(), false, &mut told).is_empty());
        let s = put(&mut d, 1, TYPE_TENEBREX, SYS, State::DeployVulnerable);
        let end = s.deploy_end.expect("timer");
        assert_eq!(
            changes(&d, &UpgradeTable::default(), false, &mut told),
            vec![(SYS, Jam::Pending(end))]
        );
        assert!(
            changes(&d, &UpgradeTable::default(), false, &mut told).is_empty(),
            "no change, no news"
        );
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        assert_eq!(
            changes(&d, &UpgradeTable::default(), false, &mut told),
            vec![(SYS, Jam::On)]
        );
        structure::remove(&mut d, 1);
        assert_eq!(
            changes(&d, &UpgradeTable::default(), false, &mut told),
            vec![(SYS, Jam::Off)]
        );
        assert!(told.is_empty());
    }

    #[test]
    fn an_anchoring_tenebrex_is_off_for_its_timer_ends_before_the_deploy() {
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::Anchoring);
        assert_eq!(old(&d, SYS), Jam::Off);
    }

    #[test]
    fn one_decode_gives_every_jammed_system_with_the_best_of_several() {
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        put(&mut d, 2, TYPE_TENEBREX, SYS, State::DeployVulnerable);
        let s2 = put(&mut d, 3, TYPE_TENEBREX, SYS + 1, State::DeployVulnerable);
        put(&mut d, 4, 990_001, SYS + 2, State::ShieldVulnerable);
        let got = states_with(&d, &UpgradeTable::default(), false);
        assert_eq!(got.len(), 2, "only systems with a Tenebrex");
        assert_eq!(got[&SYS], Jam::On);
        assert_eq!(got[&(SYS + 1)], Jam::Pending(s2.deploy_end.expect("timer")));
        for (sys, jam) in &got {
            assert_eq!(old(&d, *sys), *jam, "same answer as the per-system read");
        }
    }

    #[test]
    fn the_push_carries_system_and_time() {
        let p = CynoJammerChanged {
            to: vec![7],
            system: SYS,
            jam: Jam::Pending(42),
        };
        assert_eq!(
            p.to_push(),
            (
                "OnCynoJammerChanged",
                vec![PyValue::Int(SYS), PyValue::Int(42)]
            )
        );
        assert_eq!(p.audience(), Some(vec![7]));
    }

    // -- SOV-UPG-2 (#80): the upgrade gate -------------------------------------

    const HUB: i64 = 8_870_001;
    const ALLIANCE: i64 = 99_870_001;
    /// Invented upgrade types of `sov_upgrade::fixture_upgrades`: 9_203 enables
    /// `cyno_jammer`, 9_204 `jump_bridge`.
    const JAMMER_UPGRADE: i64 = 9_203;
    const OTHER_UPGRADE: i64 = 9_204;

    fn hub_with(d: &mut Deltas, system: i64, held: bool, ups: &[(i64, sov_upgrade::Power)]) {
        crate::sov_hub::put(
            d,
            &crate::sov_hub::Hub {
                system_id: system,
                hub_id: HUB + system % 100,
                position: [0.0; 3],
                launcher_id: None,
                launch_time: 0,
                offline_days: 0,
            },
        );
        if held {
            crate::sov::set_claim(d, system, Some((ALLIANCE, CORP)), 1);
        }
        let list: Vec<_> = ups
            .iter()
            .map(|(t, p)| sov_upgrade::Installed {
                type_id: *t,
                online: true,
                state: *p,
            })
            .collect();
        sov_upgrade::put_installed(d, HUB + system % 100, &list);
    }

    #[test]
    fn a_tenebrex_is_off_without_the_service_and_on_with_it() {
        let t = sov_upgrade::fixture_upgrades();
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        // No hub at all.
        assert_eq!(state_with(&d, &t, SYS, true), Jam::Off);
        // A held hub with an upgrade that is not the service.
        hub_with(
            &mut d,
            SYS,
            true,
            &[(OTHER_UPGRADE, sov_upgrade::Power::Online)],
        );
        assert_eq!(state_with(&d, &t, SYS, true), Jam::Off);
        // The service upgrade, installed but not Online.
        hub_with(
            &mut d,
            SYS,
            true,
            &[(JAMMER_UPGRADE, sov_upgrade::Power::Low)],
        );
        assert_eq!(state_with(&d, &t, SYS, true), Jam::Off);
        // The service upgrade Online.
        hub_with(
            &mut d,
            SYS,
            true,
            &[(JAMMER_UPGRADE, sov_upgrade::Power::Online)],
        );
        assert_eq!(state_with(&d, &t, SYS, true), Jam::On);
        // A neutral hub's upgrades do nothing.
        hub_with(
            &mut d,
            SYS,
            false,
            &[(JAMMER_UPGRADE, sov_upgrade::Power::Online)],
        );
        crate::sov::set_claim(&mut d, SYS, None, 2);
        assert_eq!(state_with(&d, &t, SYS, true), Jam::Off);
        // The switch off restores the old rule whatever the hub holds.
        assert_eq!(state_with(&d, &t, SYS, false), Jam::On);
    }

    #[test]
    fn the_gate_reads_the_system_of_the_tenebrex() {
        let t = sov_upgrade::fixture_upgrades();
        let mut d = Deltas::default();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        put(&mut d, 2, TYPE_TENEBREX, SYS + 1, State::ShieldVulnerable);
        hub_with(
            &mut d,
            SYS,
            true,
            &[(JAMMER_UPGRADE, sov_upgrade::Power::Online)],
        );
        let got = states_with(&d, &t, true);
        assert_eq!(got[&SYS], Jam::On);
        assert_eq!(got[&(SYS + 1)], Jam::Off);
        for (sys, jam) in &got {
            assert_eq!(state_with(&d, &t, *sys, true), *jam);
        }
    }

    #[test]
    fn dropping_the_upgrade_is_news_to_the_system() {
        let t = sov_upgrade::fixture_upgrades();
        let mut d = Deltas::default();
        let mut told = BTreeMap::new();
        put(&mut d, 1, TYPE_TENEBREX, SYS, State::ShieldVulnerable);
        hub_with(
            &mut d,
            SYS,
            true,
            &[(JAMMER_UPGRADE, sov_upgrade::Power::Online)],
        );
        assert_eq!(changes(&d, &t, true, &mut told), vec![(SYS, Jam::On)]);
        sov_upgrade::put_installed(&mut d, HUB + SYS % 100, &[]);
        assert_eq!(changes(&d, &t, true, &mut told), vec![(SYS, Jam::Off)]);
    }
}
