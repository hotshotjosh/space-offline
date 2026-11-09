//! Pochven: the Triglavian region (PV-0, `parity-pochven.md`).
//!
//! The client's own test for "a Pochven system" is the system's faction
//! (`idCheckers.IsTriglavianSystem:141-145` **[V]**), and every rule here agrees
//! with it: the membership is read from the content bundle's `systems.tsv` at
//! run time, never from a list of ids in this file. The Krai is the system's
//! constellation. Border follows from the gate graph (a gate whose far end is
//! in another Krai). Home is the one **[A]** input: [`HOME_SYSTEMS`]; a test
//! passes its own homes in.
//!
//! Two tests are kept apart. [`is_pochven_by`] is the client's own faction
//! test (`IsTriglavianSystem`). Everything that lists, groups or classes the
//! region ([`systems_by`], [`krai_of_by`], [`class_with`], ...) also needs the
//! system's wormhole class to be [`CLASS`]: the bundle holds one more
//! Triglavian-faction system, in wormhole space with another class, which the
//! client calls Triglavian but which is in no Krai.
//!
//! Every function has a pure core that takes the faction, the gates or the
//! homes as parameters (so a test uses an invented map), and a thin wrapper on
//! the world.

pub mod drifter;
pub mod filaments;
pub mod guard;
pub mod loot;
pub mod npc;
pub mod spool;
pub mod squads;
#[cfg(test)]
mod tests;

use crate::celestial::{Band, Catalogue, Centers, Kind, Universe};
use crate::service::Registry;
use std::collections::BTreeSet;

/// `appConst.factionTriglavian` **[V]**: re-exported, not redefined.
pub use crate::combat::FACTION_TRIGLAVIAN;
/// `appConst.factionEDENCOM` **[V]** (`eve/common/lib/appConst.py:605`).
pub const FACTION_EDENCOM: i64 = 500_027;
/// `appConst.factionDrifters` **[V]** (`appConst.py:602`).
pub const FACTION_DRIFTERS: i64 = 500_024;
/// `TRIGLAVIAN_WORMHOLE_CLASS_ID` **[V]** (`appConst.py:1597`).
pub const CLASS: i64 = 25;

/// **[A]** The three Home systems, one per Krai (Kino, Niarja, Archee): the
/// Vyraj Anchorage descriptions put one anchorage "at the heart of" each Krai
/// (`docs/re/pochven.md` §4.3, **[V]** text). Falsified by a decoded
/// `locationLists` entry, or a TQ player, naming other systems. A test never
/// reads this const: it passes its own homes. [`home_systems`] keeps only
/// those the loaded map holds as Pochven, and warns once if that is not one per
/// Krai (a bundle with other ids).
pub const HOME_SYSTEMS: [i64; 3] = [30_001_372, 30_003_504, 30_002_702];

/// The client's light year, for the reach in [`kspace_within`]
/// (`appConst.LIGHTYEAR` **[V]**, held by [`crate::celestial::LIGHTYEAR`]).
pub use crate::celestial::LIGHTYEAR as LIGHT_YEAR_M;

/// A Pochven system's place in the Krai.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemClass {
    Home,
    Internal,
    Border,
}

/// Is `system` of `faction` in the bundle's map? The pure core of
/// [`is_pochven`]: the faction is a parameter. This is the client's own test
/// (`IsTriglavianSystem`), so it is also true of the bundle's one
/// Triglavian-faction wormhole system with another class, which is in no Krai:
/// use [`in_region_by`] when the class matters.
pub fn is_pochven_by(u: &Universe, faction: i64, system: i64) -> bool {
    u.system(system)
        .is_some_and(|s| s.faction_id == Some(faction))
}

/// Is `system` of `faction` and of wormhole class [`CLASS`] (the region's
/// systems, excluding the faction's wormhole-space outlier)?
pub fn in_region_by(u: &Universe, faction: i64, system: i64) -> bool {
    u.system(system)
        .is_some_and(|s| s.faction_id == Some(faction) && s.wormhole_class_id == Some(CLASS))
}

