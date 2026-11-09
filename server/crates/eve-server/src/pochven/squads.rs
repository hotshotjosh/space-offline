//! PV-8: the squads of the Triglavian sites (`parity-pochven.md` PV-8), in the
//! shape of `wormhole_sites::squads`.
//!
//! `npc_gen::dungeon_squads` hands a Triglavian dungeon here. Its guards are
//! drawn from the faction's loaded NPC types by hull class, with a weight per
//! class by the dungeon's archetype, and carry no bounty (the Triglavian
//! types have none **[V-data]**, so the budgeted fill cannot draw them).
//! A dungeon with no spawn rows (combat 5 of 7, invasion 6 of 9
//! **[V-data]**) gets one squad at its entry object. Ore anomalies get none.

use crate::dungeon_data::Dungeon;
use crate::npc_data::{self, Role};
use crate::npc_gen::{SUPPORTED, SlotSquad, Squad};
use crate::seed::{Kind, Stream, stream};
use std::collections::BTreeMap;

/// `dunArchetypeCombatSites` **[V]** (`appConst.py`).
pub const ARCHETYPE_COMBAT: i64 = 24;
/// `dunArchetypeInvasionSites` **[V]** (`appConst.py:467`).
pub const ARCHETYPE_INVASION: i64 = 65;

/// **[A]** Squad size per spawn row, inclusive. Falsified by a site a
/// five-pilot cruiser gang finds trivial or cannot win.
pub const SQUAD: (u64, u64) = (3, 6);

/// What a site's squad is made of: a weight per hull class, and whether one
/// logistics ship is guaranteed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mix {
    /// `(class name as npc_data spells it, weight)`.
    pub weights: &'static [(&'static str, u64)],
    /// One member is a [`Role::Logistic`] type when the pool has one.
    pub logistic: bool,
}

/// **[A]** Combat anomalies (24). Falsified by a site that is never or
/// always lost by a five-pilot cruiser gang.
pub const COMBAT: Mix = Mix {
    weights: &[
        ("Frigate", 3),
        ("Destroyer", 2),
        ("Cruiser", 3),
        ("BattleCruiser", 1),
        ("Battleship", 1),
    ],
    logistic: false,
};

/// **[A]** Invasion sites (65), with a Rodiva-role (logistics) member of one of these classes guaranteed.
/// Falsified as [`COMBAT`] is.
pub const INVASION: Mix = Mix {
    weights: &[("Cruiser", 2), ("BattleCruiser", 2), ("Battleship", 3)],
    logistic: true,
};

/// The mix of an archetype; `None` for the ones that get no squad (ore
/// anomalies **[A]**, falsified by owners wanting ore sites guarded, and the
/// abyss, wormhole and beacon archetypes, which Pochven does not spawn).
pub fn mix_of(archetype: i64) -> Option<Mix> {
    match archetype {
        ARCHETYPE_COMBAT => Some(COMBAT),
        ARCHETYPE_INVASION => Some(INVASION),
        _ => None,
    }
}

/// The faction's usable types, ascending by type id inside each class.
fn buckets(data: &npc_data::Table, faction: i64) -> BTreeMap<&str, Vec<i64>> {
    let mut out: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for r in data.types.values() {
        if r.faction_id == faction && r.alias_of.is_none() && SUPPORTED.contains(r.needs) {
            out.entry(r.class.as_str()).or_default().push(r.type_id);
        }
    }
    out
}

/// The faction's usable logistics types of the classes `mix` names (so the
/// guaranteed member of an invasion squad is a cruiser or up, not a frigate),
/// ascending.
fn logistics(data: &npc_data::Table, faction: i64, mix: &Mix) -> Vec<i64> {
    data.types
        .values()
        .filter(|r| r.faction_id == faction && r.alias_of.is_none() && r.role == Role::Logistic)
        .filter(|r| SUPPORTED.contains(r.needs))
        .filter(|r| {
            mix.weights
                .iter()
                .any(|(c, w)| *w > 0 && *c == r.class.as_str())
        })
        .map(|r| r.type_id)
        .collect()
}

