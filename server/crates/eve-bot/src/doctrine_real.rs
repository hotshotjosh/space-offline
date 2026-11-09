//! **Real fits seed the doctrines** (killmail fits, items 4 and 5): the fits
//! players fly ([`crate::real_fits`]) built into [`RoleFit`]s on the paper
//! evaluation, so [`crate::doctrine_fit::catalog_in`] weighs them beside the
//! greedy fits and a bot can fly one for variety.
//!
//! **Nothing from nowhere**: a row is built only when the library's
//! `Makeable` has its hull, every module and every charge
//! ([`RealFits::usable`]); it is kept only when the engine passes it
//! ([`Profile::ok`](crate::doctrine::Profile::ok)). The seeds take one
//! relaxation: a fit whose only unmakeable parts are rigs is kept
//! ([`RealFits::usable_trimmable`]) with each such rig swapped for a makeable
//! one of its group and size, or dropped (`trim_rigs`); the engine still judges
//! the result, and a fit that lands on the shape of another seeded fit is skipped.
//!
//! **Two judges.** [`try_build`] / [`seeds`] keep the strict one: every module
//! must come online (CPU, power, capacitor). [`try_build_fitted`] /
//! [`seeds_fitted`] judge a seed as the server does ([`crate::real_fits::check`]):
//! a fit is refused only for what the server refuses (slots, hardpoints, hull
//! groups, rigs, counts, subsystems); a module that would stay offline for CPU
//! or power is **accepted** and reported ([`Seed::offline_cpu`],
//! [`Seed::offline_power`]), since the server fits it and the pilot onlines
//! what the grid carries. The capacitor stays a gate (a fit that cannot hold
//! the standard fight is no doctrine). The weapon requirement stays in both:
//! a doctrine's damage line is keyed by a [`Family`], and only a gun, a
//! launcher or a combat drone gives one; the other roles are built by the
//! greedy support fits, not seeded.

use crate::doctrine::{DOCTRINE_LEVEL, Family, Layer, RoleFit, layer_of};
use crate::doctrine_fit::{
    DRONES_ACTIVE, HullRow, Kind, Library, Part, ROUNDS_ALTERNATE, ROUNDS_PRIMARY, choose_rounds,
    profile_with_drones, skills_for,
};
use crate::hull_choice::Class;
use crate::real_fits::{RealFit, RealFits, Rules, check};
use std::collections::BTreeMap;

/// Real fits taken per DPS hull **[A]**; falsified by a soak where seeded
/// doctrines are never picked (raise it, or the fits are too weak).
pub const SEED_FITS_PER_HULL: usize = 5;

/// One built real fit.
#[derive(Debug, Clone)]
pub struct Seed {
    pub class: Class,
    pub fit: RoleFit,
    pub count: u32,
    pub hash: String,
}

impl Seed {
    /// Some module of the fit would stay offline for CPU on the paper engine
    /// (at the doctrine skills): the pilot onlines what the grid carries.
    pub fn offline_cpu(&self) -> bool {
        !self.fit.profile.cpu_ok
    }

    /// Some module would stay offline for power.
    pub fn offline_power(&self) -> bool {
        !self.fit.profile.pg_ok
    }
}

type Group = (RealFits, BTreeMap<String, Seed>);

/// The built fits, by `(hull, family, layer)`, with the table of each
/// group kept so [`RealFits::pick`] draws among them by count.
#[derive(Debug, Clone, Default)]
pub struct RealSeeds {
    groups: BTreeMap<(i64, Family, Layer), Group>,
}

impl RealSeeds {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Every seed with its family and layer.
    pub fn iter(&self) -> impl Iterator<Item = (Family, Layer, &Seed)> + '_ {
        self.groups
            .iter()
            .flat_map(|((_, f, l), (_, m))| m.values().map(move |s| (*f, *l, s)))
    }

    /// One fit of `hull` for a doctrine of `family` and `layer`, drawn by
    /// count with `roll` in `[0, 1)` ([`RealFits::pick`]).
    pub fn pick(&self, hull: i64, family: Family, layer: Layer, roll: f64) -> Option<&RoleFit> {
        let (fits, built) = self.groups.get(&(hull, family, layer))?;
        let f = fits.pick(hull, &|_| true, roll)?;
        built.get(&f.hash).map(|s| &s.fit)
    }

    /// `(seeds, with a module offline for CPU, with one offline for power)`.
    pub fn offline_counts(&self) -> (usize, usize, usize) {
        let count = |keep: &dyn Fn(&Seed) -> bool| self.iter().filter(|x| keep(x.2)).count();
        (
            count(&|_| true),
            count(&|s| s.offline_cpu()),
            count(&|s| s.offline_power()),
        )
    }
}

/// Why a real fit is not built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dropped {
    /// No gun, launcher or flyable combat drone: logistics, tackle, a bare
    /// hull (no family).
    NoWeapon,
    /// A weapon with no round in the row and none the greedy choice finds.
    NoRound,
    /// A module on a weapon hardpoint the library cannot read (a T2 or
    /// faction weapon above the meta limit, an unclassified family) beside
    /// drones: the drones are not the fit's damage.
    UnknownWeapon,
    /// The engine does not hold the fit.
    NoProfile,
    /// The fit fails CPU, power or capacitor (the strict judge), or the
    /// capacitor alone (the fit judge).
    Fails,
    /// The server would refuse the fit ([`crate::real_fits::check`]); the
    /// fit judge only.
    Refused,
}

