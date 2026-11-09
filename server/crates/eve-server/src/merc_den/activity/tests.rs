//! Den activity tests. Every id, type and number here is invented: the
//! templates are [`fixture_table`]'s, the den table [`fixture_table`] of
//! `merc_den`'s. The shipped template file is only checked to be well formed
//! and the authored `[A]` constants (`GEN_S`, `LIFETIME_S`, `PER_LEVEL`) are
//! used by name, never by value.

use super::*;
use crate::celestial::{Catalogue, Universe};
use crate::content::Content;
use crate::dungeon_data::{Dungeon, Room, Table};
use crate::gateway::get_uint;
use crate::model::{Character, SandboxConfig};
use crate::population::{self, Population, Prepared};
use std::sync::{Arc, RwLock};

/// A system of the universe fixture that holds site slots.
const SYSTEM: i64 = 39_000_001;
pub(crate) const OWNER: i64 = 1_000_841;
const OTHER: i64 = 1_000_842;
pub(crate) const DEN_A: i64 = 100_841_001;
const DEN_B: i64 = 100_841_002;
const FACTION: i64 = 599_041;
pub(crate) const NOW: u64 = 2_000_000_000;

fn dungeons(with_text: bool) -> Table {
    let d = |id: i64| Dungeon {
        id,
        archetype: crate::combat_site::ESCALATION_ARCHETYPE,
        faction: Some(FACTION),
        difficulty: Some(crate::combat_site::ESCALATION_MIN_DIFFICULTY),
        name_id: with_text.then_some(id + 1),
        rooms: vec![Room {
            id: 7,
            ..Room::default()
        }],
        ..Dungeon::default()
    };
    // The next rung of the faction's ladder: what a cleared activity site
    // would escalate to if it rolled a step.
    let mut rung = d(NEXT_RUNG_DUNGEON);
    rung.difficulty = Some(crate::combat_site::ESCALATION_MIN_DIFFICULTY + 1);
    Table {
        dungeons: [FIXTURE_DUNGEON, FIXTURE_DUNGEON2]
            .into_iter()
            .map(|id| (id, d(id)))
            .chain([(NEXT_RUNG_DUNGEON, rung)])
            .collect(),
        ..Table::default()
    }
}

/// An archetype-31 dungeon one difficulty above the templates' (no template
/// opens it).
pub(crate) const NEXT_RUNG_DUNGEON: i64 = 990_840_900;

fn bodies() -> Catalogue {
    let mut text = String::new();
    for (n, s) in Universe::fixture().systems().enumerate() {
        let x = 1.0e11 * (n as f64 + 1.0);
        text += &format!(
            "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{}\n",
            40_000_000 + n,
            s.id
        );
    }
    Catalogue::parse(&text)
}

fn skill(reg: &mut Registry, who: i64, level: i64) {
    let entries = vec![crate::skill::SkillEntry {
        type_id: SKILL_DEN_MANAGEMENT,
        level,
        points: 0,
        rank: 1,
    }];
    *reg.skills_mut(who).expect("a pilot has skills") = crate::skill::Skills::restore(entries);
}

fn den(id: i64, owner: i64, enabled: bool) -> Den {
    Den {
        id,
        owner,
        skyhook: 8_841_001,
        planet: 40_000_000,
        system: SYSTEM,
        type_id: 990_841_900,
        enabled,
        paused: false,
        dev_base: 5,
        dev_at: NOW,
        anarchy_base: 10,
        anarchy_at: NOW,
        infomorphs: 0,
        last_gen: NOW,
        seen: 0,
        held: false,
    }
}

/// A world: [`OWNER`] (den-management level `level`) with [`DEN_A`], and
/// [`OTHER`] with no skill; dens are enabled unless `off`.
fn world_with(level: i64, with_text: bool, off: bool) -> Registry {
    let mut c = Content::fixture();
    c.insert("universe", Universe::fixture());
    c.insert("dungeons", dungeons(with_text));
    c.insert("sov mercenary dens", crate::merc_den::fixture_table());
    c.insert("sov mercenary den activities", fixture_table());
    let mut reg = Registry::new(SandboxConfig {
        celestials: bodies(),
        characters: [OWNER, OTHER]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: 100_000_000 + id,
                solar_system_id: Some(SYSTEM),
                ..Character::default()
            })
            .collect(),
        ..SandboxConfig::default()
    })
    .with_content(Arc::new(c));
    let prep = Prepared::new(
        &population::Table::fixture(),
        &Universe::fixture(),
        &dungeons(with_text),
        &|_| Some(FACTION),
    );
    *reg.ext_mut::<Population>() = Population::new(prep);
    skill(&mut reg, OWNER, level);
    skill(&mut reg, OTHER, 0);
    crate::merc_den::put(reg.deltas_mut(), &den(DEN_A, OWNER, !off));
    let _ = crate::gateway_out::take(&mut reg);
    reg
}

pub(crate) fn world() -> Registry {
    world_with(1, true, false)
}