/// A class by weight among the classes the pool has; `None` with none.
fn pick_class<'a>(
    mix: &Mix,
    pool: &BTreeMap<&'a str, Vec<i64>>,
    s: &mut Stream,
) -> Option<&'a str> {
    let live: Vec<(&&str, u64)> = mix
        .weights
        .iter()
        .filter(|(c, w)| *w > 0 && pool.contains_key(c))
        .map(|(c, w)| (c, *w))
        .collect();
    let total: u64 = live.iter().map(|(_, w)| w).sum();
    if total == 0 {
        return None;
    }
    let mut at = s.range(0, total);
    for (c, w) in &live {
        if at < *w {
            return pool.get_key_value(**c).map(|(k, _)| *k);
        }
        at -= w;
    }
    None
}

/// One squad: `size` members by `mix`, a logistics ship first when asked.
fn squad_of(
    mix: &Mix,
    pool: &BTreeMap<&str, Vec<i64>>,
    logi: &[i64],
    size: (u64, u64),
    s: &mut Stream,
) -> Vec<i64> {
    let n = s.range(size.0, size.1.max(size.0) + 1);
    let mut members = Vec::new();
    if mix.logistic && !logi.is_empty() {
        members.push(logi[s.range(0, logi.len() as u64) as usize]);
    }
    while (members.len() as u64) < n {
        let Some(class) = pick_class(mix, pool, s) else {
            break;
        };
        let list = &pool[class];
        members.push(list[s.range(0, list.len() as u64) as usize]);
    }
    members
}

/// The pure core of [`squads`]: the mix and the size are parameters.
///
/// One squad per spawn row of `d`, drawn from
/// `stream(seed, Spawn, parts ++ [dungeon, index])` like
/// `npc_gen::dungeon_squads`, so the same seed gives the same squad. A
/// dungeon with no spawn rows gets one squad, `spawn` 0, at its entry object
/// (else its first object). Empty for a pool of nothing the mix names.
pub fn squads_by(
    d: &Dungeon,
    data: &npc_data::Table,
    seed: u64,
    parts: &[u64],
    faction: i64,
    mix: &Mix,
    size: (u64, u64),
) -> Vec<SlotSquad> {
    let pool = buckets(data, faction);
    if pool.is_empty() {
        return Vec::new();
    }
    let logi = logistics(data, faction, mix);
    let at: Vec<(i64, i64)> = if d.spawns.is_empty() {
        d.entry_object
            .and_then(|e| d.objects.iter().find(|o| o.id == e))
            .or_else(|| d.objects.first())
            .map(|o| (o.room, o.id))
            .into_iter()
            .collect()
    } else {
        d.spawns.iter().map(|sp| (sp.room, sp.object)).collect()
    };
    at.into_iter()
        .enumerate()
        .map(|(i, (room, object))| {
            let mut all = parts.to_vec();
            all.extend_from_slice(&[d.id as u64, i as u64]);
            let mut s = stream(seed, Kind::Spawn, &all);
            SlotSquad {
                room,
                object,
                spawn: i,
                squad: Squad {
                    faction_id: Some(faction),
                    members: squad_of(mix, &pool, &logi, size, &mut s),
                    bounty: 0.0,
                },
            }
        })
        .collect()
}