impl Dropped {
    pub fn name(self) -> &'static str {
        match self {
            Dropped::NoWeapon => "no weapon or combat drone",
            Dropped::NoRound => "no round for the weapon",
            Dropped::UnknownWeapon => "a weapon the library cannot read",
            Dropped::NoProfile => "the engine does not hold it",
            Dropped::Fails => "fails cpu, power or capacitor",
            Dropped::Refused => "the server refuses the fit",
        }
    }
}

/// **The drones a real fit flies**: of the carried drones the library knows
/// as combat drones, the most damaging first, as many as the hull's
/// bandwidth and [`DRONES_ACTIVE`] allow, grouped by type. Empty for a hull
/// with no bay or bandwidth, or no combat drone among them.
pub(crate) fn flight(lib: &Library<'_>, hull: &HullRow, carried: &[(i64, u8)]) -> Vec<(i64, u8)> {
    if hull.drone_bay <= 0.0 || hull.bandwidth <= 0.0 {
        return Vec::new();
    }
    let mut units: Vec<&Part> = carried
        .iter()
        .filter_map(|(t, n)| {
            let p = lib
                .parts
                .iter()
                .find(|p| p.type_id == *t && p.kind == Kind::Drone)?;
            Some(std::iter::repeat_n(p, usize::from(*n)))
        })
        .flatten()
        .collect();
    units.sort_by(|a, b| {
        b.magnitude
            .total_cmp(&a.magnitude)
            .then(a.type_id.cmp(&b.type_id))
    });
    let (mut bandwidth, mut volume) = (0.0, 0.0);
    let mut out: BTreeMap<i64, u8> = BTreeMap::new();
    let mut flown = 0;
    for p in units {
        if flown == DRONES_ACTIVE {
            break;
        }
        if bandwidth + p.bandwidth > hull.bandwidth || volume + p.volume > hull.drone_bay {
            continue;
        }
        bandwidth += p.bandwidth;
        volume += p.volume;
        flown += 1;
        *out.entry(p.type_id).or_insert(0) += 1;
    }
    out.into_iter().collect()
}

/// **One real fit on the paper**: modules from the row, the row's charge in
/// the guns (the greedy choice when it names none) and the best alternate,
/// the drones it flies, the doctrine's skills, the engine's numbers. A gun
/// or launcher gives its family (rockets and the T2
/// launchers included, see `Library::kind_of`); a fit with only drones is
/// [`Family::Drone`] and is judged on the drone profile ([`profile_with_drones`]). `Err` names why
/// not: no weapon, no round, the engine does not hold it, or it fails CPU,
/// power or capacitor.
pub fn try_build(lib: &Library<'_>, real: &RealFit) -> Result<(Family, RoleFit), Dropped> {
    build_judged(lib, real, None)
}

/// [`try_build`] judged by the fit, not by every module coming online: the
/// server's rules (`rules`, [`crate::real_fits::check`]) refuse
/// ([`Dropped::Refused`]) only what the server refuses, a CPU or power
/// shortfall is accepted (see [`Seed::offline_cpu`]), the capacitor still
/// gates ([`Dropped::Fails`]). The refusal is asked first, so a fit the
/// server would not take never reaches the engine. **[A]** the paper numbers
/// of a fit with offline modules count them online (overstating its damage);
/// falsified by a flown fit whose real damage is far below the paper's.
pub fn try_build_fitted(
    lib: &Library<'_>,
    real: &RealFit,
    rules: &dyn Rules,
) -> Result<(Family, RoleFit), Dropped> {
    build_judged(lib, real, Some(rules))
}

fn build_judged(
    lib: &Library<'_>,
    real: &RealFit,
    fit_rules: Option<&dyn Rules>,
) -> Result<(Family, RoleFit), Dropped> {
    if let Some(rules) = fit_rules
        && check(rules, real).iter().any(|r| r.refuses())
    {
        return Err(Dropped::Refused);
    }
    let is_gun = |w: i64| matches!(lib.kind_of(w), Some(Kind::Gun(..)));
    // The main weapon is the gun with the most copies (the lowest id at a
    // tie): a utility launcher beside the turrets does not name the family.
    let gun = real
        .modules
        .iter()
        .filter(|m| is_gun(m.0))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|m| m.0);
    // The hull row gives the bay and the bandwidth; a hull the library does
    // not list flies no drones (a gun fit of it still builds).
    let drones = lib
        .hulls
        .iter()
        .find(|h| h.type_id == real.hull)
        .map_or_else(Vec::new, |h| flight(lib, h, &real.drones));
    // A module on a weapon hardpoint the library cannot classify is a weapon
    // we cannot read: the fit is not a drone boat.
    let hidden = real.modules.iter().any(|m| {
        !is_gun(m.0)
            && lib.inst.module(m.0).is_some_and(|x| x.hardpoint.is_some())
            && lib.kind_of(m.0) != Some(Kind::Sphere)
    });
    let family = match gun.and_then(|g| lib.kind_of(g)) {
        Some(Kind::Gun(family, _)) => family,
        _ if drones.is_empty() => return Err(Dropped::NoWeapon),
        _ if hidden => return Err(Dropped::UnknownWeapon),
        _ => Family::Drone,
    };
    let (primary, alt) = match gun {
        Some(g) => {
            let (greedy, alt) = choose_rounds(lib, g).map_or((None, None), |(p, a)| (Some(p), a));
            let primary = real
                .charge_for(|w| w == g)
                .or(greedy)
                .ok_or(Dropped::NoRound)?;
            (
                Some(primary),
                alt.filter(|a| *a != primary && lib.makeable.has(*a)),
            )
        }
        None => (None, None),
    };
    let types: Vec<i64> = std::iter::once(real.hull)
        .chain(real.modules.iter().map(|m| m.0))
        .chain(primary)
        .chain(alt)
        .chain(drones.iter().map(|d| d.0))
        .collect();
    let skills = skills_for(lib.inst.table(), &types, DOCTRINE_LEVEL);
    let profile = profile_with_drones(lib, real.hull, &real.modules, primary, &skills, &drones)
        .ok_or(Dropped::NoProfile)?;
    let holds = if fit_rules.is_some() {
        profile.cap_ok
    } else {
        profile.ok()
    };
    if !holds {
        return Err(Dropped::Fails);
    }
    let guns: u32 = real
        .modules
        .iter()
        .filter(|m| matches!(lib.kind_of(m.0), Some(Kind::Gun(f, _)) if f == family))
        .map(|m| u32::from(m.1))
        .sum();
    let mut charges = Vec::new();
    if let Some(p) = primary {
        charges.push((p, ROUNDS_PRIMARY * guns.max(1)));
    }
    if let Some(a) = alt {
        charges.push((a, ROUNDS_ALTERNATE * guns.max(1)));
    }
    for &(d, n) in &drones {
        charges.push((d, u32::from(n)));
    }
    let cost = lib.price(real.hull)
        + real
            .modules
            .iter()
            .map(|m| lib.price(m.0) * f64::from(m.1))
            .sum::<f64>()
        + charges
            .iter()
            .map(|c| lib.price(c.0) * f64::from(c.1))
            .sum::<f64>();
    Ok((
        family,
        RoleFit {
            hull: real.hull,
            modules: real.modules.clone(),
            charges,
            skills,
            profile,
            cost,
        },
    ))
}

