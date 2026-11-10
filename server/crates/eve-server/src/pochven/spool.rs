//! Triglavian spooling damage: a disintegrator's multiplier climbs by
//! `damageMultiplierBonusPerCycle` (2733) per cycle up to `damageMultiplierBonusMax`
//! (2734) [V names, dogma/const.py:281-282]; the client shows the current value as
//! 5804 and the time of the max as 5818 (moduleButtonTooltip.py:893-924) [V].
//! Shared by the NPC brain (PV-5) and the player's module (PV-7).

pub const ATTR_PER_CYCLE: i64 = 2733;
pub const ATTR_MAX: i64 = 2734;
pub const ATTR_CURRENT: i64 = 5804;
pub const ATTR_MAX_TIMESTAMP: i64 = 5818;
/// The FX guid the client's TriglavianBeam class draws (effects.fsdbinary) [V-data].
pub const BEAM_GUID: &str = "effects.TriglavianBeam";
/// The Mutadaptive Remote Armor Repairer's effect: it shares the beam's guid (so
/// its start needs the `graphicInfo` dict) but does not ramp damage; the dict is
/// read from `repairMultiplierBonusPerCycle`/`Max` 2796/2797
/// (`dogma/const.py:284-285`) [V]. Effect id from the typedata effect table [V-data].
pub const REPAIR_BEAM_EFFECT: i64 = 7166;
pub const ATTR_REPAIR_PER_CYCLE: i64 = 2796;
pub const ATTR_REPAIR_MAX: i64 = 2797;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spool {
    pub target: Option<i64>,
    pub current: f64,
}

impl Spool {
    /// One cycle landed on `target`: a new target resets to 0 first [I], then
    /// `current = min(current + per_cycle, max)`. Returns the multiplier to apply to
    /// THIS cycle: `1 + current_before_increment` (the first shot is unspooled) [A].
    pub fn cycle(&mut self, target: i64, per_cycle: f64, max: f64) -> f64 {
        if self.target != Some(target) {
            self.target = Some(target);
            self.current = 0.0;
        }
        let mult = 1.0 + self.current;
        self.current = (self.current + per_cycle).min(max);
        mult
    }

    /// The client's Stop: the ramp is gone [V].
    pub fn stop(&mut self) {
        self.current = 0.0;
        self.target = None;
    }
}

/// The first guid of a comma list (§0.3.2) [I].
pub fn first_guid(g: &str) -> &str {
    g.split(',').next().unwrap_or(g).trim()
}

// ---------------------------------------------------------------------------
// PV-7: the player's disintegrator
// ---------------------------------------------------------------------------

use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "trig_spool",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Spools::default())
}

/// The dogma defaults when a type and its sheet carry neither attribute:
/// `damageMultiplierBonusPerCycle` 0.0 and `damageMultiplierBonusMax` 0.5 [V-data]
/// (the dogma attribute defaults, findings in `parity-pochven.md` §0.4).
pub const DEFAULT_PAIR: (f64, f64) = (0.0, 0.5);

/// One running disintegrator's ramp.
#[derive(Debug, Clone, PartialEq, Default)]
struct Entry {
    spool: Spool,
    /// The multiplier of each recent cycle by its index (`1 + current` before
    /// its increment): what the shot path reads, so a volley gets its own
    /// cycle's number however many cycles one tick records.
    applied: BTreeMap<i64, f64>,
}

/// How many cycle multipliers an entry keeps: a volley is planned in the tick
/// that recorded its cycle, so a handful is plenty.
const KEEP_CYCLES: usize = 8;

/// The ramps of the player's running disintegrators, by module id.
#[derive(Debug, Default)]
pub struct Spools {
    by_module: BTreeMap<i64, Entry>,
}