/// The most activities `OWNER` may run: the capacity at level 1.
fn cap(reg: &Registry) -> u32 {
    capacity_for(reg, OWNER)
}

fn drain(reg: &mut Registry) -> Vec<crate::gateway_out::Queued> {
    crate::gateway_out::take(reg)
}

fn of_type<'a>(q: &'a [crate::gateway_out::Queued], name: &str) -> Vec<&'a Vec<u8>> {
    let ty = format!("{PKG}{name}");
    q.iter().filter(|n| n.0 == ty).map(|n| &n.1).collect()
}

/// The pilots each notice of `name` is addressed to.
fn audience(q: &[crate::gateway_out::Queued], name: &str) -> Vec<i64> {
    let ty = format!("{PKG}{name}");
    q.iter()
        .filter(|n| n.0 == ty)
        .map(|n| match n.2 {
            Target::Character(c) => c,
            _ => 0,
        })
        .collect()
}

pub(crate) fn made(reg: &mut Registry, den_id: i64) -> Activity {
    generate_now(reg, den_id, NOW).expect("an activity is made")
}

fn level(d: &Den, reg: &Registry, now: u64) -> (u64, u64) {
    let def = def_for(reg, d).expect("the den's rules");
    (d.dev_level(&def, now), d.anarchy_level(&def, now))
}