/// The client's own test: the system's `factionID` is the Triglavians'.
pub fn is_pochven(u: &Universe, system: i64) -> bool {
    is_pochven_by(u, FACTION_TRIGLAVIAN, system)
}

/// As [`is_pochven`] on the world's universe; false with no universe table.
pub fn in_pochven(reg: &Registry, system: i64) -> bool {
    reg.content()
        .try_get::<Universe>()
        .is_some_and(|u| is_pochven(u, system))
}

/// The Krai (the constellation id) of a system of `faction`, else `None`.
pub fn krai_of_by(u: &Universe, faction: i64, system: i64) -> Option<i64> {
    let s = u.system(system)?;
    in_region_by(u, faction, system).then_some(s.constellation_id)
}

/// The Krai of a Pochven system, else `None`.
pub fn krai_of(u: &Universe, system: i64) -> Option<i64> {
    krai_of_by(u, FACTION_TRIGLAVIAN, system)
}

/// Every system of `faction` and wormhole class [`CLASS`], ascending.
pub fn systems_by(u: &Universe, faction: i64) -> Vec<i64> {
    u.systems()
        .filter(|s| s.faction_id == Some(faction) && s.wormhole_class_id == Some(CLASS))
        .map(|s| s.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Every Pochven system, ascending.
pub fn systems(u: &Universe) -> Vec<i64> {
    systems_by(u, FACTION_TRIGLAVIAN)
}

/// Pochven systems of one Krai, ascending.
pub fn systems_of_krai(u: &Universe, krai: i64) -> Vec<i64> {
    systems(u)
        .into_iter()
        .filter(|&s| krai_of(u, s) == Some(krai))
        .collect()
}

/// The members of `candidates` that are systems of the region, ascending: the
/// pure core of [`home_systems`].
pub fn homes_among(u: &Universe, faction: i64, candidates: &[i64]) -> Vec<i64> {
    systems_by(u, faction)
        .into_iter()
        .filter(|id| candidates.contains(id))
        .collect()
}

/// True when `homes` holds exactly one system of each Krai of `faction`.
pub fn one_home_per_krai(u: &Universe, faction: i64, homes: &[i64]) -> bool {
    let krais: BTreeSet<i64> = systems_by(u, faction)
        .into_iter()
        .filter_map(|s| krai_of_by(u, faction, s))
        .collect();
    krais.iter().all(|&k| {
        homes
            .iter()
            .filter(|&&h| krai_of_by(u, faction, h) == Some(k))
            .count()
            == 1
    })
}

/// The Home systems **[A]**: [`HOME_SYSTEMS`] that the map holds as Pochven.
/// Warns once (stderr) when that is not exactly one per Krai.
pub fn home_systems(u: &Universe) -> Vec<i64> {
    static WARN: std::sync::Once = std::sync::Once::new();
    let homes = homes_among(u, FACTION_TRIGLAVIAN, &HOME_SYSTEMS);
    if !systems(u).is_empty() && !one_home_per_krai(u, FACTION_TRIGLAVIAN, &homes) {
        WARN.call_once(|| {
            eprintln!(
                "eve-server: pochven: HOME_SYSTEMS does not give one Home system per Krai \
                 in this bundle ({homes:?}); Home/Border classes are unreliable"
            );
        });
    }
    homes
}

/// The class of a system of `faction`; `None` when it is not one.
///
/// `gates` is `(system, far system)` for every gate. `homes` is passed in.
/// - `Home` if the system is in `homes`;
/// - `Border` if a gate from it leads to a system of `faction` in another
///   constellation;
/// - otherwise `Internal`.
pub fn class_with(
    u: &Universe,
    faction: i64,
    gates: &[(i64, i64)],
    homes: &[i64],
    system: i64,
) -> Option<SystemClass> {
    let krai = krai_of_by(u, faction, system)?;
    if homes.contains(&system) {
        return Some(SystemClass::Home);
    }
    let border = gates
        .iter()
        .filter(|(from, _)| *from == system)
        .any(|&(_, to)| krai_of_by(u, faction, to).is_some_and(|k| k != krai));
    Some(if border {
        SystemClass::Border
    } else {
        SystemClass::Internal
    })
}

/// Every gate of the world as `(its system, the far system)`, from the
/// celestial catalogue (`Catalogue::system_of`, `jump_of`). Computed on call:
/// the callers are cold paths.
pub fn gates_of(cat: &Catalogue) -> Vec<(i64, i64)> {
    cat.served()
        .filter(|c| c.kind == Kind::Gate)
        .filter_map(|g| {
            let from = cat.system_of(g.id)?;
            let (_, to) = cat.jump_of(g.id)?;
            Some((from, to))
        })
        .collect()
}

/// The world's class of `system`: [`class_with`] over the catalogue's gates
/// and [`home_systems`].
pub fn class_of(reg: &Registry, system: i64) -> Option<SystemClass> {
    let u = reg.content().try_get::<Universe>()?;
    class_with(
        u,
        FACTION_TRIGLAVIAN,
        &gates_of(reg.celestials()),
        &home_systems(u),
        system,
    )
}

/// Systems of `faction` of class `c`, ascending.
pub fn systems_of_class_with(
    u: &Universe,
    faction: i64,
    gates: &[(i64, i64)],
    homes: &[i64],
    c: SystemClass,
) -> Vec<i64> {
    systems_by(u, faction)
        .into_iter()
        .filter(|&s| class_with(u, faction, gates, homes, s) == Some(c))
        .collect()
}

/// The world's systems of class `c`, ascending.
pub fn systems_of_class(reg: &Registry, c: SystemClass) -> Vec<i64> {
    let Some(u) = reg.content().try_get::<Universe>() else {
        return Vec::new();
    };
    systems_of_class_with(
        u,
        FACTION_TRIGLAVIAN,
        &gates_of(reg.celestials()),
        &home_systems(u),
        c,
    )
}

/// K-space (high, low or null band) systems within `ly` light years of
/// `origin`'s centre, ascending by id. Systems of `faction` are left out: a
/// Proximity filament lands outside Pochven. None in reach gives the single
/// nearest such system; an origin with no known centre gives none.
///
/// **Note for PV-3:** a null-band system here can be unreachable (a gateless
/// one, such as Jove space). The caller must keep only systems that have
/// gates in the celestial catalogue before landing a fleet there.
pub fn kspace_within_by(
    u: &Universe,
    faction: i64,
    centers: &Centers,
    origin: i64,
    ly: f64,
) -> Vec<i64> {
    let kspace = |id: i64| {
        u.system(id).is_some_and(|s| {
            s.faction_id != Some(faction) && matches!(s.band(), Band::High | Band::Low | Band::Null)
        })
    };
    // Nearest first; `ly` is the reach, the whole map is the fallback.
    let all = centers.systems_within_ly(origin, f64::INFINITY);
    let mut hits: Vec<i64> = all
        .iter()
        .filter(|&&(id, d)| d <= ly && kspace(id))
        .map(|&(id, _)| id)
        .collect();
    if hits.is_empty() {
        if let Some(&(id, _)) = all.iter().find(|&&(id, _)| kspace(id)) {
            hits.push(id);
        }
        return hits;
    }
    hits.sort_unstable();
    hits
}

/// [`kspace_within_by`] for the Triglavians' Pochven.
pub fn kspace_within(u: &Universe, centers: &Centers, origin: i64, ly: f64) -> Vec<i64> {
    kspace_within_by(u, FACTION_TRIGLAVIAN, centers, origin, ly)
}