/// [`try_build`] without the reason.
pub fn build(lib: &Library<'_>, real: &RealFit) -> Option<(Family, RoleFit)> {
    try_build(lib, real).ok()
}

/// **The seeds of a library**: for each DPS hull up to
/// [`SEED_FITS_PER_HULL`] usable real fits ([`RealFits::usable_trimmable`]
/// over the library's `Makeable`: a fit missing only rigs has them swapped
/// or dropped) that build and pass. Empty without a
/// table in `lib.real_fits`.
pub fn seeds(lib: &Library<'_>) -> RealSeeds {
    seeds_judged(lib, None)
}

/// [`seeds`] with each fit judged as the server does ([`try_build_fitted`]):
/// a fit with a module offline for CPU or power is kept. The caller supplies
/// the server's rules from the content (`eve-fitcheck`'s `World`).
pub fn seeds_fitted(lib: &Library<'_>, rules: &dyn Rules) -> RealSeeds {
    seeds_judged(lib, Some(rules))
}

fn seeds_judged(lib: &Library<'_>, rules: Option<&dyn Rules>) -> RealSeeds {
    let mut out = RealSeeds::default();
    let Some(table) = lib.real_fits else {
        return out;
    };
    let has = |t: i64| lib.makeable.has(t);
    for h in lib.hulls_for(crate::doctrine::Role::Dps) {
        // The first fits that build, so a row with no weapon takes no slot.
        let mut kept = 0;
        let mut trimmed_shapes: Vec<(i64, Vec<(i64, u8)>)> = Vec::new();
        let is_rig = |t: i64| is_rig(lib, t);
        for (real, missing) in table.usable_trimmable(h.type_id, &has, &is_rig) {
            if kept == SEED_FITS_PER_HULL {
                break;
            }
            // A rig no builder makes is swapped or left out, never a reason
            // to lose the fit; the engine still judges what remains.
            let trimmed;
            let real = if missing.is_empty() {
                real
            } else {
                trimmed = trim_rigs(lib, real, &missing);
                &trimmed
            };
            // A trimmed fit that lands on a fit already seeded, or a plain
            // fit that an already seeded trimmed fit landed on, is that fit
            // twice: it would double its weight, so it is skipped. (Same hull
            // and modules; charges and drones are not compared. The rows
            // come most-flown first, so either order happens.)
            let want = shape_of(real);
            let twice = if missing.is_empty() {
                trimmed_shapes.contains(&want)
            } else {
                out.iter().any(|(_, _, s)| shape(&s.fit) == want)
            };
            if twice {
                continue;
            }
            let Ok((family, fit)) = build_judged(lib, real, rules) else {
                continue;
            };
            kept += 1;
            if !missing.is_empty() {
                trimmed_shapes.push(want);
            }
            let layer = layer_of(&fit.profile.defence);
            let e = out.groups.entry((h.type_id, family, layer)).or_default();
            e.0.by_hull.entry(h.type_id).or_default().push(real.clone());
            e.1.insert(
                real.hash.clone(),
                Seed {
                    class: h.class,
                    fit,
                    count: real.count,
                    hash: real.hash.clone(),
                },
            );
        }
    }
    out
}

/// Whether `t` is a rig (it carries the rig slot effect).
fn is_rig(lib: &Library<'_>, t: i64) -> bool {
    lib.inst
        .table()
        .get(t)
        .is_some_and(|r| r.has_effect(crate::doctrine_fit::EFFECT_RIG_SLOT))
}

/// `maxGroupFitted` and `maxTypeFitted` [V-data] (`fitting.rs`).
const A_MAX_GROUP_FITTED: i64 = 1544;
const A_MAX_TYPE_FITTED: i64 = 2431;
/// `metaLevel` [V-data], the library's own meta cap applies to a swap.
const A_META_LEVEL: i64 = 633;