fn tpl(reg: &Registry, a: &Activity) -> Template {
    template_of(reg, a.template).expect("a template")
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

#[test]
fn the_shipped_file_is_well_formed() {
    let file = crate::authored::Dir::compiled_only()
        .load(FILE, SHIPPED, HEADER)
        .expect("the header matches");
    let (rows, skipped) = file.parse_rows_logged(parse_template_row);
    assert!(skipped.is_empty(), "rows skipped: {skipped:?}");
    assert!(!rows.is_empty());
    let mut keys: Vec<u32> = rows.iter().map(|r| r.0).collect();
    keys.dedup();
    assert_eq!(keys.len(), rows.len(), "a template number repeats");
}

#[test]
fn the_loader_drops_a_row_that_cannot_work() {
    let row = |cells: &str| {
        let text = format!("{HEADER}\n{cells}\n");
        let (raw, _) = crate::authored::parse(&text, HEADER).expect("the header");
        parse_template_row(&raw[0])
    };
    assert!(row("1\t11\t12\t13\t1\t-1\t0\t60\tx").is_ok());
    assert!(
        row("1\t0\t12\t13\t1\t-1\t0\t60\tx").is_err(),
        "no name text"
    );
    assert!(
        row("1\t11\t0\t13\t1\t-1\t0\t60\tx").is_err(),
        "no description"
    );
    assert!(row("1\t11\t12\t0\t1\t-1\t0\t60\tx").is_err(), "no dungeon");
    assert!(row("1\t11\t12\t13\t1\t-1\t0\t0\tx").is_err(), "no duration");
    assert!(
        row("1\t11\t12\t13\t1\t-1\t-3\t60\tx").is_err(),
        "negative bonus"
    );
    assert!(
        row("1\t11\t12\t13\t3000000000\t-1\t0\t60\tx").is_err(),
        "too big"
    );
}

#[test]
fn a_negative_impact_encodes_as_ten_bytes_and_decodes_back() {
    let t = Template {
        anarchy_impact: -4,
        ..*fixture_table().get(1).expect("a template")
    };
    let mut one = Vec::new();
    put_int32(&mut one, 5, t.anarchy_impact);
    // One tag byte and a ten-byte two's-complement varint.
    assert_eq!(one.len(), 11);
    let def = definition_bytes(&t);
    assert_eq!(get_uint(&def, 5).map(|v| v as i64 as i32), Some(-4));
    assert_eq!(
        get_uint(&def, 4).map(|v| v as i64 as i32),
        Some(t.dev_impact)
    );
    assert_eq!(get_uint(&def, 6), Some(u64::from(t.infomorph_bonus)));
    assert_eq!(
        crate::gateway::get_bytes(&def, 3).and_then(|d| get_uint(&d, 1)),
        Some(t.dungeon as u64)
    );
}

// ---------------------------------------------------------------------------
// Generation and expiry
// ---------------------------------------------------------------------------

#[test]
fn a_den_makes_one_activity_each_period_and_tells_its_owner() {
    let mut reg = world();
    tick_at(&mut reg, NOW);
    assert!(of_den(reg.deltas(), DEN_A).is_empty());
    assert_eq!(next_generation(reg.deltas(), DEN_A, NOW), NOW + GEN_S);
    tick_at(&mut reg, NOW + GEN_S - 1);
    assert!(of_den(reg.deltas(), DEN_A).is_empty(), "one second early");
    let _ = drain(&mut reg);
    tick_at(&mut reg, NOW + GEN_S);
    let acts = of_den(reg.deltas(), DEN_A);
    assert_eq!(acts.len(), 1);
    assert!(!acts[0].started);
    assert_eq!(acts[0].expiry, NOW + GEN_S + LIFETIME_S);
    assert_eq!(next_generation(reg.deltas(), DEN_A, NOW), NOW + 2 * GEN_S);
    let q = drain(&mut reg);
    let added = of_type(&q, "AddedNotice");
    assert_eq!(added.len(), 1);
    assert_eq!(audience(&q, "AddedNotice"), vec![OWNER]);
    assert_eq!(
        crate::gateway::get_bytes(added[0], 1).and_then(|u| get_bytes(&u, 1)),
        Some(acts[0].id.to_vec())
    );
    assert_eq!(read_id(added[0], 3), Some(SYSTEM as u64));
}

#[test]
fn a_late_clock_makes_what_an_early_one_would_and_stops_at_capacity() {
    // A clock that ticks every period, and one that wakes three periods late:
    // they end with the same activities and the same next time, at any
    // capacity (the property, not a figure).
    let mut early = world_with(2, true, false);
    for k in 0..=3 {
        tick_at(&mut early, NOW + k * GEN_S);
    }
    let mut reg = world_with(2, true, false);
    tick_at(&mut reg, NOW);
    tick_at(&mut reg, NOW + GEN_S);
    tick_at(&mut reg, NOW + 3 * GEN_S);
    let n = of_den(reg.deltas(), DEN_A).len();
    assert_eq!(n, of_den(early.deltas(), DEN_A).len());
    assert!(n >= 1 && n <= cap(&reg) as usize);
    assert_eq!(
        next_generation(reg.deltas(), DEN_A, NOW),
        next_generation(early.deltas(), DEN_A, NOW)
    );
    let next = next_generation(reg.deltas(), DEN_A, NOW);
    assert!(next > NOW + 3 * GEN_S, "the clock is past what was owed");
    // The clock runs on whether or not it makes anything.
    tick_at(&mut reg, next);
    assert!(of_den(reg.deltas(), DEN_A).len() <= cap(&reg) as usize);
    assert!(next_generation(reg.deltas(), DEN_A, NOW) > next);
}

#[test]
fn a_pilot_without_the_skill_has_no_capacity_and_the_den_makes_nothing() {
    let mut reg = world_with(0, true, false);
    assert_eq!(capacity_for(&reg, OWNER), 0);
    tick_at(&mut reg, NOW);
    tick_at(&mut reg, NOW + 10 * GEN_S);
    assert!(all(reg.deltas()).is_empty());
}

#[test]
fn capacity_follows_the_skill_level() {
    let mut reg = world();
    let one = capacity_for(&reg, OWNER);
    skill(&mut reg, OWNER, 3);
    assert_eq!(capacity_for(&reg, OWNER), 3 * one);
    skill(&mut reg, OWNER, 9);
    assert_eq!(
        capacity_for(&reg, OWNER),
        SKILL_TOP * one,
        "capped at the top level"
    );
}

#[test]
fn a_den_that_is_off_makes_nothing() {
    let mut reg = world_with(1, true, true);
    tick_at(&mut reg, NOW);
    tick_at(&mut reg, NOW + 5 * GEN_S);
    assert!(all(reg.deltas()).is_empty());
    assert!(next_generation(reg.deltas(), DEN_A, NOW) > NOW + 5 * GEN_S);
}

#[test]
fn nothing_is_made_when_no_template_can_open() {
    let mut reg = world_with(1, false, false);
    tick_at(&mut reg, NOW);
    tick_at(&mut reg, NOW + 2 * GEN_S);
    assert!(all(reg.deltas()).is_empty(), "the dungeons have no text");
    assert!(generate_now(&mut reg, DEN_A, NOW).is_err());
}

#[test]
fn the_same_den_and_count_pick_the_same_template() {
    let mut a = world();
    let mut b = world();
    let (x, y) = (made(&mut a, DEN_A), made(&mut b, DEN_A));
    assert_eq!(x, y);
}

#[test]
fn an_activity_not_started_runs_out_and_the_owner_is_told() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let _ = drain(&mut reg);
    tick_at(&mut reg, a.expiry - 1);
    assert!(by_id(reg.deltas(), &a.id).is_some());
    tick_at(&mut reg, a.expiry);
    assert!(by_id(reg.deltas(), &a.id).is_none());
    let q = drain(&mut reg);
    let removed = of_type(&q, "RemovedNotice");
    assert_eq!(removed.len(), 1);
    assert_eq!(read_id(removed[0], 1), Some(DEN_A as u64));
    assert_eq!(audience(&q, "RemovedNotice"), vec![OWNER]);
}

#[test]
fn a_started_activity_that_runs_out_takes_its_site_with_it() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).expect("the row").site;
    assert!(crate::escalation::get(&reg, site).is_some());
    let expiry = by_id(reg.deltas(), &a.id).expect("the row").expiry;
    tick_at(&mut reg, expiry);
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(
        crate::escalation::get(&reg, site).is_none(),
        "the site row goes"
    );
}

