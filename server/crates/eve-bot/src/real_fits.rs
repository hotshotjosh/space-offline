//! **What players fit** (killmail fits): the table `tools/killfits.py`
//! writes to `out/bots/fits.tsv` from real killmails, read so a bot can seed
//! its fit from a fit people fly instead of the greedy one alone.
//!
//! The table is optional: absent, nothing changes ([`load`] gives `None`).
//! It is CCP game data and stays under `out/`; tests use invented rows.
//!
//! Format, tab-separated, `#` starts a comment line: `hull_type_id fit_hash
//! count share_of_hull hi med lo rig sub charges drones hull_name`, where a
//! slot column is comma-separated type ids (a module repeated per copy) and
//! `charges` is `weapon:charge` pairs and `drones` is `type x count` pairs
//! (`300x2,301x1`), the drones the pilot carried. The `hull_name` column
//! is not read; a table of ten columns (no `drones`) still loads.
//!
//! **Nothing from nowhere**: [`RealFits::usable`] keeps a fit only when the
//! hull, every module and every charge pass the caller's `has` (the
//! catalog's `Makeable`), so a fit with a part no builder makes is never
//! offered; a drone no builder makes is left out of the flight when the fit
//! is built (`doctrine_real`), never flown; a rig no builder makes does not
//! drop the fit either ([`RealFits::usable_trimmable`]): the builder swaps a
//! makeable rig of the same group or leaves it out; the caller still gates the fit on the paper evaluation
//! (`doctrine_fit::profile_of`, `Profile::ok`).

use std::collections::BTreeMap;
use std::path::Path;

/// One fit of one hull, as counted.
#[derive(Debug, Clone, PartialEq)]
pub struct RealFit {
    pub hull: i64,
    pub hash: String,
    /// Killmails with exactly this fit.
    pub count: u32,
    /// `count` over every killmail of the hull.
    pub share: f64,
    /// `(module type, copies)`, every rack together, ascending by type.
    pub modules: Vec<(i64, u8)>,
    /// `(weapon type, charge type)`: the charge most often loaded in it.
    pub charges: Vec<(i64, i64)>,
    /// `(drone type, count)` carried, ascending by type; empty for a fit
    /// with no bay or a table without the column.
    pub drones: Vec<(i64, u8)>,
}

impl RealFit {
    /// The charge of the first weapon `is_gun` accepts.
    pub fn charge_for(&self, is_gun: impl Fn(i64) -> bool) -> Option<i64> {
        self.charges.iter().find(|(w, _)| is_gun(*w)).map(|c| c.1)
    }
}

/// Every fit, by hull, most common first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RealFits {
    pub by_hull: BTreeMap<i64, Vec<RealFit>>,
}

/// The `drones` column: `type x count` pairs, comma-separated. A malformed
/// pair is skipped, not an error: the drones are a hint, and one bad pair
/// must not fail the whole table.
fn drone_pairs(col: &str) -> Vec<(i64, u8)> {
    let mut out: BTreeMap<i64, u8> = BTreeMap::new();
    for pair in col.split(',').filter(|s| !s.is_empty()) {
        let Some((Ok(t), Ok(n))) = pair
            .split_once('x')
            .map(|(t, n)| (t.parse::<i64>(), n.parse::<u32>()))
        else {
            continue;
        };
        let e = out.entry(t).or_insert(0);
        *e = u8::try_from(u32::from(*e) + n).unwrap_or(u8::MAX);
    }
    out.into_iter().filter(|d| d.1 > 0).collect()
}

fn ids(col: &str) -> Result<Vec<i64>, String> {
    col.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<i64>().map_err(|e| format!("type id {s:?}: {e}")))
        .collect()
}