/// **A real fit without the rigs no builder makes**: each rig in `missing`
/// is swapped for a makeable rig of the same group, else left out. The
/// swap is deterministic: the cheapest in calibration, then the lowest type
/// id, among the rigs of the same group and the same `rigSize` as the lost
/// rig and the hull, that the hull allows, that stay within the meta cap, the
/// hull's calibration (with the rigs kept and swapped so far), and the
/// swap's own `maxTypeFitted` / `maxGroupFitted`. With no such rig, or a hull
/// that states no calibration or rig size, the rig is just dropped. The
/// engine ([`build_judged`]) judges the result as any other fit.
fn trim_rigs(lib: &Library<'_>, real: &RealFit, missing: &[(i64, u8)]) -> RealFit {
    use eve_server::rigs::{ATTR_CALIBRATION, ATTR_RIG_SIZE, ATTR_UPGRADE_COST};
    let table = lib.inst.table();
    let mut modules: BTreeMap<i64, u8> = real
        .modules
        .iter()
        .filter(|m| !missing.iter().any(|x| x.0 == m.0))
        .copied()
        .collect();
    let hull = table.get(real.hull);
    let capacity = hull.and_then(|h| h.attribute(ATTR_CALIBRATION));
    let hull_size = hull.and_then(|h| h.attribute(ATTR_RIG_SIZE));
    if let (Some(hull), Some(capacity), Some(hull_size)) = (hull, capacity, hull_size) {
        let cost_of = |t: i64| {
            table
                .get(t)
                .and_then(|r| r.attribute(ATTR_UPGRADE_COST))
                .unwrap_or(0.0)
        };
        let group_of = |t: i64| table.get(t).map(|r| r.group_id);
        let mut used: f64 = modules
            .iter()
            .filter(|m| is_rig(lib, *m.0))
            .map(|m| cost_of(*m.0) * f64::from(*m.1))
            .sum();
        for &(lost, n) in missing {
            let Some(lost_row) = table.get(lost) else {
                continue;
            };
            let group = lost_row.group_id;
            let in_group = |modules: &BTreeMap<i64, u8>| -> u32 {
                modules
                    .iter()
                    .filter(|m| group_of(*m.0) == Some(group))
                    .map(|m| u32::from(*m.1))
                    .sum()
            };
            let best = table
                .types()
                .filter(|c| {
                    c.published
                        && c.group_id == group
                        && c.type_id != lost
                        && c.has_effect(crate::doctrine_fit::EFFECT_RIG_SLOT)
                        && lib.makeable.has(c.type_id)
                        && c.attribute(A_META_LEVEL).unwrap_or(0.0) <= lib.limits.meta_max
                        && c.attribute(ATTR_RIG_SIZE).map(|s| s.round()) == Some(hull_size.round())
                        && lost_row.attribute(ATTR_RIG_SIZE).map(|s| s.round())
                            == Some(hull_size.round())
                        && eve_server::fitting::hull_allows(
                            |a| c.attribute(a),
                            hull.group_id,
                            real.hull,
                        )
                        && used + c.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0) * f64::from(n)
                            <= capacity
                        && c.attribute(A_MAX_TYPE_FITTED).is_none_or(|l| {
                            let have = modules.get(&c.type_id).copied().unwrap_or(0);
                            f64::from(have) + f64::from(n) <= l
                        })
                        && c.attribute(A_MAX_GROUP_FITTED)
                            .is_none_or(|l| f64::from(in_group(&modules)) + f64::from(n) <= l)
                })
                .min_by(|a, b| {
                    let (ca, cb) = (
                        a.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0),
                        b.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0),
                    );
                    ca.total_cmp(&cb).then(a.type_id.cmp(&b.type_id))
                });
            if let Some(c) = best {
                let e = modules.entry(c.type_id).or_insert(0);
                *e = e.saturating_add(n);
                used += c.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0) * f64::from(n);
            }
        }
    }
    RealFit {
        modules: modules.into_iter().collect(),
        ..real.clone()
    }
}

fn sorted(m: &[(i64, u8)]) -> Vec<(i64, u8)> {
    let mut m = m.to_vec();
    m.sort_unstable();
    m
}

/// A fit's identity for the seed check: hull and sorted modules.
fn shape(fit: &RoleFit) -> (i64, Vec<(i64, u8)>) {
    (fit.hull, sorted(&fit.modules))
}

/// [`shape`] of a real fit.
fn shape_of(fit: &RealFit) -> (i64, Vec<(i64, u8)>) {
    (fit.hull, sorted(&fit.modules))
}

/// Whether the doctrine's damage line is one of the real `seeds`.
fn is_seeded(d: &crate::doctrine::Doctrine, seeds: &RealSeeds) -> bool {
    let Some(line) = d.line() else {
        return false;
    };
    let want = shape(line);
    seeds.iter().any(|(_, _, s)| shape(&s.fit) == want)
}

/// **How many doctrines of the catalog fly a real fit**: the hull and sorted
/// modules of each doctrine's damage line against the seeds' (SOAK).
pub fn seeded_in(catalog: &[crate::doctrine::Doctrine], seeds: &RealSeeds) -> usize {
    catalog.iter().filter(|d| is_seeded(d, seeds)).count()
}

/// The keys of the catalog's real-seeded doctrines.
pub fn seeded_keys(
    catalog: &[crate::doctrine::Doctrine],
    seeds: &RealSeeds,
) -> std::collections::BTreeSet<String> {
    catalog
        .iter()
        .filter(|d| is_seeded(d, seeds))
        .map(|d| d.key.clone())
        .collect()
}

#[cfg(test)]
mod seeded_tests {
    use super::*;
    use crate::doctrine::testkit;

    const HULL: i64 = 990_970_001;
    const GUN: i64 = 990_970_002;
    const PLATE: i64 = 990_970_003;