impl Spools {
    /// Cycle `index` of `module` started on `target`. Returns the `(previous
    /// current, current)` pair; the multiplier for this cycle is kept, under
    /// its index, for [`factor`].
    pub fn cycle(
        &mut self,
        module: i64,
        target: i64,
        index: i64,
        per_cycle: f64,
        max: f64,
    ) -> (f64, f64) {
        let e = self.by_module.entry(module).or_default();
        let before = if e.spool.target == Some(target) {
            e.spool.current
        } else {
            0.0
        };
        let mult = e.spool.cycle(target, per_cycle, max);
        e.applied.insert(index, mult);
        while e.applied.len() > KEEP_CYCLES {
            e.applied.pop_first();
        }
        (before, e.spool.current)
    }

    /// The module stopped: its ramp is gone.
    pub fn stop(&mut self, module: i64) {
        self.by_module.remove(&module);
    }

    /// 5804 as the client should read it now (0 without a ramp).
    pub fn current(&self, module: i64) -> f64 {
        self.by_module.get(&module).map_or(0.0, |e| e.spool.current)
    }

    pub fn is_empty(&self) -> bool {
        self.by_module.is_empty()
    }

    /// Forget every ramp whose module is not in `running`.
    pub fn retain_running(&mut self, running: impl Fn(i64) -> bool) {
        self.by_module.retain(|m, _| running(*m));
    }
}

/// The damage factor the shot path applies to cycle `index` of `module`: that
/// cycle's own multiplier, `1.0` for a module with no ramp or a cycle it has no
/// number for. Read-only (`combat::plan_unit` is).
pub fn factor(w: &Registry, module: i64, index: i64) -> f64 {
    let s = w.ext::<Spools>();
    if s.is_empty() {
        return 1.0;
    }
    s.by_module
        .get(&module)
        .and_then(|e| e.applied.get(&index))
        .map_or(1.0, |m| m.max(1.0))
}

/// When 5818 says the ramp reaches its max: the start of the cycle that makes
/// `current` reach `max`, `now` for a ramp already at max (or one that never
/// climbs). `current` is the value after the cycle that started at `at`; each
/// further cycle starts one `period` later and adds `per_cycle` [I]
/// (`triglavianBeam.py:25-30` adds on each repeat).
pub fn max_timestamp(at: i64, period: i64, current: f64, per_cycle: f64, max: f64) -> i64 {
    if per_cycle <= 0.0 || current >= max {
        return at;
    }
    let n = (((max - current) / per_cycle) - 1e-9).ceil().max(0.0) as i64;
    at + n * period
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only what the [A] on the first cycle does not decide: the multiplier never
    /// falls, steps by `per_cycle` while under the cap, reaches `1 + max` and stays.
    #[test]
    fn the_multiplier_climbs_by_per_cycle_to_one_plus_the_cap_and_stays() {
        let mut s = Spool::default();
        let (p, m) = (0.25, 0.5);
        let ms: Vec<f64> = (0..8).map(|_| s.cycle(7, p, m)).collect();
        assert!(ms.windows(2).all(|w| w[0] <= w[1]), "never falls");
        assert!(
            ms.iter().all(|x| *x <= 1.0 + m + 1e-12),
            "never above the cap"
        );
        assert_eq!(*ms.last().unwrap(), 1.0 + m, "reaches 1 + max");
        assert_eq!(ms[ms.len() - 2], 1.0 + m, "and stays");
        let first_step = ms[1] - ms[0];
        assert!((first_step - p).abs() < 1e-12, "a step is per_cycle");
    }

    #[test]
    fn a_new_target_resets_the_spool() {
        let mut s = Spool::default();
        s.cycle(7, 0.25, 1.0);
        s.cycle(7, 0.25, 1.0);
        assert!(s.cycle(7, 0.25, 1.0) > Spool::default().cycle(7, 0.25, 1.0));
        assert_eq!(s.cycle(8, 0.25, 1.0), Spool::default().cycle(8, 0.25, 1.0));
        assert_eq!(s.target, Some(8));
    }

    #[test]
    fn stop_resets() {
        let mut s = Spool::default();
        s.cycle(7, 0.25, 1.0);
        s.cycle(7, 0.25, 1.0);
        s.stop();
        assert_eq!(s, Spool::default());
        assert_eq!(s.cycle(7, 0.25, 1.0), Spool::default().cycle(7, 0.25, 1.0));
    }

    #[test]
    fn first_guid_takes_the_part_before_the_comma() {
        assert_eq!(first_guid("effects.A,effects.B"), "effects.A");
        assert_eq!(first_guid("effects.A"), "effects.A");
        assert_eq!(first_guid(""), "");
    }
}