#[test]
fn a_den_going_takes_its_activities_and_clock() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    tick_at(&mut reg, NOW);
    let _ = drain(&mut reg);
    let d = crate::merc_den::by_id(reg.deltas(), DEN_A).expect("the den");
    den_removed(&mut reg, &d);
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(clock_of(reg.deltas(), DEN_A).is_none());
    let q = drain(&mut reg);
    assert_eq!(of_type(&q, "RemovedNotice").len(), 1);
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

#[test]
fn starting_twice_is_409() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    assert!(start(&mut reg, OWNER, &a.id, NOW).is_ok());
    assert_eq!(
        start(&mut reg, OWNER, &a.id, NOW),
        Err(StartRefused::AlreadyStarted)
    );
    assert_eq!(StartRefused::AlreadyStarted.status(), 409);
}

#[test]
fn an_unknown_foreign_or_run_out_activity_is_404() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    assert_eq!(
        start(&mut reg, OWNER, &[7u8; 16], NOW),
        Err(StartRefused::Unknown)
    );
    assert_eq!(
        start(&mut reg, OTHER, &a.id, NOW),
        Err(StartRefused::Unknown),
        "another pilot's"
    );
    assert_eq!(
        start(&mut reg, OWNER, &a.id, a.expiry),
        Err(StartRefused::Unknown),
        "run out"
    );
    assert_eq!(StartRefused::Unknown.status(), 404);
    // None of the refusals touched it.
    assert_eq!(by_id(reg.deltas(), &a.id), Some(a));
}

#[test]
fn a_pilot_at_capacity_is_refused_403_until_one_completes() {
    let mut reg = world();
    // A second den lets one pilot hold more started activities than a den
    // can hold.
    crate::merc_den::put(reg.deltas_mut(), &den(DEN_B, OWNER, true));
    let n = cap(&reg) as usize;
    let mut first = Vec::new();
    for _ in 0..n {
        let a = made(&mut reg, DEN_A);
        start(&mut reg, OWNER, &a.id, NOW).expect("within capacity");
        first.push(a);
    }
    let extra = made(&mut reg, DEN_B);
    let _ = drain(&mut reg);
    assert_eq!(
        start(&mut reg, OWNER, &extra.id, NOW),
        Err(StartRefused::Validation)
    );
    assert_eq!(StartRefused::Validation.status(), 403);
    assert_eq!(by_id(reg.deltas(), &extra.id), Some(extra), "unchanged");
    assert!(drain(&mut reg).is_empty(), "nothing was told");
    // One completes: the place is free again.
    let site = by_id(reg.deltas(), &first[0].id).expect("the row").site;
    assert!(completed_at(&mut reg, site, NOW));
    assert!(start(&mut reg, OWNER, &extra.id, NOW).is_ok());
}

#[test]
fn a_den_that_is_off_refuses_a_start_403() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let mut d = crate::merc_den::by_id(reg.deltas(), DEN_A).expect("the den");
    d.enabled = false;
    crate::merc_den::put(reg.deltas_mut(), &d);
    assert_eq!(
        start(&mut reg, OWNER, &a.id, NOW),
        Err(StartRefused::Validation)
    );
    assert!(!by_id(reg.deltas(), &a.id).expect("the row").started);
}

#[test]
fn a_start_whose_dungeon_cannot_open_is_refused_and_leaves_no_site() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    // The bundle loses the dungeons' text after the activity was made.
    let mut c = Content::fixture();
    c.insert("universe", Universe::fixture());
    c.insert("dungeons", dungeons(false));
    c.insert("sov mercenary dens", crate::merc_den::fixture_table());
    c.insert("sov mercenary den activities", fixture_table());
    let mut reg2 = reg.with_content(Arc::new(c));
    assert_eq!(
        start(&mut reg2, OWNER, &a.id, NOW),
        Err(StartRefused::Validation)
    );
    assert!(crate::escalation::all(&reg2).is_empty());
    assert!(!by_id(reg2.deltas(), &a.id).expect("the row").started);
}

#[test]
fn a_start_opens_a_personal_site_in_the_dens_system() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let t = tpl(&reg, &a);
    let _ = drain(&mut reg);
    let before = reg.scheduled_len();
    let bytes = start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let row = by_id(reg.deltas(), &a.id).expect("the row");
    assert!(row.started);
    assert_eq!(row.expiry, NOW + t.duration_s);
    assert_ne!(row.site, 0);
    assert!(crate::escalation::is_escalation_site(row.site));
    // The `escal` row reads back as the escalation module reads it.
    let e = crate::escalation::get(&reg, row.site).expect("an escalation row");
    assert_eq!(e.system, SYSTEM);
    assert_eq!(e.dungeon, t.dungeon);
    assert_eq!(e.chars, vec![OWNER]);
    assert!(e.expires > e.created);
    assert_eq!(e.journal, t.dungeon + 1, "the dungeon's name is the text");
    assert!(
        reg.scheduled_len() > before,
        "the transmission is scheduled"
    );
    // The answer is the attributes with `started`.
    assert_eq!(get_uint(&bytes, 3), Some(1));
    assert_eq!(read_id(&bytes, 2), Some(DEN_A as u64));
    assert_eq!(
        get_uint(&crate::gateway::get_bytes(&bytes, 4).unwrap(), 1),
        Some(row.expiry)
    );
    // Started and the new expiry are told.
    let q = drain(&mut reg);
    assert_eq!(of_type(&q, "StartedNotice").len(), 1);
    let changed = of_type(&q, "ExpiryChangedNotice");
    assert_eq!(changed.len(), 1);
    assert_eq!(
        crate::gateway::get_bytes(changed[0], 3).and_then(|t| get_uint(&t, 1)),
        Some(a.expiry),
        "the previous expiry"
    );
}

