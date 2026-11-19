//! PA-8b: `advance_system` builds its movers in parallel and gets the serial
//! result, bit for bit, on a frozen clock (invented ids throughout).

use super::*;
use crate::destiny::Mode;
use crate::space_state::{Motion, Presence};
use std::sync::Arc;
use std::time::Duration;

const SYSTEM: i64 = 30_000_001;
const PILOTS: usize = 640;
const STEPS: usize = 20;

fn pilot(i: usize) -> i64 {
    90_090_001 + i as i64
}

fn ship(i: usize) -> i64 {
    100_090_001 + i as i64
}

/// 640 pilots (a fifth orbit or follow a neighbour, a fifth fly to a point,
/// the rest sit still), 30 NPCs and 30 drone balls. Frozen clock.
fn world(pilots: usize) -> Registry {
    crate::test_clock::freeze(Duration::from_secs(1_800_000_000));
    let chars = (0..pilots)
        .map(|i| crate::model::Character {
            character_id: pilot(i),
            name: format!("Pilot {i}"),
            ship_id: ship(i),
            ship_type_id: 587,
            ..crate::model::Character::default()
        })
        .collect();
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: chars,
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(crate::content::Content::fixture()));
    for i in 0..pilots {
        crate::destiny::on_ballpark_bind(&mut reg, Some(pilot(i)));
        let at = [(i % 40) as f64 * 900.0, 0.0, (i / 40) as f64 * 900.0];
        let other = ship((i + 1) % pilots);
        let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
        p.motion = Motion::arrived(at, [0.0; 3]);
        match i % 5 {
            0 => {
                p.motion.mode = Mode::Orbit {
                    ball_id: other,
                    range: 2_000.0,
                };
                p.motion.speed_fraction = 1.0;
            }
            1 => {
                p.motion.mode = Mode::Follow {
                    ball_id: other,
                    range: 700.0,
                };
                p.motion.speed_fraction = 0.8;
            }
            2 => {
                p.motion.mode = Mode::Goto([30_000.0, 1_000.0, 20_000.0]);
                p.motion.speed_fraction = 1.0;
            }
            _ => {}
        }
    }
    for k in 0..30 {
        let at = [-5_000.0 - 300.0 * k as f64, 0.0, 100.0 * k as f64];
        crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, SYSTEM, at).expect("an id");
        let id = 195_090_001 + k;
        reg.roster_mut().arrive(Presence::arriving(
            id,
            id,
            SYSTEM,
            Motion::arrived([100.0 * k as f64, 50.0, 0.0], [10.0, 0.0, 0.0]),
        ));
    }
    reg
}

fn print(reg: &Registry) -> String {
    format!("{:?}", reg.roster().in_system(SYSTEM))
}

#[test]
fn parallel_movers_advance_like_the_serial_ones() {
    let mut worlds = [world(PILOTS), world(PILOTS), world(PILOTS)];
    assert!(worlds[0].roster().in_system(SYSTEM).len() >= MOVERS_PAR_MIN);
    assert!(
        worlds[0].roster().has_pair(SYSTEM),
        "the on-ticks path runs"
    );
    let spawned0 = crate::par::spawned_here();
    let start = print(&worlds[0]);
    for step in 0..STEPS {
        crate::test_clock::step(Duration::from_millis(250));
        worlds[0].advance_system_legacy(SYSTEM);
        let before = crate::par::spawned_here();
        worlds[1].advance_system_with(SYSTEM, 1);
        assert_eq!(
            before,
            crate::par::spawned_here(),
            "one thread spawns nothing"
        );
        worlds[2].advance_system_with(SYSTEM, 4);
        let gold = print(&worlds[0]);
        assert_eq!(print(&worlds[1]), gold, "threads 1, step {step}");
        assert_eq!(print(&worlds[2]), gold, "threads 4, step {step}");
    }
    assert_ne!(print(&worlds[0]), start, "the ships moved");
    assert!(
        crate::par::spawned_here() > spawned0,
        "threads 4 went parallel"
    );
    crate::test_clock::thaw();
}

#[test]
fn below_the_gate_nothing_is_spawned() {
    let mut reg = world(100);
    assert!(reg.roster().in_system(SYSTEM).len() < MOVERS_PAR_MIN);
    let before = crate::par::spawned_here();
    crate::test_clock::step(Duration::from_millis(250));
    reg.advance_system_with(SYSTEM, 8);
    assert_eq!(crate::par::spawned_here(), before);
    crate::test_clock::thaw();
}

