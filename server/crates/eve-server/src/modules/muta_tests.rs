//! MUTA-3b: the readers of a fitted module that fall back to its type read
//! the mutated item's own roll first (#71). Invented ids and rows only; the
//! rolled value is `FACTOR` times the source's, never the type row's.

use super::muta_fixture::{FACTOR, ITEM, registry};
use super::*;
use crate::item::Item;

/// An invented dynamic result type (no row of its own, so the type-level
/// attribute is absent).
const RESULT: i64 = 981_400_003;
const SRC_RELOAD: f64 = 2_000.0;
const SRC_NEED: f64 = 10.0;
const SRC_DURATION: f64 = 4_000.0;

fn module(type_id: i64) -> ModuleType {
    *find_in(&Content::fixture(), type_id).expect("a fixture module")
}

#[test]
fn a_mutated_modules_reload_time_is_its_roll() {
    let reg = registry(
        RESULT,
        None,
        &[(attr::RELOAD_TIME, SRC_RELOAD)],
        &[attr::RELOAD_TIME],
    );
    assert_eq!(
        reload_ms_of(&reg, ITEM, RESULT),
        (SRC_RELOAD * FACTOR) as i64
    );
    // A module that is not mutated keeps its type's value.
    let own = module(483).attribute(attr::RELOAD_TIME);
    assert_eq!(
        reload_ms_of(&reg, 1, 483),
        own.map_or(0, |v| v.round() as i64)
    );
}

#[test]
fn a_mutated_modules_capacitor_need_is_its_roll() {
    let reg = registry(
        RESULT,
        None,
        &[(attr::CAPACITOR_NEED, SRC_NEED)],
        &[attr::CAPACITOR_NEED],
    );
    // A ship id with no sheet: the no-sheet path.
    assert_eq!(capacitor_need(&reg, 0, ITEM, RESULT), SRC_NEED * FACTOR);
}

#[test]
fn a_mutated_modules_cycle_time_is_its_roll() {
    let reg = registry(
        RESULT,
        None,
        &[(attr::DURATION, SRC_DURATION)],
        &[attr::DURATION],
    );
    let t = ModuleType {
        type_id: RESULT,
        effect: Some(Effect {
            duration_attribute: attr::DURATION,
            ..module(483).effect.expect("an effect")
        }),
        attributes: &[(attr::DURATION, 1_000.0)],
        ..module(483)
    };
    assert_eq!(t.duration_ms(), 1_000, "the type row's own cycle");
    assert_eq!(
        duration_ms(&reg, 0, ITEM, &t),
        (SRC_DURATION * FACTOR) as i64
    );
}

#[test]
fn a_mutated_modules_repeat_limit_is_its_roll() {
    let reg = registry(
        RESULT,
        None,
        &[(ATTR_DISALLOW_REPEATING, 1.0)],
        &[ATTR_DISALLOW_REPEATING],
    );
    // The type row allows repeats; the roll does not.
    let t = ModuleType {
        type_id: RESULT,
        attributes: &[(ATTR_DISALLOW_REPEATING, 0.0)],
        ..module(483)
    };
    assert_eq!(repeat_of(&reg, ITEM, &t, 1000), 1);
    let plain = ModuleType {
        attributes: &[(ATTR_DISALLOW_REPEATING, 0.0)],
        ..module(483)
    };
    assert_eq!(
        repeat_of(&reg, 1, &plain, 1000),
        1000,
        "an unmutated module"
    );
}

/// The no-sheet online check reads the roll for the module being onlined and
/// for each online module already fitted. The result type is a fixture
/// miner's, so `find_in` has a row for it; its own cpu is small.
#[test]
fn the_no_sheet_online_check_reads_the_roll() {
    let out = crate::fitting::hull_in(&Content::fixture(), 603)
        .expect("Merlin")
        .cpu_output;
    let src = [(attr::CPU, out)];
    // Onlining the mutated miner: the rolled cpu is FACTOR x the output.
    let reg = registry(483, None, &src, &[attr::CPU]);
    assert!(module(483).cpu() < out, "the type row alone fits");
    assert!(budget(&reg, 0, 603, ITEM, 483).is_err());
    // Onlining a plain extender beside an online mutated miner.
    let mut reg = registry(483, None, &src, &[attr::CPU]);
    let mut items = reg.store().items().to_vec();
    items.push(Item {
        item_id: ITEM,
        type_id: 483,
        owner_id: 1,
        location_id: 9_999,
        flag_id: 27,
        quantity: 1,
        group_id: 54,
        category_id: 7,
        singleton: true,
    });
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: ITEM + 100,
        ..crate::store::Snapshot::default()
    });
    reg.modules_mut().set_online(ITEM, true);
    assert!(budget(&reg, 9_999, 603, ITEM + 1, 377).is_err());
    reg.modules_mut().set_online(ITEM, false);
    assert!(budget(&reg, 9_999, 603, ITEM + 1, 377).is_ok());
}

/// The no-sheet type fallbacks of `module_attr`, `spool_pair` and
/// `chargeRate` read the mutated item's own set first.
#[test]
fn a_mutated_modules_other_no_sheet_reads_are_its_roll() {
    use crate::pochven::spool::{ATTR_MAX, ATTR_PER_CYCLE};
    const MAX_GROUP_ACTIVE: i64 = 763;
    let reg = registry(
        RESULT,
        None,
        &[
            (MAX_GROUP_ACTIVE, 2.0),
            (ATTR_PER_CYCLE, 2.0),
            (ATTR_MAX, 5.0),
            (attr::CHARGE_RATE, 2.0),
        ],
        &[
            MAX_GROUP_ACTIVE,
            ATTR_PER_CYCLE,
            ATTR_MAX,
            attr::CHARGE_RATE,
        ],
    );
    assert_eq!(
        module_attr(&reg, 0, ITEM, RESULT, MAX_GROUP_ACTIVE),
        Some(2.0 * FACTOR)
    );
    let t = ModuleType {
        type_id: RESULT,
        attributes: &[],
        ..module(483)
    };
    assert_eq!(charge_rate_of(&reg, ITEM, &t), Some(2.0 * FACTOR));
    let r = Running {
        character_id: 0,
        ship_id: 0,
        module_id: ITEM,
        type_id: RESULT,
        effect: t.effect.expect("an effect"),
        target_id: None,
        started: 0,
        period: 1,
        repeat: 1,
        cycles: 1,
        stop_at: None,
    };
    assert_eq!(spool_pair(&reg, &r), (2.0 * FACTOR, 5.0 * FACTOR));
}