#[test]
fn two_starts_get_two_sites() {
    let mut reg = world_with(2, true, false);
    let a = made(&mut reg, DEN_A);
    let b = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("a");
    start(&mut reg, OWNER, &b.id, NOW).expect("b");
    let (sa, sb) = (
        by_id(reg.deltas(), &a.id).unwrap().site,
        by_id(reg.deltas(), &b.id).unwrap().site,
    );
    assert_ne!(sa, sb);
    assert_eq!(crate::escalation::all(&reg).len(), 2);
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[test]
fn completion_moves_the_dens_levels_by_the_impacts_and_adds_the_bonus() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let t = tpl(&reg, &a);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    let d0 = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    let (dev0, anarchy0) = level(&d0, &reg, NOW);
    let _ = drain(&mut reg);
    assert!(completed_at(&mut reg, site, NOW));
    let d1 = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    let (dev1, anarchy1) = level(&d1, &reg, NOW);
    assert_eq!(dev1 as i64, dev0 as i64 + i64::from(t.dev_impact));
    assert_eq!(
        anarchy1 as i64,
        anarchy0 as i64 + i64::from(t.anarchy_impact)
    );
    assert!(t.anarchy_impact < 0 && dev1 > dev0 && anarchy1 < anarchy0);
    assert_eq!(d1.infomorphs, d0.infomorphs + t.infomorph_bonus);
    assert!(by_id(reg.deltas(), &a.id).is_none(), "the activity goes");
    let q = drain(&mut reg);
    let done = of_type(&q, "CompletedNotice");
    assert_eq!(done.len(), 1);
    assert_eq!(read_id(done[0], 1), Some(DEN_A as u64));
    assert_eq!(
        get_uint(done[0], 3).map(|v| v as i64 as i32),
        Some(t.dev_impact)
    );
    assert_eq!(
        get_uint(done[0], 4).map(|v| v as i64 as i32),
        Some(t.anarchy_impact)
    );
    assert_eq!(get_uint(done[0], 5), Some(u64::from(t.infomorph_bonus)));
    assert_eq!(audience(&q, "CompletedNotice"), vec![OWNER]);
    assert_eq!(
        of_type(&q, "EvolutionStateChangedNotice").len(),
        0,
        "the evolution notice is the den api's, not the activity's"
    );
    assert!(
        q.iter()
            .any(|n| n.0.ends_with("api.EvolutionStateChangedNotice")),
        "the den window is told the new levels"
    );
    // Cleared once: a second completion of the same site pays nothing.
    assert!(!completed_at(&mut reg, site, NOW));
    let d2 = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    assert_eq!(level(&d2, &reg, NOW), (dev1, anarchy1));
}

#[test]
fn the_levels_are_settled_before_the_impact_and_never_go_below_zero() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let t = tpl(&reg, &a);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    // Some natural rise has happened since the den's levels were taken, and
    // the anarchy sits below the impact.
    let mut d = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    d.dev_at = NOW - 1_000;
    d.anarchy_base = 1;
    d.anarchy_at = NOW;
    crate::merc_den::put(reg.deltas_mut(), &d);
    let def = def_for(&reg, &d).unwrap();
    let natural = d.dev_level(&def, NOW);
    assert!(natural > d.dev_base, "the fixture rises within 1000 s");
    assert!(completed_at(&mut reg, site, NOW));
    let after = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    assert_eq!(
        after.dev_level(&def, NOW) as i64,
        natural as i64 + i64::from(t.dev_impact)
    );
    assert_eq!(
        after.dev_at, NOW,
        "the levels are re-based at the completion"
    );
    assert_eq!(after.anarchy_level(&def, NOW), 0);
}

#[test]
fn the_bonus_stops_at_the_cargo_cap() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let t = tpl(&reg, &a);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    let mut d = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    let def = def_for(&reg, &d).unwrap();
    assert!(t.infomorph_bonus > 1);
    d.infomorphs = def.cargo_cap - 1;
    d.last_gen = NOW;
    crate::merc_den::put(reg.deltas_mut(), &d);
    assert!(completed_at(&mut reg, site, NOW));
    let after = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    assert_eq!(after.infomorphs, def.cargo_cap);
}

