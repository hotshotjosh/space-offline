//! PV-4: the Triglavian NPC types, merged into [`npc_data::Table`]
//! (`parity-pochven.md` PV-4).
//!
//! `pochven/triglavians.tsv` is written by `tools/content_pochven.py` from the
//! client's own type data: the entity types of the Triglavian faction whose
//! name ends in a player ship's name. [`rows`] reads it; [`with_triglavians`]
//! adds one [`NpcTypeRow`] per row (context `Deadspace`, tier deadspace) and
//! the faction's wreck by class, as `wormhole_sites::with_sleepers` does for
//! the Sleepers. Wrecks are kept per class (as the spec says): a later lane that
//! needs a wreck per rank or per type must add it, this table has one per class.
//! With no bundle, or a bundle built before the section, the
//! table is left as it was.

use crate::content::Sources;
use crate::npc_data::{self, Faction, Needs, NpcTypeRow, Role, Tier};

/// The file, relative to the bundle.
pub const FILE: &str = "pochven/triglavians.tsv";
/// Its header line (no `#`: a `#` line is a comment to [`crate::authored`]).
pub const HEADER: &str = "type_id\tgroup_id\tclass\trank\tneeds\trole\twreck_type_id\twhy";

/// One `triglavians.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct TrigRow {
    pub type_id: i64,
    pub group_id: i64,
    /// `Frigate`, `Destroyer`, ... (the player hull the name ends with).
    pub class: String,
    /// The name minus its last word; informational.
    pub rank: String,
    pub needs: Needs,
    pub role: Role,
    pub wreck_type_id: Option<i64>,
}

fn parse_row(r: &crate::authored::Row) -> Result<TrigRow, String> {
    Ok(TrigRow {
        type_id: r.i64(0)?,
        group_id: r.i64(1)?,
        class: r.field(2).to_owned(),
        rank: r.field(3).to_owned(),
        needs: Needs::parse(r.field(4))?,
        role: Role::parse(r.field(5))?,
        wreck_type_id: match r.field(6) {
            "" => None,
            _ => Some(r.i64(6)?),
        },
    })
}