/// Read the table's text. A malformed line is an error naming it.
pub fn parse(text: &str) -> Result<RealFits, String> {
    let mut out = RealFits::default();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let row = (|| -> Result<RealFit, String> {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 10 {
                return Err(format!("{} columns, want at least 10", c.len()));
            }
            let num = |i: usize| c[i].parse::<f64>().map_err(|e| format!("column {i}: {e}"));
            let mut modules: BTreeMap<i64, u8> = BTreeMap::new();
            for col in &c[4..9] {
                for t in ids(col)? {
                    let e = modules.entry(t).or_insert(0);
                    *e = e.saturating_add(1);
                }
            }
            let mut charges = Vec::new();
            for pair in c[9].split(',').filter(|s| !s.is_empty()) {
                let (w, ch) = pair
                    .split_once(':')
                    .ok_or_else(|| format!("charge pair {pair:?}"))?;
                let w = w.parse::<i64>().map_err(|e| format!("weapon {w:?}: {e}"))?;
                let ch = ch
                    .parse::<i64>()
                    .map_err(|e| format!("charge {ch:?}: {e}"))?;
                charges.push((w, ch));
            }
            let drones = c.get(10).map_or_else(Vec::new, |col| drone_pairs(col));
            Ok(RealFit {
                hull: num(0)? as i64,
                hash: c[1].to_owned(),
                count: num(2)? as u32,
                share: num(3)?,
                modules: modules.into_iter().collect(),
                charges,
                drones,
            })
        })()
        .map_err(|e| format!("line {}: {e}", n + 1))?;
        out.by_hull.entry(row.hull).or_default().push(row);
    }
    for fits in out.by_hull.values_mut() {
        fits.sort_by(|a, b| b.count.cmp(&a.count).then(a.hash.cmp(&b.hash)));
    }
    Ok(out)
}

/// The table at `path`; `Ok(None)` when there is no file (today's
/// behaviour), an error when it is there and unreadable.
pub fn load(path: &Path) -> Result<Option<RealFits>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl RealFits {
    /// The hull's fits that carry at least one module or drone and whose
    /// hull, modules and charges pass `has`, most common first. An
    /// unmakeable drone does not drop the row: the builder leaves it out of
    /// the flight.
    pub fn usable<'a>(
        &'a self,
        hull: i64,
        has: &'a dyn Fn(i64) -> bool,
    ) -> impl Iterator<Item = &'a RealFit> + 'a {
        self.by_hull
            .get(&hull)
            .into_iter()
            .flatten()
            .filter(move |f| {
                has(f.hull)
                    && !(f.modules.is_empty() && f.drones.is_empty())
                    && f.modules.iter().all(|m| has(m.0))
                    && f.charges.iter().all(|c| has(c.1))
            })
    }

    /// [`RealFits::usable`], but a fit whose only parts that fail `has` are
    /// rigs (`is_rig`) is kept, with those rigs listed beside it as
    /// `(type, copies)`: the builder drops them or swaps a makeable rig of
    /// the same group in (`doctrine_real`). An unmakeable gun, module or
    /// charge, or hull, still drops the fit, and the list is empty for a fit
    /// that [`RealFits::usable`] keeps.
    pub fn usable_trimmable<'a>(
        &'a self,
        hull: i64,
        has: &'a dyn Fn(i64) -> bool,
        is_rig: &'a dyn Fn(i64) -> bool,
    ) -> impl Iterator<Item = (&'a RealFit, Vec<(i64, u8)>)> + 'a {
        self.by_hull
            .get(&hull)
            .into_iter()
            .flatten()
            .filter_map(move |f| {
                if !has(f.hull)
                    || (f.modules.is_empty() && f.drones.is_empty())
                    || !f.charges.iter().all(|c| has(c.1))
                {
                    return None;
                }
                let missing: Vec<(i64, u8)> =
                    f.modules.iter().copied().filter(|m| !has(m.0)).collect();
                if !missing.iter().all(|m| is_rig(m.0)) {
                    return None;
                }
                Some((f, missing))
            })
    }

    /// One usable fit drawn by count: `roll` in `[0, 1)` walks the
    /// cumulative counts, so a fit twice as common is drawn twice as often.
    /// `None` when the hull has no usable fit.
    pub fn pick<'a>(
        &'a self,
        hull: i64,
        has: &'a dyn Fn(i64) -> bool,
        roll: f64,
    ) -> Option<&'a RealFit> {
        let fits: Vec<&RealFit> = self.usable(hull, has).collect();
        let total: u64 = fits.iter().map(|f| u64::from(f.count)).sum();
        if total == 0 {
            return None;
        }
        let mut at = roll.clamp(0.0, 1.0) * total as f64;
        for f in &fits {
            at -= f64::from(f.count);
            if at < 0.0 {
                return Some(f);
            }
        }
        fits.last().copied()
    }
}

// ---- the fitting check -------------------------------------------------

