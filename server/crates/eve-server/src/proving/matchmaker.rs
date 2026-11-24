//! **The Proving Grounds matchmaker** (issue #65, PG-2a): who plays whom. Pure:
//! it reads the queue's entries and a match type and answers teams. The world
//! side (validation, the keys, the arena) is `queue::match_round`.
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **A match needs `numberOfFleets` complete teams; the match type's
//!   `min_queue_multiplier` is ignored** (owner decision Q4, 2026-11-24).
//!   Falsified by TQ material that makes a match wait for more queued teams.
//! - **A format of size 1 is one queued pilot per team; a larger size is
//!   the queued pilots of one fleet, exactly `fleet_size`; an incomplete
//!   fleet waits.** A fleet with more queued pilots than `fleet_size` plays
//!   its oldest `fleet_size`. Falsified by a client label text, or TQ
//!   material, that says otherwise.
//! - **Oldest first**: teams are ordered by their oldest member's `since`,
//!   and the first `numberOfFleets` that do not share a fleet play. One
//!   fleet never faces itself in a solo format. Falsified by a match on TQ
//!   between two members of one fleet.
//! - **The amount in a composition refusal is the cap**, not the count in
//!   the team ([`composition_errors`]).

use super::MatchType;
use super::queue::Entry;
use crate::abyss::Refusal;
use std::collections::BTreeMap;

/// `MAX_OF_SHIP_TYPE_RESTRICTIONS` **[V]** (`docs/re/proving-grounds.md` §4).
pub const MAX_OF_SHIP_TYPE: i64 = 36;
/// `MAX_OF_SHIP_GROUP_RESTRICTIONS` **[V]**.
pub const MAX_OF_SHIP_GROUP: i64 = 37;

/// One side of a match: the queued pilots who play together.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    /// The fleet the pilots queued in; 0 for a solo pilot outside a fleet.
    pub fleet_id: i64,
    /// Oldest first.
    pub members: Vec<Entry>,
}

impl Team {
    /// When the team's oldest member queued.
    pub fn since(&self) -> i64 {
        self.members
            .iter()
            .map(|e| e.since)
            .min()
            .unwrap_or(i64::MAX)
    }
}

/// The complete teams of `mt` in schedule `schedule_id` among `entries`,
/// ordered by their oldest member ([A], module doc). Entries of another
/// match type or schedule are ignored.
pub fn teams(entries: &[Entry], mt: &MatchType, schedule_id: i64) -> Vec<Team> {
    let mut mine: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.match_type_id == mt.id && e.schedule_id == schedule_id)
        .collect();
    mine.sort_by_key(|e| (e.since, e.char_id));
    let mut out: Vec<Team> = Vec::new();
    if mt.fleet_size <= 1 {
        for e in mine {
            out.push(Team {
                fleet_id: e.fleet_id,
                members: vec![e.clone()],
            });
        }
    } else {
        let size = usize::try_from(mt.fleet_size).unwrap_or(usize::MAX);
        let mut by_fleet: BTreeMap<i64, Vec<Entry>> = BTreeMap::new();
        for e in mine.into_iter().filter(|e| e.fleet_id != 0) {
            by_fleet.entry(e.fleet_id).or_default().push(e.clone());
        }
        for (fleet_id, mut members) in by_fleet {
            if members.len() >= size {
                members.truncate(size);
                out.push(Team { fleet_id, members });
            }
        }
    }
    out.sort_by_key(|t| (t.since(), t.members[0].char_id));
    out
}

/// The teams that play: the first `numberOfFleets` of `teams` (oldest first)
/// that do not share a fleet. Empty when there are not enough.
pub fn form(teams: &[Team], mt: &MatchType) -> Vec<Team> {
    let need = usize::try_from(mt.number_of_fleets.max(1)).unwrap_or(1);
    let mut out: Vec<Team> = Vec::new();
    for t in teams {
        if t.fleet_id != 0 && out.iter().any(|o| o.fleet_id == t.fleet_id) {
            continue;
        }
        out.push(t.clone());
        if out.len() == need {
            return out;
        }
    }
    Vec::new()
}