    fn fit(hull: i64, modules: Vec<(i64, u8)>) -> RoleFit {
        let mut f = testkit::doc(testkit::brawler(testkit::flat_tank(1.0, 1.0, 1.0)), 1.0)
            .line()
            .expect("a line")
            .clone();
        f.hull = hull;
        f.modules = modules;
        f
    }

    fn doctrine(key: &str, f: RoleFit) -> crate::doctrine::Doctrine {
        let mut d = testkit::doc(testkit::brawler(testkit::flat_tank(1.0, 1.0, 1.0)), 1.0);
        d.key = key.to_owned();
        d.roles[0].2 = f;
        d
    }

    fn seeds_of(f: &RoleFit) -> RealSeeds {
        let mut out = RealSeeds::default();
        let e = out
            .groups
            .entry((f.hull, Family::Projectile, Layer::Armour))
            .or_default();
        e.1.insert(
            "invented".to_owned(),
            Seed {
                class: Class::Cruiser,
                fit: f.clone(),
                count: 3,
                hash: "invented".to_owned(),
            },
        );
        out
    }

    #[test]
    fn a_doctrine_is_seeded_when_hull_and_sorted_modules_match() {
        let real = fit(HULL, vec![(GUN, 3), (PLATE, 2)]);
        let seeds = seeds_of(&real);
        let same_other_order = doctrine("a", fit(HULL, vec![(PLATE, 2), (GUN, 3)]));
        let other_module = doctrine("b", fit(HULL, vec![(GUN, 4), (PLATE, 2)]));
        let other_hull = doctrine("c", fit(HULL + 1, vec![(GUN, 3), (PLATE, 2)]));
        let cat = vec![same_other_order, other_module, other_hull];
        assert_eq!(seeded_in(&cat, &seeds), 1);
        assert_eq!(
            seeded_keys(&cat, &seeds).into_iter().collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(seeded_in(&cat, &RealSeeds::default()), 0);
        assert_eq!(seeded_in(&[], &seeds), 0);
    }
}

#[cfg(test)]
mod fitted_tests {
    use super::*;
    use crate::doctrine::Makeable;
    use crate::doctrine::testkit::{CRAMPED, FRIGATE, SMALL_PROJECTILE, world};
    use crate::doctrine_fit::Limits;
    use crate::real_fits::{HullInfo, ModuleInfo, PartKind};
    use eve_server::fitting::{Hardpoint, HullFit, Rack};

    /// The server's rules for the invented world: every hull has three high
    /// slots and three turrets, the gun is a high-rack turret; `refuse`
    /// makes every module a hull restriction.
    struct Rules3 {
        refuse: bool,
    }

    impl Rules for Rules3 {
        fn hull(&self, t: i64) -> Option<HullInfo> {
            Some(HullInfo {
                fit: HullFit {
                    type_id: t,
                    hi: 3,
                    med: 3,
                    low: 3,
                    rig: 0,
                    turrets: 3,
                    launchers: 0,
                    cpu_output: 0.0,
                    power_output: 0.0,
                },
                group_id: 25,
                category_id: 6,
                rig_size: 1,
                calibration: 0.0,
                modular: false,
            })
        }
        fn module(&self, t: i64) -> Option<ModuleInfo> {
            (t == SMALL_PROJECTILE || t == CAP_GUN).then(|| ModuleInfo {
                hardpoint: Some(Hardpoint::Turret),
                ..ModuleInfo::plain(PartKind::Rack(Rack::High), 55, 5.0)
            })
        }
        fn allows(&self, _: i64, _: i64, _: i64) -> bool {
            !self.refuse
        }
        fn power(&self, _: &RealFit) -> Option<(bool, bool)> {
            Some((false, false))
        }
    }

    /// An invented gun, a copy of the small projectile gun with its own id
    /// and the capacitor need `need` per shot.
    const CAP_GUN: i64 = 970_001_050;

    /// The invented world with `CAP_GUN` added.
    fn world_with_cap_gun(need: f64) -> eve_server::typedata::Installed {
        let text = crate::doctrine::testkit::world_text();
        let gun = text
            .lines()
            .find(|l| l.starts_with(&format!("type\t{SMALL_PROJECTILE}\t")))
            .expect("the small gun row");
        let mut cols: Vec<String> = gun.split('\t').map(str::to_owned).collect();
        cols[1] = CAP_GUN.to_string();
        let attrs = cols.iter().position(|c| c.contains("6=0.0;")).unwrap();
        cols[attrs] = cols[attrs].replace("6=0.0;", &format!("6={need};"));
        let table = eve_server::typedata::Table::parse(&format!("{text}{}\n", cols.join("\t")))
            .expect("the world with the cap gun parses");
        eve_server::typedata::Installed::build(table)
    }

    fn lib_of(inst: &eve_server::typedata::Installed) -> Library<'_> {
        Library::build(inst, &Makeable::all(), &|_| 100.0, Limits::default())
    }

    fn row(hull: i64, guns: u8) -> RealFit {
        RealFit {
            hull,
            hash: format!("h{hull}x{guns}"),
            count: 4,
            share: 0.5,
            modules: if guns == 0 {
                Vec::new()
            } else {
                vec![(SMALL_PROJECTILE, guns)]
            },
            charges: Vec::new(),
            drones: Vec::new(),
        }
    }