// **Would the server take this fit?** The server's rules, asked of a
// [`RealFit`]: `eve_server::fitting::plan_fit_in` for the rack modules (slots,
// hardpoints, the hull's allowed groups, module size, `maxGroupFitted`,
// `maxTypeFitted`), `eve_server::rigs::plan_fit` for rigs (kind, hull groups,
// rig slots, rig size, calibration, and, after #177, `maxGroupFitted` /
// `maxTypeFitted` over the other rigs; it asks no module size), `eve_server::subsystems` for T3 subsystems (one per slot flag,
// the hull's racks are the base plus the subsystems' modifiers).
//
// **CPU and power are not refusals**: the server fits a module whatever the
// grid, and the budget applies only when a module goes online
// (`modules.rs`), where the module just stays offline. They are reported
// apart ([`Reason::OfflineCpu`], [`Reason::OfflinePower`]) from the paper
// engine at the doctrine skills.
//
// Skills are not a reason: the doctrine fits at `DOCTRINE_LEVEL` in every
// skill the parts need and every fit-budget and rigging skill
// (`doctrine_fit::skills_for`) **[A]**; falsified by a flown killmail fit
// that the engine shows over budget at level V.
//
// The check is generic over [`Rules`] so a unit test drives it with
// invented numbers; `src/bin/eve-fitcheck.rs` supplies the content's.
pub use eve_server::fitting::{Hardpoint, HullFit, Rack};

/// The rig rack has at most this many slots (`rigs::LAST_RIG_FLAG`).
const MAX_RIG_SLOTS: i64 = 3;

/// What kind of part a type is to the fitting rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PartKind {
    /// Fitted in a rack of the three.
    Rack(Rack),
    /// A rig (`rigs::rig_in`).
    Rig,
    /// A T3 subsystem.
    Subsystem,
    /// Not a module the server can fit (no slot effect it knows).
    Unfittable,
}

/// One module type, as the fitting rules read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleInfo {
    pub kind: PartKind,
    pub hardpoint: Option<Hardpoint>,
    pub group_id: i64,
    pub volume: f64,
    /// `maxGroupFitted`, `maxTypeFitted`.
    pub max_group: Option<i64>,
    pub max_type: Option<i64>,
    /// A rig's `rigSize` and `upgradeCost`.
    pub rig_size: i64,
    pub upgrade_cost: f64,
    /// A subsystem's `subSystemSlot` flag (a visible one), and the one hull
    /// type it is for (`fitsToShipType`).
    pub subsystem_slot: Option<i64>,
    pub fits_ship: Option<i64>,
    /// A subsystem's `[hi, med, low, turret, launcher]` modifiers.
    pub subsystem_mods: [i64; 5],
}

impl ModuleInfo {
    /// A plain rack module (or any part) with no rig or subsystem numbers.
    pub fn plain(kind: PartKind, group_id: i64, volume: f64) -> ModuleInfo {
        ModuleInfo {
            kind,
            hardpoint: None,
            group_id,
            volume,
            max_group: None,
            max_type: None,
            rig_size: 0,
            upgrade_cost: 0.0,
            subsystem_slot: None,
            fits_ship: None,
            subsystem_mods: [0; 5],
        }
    }
}

/// One hull, as the fitting rules read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullInfo {
    /// The hull's own row; a modular hull's has no slots.
    pub fit: HullFit,
    pub group_id: i64,
    pub category_id: i64,
    /// `rigSize` and `upgradeCapacity` (`rigs::hull_rigs_in`).
    pub rig_size: i64,
    pub calibration: f64,
    /// Whether the slots come from subsystems (`subsystems::is_modular_group`).
    pub modular: bool,
}

/// The world's numbers the check asks.
pub trait Rules {
    fn hull(&self, type_id: i64) -> Option<HullInfo>;
    fn module(&self, type_id: i64) -> Option<ModuleInfo>;
    /// `eve_server::fitting::hull_allows` for this module on this hull.
    fn allows(&self, module: i64, hull_group: i64, hull_type: i64) -> bool;
    /// `(cpu_ok, power_ok)` on the paper engine at the doctrine skills;
    /// `None` when the engine does not evaluate the fit.
    fn power(&self, fit: &RealFit) -> Option<(bool, bool)>;
}

/// Why a fit is refused, or what else the check found. A fit can have
/// several. [`Reason::refuses`] tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    UnknownHull,
    UnknownModule,
    /// Rig slots: the hull has none, or fewer than the fit's rigs.
    RigSlots,
    /// A rig whose `rigSize` is not the hull's.
    RigSize,
    /// The rigs' `upgradeCost` adds up over the hull's `upgradeCapacity`.
    Calibration,
    HighSlots,
    MedSlots,
    LowSlots,
    Turrets,
    Launchers,
    /// The module names hull groups or types that leave this hull out.
    HullRestriction,
    /// Over the non-capital module volume (not asked of a rig).
    TooBig,
    MaxGroupFitted,
    MaxTypeFitted,
    /// A subsystem for another hull or a non-modular one, with no slot, or
    /// two in one slot.
    SubsystemSlot,
    /// Not a refusal: a module would stay offline for CPU.
    OfflineCpu,
    /// Not a refusal: a module would stay offline for power.
    OfflinePower,
    /// Not a refusal: the paper engine does not evaluate the fit.
    NoEngine,
}