/// The composition rules of a team, `(character, ship type, ship group)` per
/// pilot **[V]** codes (§4), **[A]** that the amount is the cap: more of one
/// ship type than `max_of_ship_type` gives `(36, (amount, typeID))`, more of
/// one group than `max_of_ship_group` gives `(37, (amount, groupID))`, types
/// then groups, each in ascending id order. Falsified by a label text that
/// shows the count in the team instead of the cap.
pub fn composition_errors(team: &[(i64, i64, i64)], mt: &MatchType) -> Vec<Refusal> {
    let mut errors = Vec::new();
    let mut check = |cap: Option<i64>, code: i64, pick: fn(&(i64, i64, i64)) -> i64| {
        let Some(cap) = cap else { return };
        let mut counts: BTreeMap<i64, i64> = BTreeMap::new();
        for p in team {
            *counts.entry(pick(p)).or_default() += 1;
        }
        for (id, n) in counts {
            if n > cap {
                errors.push((code, vec![cap, id]));
            }
        }
    };
    check(mt.max_of_ship_type_restriction, MAX_OF_SHIP_TYPE, |p| p.1);
    check(mt.max_of_ship_group_restriction, MAX_OF_SHIP_GROUP, |p| p.2);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATCH: i64 = 884_001;
    const OTHER_MATCH: i64 = 884_002;
    const SCHED: i64 = 884_101;
    const OTHER_SCHED: i64 = 884_102;
    const TYPE_X: i64 = 990_901;
    const TYPE_Y: i64 = 990_902;
    const GROUP_G: i64 = 990_931;
    const GROUP_H: i64 = 990_932;

    fn mt(id: i64, fleets: i64, size: i64) -> MatchType {
        MatchType {
            id,
            number_of_fleets: fleets,
            fleet_size: size,
            ..MatchType::default()
        }
    }

    fn entry(who: i64, fleet: i64, since: i64) -> Entry {
        Entry {
            char_id: who,
            item_id: 0,
            type_id: 0,
            match_type_id: MATCH,
            schedule_id: SCHED,
            ship_id: 0,
            system_id: 0,
            since,
            fleet_id: fleet,
        }
    }

    fn chars(t: &[Team]) -> Vec<Vec<i64>> {
        t.iter()
            .map(|t| t.members.iter().map(|e| e.char_id).collect())
            .collect()
    }

    #[test]
    fn a_2x1_with_one_pilot_forms_nothing() {
        let m = mt(MATCH, 2, 1);
        let t = teams(&[entry(990_101, 0, 5)], &m, SCHED);
        assert_eq!(chars(&t), vec![vec![990_101]]);
        assert!(form(&t, &m).is_empty());
    }

    #[test]
    fn two_pilots_form_in_since_order_and_a_third_waits() {
        let m = mt(MATCH, 2, 1);
        // Queue order differs from `since` order on purpose.
        let e = [
            entry(990_103, 0, 30),
            entry(990_101, 0, 10),
            entry(990_102, 0, 20),
        ];
        let t = teams(&e, &m, SCHED);
        assert_eq!(chars(&t), vec![vec![990_101], vec![990_102], vec![990_103]]);
        assert_eq!(chars(&form(&t, &m)), vec![vec![990_101], vec![990_102]]);
    }

    #[test]
    fn one_fleet_never_faces_itself_in_a_solo_format() {
        let m = mt(MATCH, 2, 1);
        let e = [
            entry(990_101, 7, 10),
            entry(990_102, 7, 20),
            entry(990_103, 8, 30),
        ];
        let formed = form(&teams(&e, &m, SCHED), &m);
        assert_eq!(chars(&formed), vec![vec![990_101], vec![990_103]]);
        let only_one_fleet = [entry(990_101, 7, 10), entry(990_102, 7, 20)];
        assert!(form(&teams(&only_one_fleet, &m, SCHED), &m).is_empty());
    }

    #[test]
    fn a_2x2_needs_a_complete_fleet_and_the_other_fleet_completes() {
        let m = mt(MATCH, 2, 2);
        let mut e = vec![
            entry(990_101, 7, 10),
            entry(990_102, 7, 11),
            entry(990_103, 8, 12),
        ];
        let t = teams(&e, &m, SCHED);
        assert_eq!(chars(&t), vec![vec![990_101, 990_102]], "fleet 8 is short");
        assert!(form(&t, &m).is_empty());
        e.push(entry(990_104, 8, 13));
        let t = teams(&e, &m, SCHED);
        assert_eq!(
            chars(&form(&t, &m)),
            vec![vec![990_101, 990_102], vec![990_103, 990_104]]
        );
    }

    #[test]
    fn a_pilot_outside_a_fleet_never_makes_a_fleet_team() {
        let m = mt(MATCH, 2, 2);
        let e = [entry(990_101, 0, 10), entry(990_102, 0, 11)];
        assert!(teams(&e, &m, SCHED).is_empty());
    }

    #[test]
    fn a_big_fleet_plays_its_oldest_members() {
        let m = mt(MATCH, 2, 2);
        let e = [
            entry(990_103, 7, 30),
            entry(990_101, 7, 10),
            entry(990_102, 7, 20),
        ];
        assert_eq!(chars(&teams(&e, &m, SCHED)), vec![vec![990_101, 990_102]]);
    }

    #[test]
    fn schedules_and_match_types_are_never_mixed() {
        let m = mt(MATCH, 2, 1);
        let mut other_sched = entry(990_102, 0, 20);
        other_sched.schedule_id = OTHER_SCHED;
        let mut other_match = entry(990_103, 0, 30);
        other_match.match_type_id = OTHER_MATCH;
        let e = [entry(990_101, 0, 10), other_sched, other_match];
        let t = teams(&e, &m, SCHED);
        assert_eq!(chars(&t), vec![vec![990_101]]);
        assert!(
            form(&t, &m).is_empty(),
            "one pilot of this event is not a match"
        );
        let t = teams(&e, &m, OTHER_SCHED);
        assert_eq!(chars(&t), vec![vec![990_102]]);
    }

    #[test]
    fn the_team_with_the_oldest_member_plays_first() {
        let m = mt(MATCH, 2, 2);
        // Fleet 8's oldest pilot queued before anyone else, its other much later.
        let e = [
            entry(990_101, 7, 50),
            entry(990_102, 7, 51),
            entry(990_103, 8, 10),
            entry(990_104, 8, 60),
            entry(990_105, 9, 20),
            entry(990_106, 9, 21),
        ];
        let formed = form(&teams(&e, &m, SCHED), &m);
        assert_eq!(
            chars(&formed),
            vec![vec![990_103, 990_104], vec![990_105, 990_106]]
        );
    }

    #[test]
    fn composition_caps_name_the_cap_and_the_id() {
        let m = MatchType {
            max_of_ship_type_restriction: Some(2),
            max_of_ship_group_restriction: Some(3),
            ..mt(MATCH, 2, 5)
        };
        let ok = [
            (990_101, TYPE_X, GROUP_G),
            (990_102, TYPE_X, GROUP_G),
            (990_103, TYPE_Y, GROUP_G),
            (990_104, TYPE_Y, GROUP_H),
        ];
        assert!(composition_errors(&ok, &m).is_empty());
        let too_many_type = [
            (990_101, TYPE_X, GROUP_G),
            (990_102, TYPE_X, GROUP_G),
            (990_103, TYPE_X, GROUP_H),
        ];
        assert_eq!(
            composition_errors(&too_many_type, &m),
            vec![(MAX_OF_SHIP_TYPE, vec![2, TYPE_X])]
        );
        let too_many_group = [
            (990_101, TYPE_X, GROUP_G),
            (990_102, TYPE_X, GROUP_G),
            (990_103, TYPE_Y, GROUP_G),
            (990_104, TYPE_Y, GROUP_G),
        ];
        assert_eq!(
            composition_errors(&too_many_group, &m),
            vec![(MAX_OF_SHIP_GROUP, vec![3, GROUP_G])]
        );
        let none = mt(MATCH, 2, 5);
        assert!(composition_errors(&too_many_group, &none).is_empty());
    }
}