    #[test]
    fn a_fit_over_the_grid_is_kept_by_the_fit_judge_and_reported_offline() {
        let inst = world();
        let lib = lib_of(&inst);
        let starved = row(CRAMPED, 3);
        // The strict judge wants every module online; this hull has a little
        // CPU and power, less than three guns need.
        assert_eq!(try_build(&lib, &starved).unwrap_err(), Dropped::Fails);
        let (family, fit) = try_build_fitted(&lib, &starved, &Rules3 { refuse: false })
            .expect("the server takes it; the module stays offline");
        assert_eq!(family, Family::Projectile);
        assert!(!fit.profile.cpu_ok, "reported, not hidden");
        // The same hull with room is online and is not reported.
        let (_, roomy) =
            try_build_fitted(&lib, &row(FRIGATE, 1), &Rules3 { refuse: false }).expect("fits");
        assert!(roomy.profile.cpu_ok && roomy.profile.pg_ok);
    }

    #[test]
    fn a_fit_the_server_refuses_is_dropped_even_when_it_would_come_online() {
        let inst = world();
        let lib = lib_of(&inst);
        let fine = row(FRIGATE, 1);
        assert!(try_build(&lib, &fine).is_ok());
        assert_eq!(
            try_build_fitted(&lib, &fine, &Rules3 { refuse: true }).unwrap_err(),
            Dropped::Refused
        );
        // Too many guns for the hardpoints: refused, however the grid does.
        assert_eq!(
            try_build_fitted(&lib, &row(FRIGATE, 4), &Rules3 { refuse: false }).unwrap_err(),
            Dropped::Refused
        );
    }

    #[test]
    fn the_capacitor_still_gates_under_the_fit_judge() {
        let rules = Rules3 { refuse: false };
        let mut starved = row(FRIGATE, 1);
        starved.modules = vec![(CAP_GUN, 1)];
        // Within CPU and power, but the gun drains far more than the hull
        // recharges: dropped by both judges.
        let inst = world_with_cap_gun(100_000.0);
        let lib = lib_of(&inst);
        assert_eq!(
            try_build_fitted(&lib, &starved, &rules).unwrap_err(),
            Dropped::Fails
        );
        // The same row with the drain gone builds.
        let inst = world_with_cap_gun(0.0);
        let lib = lib_of(&inst);
        let (_, fit) = try_build_fitted(&lib, &starved, &rules).expect("cap holds");
        assert!(fit.profile.cpu_ok && fit.profile.pg_ok && fit.profile.cap_ok);
    }

    #[test]
    fn the_weapon_requirement_stays_under_the_fit_judge() {
        let inst = world();
        let lib = lib_of(&inst);
        let bare = row(FRIGATE, 0);
        // A bare hull has no module: nothing the server refuses, still no
        // damage line to key a doctrine by.
        assert_eq!(
            try_build_fitted(&lib, &bare, &Rules3 { refuse: false }).unwrap_err(),
            Dropped::NoWeapon
        );
    }

    #[test]
    fn seeds_fitted_keeps_the_offline_fit_and_counts_it() {
        let inst = world();
        let mut lib = lib_of(&inst);
        let table = RealFits {
            by_hull: BTreeMap::from([(CRAMPED, vec![row(CRAMPED, 3)])]),
        };
        lib.real_fits = Some(&table);
        assert!(
            lib.hulls_for(crate::doctrine::Role::Dps)
                .any(|h| h.type_id == CRAMPED),
            "the hull is a DPS hull"
        );
        assert!(seeds(&lib).is_empty(), "the strict judge drops it");
        let kept = seeds_fitted(&lib, &Rules3 { refuse: false });
        assert_eq!(kept.offline_counts(), (1, 1, 1));
        assert!(kept.iter().all(|(_, _, s)| s.offline_cpu()));
        let refused = seeds_fitted(&lib, &Rules3 { refuse: true });
        assert!(refused.is_empty());
    }
}

#[cfg(test)]
mod rig_tests {
    use super::*;
    use crate::doctrine::Makeable;
    use crate::doctrine::testkit::{FRIGATE, SMALL_PROJECTILE, world_text};
    use crate::doctrine_fit::Limits;
    use eve_server::rigs::{ATTR_CALIBRATION, ATTR_RIG_SIZE, ATTR_UPGRADE_COST};

    const GROUP_A: i64 = 7_001;
    const GROUP_B: i64 = 7_002;
    const GROUP_C: i64 = 7_003;
    /// Lost to the builders in the tests; `GROUP_A`, small, cost 50.
    const LOST: i64 = 990_990_001;
    /// `GROUP_B`, small, cost 70: kept beside the lost one.
    const KEPT: i64 = 990_990_002;
    /// `GROUP_A`, small, cost 60.
    const DEAR: i64 = 990_990_003;
    /// `GROUP_A`, small, cost 20; and a twin with a higher id.
    const CHEAP: i64 = 990_990_004;
    const CHEAP_TWIN: i64 = 990_990_005;
    /// `GROUP_A`, medium, cost 5: the wrong size for a small hull.
    const MEDIUM: i64 = 990_990_006;
    /// `GROUP_C`, small, cost 10, lost; nothing else in its group.
    const LONE: i64 = 990_990_007;
    /// The group of the rigs the swap-clause tests add; `CAND` is the only
    /// rig of it a builder makes, `GONE` the one it lost.
    const GROUP_D: i64 = 7_004;
    const GONE: i64 = 990_990_010;
    const CAND: i64 = 990_990_011;
    /// The hull's calibration in the invented world.
    const CAPACITY: f64 = 100.0;

    /// The invented world: the testkit one, the rig slot effect, the rigs
    /// above, and the frigate stating a calibration and a small rig size.
    fn world_with_rigs() -> eve_server::typedata::Installed {
        world_with(true, true, &[])
    }