#[test]
fn a_site_that_is_no_activitys_changes_nothing() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let before = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    assert!(!completed_at(&mut reg, 12_345, NOW), "no such site");
    // A started activity's site is the only one that counts; this one is not
    // started yet.
    assert!(!completed_at(&mut reg, 0, NOW));
    assert_eq!(crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap(), before);
    assert_eq!(by_id(reg.deltas(), &a.id), Some(a));
}

#[test]
fn the_gm_completion_needs_a_started_activity() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    assert!(complete_now(&mut reg, &a.id, NOW).is_err());
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    assert!(complete_now(&mut reg, &a.id, NOW).is_ok());
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(
        crate::escalation::get(&reg, site).is_none(),
        "the site row goes"
    );
}

// ---------------------------------------------------------------------------
// The gateway arm
// ---------------------------------------------------------------------------

fn ground(reg: Registry) -> Ground {
    Ground::Server(Arc::new(RwLock::new(reg)))
}

fn ask(g: &Ground, name: &str, body: &[u8], who: i64) -> Reply {
    gateway(&format!("{PKG}{name}"), body, who, g).expect("the activity arm owns it")
}

fn start_body(uuid: &[u8; 16]) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &uuid_msg(uuid));
    b
}

fn payload(r: &Reply) -> &[u8] {
    &r.payload.as_ref().expect("a payload").1
}

fn entries(b: &[u8]) -> Vec<Vec<u8>> {
    crate::gateway::field(b, 1)
}

fn den_body(n: i64) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &id(n as u64));
    b
}

#[test]
fn get_all_lists_the_pilots_own_activities_across_dens() {
    let mut reg = world();
    crate::merc_den::put(reg.deltas_mut(), &den(DEN_B, OWNER, true));
    let a = made(&mut reg, DEN_A);
    let b = made(&mut reg, DEN_B);
    let g = ground(reg);
    let r = ask(&g, "GetAllRequest", &[], OWNER);
    assert_eq!(r.status, 200);
    assert_eq!(
        r.payload.as_ref().unwrap().0,
        format!("{PKG}GetAllResponse")
    );
    let list = entries(payload(&r));
    assert_eq!(list.len(), 2);
    let ids: Vec<Vec<u8>> = list
        .iter()
        .map(|e| get_bytes(&get_bytes(e, 1).unwrap(), 1).unwrap())
        .collect();
    assert!(ids.contains(&a.id.to_vec()) && ids.contains(&b.id.to_vec()));
    // The attributes carry the den and the system.
    let attrs = get_bytes(&list[0], 2).unwrap();
    assert!(read_id(&attrs, 2).is_some() && read_id(&attrs, 5) == Some(SYSTEM as u64));
    // Another pilot sees none.
    let none = ask(&g, "GetAllRequest", &[], OTHER);
    assert_eq!(none.status, 200);
    assert!(entries(payload(&none)).is_empty());
}

#[test]
fn get_for_den_lists_its_activities_and_the_next_generation_time() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    tick_at(&mut reg, NOW);
    let g = ground(reg);
    let r = ask(&g, "GetForMercenaryDenRequest", &den_body(DEN_A), OWNER);
    assert_eq!(r.status, 200);
    assert_eq!(entries(payload(&r)).len(), 1);
    let when = get_bytes(payload(&r), 2).and_then(|t| get_uint(&t, 1));
    assert_eq!(when, Some(NOW + GEN_S));
    let first = get_bytes(&entries(payload(&r))[0], 1).and_then(|u| get_bytes(&u, 1));
    assert_eq!(first, Some(a.id.to_vec()));
    // 404 for an unknown den, another pilot's den and a body with no den.
    for (who, body) in [
        (OWNER, den_body(DEN_A + 77)),
        (OTHER, den_body(DEN_A)),
        (OWNER, Vec::new()),
    ] {
        assert_eq!(ask(&g, "GetForMercenaryDenRequest", &body, who).status, 404);
    }
}

#[test]
fn get_capacity_is_the_skill_scaled_capacity() {
    let reg = world();
    let want = capacity_for(&reg, OWNER);
    let g = ground(reg);
    let r = ask(&g, "GetCapacityRequest", &[], OWNER);
    assert_eq!(r.status, 200);
    assert_eq!(get_uint(payload(&r), 1), Some(u64::from(want)));
    let r = ask(&g, "GetCapacityRequest", &[], OTHER);
    assert_eq!(
        get_uint(payload(&r), 1).unwrap_or(0),
        0,
        "no skill, no capacity"
    );
}

#[test]
fn start_over_the_wire_answers_the_clients_statuses() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    let g = ground(reg);
    // 404: unknown, another pilot's, no uuid at all.
    assert_eq!(
        ask(&g, "StartRequest", &start_body(&[9u8; 16]), OWNER).status,
        404
    );
    assert_eq!(
        ask(&g, "StartRequest", &start_body(&a.id), OTHER).status,
        404
    );
    assert_eq!(ask(&g, "StartRequest", &[], OWNER).status, 404);
    // 200 with the attributes, started.
    let ok = ask(&g, "StartRequest", &start_body(&a.id), OWNER);
    assert_eq!(ok.status, 200);
    assert_eq!(
        ok.payload.as_ref().unwrap().0,
        format!("{PKG}StartResponse")
    );
    let attrs = get_bytes(payload(&ok), 1).expect("the attributes");
    assert_eq!(get_uint(&attrs, 3), Some(1));
    // 409 the second time.
    assert_eq!(
        ask(&g, "StartRequest", &start_body(&a.id), OWNER).status,
        409
    );
    // The written row is in the world.
    let Ground::Server(w) = &g else {
        panic!("server")
    };
    assert!(by_id(w.read().unwrap().deltas(), &a.id).unwrap().started);
}