#[cfg(test)]
mod world {
    //! PV-7: a fitted disintegrator through the module and shot paths. Every number
    //! here is invented; nothing is the client's.
    use super::*;
    use crate::modules::MSEC;
    use crate::service::Registry;
    use eve_marshal::PyValue;
    use std::time::{Duration, Instant};

    const ALICE: i64 = 90_268_001;
    const BOB: i64 = 90_268_002;
    const ALICE_SHIP: i64 = 100_268_001;
    const BOB_SHIP: i64 = 100_268_002;
    const GUN: i64 = 100_268_101;
    const GUN_TYPE: i64 = 990_268_001;
    const HULL: i64 = 990960587;
    /// Invented: 0.25 a cycle up to 1.0, a 2.5 s cycle.
    const PER_CYCLE: f64 = 0.25;
    const MAX: f64 = 1.0;
    const PERIOD_MS: i64 = 2_500;

    fn fixture() -> String {
        format!(
            "\
attribute\t51\t0.0\tspeed
attribute\t64\t1.0\tdamageMultiplier
attribute\t114\t0.0\temDamage
attribute\t116\t0.0\texplosiveDamage
attribute\t117\t0.0\tkineticDamage
attribute\t118\t0.0\tthermalDamage
attribute\t2733\t0.0\tdamageMultiplierBonusPerCycle
attribute\t2734\t0.5\tdamageMultiplierBonusMax
type\t{HULL}\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t\t9=300.0\tTest Hull
type\t{GUN_TYPE}\t53\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t990268\t12,16,42,990268\t51={PERIOD_MS}.0;64=2.0;117=10.0;2733={PER_CYCLE};2734={MAX}\tTest Disintegrator
effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
effect\t42\t0\t0\t0\t0\t0\t0\t0\tturretFitted\t
effect\t990268\t2\t1\t51\t0\t0\t0\t0\tdisintegratorTest\teffects.TriglavianBeam,effects.AttackMode
"
        )
    }

    fn world() -> Registry {
        world_from(fixture())
    }

