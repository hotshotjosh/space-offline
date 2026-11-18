//! 3707965, spec F (F3): launched drones follow a drone navigation
//! computer's burst. **Every id and number here is invented**
//! (the 981_6xx_xxx band): no client value is asserted.

use super::tests::{ALICE, ALICE_SHIP, BIG_BAY, ROWS, SK_DRONES, undock, world_in};
use super::*;
use crate::grid::tests::{drain, sent_to};

const DRONE_T: i64 = 981_600_002;
const DNC: i64 = 981_600_003;
/// An invented module of a group that is not the navigation computer's
/// (646), with the same speed effects.
const LINK: i64 = 981_600_005;

const E_DNC_ONLINE: i64 = 981_600_011;
const E_DNC_BURST: i64 = 981_600_012;
/// Module attributes: the speed bonus (percent) and its burst factor.
const A_SPEED: i64 = 981_600_501;
const A_FACTOR: i64 = 981_600_502;
const SEC: i64 = 1_000 * crate::modules::MSEC;
const T0: i64 = 133_000_000_000_000_000;

fn content() -> crate::content::Content {
    let rows = format!(
        "attribute\t{A_SPEED}\t0.0\tinventedSpeedBonus\n\
attribute\t{A_FACTOR}\t1.0\tinventedBurstFactor\n\
type\t{DRONE_T}\t100\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t10\t10\t\
9=111.0;37=1000.0;51=3000.0;54=2000.0;64=2.0;70=50.0;117=16.0;182=990581003.0;263=40.0;265=60.0;277=1.0;416=500.0;479=100000.0;482=1.0;508=250.0;552=20.0;1272=5.0\tInvented Drone\n\
type\t{DNC}\t644\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_DNC_BURST}\t12,16,{E_DNC_ONLINE},{E_DNC_BURST}\t\
73=10000.0;1014=1.0;{A_SPEED}=20.0;{A_FACTOR}=3.0\tInvented Navigation Computer\n\
type\t{LINK}\t646\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_DNC_BURST}\t12,16,{E_DNC_ONLINE},{E_DNC_BURST}\t\
73=10000.0;1014=1.0;{A_SPEED}=20.0;{A_FACTOR}=3.0\tInvented Tracking Link\n\

effect\t{E_DNC_ONLINE}\t4\t0\t0\t0\t0\t0\t0\tinventedDncOnline\t\n\
effect\t{E_DNC_BURST}\t1\t0\t73\t0\t0\t0\t0\tinventedDncBurst\t\n\
modifier\t{E_DNC_ONLINE}\tcharID\tOwnerRequiredSkillModifier\t37\t{A_SPEED}\t6\t990581003\t\n\
modifier\t{E_DNC_BURST}\titemID\tItemModifier\t{A_SPEED}\t{A_FACTOR}\t4\t\t\n"
    );
    let text = format!("{}{ROWS}{rows}", typedata::FIXTURE);
    let table = typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", typedata::Installed::build(table));
    c
}

fn world(hull: i64) -> Registry {
    world_in(
        content(),
        hull,
        0,
        &[(SKILL_DRONES, 5), (SK_DRONES, 1)],
        DRONE_T,
    )
}

fn fit(reg: &mut Registry, type_id: i64, flag: i64) -> i64 {
    let made = reg
        .apply_items(&[Edit::CreateAssembled {
            type_id,
            to: Slot::new(ALICE, ALICE_SHIP, flag),
        }])
        .expect("fitted");
    let _ = reg.take_pending();
    made[0].item_id()
}

fn speed_pushes(reg: &mut Registry) -> Vec<f64> {
    let out = drain(reg);
    // Nobody is sent the same update twice.
    for (to, _, _) in &out {
        let mut once = to.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), to.len(), "a repeated audience: {to:?}");
    }
    sent_to(&out, ALICE)
        .into_iter()
        .filter(|(n, _)| n == "SetMaxSpeed")
        .filter_map(|(_, a)| match a[1] {
            PyValue::Real(v) => Some(v),
            _ => None,
        })
        .collect()
}

/// F3: a drone out when a navigation computer's burst starts flies the
/// faster, is told so, and flies the slower again when the burst ends.
#[test]
fn a_launched_drone_follows_a_navigation_computers_burst() {
    let mut reg = world(BIG_BAY);
    let module = fit(&mut reg, DNC, 27);
    let made = reg
        .apply_items(&[Edit::Create {
            type_id: DRONE_T,
            quantity: 1,
            to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
        }])
        .expect("created");
    let id = made[0].item_id();
    undock(&mut reg, ALICE);
    let answer = launch(&mut reg, Some(ALICE), &[(id, 1)]);
    let _ = drain(&mut reg);
    let drone = reg
        .modules()
        .drones()
        .get(id)
        .copied()
        .unwrap_or_else(|| panic!("out: {answer:?}"));
    // Online: 1000 x (1 + 20 / 100).
    assert!((f64::from(drone.kind.hull.max_velocity) - 1200.0).abs() < 1e-3);

    crate::modules::activate(
        &mut reg,
        Some(ALICE),
        module,
        "inventedDncBurst",
        None,
        1000,
        T0,
    )
    .expect("the burst runs");
    // Active: 20 x 3 = 60 percent.
    let flying = hull_of(&reg, id).expect("a drone").max_velocity;
    assert!((f64::from(flying) - 1600.0).abs() < 1e-3, "{flying}");
    let told = speed_pushes(&mut reg);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!((told[0] - 1600.0).abs() < 1e-3);

    crate::modules::tick(&mut reg, T0 + 11 * SEC);
    let flying = hull_of(&reg, id).expect("a drone").max_velocity;
    assert!((f64::from(flying) - 1200.0).abs() < 1e-3, "{flying}");
    let told = speed_pushes(&mut reg);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!((told[0] - 1200.0).abs() < 1e-3);
}

/// F3 is not gated on the navigation computer's group: a module of another
/// group (an invented one, 646) that changes a launched drone's speed does
/// it the same way.
#[test]
fn a_module_of_another_group_also_moves_launched_drones() {
    let mut reg = world(BIG_BAY);
    let module = fit(&mut reg, LINK, 27);
    let made = reg
        .apply_items(&[Edit::Create {
            type_id: DRONE_T,
            quantity: 1,
            to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
        }])
        .expect("created");
    let id = made[0].item_id();
    undock(&mut reg, ALICE);
    let _ = launch(&mut reg, Some(ALICE), &[(id, 1)]);
    let _ = drain(&mut reg);
    crate::modules::activate(
        &mut reg,
        Some(ALICE),
        module,
        "inventedDncBurst",
        None,
        1000,
        T0,
    )
    .expect("the burst runs");
    let told = speed_pushes(&mut reg);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!((told[0] - 1600.0).abs() < 1e-3);
}