/// The Triglavian rows of the bundle; **empty without a bundle** (the fixture
/// world's ids mean nothing) and, logged once, with a bundle that lacks the
/// file. A row that does not parse is logged and skipped.
///
/// # Errors
///
/// A file whose header is wrong.
pub fn rows(src: &Sources) -> Result<Vec<TrigRow>, String> {
    let Some(path) = src.bundle.and_then(|b| b.path(FILE)) else {
        if src.bundle.is_some() {
            crate::errln!("content: {FILE} absent from the bundle; no Triglavian NPC types");
        }
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (raw, _) = crate::authored::parse(&text, HEADER).map_err(|e| format!("{FILE}: {e}"))?;
    Ok(raw
        .iter()
        .filter_map(|r| match parse_row(r) {
            Ok(t) => Some(t),
            Err(e) => {
                crate::errln!("content: {FILE}: a row skipped: {e}");
                None
            }
        })
        .collect())
}

/// **Merge the Triglavians into `npc_data`**: one type row per [`TrigRow`]
/// (context `Deadspace`, tier deadspace, `faction`), and the faction's wreck
/// per class from the first row of each class that has one. The faction row
/// is created when absent, owned by [`crate::abyss::WAVE_OWNER`]; one the
/// table already holds keeps its own corporation. A type already listed is left alone, as is a
/// wreck the faction row already holds.
pub fn with_triglavians(mut t: npc_data::Table, rows: &[TrigRow], faction: i64) -> npc_data::Table {
    if rows.is_empty() {
        return t;
    }
    let mut wrecks = std::collections::BTreeMap::new();
    for r in rows {
        if let Some(w) = r.wreck_type_id {
            wrecks.entry(r.class.clone()).or_insert(w);
        }
        t.types.entry(r.type_id).or_insert_with(|| NpcTypeRow {
            type_id: r.type_id,
            group_id: r.group_id,
            faction_id: faction,
            context: crate::npc_gen::DEADSPACE.to_owned(),
            class: r.class.clone(),
            tier: Tier::Deadspace,
            alias_of: None,
            needs: r.needs,
            role: r.role,
        });
    }
    let f = t.factions.entry(faction).or_insert_with(|| Faction {
        faction_id: faction,
        corporation_id: crate::abyss::WAVE_OWNER,
        name: "Triglavian Collective".to_owned(),
        ..Faction::default()
    });
    for (class, w) in wrecks {
        f.wrecks.entry(class).or_insert(w);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented faction, corporation and types.
    const FAC: i64 = 599_026;
    const CORP: i64 = 1_990_298;

    fn row(type_id: i64, class: &str, wreck: Option<i64>) -> TrigRow {
        TrigRow {
            type_id,
            group_id: 990_700,
            class: class.to_owned(),
            rank: "Zed".to_owned(),
            needs: Needs::TURRET,
            role: Role::Dps,
            wreck_type_id: wreck,
        }
    }

    #[test]
    fn rows_become_npc_types_of_the_faction() {
        let t = with_triglavians(
            npc_data::Table::fixture(),
            &[
                row(990_711, "Frigate", Some(990_721)),
                row(990_712, "Cruiser", None),
            ],
            FAC,
        );
        let r = &t.types[&990_711];
        assert_eq!(
            (r.faction_id, r.class.as_str(), r.tier),
            (FAC, "Frigate", Tier::Deadspace)
        );
        assert_eq!(r.context, crate::npc_gen::DEADSPACE);
        assert_eq!((r.needs, r.role), (Needs::TURRET, Role::Dps));
        assert_eq!(t.types[&990_712].class, "Cruiser");
        let f = &t.factions[&FAC];
        assert_eq!(f.wrecks.get("Frigate"), Some(&990_721));
        assert_eq!(f.wrecks.get("Cruiser"), None);
    }

    #[test]
    fn an_existing_faction_row_keeps_its_corporation() {
        let mut base = npc_data::Table::fixture();
        base.factions.insert(
            FAC,
            npc_data::Faction {
                faction_id: FAC,
                corporation_id: CORP,
                ..npc_data::Faction::default()
            },
        );
        let t = with_triglavians(base, &[row(990_711, "Frigate", Some(990_721))], FAC);
        assert_eq!(t.factions[&FAC].corporation_id, CORP);
        assert_eq!(t.factions[&FAC].wrecks.get("Frigate"), Some(&990_721));
    }

    #[test]
    fn a_type_already_listed_is_kept() {
        let mut base = npc_data::Table::fixture();
        let mut old = row(990_711, "Battleship", None);
        old.role = Role::Tackler;
        let first = with_triglavians(base.clone(), &[old], FAC);
        base = first.clone();
        let t = with_triglavians(base, &[row(990_711, "Frigate", Some(990_721))], FAC);
        assert_eq!(t.types[&990_711], first.types[&990_711]);
        assert_eq!(t.types[&990_711].class, "Battleship");
    }

    #[test]
    fn the_wreck_of_a_class_is_its_first_rows() {
        let t = with_triglavians(
            npc_data::Table::fixture(),
            &[
                row(990_711, "Frigate", None),
                row(990_712, "Frigate", Some(990_721)),
                row(990_713, "Frigate", Some(990_722)),
            ],
            FAC,
        );
        assert_eq!(t.factions[&FAC].wrecks.get("Frigate"), Some(&990_721));
    }

    #[test]
    fn no_rows_leave_the_table_unchanged() {
        let base = npc_data::Table::fixture();
        let t = with_triglavians(base.clone(), &[], FAC);
        assert_eq!(t.types.len(), base.types.len());
        assert_eq!(t.factions.len(), base.factions.len());
        assert!(!t.factions.contains_key(&FAC));
    }

    #[test]
    fn a_row_parses_and_a_bad_header_is_refused() {
        let text =
            format!("# c\n{HEADER}\n990711\t990700\tFrigate\tZed\tTURRET\tDPS\t990721\twhy\n");
        let (raw, _) = crate::authored::parse(&text, HEADER).unwrap();
        let r = parse_row(&raw[0]).unwrap();
        assert_eq!(
            (r.type_id, r.class.as_str(), r.wreck_type_id),
            (990_711, "Frigate", Some(990_721))
        );
        assert!(crate::authored::parse("type_id\n", HEADER).is_err());
    }
}
