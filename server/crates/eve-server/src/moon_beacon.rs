//! Moon mining beacons (#52): where a moon's beacon is, from the bundle.
//!
//! The client has every beacon position from its own static data
//! (`moonmining/miningBeacons.py` `GetMiningBeaconPositionForMoon` reads
//! `mapSolarSystemContentCache.moons[moonID].miningBeacon.position`) **[V]**,
//! so the server must agree with it: `moon/beacons.tsv` (`tools/content_moon.py`)
//! carries the same positions. Three things read them:
//!
//! - the deployment rules (`structure::find_deployment_conflict`): a drilling
//!   platform may sit by its moon's beacon, an auto moon miner must;
//! - the `moonMiningPoint` warp (`warpSubjects.MOON_MINING_POINT`, sent by
//!   `movementFunctions.py:457-475`): [`warp_target`];
//! - the moon's slim `beaconItemID` (`menusvc.py:2058-2075`): [`slim_extras`],
//!   for a moon whose row carries a beacon item id.
//!
//! With no bundle the table is empty: no beacon, no exemption, no warp.

use crate::content::{Boxed, Origin, Sources, TableSpec};
use crate::model::Target;
use crate::service::Registry;
use crate::slim_extra::SlimExtras;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The bundle file and its header line (`schema.md`).
pub const FILE: (&str, &str) = (
    "moon/beacons.tsv",
    "#moon_id\tsystem_id\tx\ty\tz\tbeacon_id",
);

/// The registration row in `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "moon_beacons",
    load,
    fixture: || Box::new(Beacons::default()) as Boxed,
};

/// `moonmining.const.MAXIMUM_MINING_BEACON_DISTANCE`, metres **[V]**.
pub const MAXIMUM_MINING_BEACON_DISTANCE: f64 = 250_000.0;
/// `warpSubjects.MOON_MINING_POINT` **[V]**.
pub const WARP_SUBJECT: &[u8] = b"moonMiningPoint";
/// `typeMoonMiningBeacon` **[V]**.
pub const TYPE_MOON_MINING_BEACON: i64 = 46_329;

/// One moon's beacon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Beacon {
    pub moon_id: i64,
    pub system_id: i64,
    pub pos: [f64; 3],
    /// `miningBeacon.itemID` **[I]**; `None` when the bundle has none.
    pub beacon_id: Option<i64>,
}

/// Every beacon, by moon and by system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Beacons {
    by_moon: BTreeMap<i64, Beacon>,
    by_system: BTreeMap<i64, Vec<i64>>,
}

impl Beacons {
    /// From the file's data lines. A malformed line is an error naming it.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Beacons, String> {
        Self::parse_iter(lines.iter().map(AsRef::as_ref))
    }

    /// The same, line by line, so a large file is never held twice.
    pub fn parse_iter<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Beacons, String> {
        let mut out = Beacons::default();
        for (i, l) in lines.enumerate() {
            let b =
                parse_row(l).ok_or_else(|| format!("{}: data line {}: {:?}", FILE.0, i + 1, l))?;
            out.put(b);
        }
        Ok(out)
    }

    fn put(&mut self, b: Beacon) {
        if let Some(old) = self.by_moon.insert(b.moon_id, b)
            && let Some(v) = self.by_system.get_mut(&old.system_id)
        {
            v.retain(|m| *m != old.moon_id);
        }
        let v = self.by_system.entry(b.system_id).or_default();
        v.push(b.moon_id);
        v.sort_unstable();
    }

    /// `GetMiningBeaconPositionForMoon`.
    pub fn of_moon(&self, moon: i64) -> Option<&Beacon> {
        self.by_moon.get(&moon)
    }

    /// The beacons of `system`, by moon id.
    pub fn in_system(&self, system: i64) -> Vec<Beacon> {
        self.by_system
            .get(&system)
            .into_iter()
            .flatten()
            .filter_map(|m| self.by_moon.get(m).copied())
            .collect()
    }

    /// `FindMoonBeaconInRangeOfPosition`: the **nearest** beacon within `range`
    /// **[I]** (the client takes the first in dict order; beacons sit far further
    /// apart than 250 km, so they agree).
    pub fn moon_in_range(&self, system: i64, pos: [f64; 3], range: f64) -> Option<i64> {
        self.in_system(system)
            .into_iter()
            .map(|b| (crate::structure::distance(pos, b.pos), b.moon_id))
            .filter(|(d, _)| *d < range)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, m)| m)
    }

    #[cfg(test)]
    pub fn insert(&mut self, b: Beacon) {
        self.put(b);
    }
}