    /// A rig row: small unless `size` says otherwise, `extra` attributes
    /// after the three fixed ones (`;`-led), `HULLGROUP` / `HULLTYPE` in it
    /// are replaced by the frigate's own.
    fn rig(id: i64, group: i64, cost: f64, size: f64, meta: f64, extra: &str) -> String {
        format!(
            "type\t{id}\t{group}\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
             {ATTR_UPGRADE_COST}={cost};{ATTR_RIG_SIZE}={size};{A_META_LEVEL}={meta}{extra}\tInvented rig\n"
        )
    }

    /// The world with the frigate stating a calibration (when `cap`) and a
    /// small rig size (when `size`), and the `extra` rows added.
    fn world_with(cap: bool, size: bool, extra: &[String]) -> eve_server::typedata::Installed {
        let mut text = world_text();
        let hull = text
            .lines()
            .find(|l| l.starts_with(&format!("type\t{FRIGATE}\t")))
            .expect("the frigate row")
            .to_owned();
        let mut cols: Vec<String> = hull.split('\t').map(str::to_owned).collect();
        let hull_group = cols[2].clone();
        let at = cols.len() - 2;
        if cap {
            cols[at] = format!("{};{ATTR_CALIBRATION}={CAPACITY}", cols[at]);
        }
        if size {
            cols[at] = format!("{};{ATTR_RIG_SIZE}=1.0", cols[at]);
        }
        text = text.replace(&hull, &cols.join("\t"));
        text.push_str("effect\t2663\t0\t0\t0\t0\t0\t0\t0\trigSlot\t\n");
        for (id, group, cost, size) in [
            (LOST, GROUP_A, 50.0, 1.0),
            (KEPT, GROUP_B, 70.0, 1.0),
            (DEAR, GROUP_A, 60.0, 1.0),
            (CHEAP, GROUP_A, 20.0, 1.0),
            (CHEAP_TWIN, GROUP_A, 20.0, 1.0),
            (MEDIUM, GROUP_A, 5.0, 2.0),
            (LONE, GROUP_C, 10.0, 1.0),
        ] {
            text.push_str(&rig(id, group, cost, size, 0.0, ""));
        }
        for row in extra {
            text.push_str(
                &row.replace("HULLGROUP", &hull_group)
                    .replace("HULLTYPE", &FRIGATE.to_string()),
            );
        }
        let table = eve_server::typedata::Table::parse(&text).expect("the rig world parses");
        eve_server::typedata::Installed::build(table)
    }

    fn fit(modules: Vec<(i64, u8)>) -> RealFit {
        fit_h("invented", modules)
    }

    fn fit_h(hash: &str, modules: Vec<(i64, u8)>) -> RealFit {
        RealFit {
            hull: FRIGATE,
            hash: hash.to_owned(),
            count: 4,
            share: 0.5,
            modules,
            charges: Vec::new(),
            drones: Vec::new(),
        }
    }

    /// The modules of every seed `fits` build into (sorted) in `inst` when
    /// `gone` are not makeable.
    fn built_all(
        inst: &eve_server::typedata::Installed,
        fits: Vec<RealFit>,
        gone: &[i64],
    ) -> Vec<Vec<(i64, u8)>> {
        let ok: Vec<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            .filter(|t| !gone.contains(t))
            .collect();
        let mut lib = Library::build(inst, &Makeable::of(ok), &|_| 100.0, Limits::default());
        let table = RealFits {
            by_hull: BTreeMap::from([(FRIGATE, fits)]),
        };
        lib.real_fits = Some(&table);
        let mut out: Vec<Vec<(i64, u8)>> = seeds(&lib)
            .iter()
            .map(|(_, _, s)| sorted(&s.fit.modules))
            .collect();
        out.sort();
        out
    }

    /// The modules of the one seed `real` builds into when `gone` are not
    /// makeable, or `None` when no seed is built.
    fn built(real: RealFit, gone: &[i64]) -> Option<Vec<(i64, u8)>> {
        let mut all = built_all(&world_with_rigs(), vec![real], gone);
        assert!(all.len() <= 1, "one fit in, at most one seed out");
        all.pop()
    }

    /// `[gun, GONE x n]` with `GONE` lost and only `CAND` (with `cand_meta`
    /// and `cand_extra` attributes) to swap in, on a hull with or without
    /// its calibration and rig size; `held` copies of `CAND` are already on
    /// the fit.
    fn swap(
        cand_meta: f64,
        cand_extra: &str,
        n: u8,
        held: u8,
        cap: bool,
        size: bool,
    ) -> Option<Vec<(i64, u8)>> {
        let extra = [
            rig(GONE, GROUP_D, 10.0, 1.0, 0.0, ""),
            rig(CAND, GROUP_D, 10.0, 1.0, cand_meta, cand_extra),
        ];
        let mut modules = vec![(SMALL_PROJECTILE, 1), (GONE, n)];
        if held > 0 {
            modules.push((CAND, held));
        }
        let mut all = built_all(&world_with(cap, size, &extra), vec![fit(modules)], &[GONE]);
        all.pop()
    }

    fn gun_and(rig: Option<(i64, u8)>) -> Option<Vec<(i64, u8)>> {
        let mut v = vec![(SMALL_PROJECTILE, 1)];
        v.extend(rig);
        v.sort_unstable();
        Some(v)
    }

    #[test]
    fn a_fit_with_an_unmakeable_rig_builds_without_it_when_nothing_replaces_it() {
        let real = fit(vec![(SMALL_PROJECTILE, 1), (LONE, 1)]);
        assert_eq!(
            built(real.clone(), &[LONE]),
            Some(vec![(SMALL_PROJECTILE, 1)])
        );
        // Control: the same fit with the rig makeable keeps it.
        assert_eq!(
            built(real, &[]),
            Some(vec![(SMALL_PROJECTILE, 1), (LONE, 1)])
        );
    }