#[test]
fn the_arm_owns_four_requests_and_the_den_arm_routes_them() {
    let g = Ground::local();
    for name in ["GetAllRequest", "GetCapacityRequest"] {
        let r = ask(&g, name, &[], OWNER);
        assert_eq!(r.status, 200, "{name} on a bare ground");
    }
    for name in ["GetForMercenaryDenRequest", "StartRequest"] {
        assert_eq!(
            ask(&g, name, &[], OWNER).status,
            404,
            "{name} on a bare ground"
        );
    }
    assert!(gateway(&format!("{PKG}SomethingElseRequest"), &[], OWNER, &g).is_none());
    assert!(
        gateway(
            "eve_public.sovereignty.mercenaryden.api.GetAsOwnerRequest",
            &[],
            OWNER,
            &g
        )
        .is_none()
    );
    // Through the den arm, which the chain asks.
    let r = crate::merc_den::gateway(&format!("{PKG}GetAllRequest"), &[], OWNER, &g);
    assert_eq!(r.map(|r| r.status), Some(200));
}

// ---------------------------------------------------------------------------
// The clock stage and the GM line
// ---------------------------------------------------------------------------

#[test]
fn the_dens_clock_runs_the_activity_clock_and_a_den_whose_item_is_gone_takes_them_along() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    // This world has no item for the den: the den clock finds it gone,
    // removes it and, with it, its activity.
    crate::merc_den::tick_at(&mut reg, NOW);
    assert!(crate::merc_den::by_id(reg.deltas(), DEN_A).is_none());
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(clock_of(reg.deltas(), DEN_A).is_none());
}

#[test]
fn the_gm_line_generates_lists_and_completes() {
    let mut reg = world();
    let run =
        |reg: &mut Registry, line: &[&str]| crate::merc_den::slash_cmd::run(reg, None, line, NOW);
    let said = run(&mut reg, &["generate", &DEN_A.to_string()]);
    assert!(said.starts_with("activity "), "{said}");
    let a = of_den(reg.deltas(), DEN_A)[0];
    let said = run(&mut reg, &["list", &DEN_A.to_string()]);
    assert!(said.contains(&hex(&a.id)), "{said}");
    assert!(run(&mut reg, &["complete", &hex(&a.id)]).contains("not started"));
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    assert_eq!(run(&mut reg, &["complete", &hex(&a.id)]), "completed");
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(run(&mut reg, &["generate", "abc"]).starts_with("usage"));
    assert!(run(&mut reg, &["generate", "424242"]).contains("no mercenary den"));
}

#[test]
fn the_uuid_spelling_round_trips_and_refuses_the_wrong_length() {
    let id = uuid_of(DEN_A, 3);
    assert_eq!(unhex(&hex(&id)), Some(id));
    assert_eq!(unhex("abcd"), None);
    assert_eq!(unhex(&"zz".repeat(16)), None);
    assert_ne!(uuid_of(DEN_A, 1), uuid_of(DEN_A, 2));
    assert_ne!(uuid_of(DEN_A, 1), uuid_of(DEN_B, 1));
}

#[test]
fn a_control_row_is_never_taken_for_an_activity() {
    let mut reg = world();
    tick_at(&mut reg, NOW);
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    // The clock row and the site counter are both in the kind now.
    assert!(reg.deltas().of(KIND).len() > 1);
    assert_eq!(all(reg.deltas()).len(), 1);
}

// ---------------------------------------------------------------------------
// One clock; a recycled site id (review r1, finding 1)
// ---------------------------------------------------------------------------

/// The `escal` row of `site`, with its pilots list replaced by `chars`.
fn retarget(reg: &mut Registry, site: i64, chars: i64) {
    let key = vec![site.to_string()];
    let mut row = reg
        .deltas()
        .of(crate::escalation::KIND)
        .get(&key)
        .expect("the escal row")
        .clone();
    *row.last_mut().expect("a pilots cell") = chars.to_string();
    reg.deltas_mut().put(crate::escalation::KIND, key, row);
}

#[test]
fn the_activity_and_its_site_end_together_on_a_scaled_clock() {
    let mut reg = world();
    crate::game_clock::set(
        &mut reg,
        crate::game_clock::GameClock::new(10).expect("a scale"),
    );
    let a = made(&mut reg, DEN_A);
    let t = tpl(&reg, &a);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let row = by_id(reg.deltas(), &a.id).expect("the row");
    let e = crate::escalation::get(&reg, row.site).expect("the site");
    let site_secs = ((e.expires - e.created) / 10_000_000) as u64;
    assert!(site_secs < t.duration_s, "the scale shortens the site");
    assert!(
        row.expiry - NOW >= site_secs && row.expiry - NOW <= site_secs + 1,
        "the activity ends with its site, not at the unscaled time"
    );
}