impl Reason {
    /// A stable word for the tool's output and the summary.
    pub fn word(self) -> &'static str {
        match self {
            Reason::UnknownHull => "unknown_hull",
            Reason::UnknownModule => "unfittable_module",
            Reason::RigSlots => "rig_slots",
            Reason::RigSize => "rig_size",
            Reason::Calibration => "calibration",
            Reason::HighSlots => "high_slots",
            Reason::MedSlots => "med_slots",
            Reason::LowSlots => "low_slots",
            Reason::Turrets => "turret_hardpoints",
            Reason::Launchers => "launcher_hardpoints",
            Reason::HullRestriction => "hull_restriction",
            Reason::TooBig => "module_too_big",
            Reason::MaxGroupFitted => "max_group_fitted",
            Reason::MaxTypeFitted => "max_type_fitted",
            Reason::SubsystemSlot => "subsystem_slot",
            Reason::OfflineCpu => "offline_cpu",
            Reason::OfflinePower => "offline_power",
            Reason::NoEngine => "no_engine",
        }
    }

    /// Whether the server would refuse the fit for this reason.
    pub fn refuses(self) -> bool {
        !matches!(
            self,
            Reason::OfflineCpu | Reason::OfflinePower | Reason::NoEngine
        )
    }
}

/// Every reason `rules` would refuse `fit`, and the findings that are not
/// refusals, sorted and distinct; empty when the fit would be taken and
/// would come online. A subsystem hull's racks are its base plus each
/// distinct slot's subsystem modifiers (`subsystems::hull_fit`).
pub fn check(rules: &dyn Rules, fit: &RealFit) -> Vec<Reason> {
    let mut out = std::collections::BTreeSet::new();
    let Some(hull) = rules.hull(fit.hull) else {
        return vec![Reason::UnknownHull];
    };
    let mut racks = [0i64; 3];
    let (mut rigs, mut turrets, mut launchers) = (0i64, 0i64, 0i64);
    let mut calibration = 0.0;
    let mut slots: BTreeMap<i64, u8> = BTreeMap::new();
    let mut have = hull.fit;
    let mut by_group: BTreeMap<i64, i64> = BTreeMap::new();
    let mut limited: Vec<(i64, i64)> = Vec::new();
    let mut rig_groups: BTreeMap<i64, i64> = BTreeMap::new();
    let mut rig_limited: Vec<(i64, i64)> = Vec::new();
    for &(t, n) in &fit.modules {
        let copies = i64::from(n);
        let Some(m) = rules.module(t) else {
            out.insert(Reason::UnknownModule);
            continue;
        };
        // Only the rack and rig paths ask the hull's allowed groups; the
        // server's subsystem path does not.
        if matches!(m.kind, PartKind::Rack(_) | PartKind::Rig)
            && !rules.allows(t, hull.group_id, fit.hull)
        {
            out.insert(Reason::HullRestriction);
        }
        match m.kind {
            PartKind::Rack(Rack::High) => racks[0] += copies,
            PartKind::Rack(Rack::Medium) => racks[1] += copies,
            PartKind::Rack(Rack::Low) => racks[2] += copies,
            PartKind::Rig => {
                // `rigs::plan_fit` asks no module size; its max counts run
                // over the other rigs only (#177).
                rigs += copies;
                calibration += m.upgrade_cost * copies as f64;
                if m.rig_size != hull.rig_size {
                    out.insert(Reason::RigSize);
                }
                *rig_groups.entry(m.group_id).or_insert(0) += copies;
                if let Some(l) = m.max_group {
                    rig_limited.push((m.group_id, l));
                }
                if m.max_type.is_some_and(|l| copies > l) {
                    out.insert(Reason::MaxTypeFitted);
                }
                continue;
            }
            PartKind::Subsystem => {
                let ok_ship = hull.modular && m.fits_ship == Some(fit.hull);
                match m.subsystem_slot {
                    Some(s) if ok_ship => {
                        let e = slots.entry(s).or_insert(0);
                        *e = e.saturating_add(n);
                    }
                    _ => {
                        out.insert(Reason::SubsystemSlot);
                    }
                }
                continue;
            }
            PartKind::Unfittable => {
                out.insert(Reason::UnknownModule);
                continue;
            }
        }
        match m.hardpoint {
            Some(Hardpoint::Turret) => turrets += copies,
            Some(Hardpoint::Launcher) => launchers += copies,
            None => {}
        }
        if eve_server::fitting::module_too_big(m.volume, hull.group_id, hull.category_id) {
            out.insert(Reason::TooBig);
        }
        *by_group.entry(m.group_id).or_insert(0) += copies;
        if let Some(l) = m.max_group {
            limited.push((m.group_id, l));
        }
        if m.max_type.is_some_and(|l| copies > l) {
            out.insert(Reason::MaxTypeFitted);
        }
    }
    // Stricter than the server where limits differ inside one group: the
    // server's answer there depends on the order the modules are fitted in;
    // here any module whose own limit the group's count passes counts.
    if limited.iter().any(|&(g, l)| by_group[&g] > l)
        || rig_limited.iter().any(|&(g, l)| rig_groups[&g] > l)
    {
        out.insert(Reason::MaxGroupFitted);
    }
    // The subsystems' modifiers, one subsystem per slot (a second in one
    // slot is refused); each is read once however it was listed.
    for (&flag, &n) in &slots {
        if n > 1 {
            out.insert(Reason::SubsystemSlot);
        }
        for &(t, _) in &fit.modules {
            if let Some(m) = rules.module(t)
                && m.kind == PartKind::Subsystem
                && m.subsystem_slot == Some(flag)
            {
                have.hi += m.subsystem_mods[0];
                have.med += m.subsystem_mods[1];
                have.low += m.subsystem_mods[2];
                have.turrets += m.subsystem_mods[3];
                have.launchers += m.subsystem_mods[4];
                break;
            }
        }
    }
    for (used, avail, why) in [
        (racks[0], have.hi, Reason::HighSlots),
        (racks[1], have.med, Reason::MedSlots),
        (racks[2], have.low, Reason::LowSlots),
        (turrets, have.turrets, Reason::Turrets),
        (launchers, have.launchers, Reason::Launchers),
        (rigs, have.rig.min(MAX_RIG_SLOTS), Reason::RigSlots),
    ] {
        if used > avail {
            out.insert(why);
        }
    }
    if calibration > hull.calibration {
        out.insert(Reason::Calibration);
    }
    match rules.power(fit) {
        Some((cpu, pg)) => {
            if !cpu {
                out.insert(Reason::OfflineCpu);
            }
            if !pg {
                out.insert(Reason::OfflinePower);
            }
        }
        None => {
            out.insert(Reason::NoEngine);
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: hull 9001, modules 100-104, charges 200-201.
    const TABLE: &str = "#hull_type_id\tfit_hash\tcount\tshare_of_hull\thi\tmed\tlo\trig\tsub\tcharges\tdrones\thull_name\n\
9001\taaa\t30\t0.3000\t100,100,100\t101\t102,102\t103\t\t100:200\t\tTest Hull\n\
9001\tbbb\t10\t0.1000\t100,100,100\t101\t104,102\t\t\t100:201\t\tTest Hull\n\
9001\tccc\t60\t0.6000\t\t\t\t\t\t\t\tTest Hull\n";

    #[test]
    fn parse_merges_racks_and_sorts_by_count() {
        let t = parse(TABLE).unwrap();
        let fits = &t.by_hull[&9001];
        assert_eq!(
            fits.iter().map(|f| f.hash.as_str()).collect::<Vec<_>>(),
            ["ccc", "aaa", "bbb"]
        );
        assert_eq!(
            fits[1].modules,
            vec![(100, 3), (101, 1), (102, 2), (103, 1)]
        );
        assert_eq!(fits[1].charges, vec![(100, 200)]);
        assert_eq!(fits[1].charge_for(|w| w == 100), Some(200));
        assert_eq!(fits[1].charge_for(|w| w == 101), None);
    }

    #[test]
    fn usable_trimmable_keeps_a_fit_missing_only_rigs() {
        let t = parse(TABLE).unwrap();
        let is_rig = |x: i64| x == 103;
        let names = |has: &dyn Fn(i64) -> bool| -> Vec<(String, Vec<(i64, u8)>)> {
            t.usable_trimmable(9001, has, &is_rig)
                .map(|(f, m)| (f.hash.clone(), m))
                .collect()
        };
        // Rig 103 is unmakeable: the strict filter loses the fit, this keeps
        // it and names the rig.
        let no_rig = |x: i64| x != 103;
        let strict: Vec<&str> = t.usable(9001, &no_rig).map(|f| f.hash.as_str()).collect();
        assert_eq!(strict, ["bbb"]);
        assert_eq!(
            names(&no_rig),
            [
                ("aaa".to_owned(), vec![(103, 1)]),
                ("bbb".to_owned(), vec![])
            ]
        );
        // A gun, a module or a charge that is unmakeable still drops it.
        assert_eq!(names(&|x| x != 100).len(), 0);
        assert_eq!(names(&|x| x != 102).len(), 0);
        assert_eq!(names(&|x| x != 200).len(), 1);
        // Nothing missing: both fits, no rigs listed.
        assert!(names(&|_| true).iter().all(|(_, m)| m.is_empty()));
    }

    #[test]
    fn usable_drops_empty_fits_and_unmakeable_parts() {
        let t = parse(TABLE).unwrap();
        let all = |_: i64| true;
        let got: Vec<&str> = t.usable(9001, &all).map(|f| f.hash.as_str()).collect();
        assert_eq!(got, ["aaa", "bbb"]);
        let no_104 = |x: i64| x != 104;
        let got: Vec<&str> = t.usable(9001, &no_104).map(|f| f.hash.as_str()).collect();
        assert_eq!(got, ["aaa"]);
        let no_charge = |x: i64| x != 200;
        let got: Vec<&str> = t
            .usable(9001, &no_charge)
            .map(|f| f.hash.as_str())
            .collect();
        assert_eq!(got, ["bbb"]);
        let no_hull = |x: i64| x != 9001;
        assert_eq!(t.usable(9001, &no_hull).count(), 0);
        assert_eq!(t.usable(9002, &all).count(), 0);
    }

    #[test]
    fn pick_draws_by_count() {
        let t = parse(TABLE).unwrap();
        let all = |_: i64| true;
        // aaa 30 and bbb 10 of 40: rolls below 0.75 draw aaa.
        assert_eq!(t.pick(9001, &all, 0.0).unwrap().hash, "aaa");
        assert_eq!(t.pick(9001, &all, 0.74).unwrap().hash, "aaa");
        assert_eq!(t.pick(9001, &all, 0.76).unwrap().hash, "bbb");
        assert_eq!(t.pick(9001, &all, 1.0).unwrap().hash, "bbb");
        assert!(t.pick(9002, &all, 0.5).is_none());
    }

    #[test]
    fn the_drones_column_is_read() {
        let t = parse(
            "9001\td1\t5\t0.5\t\t\t\t\t\t\t300x2,301x1,300x1\tTest Hull\n\
9001\td2\t4\t0.4\t100\t\t\t\t\t\t\tTest Hull\n\
9001\td3\t3\t0.3\t100\t\t\t\t\t\n",
        )
        .unwrap();
        let f = &t.by_hull[&9001];
        assert_eq!(f[0].drones, vec![(300, 3), (301, 1)]);
        assert!(f[1].drones.is_empty() && f[2].drones.is_empty());
        // A drone-only fit is usable; an unmakeable drone does not drop it.
        let all = |_: i64| true;
        assert_eq!(t.usable(9001, &all).count(), 3);
        let no_300 = |x: i64| x != 300;
        assert_eq!(t.usable(9001, &no_300).count(), 3);
        // A malformed pair is skipped; the good ones and the table load.
        let t = parse("9001\tx\t1\t0.1\t100\t\t\t\t\t\t300,y x2,302x1,303xq\tH\n").unwrap();
        assert_eq!(t.by_hull[&9001][0].drones, vec![(302, 1)]);
    }

    #[test]
    fn a_bad_line_is_named_and_a_missing_file_is_none() {
        let e = parse("9001\tx\t1\n").unwrap_err();
        assert!(e.starts_with("line 1:"), "{e}");
        let e = parse("9001\tx\t1\t0.1\tq\t\t\t\t\t\n").unwrap_err();
        assert!(e.contains("type id"), "{e}");
        assert_eq!(load(Path::new("/nonexistent/probe/fits.tsv")), Ok(None));
    }

    // ---- the fitting check, over invented numbers ----

    /// Invented world. Hull 9001: group 50, 2 high 1 med 1 low 2 rig, 1
    /// turret, rig size 2, calibration 400. Hull 9100 is modular: no slots.
    /// Modules: 100 turret (high), 101 med (max one of the type), 102 and
    /// 104 low (group 7, at most one per fit; 104 only for hull 9002), 105
    /// big high module, 106 unfittable, 110 and 111 rigs (cost 250, size 2
    /// and 3), 120 a rig that also names a group limit and a volume a
    /// module would be refused for, 200-201 subsystems for hull 9100 in
    /// slots 125 and 126.
    struct Fake {
        /// `power` answer: `None` for no engine.
        engine: Option<(bool, bool)>,
    }

    impl Rules for Fake {
        fn hull(&self, t: i64) -> Option<HullInfo> {
            // 9200 has four rig slots and calibration to spare: the server
            // fits at most three (`rigs::LAST_RIG_FLAG`).
            let (modular, hi, med, low, rig, calibration) = match t {
                9001 => (false, 2, 1, 1, 2, 400.0),
                9100 => (true, 0, 0, 0, 2, 400.0),
                9200 => (false, 2, 1, 1, 4, 2000.0),
                _ => return None,
            };
            Some(HullInfo {
                fit: HullFit {
                    type_id: t,
                    hi,
                    med,
                    low,
                    rig,
                    turrets: i64::from(!modular),
                    launchers: i64::from(!modular),
                    cpu_output: 100.0,
                    power_output: 100.0,
                },
                group_id: 50,
                category_id: 6,
                rig_size: 2,
                calibration,
                modular,
            })
        }
        fn module(&self, t: i64) -> Option<ModuleInfo> {
            let p = ModuleInfo::plain;
            Some(match t {
                100 => ModuleInfo {
                    hardpoint: Some(Hardpoint::Turret),
                    ..p(PartKind::Rack(Rack::High), 5, 5.0)
                },
                101 => ModuleInfo {
                    max_type: Some(1),
                    ..p(PartKind::Rack(Rack::Medium), 6, 5.0)
                },
                102 | 104 => ModuleInfo {
                    max_group: Some(1),
                    ..p(PartKind::Rack(Rack::Low), 7, 5.0)
                },
                105 => p(PartKind::Rack(Rack::High), 9, 9999.0),
                107 => ModuleInfo {
                    hardpoint: Some(Hardpoint::Launcher),
                    ..p(PartKind::Rack(Rack::High), 12, 5.0)
                },
                106 => p(PartKind::Unfittable, 10, 5.0),
                110 | 111 => ModuleInfo {
                    rig_size: if t == 110 { 2 } else { 3 },
                    upgrade_cost: 250.0,
                    ..p(PartKind::Rig, 8, 5.0)
                },
                // 121 and 122: two types of one capped rig group.
                121 | 122 => ModuleInfo {
                    max_group: (t == 121).then_some(1),
                    rig_size: 2,
                    ..p(PartKind::Rig, 13, 5.0)
                },
                // 130: a rig the hull may not take.
                130 => ModuleInfo {
                    rig_size: 2,
                    ..p(PartKind::Rig, 14, 5.0)
                },
                120 => ModuleInfo {
                    max_type: Some(1),
                    max_group: Some(1),
                    rig_size: 2,
                    ..p(PartKind::Rig, 8, 9999.0)
                },
                200 | 201 => ModuleInfo {
                    subsystem_slot: Some(if t == 200 { 125 } else { 126 }),
                    fits_ship: Some(9100),
                    subsystem_mods: [3, 2, 1, 2, 0],
                    ..p(PartKind::Subsystem, 11, 5.0)
                },
                _ => return None,
            })
        }
        fn allows(&self, module: i64, _g: i64, hull: i64) -> bool {
            // 104 is for hull 9002 only; 130 (a rig) and 200 (a subsystem) are
            // refused on every hull: the subsystem path must not ask.
            !matches!(module, 130 | 200) && (module != 104 || hull == 9002)
        }
        fn power(&self, _fit: &RealFit) -> Option<(bool, bool)> {
            self.engine
        }
    }

    fn fit_of(hull: i64, modules: &[(i64, u8)]) -> RealFit {
        RealFit {
            hull,
            hash: "x".into(),
            count: 1,
            share: 0.1,
            modules: modules.to_vec(),
            charges: vec![],
            drones: vec![],
        }
    }

    const FINE: Fake = Fake {
        engine: Some((true, true)),
    };

    #[test]
    fn check_names_each_refusal() {
        use Reason::*;
        let c = |h, m: &[(i64, u8)]| check(&FINE, &fit_of(h, m));
        assert_eq!(c(9001, &[(100, 1), (101, 1), (110, 1)]), vec![]);
        assert_eq!(c(9999, &[(100, 1)]), vec![UnknownHull]);
        assert_eq!(c(9001, &[(100, 3)]), vec![HighSlots, Turrets]);
        assert_eq!(c(9001, &[(101, 2)]), vec![MedSlots, MaxTypeFitted]);
        assert_eq!(c(9001, &[(104, 1)]), vec![HullRestriction]);
        assert_eq!(c(9001, &[(105, 1)]), vec![TooBig]);
        // One launcher hardpoint: a second launcher is refused.
        assert_eq!(c(9001, &[(107, 1)]), vec![]);
        assert_eq!(c(9001, &[(107, 2)]), vec![Launchers]);
        assert_eq!(c(9001, &[(106, 1)]), vec![UnknownModule]);
        assert_eq!(c(9001, &[(777, 1)]), vec![UnknownModule]);
        // 102 twice: one low slot, and the group holds one.
        assert_eq!(c(9001, &[(102, 2)]), vec![LowSlots, MaxGroupFitted]);
    }

    #[test]
    fn rigs_follow_the_servers_rig_rules() {
        use Reason::*;
        let c = |m: &[(i64, u8)]| check(&FINE, &fit_of(9001, m));
        // Two rigs of the hull's size, 500 of 400 calibration.
        assert_eq!(c(&[(110, 2)]), vec![Calibration]);
        assert_eq!(c(&[(110, 1)]), vec![]);
        // A large rig on a medium hull; three rigs on two slots.
        assert_eq!(c(&[(111, 1)]), vec![RigSize]);
        assert_eq!(c(&[(110, 3)]), vec![RigSlots, Calibration]);
        // A rig is asked no module size (120 is 9999 m3) but, over the other
        // rigs, its max counts (#177): two of 120 pass its type and group
        // limits, one does not.
        assert_eq!(c(&[(120, 1)]), vec![]);
        assert_eq!(c(&[(120, 2)]), vec![MaxGroupFitted, MaxTypeFitted]);
        // Two types of one capped group: the group only.
        assert_eq!(c(&[(121, 1), (122, 1)]), vec![MaxGroupFitted]);
        // The hull's allowed groups are asked of a rig.
        assert_eq!(c(&[(130, 1)]), vec![HullRestriction]);
        // The rig rack has three flags however many slots the row says.
        let big = |m: &[(i64, u8)]| check(&FINE, &fit_of(9200, m));
        assert_eq!(big(&[(110, 3)]), vec![]);
        assert_eq!(big(&[(110, 4)]), vec![RigSlots]);
    }

    #[test]
    fn a_subsystem_hull_has_the_subsystems_slots() {
        use Reason::*;
        let c = |m: &[(i64, u8)]| check(&FINE, &fit_of(9100, m));
        // Two subsystems give 6 high, 4 med, 2 low, 4 turrets.
        let subs = [(200, 1), (201, 1)];
        let mut ok = subs.to_vec();
        ok.extend([(100, 4), (101, 1)]);
        assert_eq!(c(&ok), vec![]);
        let mut over = subs.to_vec();
        over.push((100, 5));
        assert_eq!(c(&over), vec![Turrets]);
        // No subsystem: no slots at all.
        assert_eq!(c(&[(101, 1)]), vec![MedSlots]);
        // One subsystem is not enough for what two give.
        assert_eq!(c(&[(200, 1), (100, 4)]), vec![HighSlots, Turrets]);
        // Two in one slot, or on a hull that is not modular.
        assert_eq!(c(&[(200, 2)]), vec![SubsystemSlot]);
        assert_eq!(
            check(&FINE, &fit_of(9001, &[(200, 1)])),
            vec![SubsystemSlot]
        );
        // Rigs are counted on a modular hull too.
        assert_eq!(c(&[(110, 3)]), vec![RigSlots, Calibration]);
    }

    #[test]
    fn the_budget_is_reported_apart_from_the_refusals() {
        use Reason::*;
        let m = [(100, 1)];
        let over = Fake {
            engine: Some((false, false)),
        };
        let got = check(&over, &fit_of(9001, &m));
        assert_eq!(got, vec![OfflineCpu, OfflinePower]);
        assert!(got.iter().all(|r| !r.refuses()));
        let none = Fake { engine: None };
        let got = check(&none, &fit_of(9001, &m));
        assert_eq!(got, vec![NoEngine]);
        assert!(!NoEngine.refuses() && HighSlots.refuses());
    }
}
