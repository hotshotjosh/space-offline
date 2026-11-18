//! The Aberrant modules' tests (3707965, spec F, F1): an active module that
//! multiplies **its own** attribute, which its online effect carries to the
//! ship, must refresh the ball's top speed.
//! **Every id and number here is invented**: types 990,720,xxx, a 10 s
//! burst, a bonus of 8 percent tripled while it runs.

use super::*;
use crate::grid::tests::{drain, sent_to};
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
const SEC: i64 = 1_000 * MSEC;
const ALICE: i64 = 90_720_001;
const SHIP: i64 = 100_720_001;
const SYSTEM: i64 = 30_720_001;
const HI_SLOT_0: i64 = 27;
const HULL: i64 = 990_720_001;
const OVERDRIVE: i64 = 990_720_002;
const RECHARGER: i64 = 990_720_003;
const M_OVERDRIVE: i64 = 100_720_101;
const M_RECHARGER: i64 = 100_720_102;
/// Invented effects: a burst (activation) and the online effect that
/// carries the module's own number to the ship.
const E_BURST_SPEED: i64 = 990_720_011;
const E_ONLINE_SPEED: i64 = 990_720_012;
const E_BURST_CAP: i64 = 990_720_013;
const E_ONLINE_CAP: i64 = 990_720_014;
/// Invented module attributes: the bonus (percent) and its burst factor.
const A_BONUS: i64 = 990_720_501;
const A_FACTOR: i64 = 990_720_502;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let module = |id: i64, burst: i64, online: i64, bonus: f64| {
        format!(
            "type\t{id}\t990720\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{burst}\t12,16,{online},{burst}\t\
             73=10000.0;1014=1.0;{A_BONUS}={bonus};{A_FACTOR}=3.0\tInvented {id}\n"
        )
    };
    let rows = [
        crate::typedata::FIXTURE,
        // Top speed 400, capacitor 1000 with a recharge time of 100 s.
        &format!(
            "type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
             37=400.0;55=100000.0;482=1000.0\tInvented Hull\n"
        ),
        &module(OVERDRIVE, E_BURST_SPEED, E_ONLINE_SPEED, 8.0),
        // A negative percent on the recharge time: a faster recharge.
        &module(RECHARGER, E_BURST_CAP, E_ONLINE_CAP, -20.0),
        &format!("attribute\t{A_BONUS}\t0.0\tinventedBonus\n"),
        &format!("attribute\t{A_FACTOR}\t1.0\tinventedFactor\n"),
        &format!("effect\t{E_BURST_SPEED}\t1\t0\t73\t0\t0\t0\t0\tinventedBurstSpeed\t\n"),
        &format!("effect\t{E_BURST_CAP}\t1\t0\t73\t0\t0\t0\t0\tinventedBurstCap\t\n"),
        &format!("effect\t{E_ONLINE_SPEED}\t4\t0\t0\t0\t0\t0\t0\tinventedOnlineSpeed\t\n"),
        &format!("effect\t{E_ONLINE_CAP}\t4\t0\t0\t0\t0\t0\t0\tinventedOnlineCap\t\n"),
        // The burst: the module's own bonus times its factor.
        &format!("modifier\t{E_BURST_SPEED}\titemID\tItemModifier\t{A_BONUS}\t{A_FACTOR}\t4\t\t\n"),
        &format!("modifier\t{E_BURST_CAP}\titemID\tItemModifier\t{A_BONUS}\t{A_FACTOR}\t4\t\t\n"),
        // The carry to the ship: a percent on the top speed, or on the
        // capacitor's recharge time (55).
        &format!("modifier\t{E_ONLINE_SPEED}\tshipID\tItemModifier\t37\t{A_BONUS}\t6\t\t\n"),
        &format!("modifier\t{E_ONLINE_CAP}\tshipID\tItemModifier\t55\t{A_BONUS}\t6\t\t\n"),
    ]
    .concat();
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: SHIP,
            ship_type_id: HULL,
            solar_system_id: Some(SYSTEM),
            ..crate::model::Character::default()
        }],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = HULL;
        i.group_id = 25;
    }
    for (n, (id, ty)) in [(M_OVERDRIVE, OVERDRIVE), (M_RECHARGER, RECHARGER)]
        .into_iter()
        .enumerate()
    {
        let t = crate::item::find_in(&content, ty).expect("a test type");
        items.push(crate::item::Item {
            item_id: id,
            type_id: ty,
            owner_id: ALICE,
            location_id: SHIP,
            flag_id: HI_SLOT_0 + n as i64,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_721_000,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let _ = drain(&mut reg);
    reg
}

fn go(reg: &mut Registry, module: i64, effect: &str, t: i64) {
    activate(reg, Some(ALICE), module, effect, None, 1000, t).expect("activates");
}

/// Every `SetMaxSpeed` Alice was sent, in order.
fn speeds(reg: &mut Registry) -> Vec<f64> {
    sent_to(&drain(reg), ALICE)
        .into_iter()
        .filter(|(n, _)| n == "SetMaxSpeed")
        .filter_map(|(_, a)| match a[1] {
            PyValue::Real(v) => Some(v),
            _ => None,
        })
        .collect()
}

fn top_speed(reg: &Registry) -> f64 {
    f64::from(crate::dogma::hull(reg, SHIP).expect("a sheet").max_velocity)
}

/// P2 / F1: the burst triples the module's 8 percent while it runs, so the
/// top speed goes 432 -> 496 -> 432, and the ball is told both ways even
/// though no propulsion effect is involved.
#[test]
fn an_active_burst_refreshes_the_balls_top_speed_and_so_does_its_end() {
    let mut reg = world();
    // Online: 400 x 1.08.
    assert!((top_speed(&reg) - 432.0).abs() < 1e-3, "{}", top_speed(&reg));
    go(&mut reg, M_OVERDRIVE, "inventedBurstSpeed", T0);
    // Active: 8 x 3 = 24 percent.
    assert!((top_speed(&reg) - 496.0).abs() < 1e-3, "{}", top_speed(&reg));
    let told = speeds(&mut reg);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!((told[0] - 496.0).abs() < 1e-3);
    // One cycle (disallowRepeatingActivation), then the end.
    tick(&mut reg, T0 + 11 * SEC);
    assert!(reg.modules().running(M_OVERDRIVE).is_none());
    let told = speeds(&mut reg);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!((told[0] - 432.0).abs() < 1e-3, "{told:?}");
}

/// An effect that does not touch the speed still sends nothing.
#[test]
fn a_burst_that_leaves_the_speed_alone_sends_no_speed() {
    let mut reg = world();
    go(&mut reg, M_RECHARGER, "inventedBurstCap", T0);
    assert!(speeds(&mut reg).is_empty());
}