#[test]
fn a_started_activity_whose_site_row_is_gone_is_dropped_and_told() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    // The escalation module ran the site out first.
    assert!(crate::escalation::completed(&mut reg, site));
    let _ = drain(&mut reg);
    tick_at(&mut reg, NOW + 1);
    assert!(by_id(reg.deltas(), &a.id).is_none());
    let q = drain(&mut reg);
    assert_eq!(audience(&q, "RemovedNotice"), vec![OWNER]);
}

#[test]
fn a_recycled_site_id_is_not_the_activitys_to_vacate_or_complete() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let site = by_id(reg.deltas(), &a.id).unwrap().site;
    let before = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    // The id now belongs to another pilot's escalation.
    retarget(&mut reg, site, OTHER);
    assert!(!completed_at(&mut reg, site, NOW), "no completion, no pay");
    assert_eq!(crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap(), before);
    assert!(by_id(reg.deltas(), &a.id).is_some(), "left for the clock");
    assert!(
        complete_now(&mut reg, &a.id, NOW).is_ok(),
        "the GM line still settles the activity"
    );
    assert!(
        crate::escalation::get(&reg, site).is_some(),
        "the other pilot's site row is left alone"
    );
    // A run-out activity does not take the other pilot's site either.
    let b = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &b.id, NOW).expect("it starts");
    let site_b = by_id(reg.deltas(), &b.id).unwrap().site;
    retarget(&mut reg, site_b, OTHER);
    let expiry = by_id(reg.deltas(), &b.id).unwrap().expiry;
    tick_at(&mut reg, expiry);
    assert!(by_id(reg.deltas(), &b.id).is_none());
    assert!(crate::escalation::get(&reg, site_b).is_some());
}

#[test]
fn a_clear_after_the_activity_ran_out_pays_nothing() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let row = by_id(reg.deltas(), &a.id).unwrap();
    let before = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    assert!(!completed_at(&mut reg, row.site, row.expiry));
    assert_eq!(crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap(), before);
}

#[test]
fn a_completion_that_cannot_settle_tells_the_owner_it_is_gone() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    // A started row whose template is no longer in the table.
    let gone = Activity {
        template: 4_000_000_000,
        started: true,
        ..a
    };
    put(reg.deltas_mut(), &gone);
    let _ = drain(&mut reg);
    assert!(complete_now(&mut reg, &gone.id, NOW).is_err());
    assert!(by_id(reg.deltas(), &gone.id).is_none());
    let q = drain(&mut reg);
    assert_eq!(audience(&q, "RemovedNotice"), vec![OWNER]);
}

#[test]
fn an_activity_whose_template_is_gone_still_takes_its_own_site_with_it() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let mut row = by_id(reg.deltas(), &a.id).unwrap();
    let site = row.site;
    // Content reloaded without the template.
    row.template = u32::MAX;
    put(reg.deltas_mut(), &row);
    assert!(template_of(&reg, row.template).is_none());
    tick_at(&mut reg, NOW + 1);
    assert!(by_id(reg.deltas(), &a.id).is_none());
    assert!(
        crate::escalation::get(&reg, site).is_none(),
        "the escalation row goes with the activity"
    );
}

#[test]
fn a_clear_that_consumed_a_row_with_no_template_counts_as_taken() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let mut row = by_id(reg.deltas(), &a.id).unwrap();
    row.template = u32::MAX;
    put(reg.deltas_mut(), &row);
    let before = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    // The row was consumed, so the caller must roll no escalation step.
    assert!(completed_at(&mut reg, row.site, NOW));
    assert!(by_id(reg.deltas(), &a.id).is_none(), "the row is gone");
    assert_eq!(
        crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap(),
        before,
        "and nothing was paid"
    );
    // A second clear finds no row.
    assert!(!completed_at(&mut reg, row.site, NOW));
}

#[test]
fn a_stale_row_for_the_same_site_does_not_shadow_the_live_activity() {
    let mut reg = world();
    let a = made(&mut reg, DEN_A);
    start(&mut reg, OWNER, &a.id, NOW).expect("it starts");
    let live = by_id(reg.deltas(), &a.id).unwrap();
    let before = crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap();
    // A run-out row that names the same site, ordered before the live one.
    let stale = Activity {
        id: [0; 16],
        expiry: NOW,
        ..live
    };
    put(reg.deltas_mut(), &stale);
    assert!(completed_at(&mut reg, live.site, NOW + 1));
    assert_ne!(crate::merc_den::by_id(reg.deltas(), DEN_A).unwrap(), before);
    assert!(by_id(reg.deltas(), &a.id).is_none());
}