/// **A Triglavian site's squads**, by its archetype's [`mix_of`] and
/// [`SQUAD`]; none for an archetype without a mix.
pub fn squads(
    d: &Dungeon,
    data: &npc_data::Table,
    seed: u64,
    parts: &[u64],
    faction: i64,
) -> Vec<SlotSquad> {
    match mix_of(d.archetype) {
        Some(mix) => squads_by(d, data, seed, parts, faction, &mix, SQUAD),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Object, Spawn};
    use crate::npc_data::{Needs, NpcTypeRow, Tier};

    /// Invented faction, dungeons and types. The mixes below are the tests'
    /// own: the `[A]` mixes and the `[A]` ore rule are never asserted here.
    const FAC: i64 = 599_026;
    /// An archetype no mix is defined for.
    const NO_MIX: i64 = 999_001;
    const SIZE: (u64, u64) = (3, 5);
    const MIX: Mix = Mix {
        weights: &[("Frigate", 3), ("Cruiser", 2), ("Battleship", 1)],
        logistic: false,
    };
    const LOGI_MIX: Mix = Mix {
        weights: &[("Cruiser", 1), ("Battleship", 1)],
        logistic: true,
    };
    const SEED: u64 = 0x70_5eed;

    fn typ(type_id: i64, class: &str, role: Role) -> NpcTypeRow {
        NpcTypeRow {
            type_id,
            group_id: 990_700,
            faction_id: FAC,
            context: crate::npc_gen::DEADSPACE.to_owned(),
            class: class.to_owned(),
            tier: Tier::Deadspace,
            alias_of: None,
            needs: Needs::TURRET,
            role,
        }
    }

    fn data() -> npc_data::Table {
        let mut t = npc_data::Table::fixture();
        for r in [
            typ(990_711, "Frigate", Role::Dps),
            typ(990_712, "Frigate", Role::Dps),
            typ(990_713, "Destroyer", Role::Dps),
            typ(990_714, "Cruiser", Role::Dps),
            typ(990_715, "Cruiser", Role::Logistic),
            typ(990_716, "BattleCruiser", Role::Dps),
            typ(990_717, "Battleship", Role::Dps),
            typ(990_718, "Frigate", Role::Logistic),
        ] {
            t.types.insert(r.type_id, r);
        }
        t
    }

    fn object(room: i64, id: i64) -> Object {
        Object {
            room,
            id,
            type_id: 990_800,
            ..Object::default()
        }
    }

    fn dungeon(archetype: i64, spawns: Vec<Spawn>) -> Dungeon {
        Dungeon {
            id: 999_901,
            archetype,
            faction: Some(FAC),
            entry_object: Some(7),
            objects: vec![object(3, 5), object(4, 7), object(4, 9)],
            spawns,
            ..Dungeon::default()
        }
    }

    fn spawn(room: i64, object: i64) -> Spawn {
        Spawn {
            room,
            object,
            ..Spawn::default()
        }
    }

    fn class_of(t: &npc_data::Table, id: i64) -> &str {
        &t.types[&id].class
    }

    #[test]
    fn a_spawnless_dungeon_gets_one_squad_at_its_entry() {
        let (t, d) = (data(), dungeon(NO_MIX, vec![]));
        let squads = |d: &Dungeon| squads_by(d, &t, SEED, &[1], FAC, &MIX, SIZE);
        let s = squads(&d);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].room, s[0].object, s[0].spawn), (4, 7, 0));
        assert!((SIZE.0..=SIZE.1).contains(&(s[0].squad.members.len() as u64)));
        assert_eq!(s[0].squad.bounty, 0.0);
        assert_eq!(s[0].squad.faction_id, Some(FAC));
        // Without an entry object, the first object stands in.
        let mut d = d;
        d.entry_object = None;
        let s = squads(&d);
        assert_eq!((s[0].room, s[0].object), (3, 5));
        // A dungeon with spawn rows gets one squad per row, at its own object.
        d.spawns = vec![spawn(3, 5), spawn(4, 9)];
        let s = squads(&d);
        assert_eq!(
            s.iter()
                .map(|q| (q.room, q.object, q.spawn))
                .collect::<Vec<_>>(),
            [(3, 5, 0), (4, 9, 1)]
        );
    }

    #[test]
    fn the_hulls_follow_the_weights_passed_in() {
        let (t, d) = (data(), dungeon(NO_MIX, vec![]));
        for (only, want) in [("Frigate", "Frigate"), ("Battleship", "Battleship")] {
            let mix = Mix {
                weights: if only == "Frigate" {
                    &[("Frigate", 1), ("Cruiser", 0)]
                } else {
                    &[("Battleship", 1), ("Frigate", 0)]
                },
                logistic: false,
            };
            for seed in 0..20 {
                let s = squads_by(&d, &t, seed, &[1], FAC, &mix, (4, 6));
                assert!(!s[0].squad.members.is_empty());
                assert!(
                    s[0].squad.members.iter().all(|m| class_of(&t, *m) == want),
                    "{only}"
                );
            }
        }
        // A class the pool lacks is skipped, not drawn as nothing.
        let mix = Mix {
            weights: &[("Dreadnought", 9), ("Destroyer", 1)],
            logistic: false,
        };
        let s = squads_by(&d, &t, SEED, &[1], FAC, &mix, (3, 3));
        assert_eq!(s[0].squad.members, [990_713; 3]);
        // A mix that names nothing the pool has gives an empty squad.
        let mix = Mix {
            weights: &[("Dreadnought", 1)],
            logistic: false,
        };
        let s = squads_by(&d, &t, SEED, &[1], FAC, &mix, (3, 3));
        assert!(s[0].squad.members.is_empty());
    }

    #[test]
    fn a_logistic_mix_holds_a_logistics_ship_of_its_own_classes() {
        let (t, d) = (data(), dungeon(NO_MIX, vec![]));
        for seed in 0..20 {
            let s = squads_by(&d, &t, seed, &[1], FAC, &LOGI_MIX, SIZE);
            let m = &s[0].squad.members;
            // The Frigate logistics type is outside the mix's classes.
            assert_eq!(m[0], 990_715, "{m:?}");
            assert!(!m.contains(&990_718), "{m:?}");
            assert!(
                m.iter()
                    .all(|t_| matches!(class_of(&t, *t_), "Cruiser" | "Battleship"))
            );
        }
        // No logistics type of those classes: the guarantee is dropped, not faked.
        let mut t = t;
        t.types.remove(&990_715);
        let s = squads_by(&d, &t, SEED, &[1], FAC, &LOGI_MIX, SIZE);
        assert!(!s[0].squad.members.is_empty());
        assert!(!s[0].squad.members.contains(&990_718));
    }

    #[test]
    fn an_archetype_without_a_mix_gets_no_squad() {
        let t = data();
        assert_eq!(mix_of(NO_MIX), None);
        assert!(squads(&dungeon(NO_MIX, vec![]), &t, SEED, &[1], FAC).is_empty());
        assert!(squads(&dungeon(NO_MIX, vec![spawn(3, 5)]), &t, SEED, &[1], FAC).is_empty());
    }

    #[test]
    fn the_same_seed_gives_the_same_squad() {
        let (t, d) = (data(), dungeon(NO_MIX, vec![]));
        let by = |seed, site| squads_by(&d, &t, seed, &[site], FAC, &MIX, SIZE);
        let a = by(SEED, 1);
        assert_eq!(a, by(SEED, 1));
        let differs =
            (0..16).any(|n| by(SEED + n, 1) != a) && (0..16).any(|n| by(SEED, n + 2) != a);
        assert!(differs, "the seed and the site both move the draw");
    }

    #[test]
    fn another_factions_types_and_unsupported_needs_are_not_drawn() {
        let mut t = data();
        t.types.values_mut().for_each(|r| r.faction_id += 1);
        let d = dungeon(NO_MIX, vec![]);
        assert!(squads_by(&d, &t, SEED, &[1], FAC, &MIX, SIZE).is_empty());
        let mut t = data();
        t.types.get_mut(&990_714).unwrap().needs = Needs(!SUPPORTED.0);
        t.types.get_mut(&990_715).unwrap().alias_of = Some(990_717);
        for seed in 0..20 {
            let s = squads_by(&d, &t, seed, &[1], FAC, &LOGI_MIX, SIZE);
            let m = &s[0].squad.members;
            assert!(!m.contains(&990_714) && !m.contains(&990_715), "{m:?}");
            assert!(m.iter().all(|t_| *t_ == 990_717), "{m:?}");
        }
    }
}