/// Two pilots on a collision course: the first flying at the second, who sits
/// still 1,500 m ahead. Records written at `since`.
fn collision_course() -> Registry {
    let mut reg = world(2);
    let since = crate::space_state::now_seconds();
    let hull = |reg: &Registry, i: usize| {
        let p = reg.roster().get(pilot(i)).expect("in space");
        (p.ship_id, p.solar_system_id)
    };
    assert_eq!(hull(&reg, 0).1, SYSTEM);
    for (i, x) in [(0usize, 0.0), (1, 1_500.0)] {
        let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
        p.motion = Motion::arrived([x, 0.0, 0.0], [0.0; 3]);
        p.since = since;
        p.entered = since - 100.0;
        p.ticked = None;
    }
    let p = reg.roster_mut().get_mut(pilot(0)).expect("in space");
    p.motion.velocity = [300.0, 0.0, 0.0];
    p.motion.mode = Mode::Goto([1.0e9, 0.0, 0.0]);
    p.motion.speed_fraction = 1.0;
    reg
}

/// **A ship read alone is bumped like the system step bumps it** (review
/// finding 1). The read path used to advance one ship in the closed form,
/// which flies through a contact, so whether two ships bumped depended on
/// whether `advance_system` happened to run during the contact. Here the
/// first ship is read alone every second, as `advance_character` does before
/// each command and each position read, and both must end where a system
/// advance every second puts them; the second ship sits still, so only a
/// bump moves it.
#[test]
fn a_ship_read_alone_across_a_contact_is_bumped() {
    let mut by_system = collision_course();
    let mut by_reads = collision_course();
    for _ in 0..12 {
        crate::test_clock::step(Duration::from_secs(1));
        by_system.advance_system(SYSTEM);
        by_reads.advance_character(pilot(0));
    }
    by_reads.advance_character(pilot(1));
    let x = |reg: &Registry, i: usize| {
        reg.roster()
            .get(pilot(i))
            .expect("in space")
            .motion
            .position[0]
    };
    // Nothing but a bump moves the ship that sits still.
    assert!(x(&by_system, 1) > 1_500.0, "the system step hits it");
    assert!(
        x(&by_reads, 1) > 1_500.0,
        "read alone, the first ship flew through the second: {} {}",
        x(&by_reads, 0),
        x(&by_reads, 1)
    );
    for i in 0..2 {
        assert!(
            (x(&by_reads, i) - x(&by_system, i)).abs() < 1e-6,
            "pilot {i}: {} against {}",
            x(&by_reads, i),
            x(&by_system, i)
        );
    }
    crate::test_clock::thaw();
}

/// The gate stays closed for ships that cannot touch: reading one alone
/// leaves the closed form.
#[test]
fn ships_far_apart_are_still_read_alone() {
    let mut reg = collision_course();
    reg.roster_mut()
        .get_mut(pilot(1))
        .expect("in space")
        .motion
        .position = [9.0e7, 0.0, 0.0];
    assert!(!reg.steps_as_system(SYSTEM));
    crate::test_clock::step(Duration::from_secs(5));
    reg.advance_character(pilot(0));
    assert!(
        reg.roster()
            .get(pilot(0))
            .expect("in space")
            .ticked
            .is_none()
    );
    crate::test_clock::thaw();
}

/// Release only: `cargo test --release -p eve-server advance_tests::bench_gate_reads
/// -- --ignored --nocapture`. What the read-path gate costs per call: a miss
/// (computed) against a hit (kept for the tick and the roster revision).
#[test]
#[ignore]
fn bench_gate_reads() {
    let mut reg = world(PILOTS);
    for i in 0..PILOTS {
        let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
        p.motion.mode = Mode::Stop;
        p.motion.velocity = if i % 4 == 0 {
            [50.0, 0.0, 0.0]
        } else {
            [0.0; 3]
        };
    }
    let reps = 200;
    let t = std::time::Instant::now();
    for _ in 0..reps {
        // A structural write each time: the kept answer is never valid.
        reg.roster_mut().leave(-1);
        let p = *reg.roster().get(pilot(0)).expect("in space");
        reg.roster_mut().arrive(p);
        std::hint::black_box(reg.steps_as_system(SYSTEM));
    }
    let miss = t.elapsed().as_secs_f64() * 1e6 / reps as f64;
    let t = std::time::Instant::now();
    for _ in 0..reps {
        std::hint::black_box(reg.steps_as_system(SYSTEM));
    }
    let hit = t.elapsed().as_secs_f64() * 1e6 / reps as f64;
    eprintln!("bench_gate_reads: {PILOTS} pilots, miss {miss:.1} us, hit {hit:.1} us per call");
    crate::test_clock::thaw();
}