    #[test]
    fn a_swap_is_the_cheapest_rig_of_the_group_and_size_within_the_calibration() {
        // 70 of 100 calibration is taken by the kept rig, so 30 is left: the
        // dear one (60) does not fit, the medium one is the wrong size, and
        // of the two cheap twins the lower id wins.
        let real = fit(vec![(SMALL_PROJECTILE, 1), (LOST, 1), (KEPT, 1)]);
        assert_eq!(
            built(real, &[LOST]),
            Some(vec![(SMALL_PROJECTILE, 1), (KEPT, 1), (CHEAP, 1)])
        );
        // The same without the cheap ones: nothing fits, the rig is left out.
        let real = fit(vec![(SMALL_PROJECTILE, 1), (LOST, 1), (KEPT, 1)]);
        assert_eq!(
            built(real, &[LOST, CHEAP, CHEAP_TWIN]),
            Some(vec![(SMALL_PROJECTILE, 1), (KEPT, 1)])
        );
    }

    #[test]
    fn an_unmakeable_gun_still_drops_the_fit() {
        let real = fit(vec![(SMALL_PROJECTILE, 1), (CHEAP, 1)]);
        assert_eq!(built(real, &[SMALL_PROJECTILE]), None);
    }

    #[test]
    fn a_trimmed_fit_with_no_weapon_left_is_still_dropped_by_the_builder() {
        let real = fit(vec![(LONE, 1)]);
        assert_eq!(built(real, &[LONE]), None);
    }

    #[test]
    fn a_swap_stays_within_the_meta_cap() {
        let cap = Limits::default().meta_max;
        assert_eq!(swap(cap + 1.0, "", 1, 0, true, true), gun_and(None));
        // Control: at the cap the rig is allowed.
        assert_eq!(swap(cap, "", 1, 0, true, true), gun_and(Some((CAND, 1))));
    }

    #[test]
    fn a_swap_must_fit_the_hull_by_group_and_by_type() {
        for attr in [1298, 1302] {
            assert_eq!(
                swap(0.0, &format!(";{attr}=1.0"), 1, 0, true, true),
                gun_and(None)
            );
        }
        // Control: the same attribute naming this hull allows it.
        assert_eq!(
            swap(0.0, ";1298=HULLGROUP", 1, 0, true, true),
            gun_and(Some((CAND, 1)))
        );
        assert_eq!(
            swap(0.0, ";1302=HULLTYPE", 1, 0, true, true),
            gun_and(Some((CAND, 1)))
        );
    }

    #[test]
    fn a_swap_respects_max_type_fitted_with_the_copies_already_on_the_fit() {
        // Two copies of a rig limited to one: not swapped in.
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_TYPE_FITTED}=1.0"), 2, 0, true, true),
            gun_and(None)
        );
        // One copy already fitted and a limit of one: the swap adds none.
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_TYPE_FITTED}=1.0"), 1, 1, true, true),
            gun_and(Some((CAND, 1)))
        );
        // Controls: a limit of two allows both cases.
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_TYPE_FITTED}=2.0"), 2, 0, true, true),
            gun_and(Some((CAND, 2)))
        );
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_TYPE_FITTED}=2.0"), 1, 1, true, true),
            gun_and(Some((CAND, 2)))
        );
    }

    #[test]
    fn a_swap_respects_max_group_fitted() {
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_GROUP_FITTED}=1.0"), 2, 0, true, true),
            gun_and(None)
        );
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_GROUP_FITTED}=1.0"), 1, 1, true, true),
            gun_and(Some((CAND, 1)))
        );
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_GROUP_FITTED}=2.0"), 2, 0, true, true),
            gun_and(Some((CAND, 2)))
        );
        assert_eq!(
            swap(0.0, &format!(";{A_MAX_GROUP_FITTED}=2.0"), 1, 1, true, true),
            gun_and(Some((CAND, 2)))
        );
    }

    #[test]
    fn a_hull_without_calibration_or_rig_size_gets_no_swap() {
        assert_eq!(swap(0.0, "", 1, 0, false, true), gun_and(None));
        assert_eq!(swap(0.0, "", 1, 0, true, false), gun_and(None));
        // Control: with both stated the swap is made.
        assert_eq!(swap(0.0, "", 1, 0, true, true), gun_and(Some((CAND, 1))));
    }

    #[test]
    fn a_trimmed_fit_equal_to_a_seeded_one_is_not_seeded_twice() {
        let extra = [
            rig(GONE, GROUP_D, 10.0, 1.0, 0.0, ""),
            rig(CAND, GROUP_D, 10.0, 1.0, 0.0, ""),
        ];
        let inst = world_with(true, true, &extra);
        let fits = || {
            vec![
                fit_h("plain", vec![(SMALL_PROJECTILE, 1), (CAND, 1)]),
                fit_h("lost", vec![(SMALL_PROJECTILE, 1), (GONE, 1)]),
            ]
        };
        // The lost rig swaps to `CAND`: the same fit as the first.
        assert_eq!(
            built_all(&inst, fits(), &[GONE]),
            vec![vec![(SMALL_PROJECTILE, 1), (CAND, 1)]]
        );
        // Control: with the rig makeable the two fits differ and both seed.
        assert_eq!(built_all(&inst, fits(), &[]).len(), 2);
        // Rows come most-flown first: the trimmed fit may come before the
        // plain one, and the plain one is then the duplicate.
        let mut reversed = fits();
        reversed.reverse();
        assert_eq!(
            built_all(&inst, reversed, &[GONE]),
            vec![vec![(SMALL_PROJECTILE, 1), (CAND, 1)]]
        );
    }
}