fn parse_row(line: &str) -> Option<Beacon> {
    let f: Vec<&str> = line.split('\t').collect();
    let [moon, system, x, y, z, item] = f[..] else {
        return None;
    };
    let item: i64 = item.parse().ok()?;
    let pos = [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?];
    if !pos.iter().all(|v: &f64| v.is_finite()) {
        return None;
    }
    Some(Beacon {
        moon_id: moon.parse().ok()?,
        system_id: system.parse().ok()?,
        pos,
        beacon_id: (item != 0).then_some(item),
    })
}

fn load(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => {
            src.note(Origin::Bundle);
            read_file(b)?
        }
        None => Beacons::default(),
    }))
}

/// The bundle's file read line by line (it has one row per moon, some 300,000):
/// neither the text nor a `Vec<String>` of it is held. Absent file: empty;
/// a wrong header line is an error naming the file, as `Bundle::data_lines`.
fn read_file(b: &crate::content::Bundle) -> Result<Beacons, String> {
    use std::io::BufRead;
    let Some(p) = b.path(FILE.0) else {
        return Ok(Beacons::default());
    };
    let f = std::fs::File::open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut lines = std::io::BufReader::new(f).lines();
    let found = match lines.next() {
        Some(l) => l.map_err(|e| format!("{}: {e}", p.display()))?,
        None => String::new(),
    };
    if found != FILE.1 {
        return Err(format!(
            "{}: header is\n  {found}\nexpected\n  {}\nRe-extract with tools/up.sh",
            p.display(),
            FILE.1
        ));
    }
    let mut err = None;
    let rows = lines.map_while(|l| match l {
        Ok(l) => Some(l),
        Err(e) => {
            err = Some(format!("{}: {e}", p.display()));
            None
        }
    });
    let mut out = Beacons::default();
    for (i, l) in rows.enumerate() {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let beacon =
            parse_row(&l).ok_or_else(|| format!("{}: data line {}: {:?}", FILE.0, i + 1, l))?;
        out.put(beacon);
    }
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

static EMPTY: Beacons = Beacons {
    by_moon: BTreeMap::new(),
    by_system: BTreeMap::new(),
};

/// `reg.content().try_get::<Beacons>()` or an empty table.
pub fn beacons(reg: &Registry) -> &Beacons {
    reg.content().try_get::<Beacons>().unwrap_or(&EMPTY)
}

/// The moon whose beacon is within 250 km of `pos` in `system` (the extraction
/// scheduling and the Metenox call this).
pub fn moon_in_range(reg: &Registry, system: i64, pos: [f64; 3]) -> Option<i64> {
    beacons(reg).moon_in_range(system, pos, MAXIMUM_MINING_BEACON_DISTANCE)
}

/// The `moonMiningPoint` warp (`warp_subject::RESOLVERS`): the beacon of moon
/// `id`, in the actor's system. The client warps to its default range itself.
pub fn warp_target(w: &Registry, actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    let b = beacons(w).of_moon(id)?;
    if w.roster().get(actor)?.solar_system_id != b.system_id {
        return None;
    }
    Some(Target {
        position: b.pos,
        radius: 0.0,
        warp_in: Some(b.pos),
    })
}

/// `beaconItemID` (`moonmining.const.MOON_BEACONID_NAME`) on a moon's slim, for
/// moons whose row has a beacon item id.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> SlimExtras {
    let t = beacons(w);
    ids.iter()
        .filter_map(|id| {
            let item = t.of_moon(*id)?.beacon_id?;
            Some((*id, vec![("beaconItemID", PyValue::Int(item))]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::model::SandboxConfig;
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const SYS: i64 = 39_990_001;
    const OTHER_SYS: i64 = 39_990_002;
    const M1: i64 = 99_000_101;
    const M2: i64 = 99_000_102;
    const PILOT: i64 = 1_990_001;

    fn two() -> Beacons {
        Beacons::parse(&[
            format!("{M2}\t{SYS}\t5000000.0\t0.0\t0.0\t0"),
            format!("{M1}\t{SYS}\t0.0\t0.0\t0.0\t99000900"),
        ])
        .expect("parses")
    }

    #[test]
    fn the_table_parses_and_a_bad_line_names_itself() {
        let t = two();
        assert_eq!(t.of_moon(M1).and_then(|b| b.beacon_id), Some(99_000_900));
        assert_eq!(t.of_moon(M2).map(|b| b.beacon_id), Some(None));
        let e = Beacons::parse(&["1\t2\tx\t0\t0\t0"]).expect_err("bad");
        assert!(e.contains(FILE.0) && e.contains("data line 1"), "{e}");
        assert!(Beacons::parse(&["1\t2\t0\t0\t0"]).is_err(), "short");
    }

    #[test]
    fn in_system_lists_by_moon() {
        let t = two();
        let ids: Vec<i64> = t.in_system(SYS).iter().map(|b| b.moon_id).collect();
        assert_eq!(ids, vec![M1, M2]);
        assert!(t.in_system(OTHER_SYS).is_empty());
    }

    #[test]
    fn moon_in_range_takes_the_nearest_within_range() {
        let mut t = Beacons::default();
        for (m, x) in [(M1, 0.0), (M2, 100_000.0)] {
            t.insert(Beacon {
                moon_id: m,
                system_id: SYS,
                pos: [x, 0.0, 0.0],
                beacon_id: None,
            });
        }
        let r = MAXIMUM_MINING_BEACON_DISTANCE;
        assert_eq!(t.moon_in_range(SYS, [60_000.0, 0.0, 0.0], r), Some(M2));
        assert_eq!(t.moon_in_range(SYS, [40_000.0, 0.0, 0.0], r), Some(M1));
    }

    #[test]
    fn exactly_the_range_is_not_in_range() {
        let t = two();
        let r = MAXIMUM_MINING_BEACON_DISTANCE;
        assert_eq!(t.moon_in_range(SYS, [r, 0.0, 0.0], r), None);
        assert_eq!(t.moon_in_range(SYS, [r - 1.0, 0.0, 0.0], r), Some(M1));
    }

    #[test]
    fn nothing_in_range_is_none() {
        let t = two();
        let r = MAXIMUM_MINING_BEACON_DISTANCE;
        assert_eq!(t.moon_in_range(SYS, [2_000_000.0, 0.0, 0.0], r), None);
        assert_eq!(t.moon_in_range(OTHER_SYS, [0.0; 3], r), None);
        assert_eq!(Beacons::default().moon_in_range(SYS, [0.0; 3], r), None);
    }

    fn world(t: Beacons, system: i64) -> Registry {
        let mut c = Content::fixture();
        c.insert("moon_beacons", t);
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            PILOT,
            100_000_000 + PILOT,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        reg
    }

    #[test]
    fn the_warp_target_is_the_beacon_and_refuses_another_system() {
        let reg = world(two(), SYS);
        let t = warp_target(&reg, PILOT, M2, None).expect("a target");
        assert_eq!(t.position, [5_000_000.0, 0.0, 0.0]);
        assert_eq!(t.warp_in, Some(t.position));
        assert!(
            warp_target(&reg, PILOT, M2 + 50, None).is_none(),
            "no beacon"
        );
        let elsewhere = world(two(), OTHER_SYS);
        assert!(warp_target(&elsewhere, PILOT, M2, None).is_none());
        assert_eq!(WARP_SUBJECT, b"moonMiningPoint");
    }

    #[test]
    fn a_moon_slim_names_its_beacon_item_only_when_it_has_one() {
        let reg = world(two(), SYS);
        let x = slim_extras(&reg, &[M1, M2, 5]);
        assert_eq!(x.len(), 1);
        assert_eq!(x[0].0, M1);
        assert_eq!(x[0].1[0].0, "beaconItemID");
    }
}