    fn world_from(fx: String) -> Registry {
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(crate::typedata::Table::parse(&fx).expect("parses")),
        );
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: HULL,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = HULL;
            i.group_id = 25;
            i.category_id = crate::typedata::CATEGORY_SHIP;
        }
        items.push(crate::item::Item {
            item_id: GUN,
            type_id: GUN_TYPE,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: 27,
            quantity: 1,
            group_id: 53,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_269_000,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(GUN, true);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        let _ = reg.take_pending();
        reg
    }

    /// What a step pushed: `(audience, event, args)`.
    type Pushes = Vec<(Vec<i64>, &'static str, Vec<PyValue>)>;

    fn all(reg: &mut Registry) -> Pushes {
        let mut out: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (n, a) = p.to_push();
                (p.audience().unwrap_or_default(), n, a)
            })
            .collect();
        out.extend(
            reg.take_due(Instant::now() + Duration::from_secs(3600))
                .into_iter()
                .map(|p| {
                    let (n, a) = p.to_push();
                    (p.audience().unwrap_or_default(), n, a)
                }),
        );
        out
    }

    /// The damage Bob's combat log reports for the volley that hit him: the sum of
    /// its `damageAttributes`.
    fn hit(pushes: &[(Vec<i64>, &'static str, Vec<PyValue>)]) -> Option<f64> {
        pushes
            .iter()
            .filter(|(to, n, _)| *n == "OnDamageMessage" && *to == vec![BOB])
            .find_map(
                |(_, _, a)| match crate::pyutil::dict_get(&a[0], b"damageAttributes") {
                    Some(PyValue::Dict(rows)) => Some(
                        rows.iter()
                            .map(|(v, _)| if let PyValue::Real(r) = v { *r } else { 0.0 })
                            .sum(),
                    ),
                    _ => None,
                },
            )
    }

    /// Every volley that hit Bob in `pushes`, in order.
    fn hits_of(pushes: &[(Vec<i64>, &'static str, Vec<PyValue>)]) -> Vec<f64> {
        pushes
            .iter()
            .filter(|(to, n, _)| *n == "OnDamageMessage" && *to == vec![BOB])
            .filter_map(
                |(_, _, a)| match crate::pyutil::dict_get(&a[0], b"damageAttributes") {
                    Some(PyValue::Dict(rows)) => Some(
                        rows.iter()
                            .map(|(v, _)| if let PyValue::Real(r) = v { *r } else { 0.0 })
                            .sum(),
                    ),
                    _ => None,
                },
            )
            .collect()
    }

    fn attribute(pushes: &[(Vec<i64>, &'static str, Vec<PyValue>)], attr: i64) -> Vec<(f64, f64)> {
        pushes
            .iter()
            .filter(|(_, n, a)| {
                *n == "OnModuleAttributeChange"
                    && a[1] == PyValue::Int(GUN)
                    && a[2] == PyValue::Int(attr)
            })
            .filter_map(|(_, _, a)| match (&a[4], &a[5]) {
                (PyValue::Real(n), PyValue::Real(o)) => Some((*n, *o)),
                _ => None,
            })
            .collect()
    }

    /// Activate the gun on Bob and run `cycles` boundaries; returns what was
    /// pushed and the damage of each cycle's volley.
    fn shoot(reg: &mut Registry, t0: i64, cycles: i64) -> (Vec<f64>, Vec<Pushes>) {
        crate::modules::activate(
            reg,
            Some(ALICE),
            GUN,
            "disintegratorTest",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("activates");
        let mut hits = Vec::new();
        let mut batches = Vec::new();
        for k in 0..cycles {
            let now = t0 + k * PERIOD_MS * MSEC;
            crate::modules::tick(reg, now);
            crate::combat::tick(reg, now);
            let pushes = all(reg);
            hits.push(hit(&pushes).expect("a volley landed"));
            batches.push(pushes);
        }
        (hits, batches)
    }

    #[test]
    fn a_beam_module_ramps_through_the_shot_path() {
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let (hits, _) = shoot(&mut reg, t0, 3);
        assert!(hits[1] > hits[0], "{hits:?}");
        assert!(hits[2] > hits[1], "{hits:?}");
        // Each volley is the cycle's own multiplier over the first, as a fresh
        // ramp gives it (cycle 0 is unspooled).
        let mut replay = Spool::default();
        let mult: Vec<f64> = (0..3)
            .map(|_| replay.cycle(BOB_SHIP, PER_CYCLE, MAX))
            .collect();
        for k in 1..3 {
            assert!(
                (hits[k] / hits[0] - mult[k] / mult[0]).abs() < 1e-9,
                "{hits:?} {mult:?}"
            );
        }
    }

    #[test]
    fn stopping_resets_and_pushes_zero_current() {
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let (_, _) = shoot(&mut reg, t0, 3);
        assert!(reg.ext::<Spools>().current(GUN) > 0.0);
        let at = t0 + 2 * PERIOD_MS * MSEC;
        crate::modules::deactivate(&mut reg, Some(ALICE), GUN, "disintegratorTest", at + 1)
            .expect("deactivates");
        let end = t0 + 3 * PERIOD_MS * MSEC;
        crate::modules::tick(&mut reg, end);
        let pushes = all(&mut reg);
        let zeros: Vec<_> = attribute(&pushes, ATTR_CURRENT)
            .into_iter()
            .filter(|(n, _)| *n == 0.0)
            .collect();
        assert_eq!(zeros.len(), 1, "one reset to 0: {pushes:?}");
        assert!(zeros[0].1 > 0.0, "from a ramp above 0");
        // The sweep of stopped modules runs at the start of the next advance.
        crate::modules::tick(&mut reg, end + 1);
        assert_eq!(reg.ext::<Spools>().current(GUN), 0.0);
        assert_eq!(factor(&reg, GUN, 2), 1.0);
    }

    #[test]
    fn a_new_target_resets() {
        let mut s = Spools::default();
        assert_eq!(s.cycle(5, 7, 0, PER_CYCLE, MAX), (0.0, PER_CYCLE));
        assert_eq!(
            s.cycle(5, 7, 0, PER_CYCLE, MAX),
            (PER_CYCLE, 2.0 * PER_CYCLE)
        );
        assert_eq!(s.cycle(5, 8, 0, PER_CYCLE, MAX), (0.0, PER_CYCLE));
        assert_eq!(
            s.cycle(6, 8, 0, PER_CYCLE, MAX),
            (0.0, PER_CYCLE),
            "per module"
        );
    }

    #[test]
    fn the_max_timestamp_is_now_plus_the_remaining_cycles() {
        let p = PERIOD_MS * MSEC;
        // After the first cycle: 0.25 of 1.0, three more to go.
        assert_eq!(max_timestamp(1_000, p, 0.25, 0.25, 1.0), 1_000 + 3 * p);
        assert_eq!(max_timestamp(1_000, p, 0.5, 0.25, 1.0), 1_000 + 2 * p);
        // Not an exact multiple: it takes the next whole cycle.
        assert_eq!(max_timestamp(1_000, p, 0.25, 0.4, 1.0), 1_000 + 2 * p);
        // At max, or never climbing: now.
        assert_eq!(max_timestamp(1_000, p, 1.0, 0.25, 1.0), 1_000);
        assert_eq!(max_timestamp(1_000, p, 0.0, 0.0, 1.0), 1_000);
        // And the push says it, from the fitted pair.
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let (_, batches) = shoot(&mut reg, t0, 1);
        let ts = attribute(&batches[0], ATTR_MAX_TIMESTAMP);
        assert_eq!(ts.len(), 1, "{:?}", batches[0]);
        assert_eq!(ts[0].0, (t0 + 3 * p) as f64);
        let cur = attribute(&batches[0], ATTR_CURRENT);
        assert_eq!(cur, vec![(PER_CYCLE, 0.0)]);
    }

    #[test]
    fn the_start_fx_is_a_triglavian_fx_and_the_stop_is_plain() {
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let (_, batches) = shoot(&mut reg, t0, 1);
        let fx: Vec<_> = batches[0]
            .iter()
            .filter(|(_, n, _)| *n == "OnSpecialFX")
            .collect();
        assert_eq!(fx.len(), 1, "{fx:?}");
        let args = &fx[0].2;
        assert_eq!(
            args[5],
            PyValue::Bytes(BEAM_GUID.as_bytes().to_vec()),
            "first guid only"
        );
        assert_eq!(args[7], PyValue::Int(1), "start");
        let PyValue::Dict(info) = &args[13] else {
            panic!("graphicInfo is a dict: {:?}", args[13]);
        };
        let get = |k: &str| {
            info.iter()
                .find(|(_, key)| *key == crate::pyutil::bstr(k))
                .map(|(v, _)| v.clone())
        };
        assert_eq!(
            get("mulitplierBonusPerCycle"),
            Some(PyValue::Real(PER_CYCLE))
        );
        assert_eq!(get("multiplierBonusMax"), Some(PyValue::Real(MAX)));

        crate::modules::deactivate(&mut reg, Some(ALICE), GUN, "disintegratorTest", t0 + 1)
            .expect("deactivates");
        crate::modules::tick(&mut reg, t0 + 2 * PERIOD_MS * MSEC);
        let pushes = all(&mut reg);
        let stops: Vec<_> = pushes
            .iter()
            .filter(|(_, n, a)| *n == "OnSpecialFX" && a[7] == PyValue::Int(0))
            .collect();
        assert_eq!(stops.len(), 1, "{pushes:?}");
        assert_eq!(
            stops[0].2[13],
            PyValue::None,
            "a stop carries no graphicInfo"
        );
        assert_eq!(stops[0].2[5], PyValue::Bytes(BEAM_GUID.as_bytes().to_vec()));
    }

    #[test]
    fn a_sheet_bonus_reaches_the_pair() {
        let fx = fixture().replace(
            "\t0\t\t9=300.0\tTest Hull",
            "\t0\t990269\t9=300.0;990001=100.0\tTest Hull",
        ) + "attribute\t990001\t0.0\tprobeBonus\neffect\t990269\t0\t0\t0\t0\t0\t0\t0\tprobeHullBonus\t\nmodifier\t990269\tshipID\tLocationModifier\t2734\t990001\t6\t\t\n";
        let mut reg = world_from(fx);
        let t0 = crate::pyutil::filetime_now();
        let p = PERIOD_MS * MSEC;
        let (hits, batches) = shoot(&mut reg, t0, 6);
        let ts = attribute(&batches[0], ATTR_MAX_TIMESTAMP);
        assert_eq!(
            ts[0].0,
            (t0 + 7 * p) as f64,
            "the sheet's 2.0 max, not the type's 1.0"
        );
        assert!(
            hits[5] > hits[4],
            "still climbing past the type's max: {hits:?}"
        );
    }

    /// A stall puts cycles 1 and 2 in one `modules::tick`: each volley keeps the
    /// multiplier of its own cycle, as in a run with no stall.
    #[test]
    fn two_cycles_in_one_tick_each_keep_their_own_multiplier() {
        let (free, _) = shoot(&mut world(), crate::pyutil::filetime_now(), 3);
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let p = PERIOD_MS * MSEC;
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "disintegratorTest",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("activates");
        crate::modules::tick(&mut reg, t0);
        crate::combat::tick(&mut reg, t0);
        let h0 = hit(&all(&mut reg)).expect("cycle 0 landed");
        crate::modules::tick(&mut reg, t0 + 2 * p);
        crate::combat::tick(&mut reg, t0 + 2 * p);
        let hs = hits_of(&all(&mut reg));
        assert_eq!(hs.len(), 2, "{hs:?}");
        assert!((h0 - free[0]).abs() < 1e-6, "{h0} {free:?}");
        assert!((hs[0] - free[1]).abs() < 1e-6, "{hs:?} {free:?}");
        assert!((hs[1] - free[2]).abs() < 1e-6, "{hs:?} {free:?}");
    }

    /// A cycle recorded in the same tick that stops the module still gets its
    /// own multiplier (the sweep must not run before the volley is planned).
    #[test]
    fn a_cycle_recorded_in_the_tick_that_stops_the_module_keeps_its_multiplier() {
        let (free, _) = shoot(&mut world(), crate::pyutil::filetime_now(), 2);
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let p = PERIOD_MS * MSEC;
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "disintegratorTest",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("activates");
        crate::modules::tick(&mut reg, t0);
        crate::combat::tick(&mut reg, t0);
        let _ = all(&mut reg);
        crate::modules::deactivate(&mut reg, Some(ALICE), GUN, "disintegratorTest", t0 + p + 1)
            .expect("deactivates");
        crate::modules::tick(&mut reg, t0 + 3 * p);
        crate::combat::tick(&mut reg, t0 + 3 * p);
        let hs = hits_of(&all(&mut reg));
        assert!(!hs.is_empty(), "cycle 1 landed");
        assert!((hs[0] - free[1]).abs() < 1e-6, "{hs:?} {free:?}");
    }

    #[test]
    fn a_restart_starts_unspooled() {
        // A stop and a fresh activation (the reload path) starts unspooled.
        let mut reg = world();
        let t0 = crate::pyutil::filetime_now();
        let p = PERIOD_MS * MSEC;
        let (h, _) = shoot(&mut reg, t0, 3);
        crate::modules::deactivate(
            &mut reg,
            Some(ALICE),
            GUN,
            "disintegratorTest",
            t0 + 2 * p + 1,
        )
        .unwrap();
        crate::modules::tick(&mut reg, t0 + 3 * p);
        crate::combat::tick(&mut reg, t0 + 3 * p);
        let _ = all(&mut reg);
        let t1 = t0 + 20 * p;
        let (h2, _) = shoot(&mut reg, t1, 2);
        assert!((h2[0] - h[0]).abs() < 1e-6, "{h:?} {h2:?}");
        assert!((h2[1] - h[1]).abs() < 1e-6, "{h:?} {h2:?}");
    }

    /// The Mutadaptive repairer shares the beam's guid: its start carries a
    /// `graphicInfo` from 2796/2797, but it has no ramp and no 5804/5818.
    #[test]
    fn the_repair_beam_has_graphic_info_but_no_spool() {
        let fx = fixture()
            .replace(
                "attribute\t2733\t0.0\tdamageMultiplierBonusPerCycle\n",
                "attribute\t2733\t0.0\tdamageMultiplierBonusPerCycle\nattribute\t2796\t0.0\trepairMultiplierBonusPerCycle\nattribute\t2797\t0.0\trepairMultiplierBonusMax\n",
            )
            .replace(
                &format!("2733={PER_CYCLE};2734={MAX}"),
                "2733=0.75;2734=3.0;2796=0.125;2797=0.5",
            )
            .replace(
                "effect\t990268\t",
                &format!("effect\t{REPAIR_BEAM_EFFECT}\t"),
            )
            .replace("disintegratorTest", "repairBeamTest")
            .replace(
                "\t990268\t12,16,42,990268\t",
                &format!("\t{REPAIR_BEAM_EFFECT}\t12,16,42,{REPAIR_BEAM_EFFECT}\t"),
            );
        let mut reg = world_from(fx);
        let t0 = crate::pyutil::filetime_now();
        crate::modules::activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "repairBeamTest",
            Some(BOB_SHIP),
            1000,
            t0,
        )
        .expect("activates");
        crate::modules::tick(&mut reg, t0 + PERIOD_MS * MSEC);
        let pushes = all(&mut reg);
        let start = pushes
            .iter()
            .find(|(_, n, a)| *n == "OnSpecialFX" && a[7] == PyValue::Int(1))
            .expect("a start FX");
        assert_eq!(start.2[5], PyValue::Bytes(BEAM_GUID.as_bytes().to_vec()));
        let PyValue::Dict(info) = &start.2[13] else {
            panic!("graphicInfo is a dict: {:?}", start.2[13]);
        };
        let get = |k: &str| {
            info.iter()
                .find(|(_, key)| *key == crate::pyutil::bstr(k))
                .map(|(v, _)| v.clone())
        };
        assert_eq!(get("mulitplierBonusPerCycle"), Some(PyValue::Real(0.125)));
        assert_eq!(get("multiplierBonusMax"), Some(PyValue::Real(0.5)));
        assert!(attribute(&pushes, ATTR_CURRENT).is_empty(), "{pushes:?}");
        assert!(attribute(&pushes, ATTR_MAX_TIMESTAMP).is_empty());
        assert!(reg.ext::<Spools>().is_empty());
    }
}
