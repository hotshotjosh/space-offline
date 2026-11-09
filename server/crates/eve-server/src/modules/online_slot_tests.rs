//! #169 MOD-ONLINE-SLOT: a module stack in a slot is not a fitted module to
//! anyone, and a module put in with `fit_new` is online to godma's read and
//! to `set_online` alike. **Every id here is invented** (990,169,xxx types).

use super::*;

const ALICE: i64 = 90_169_001;
const SHIP: i64 = 100_169_001;
const HULL: i64 = 990_169_001;
const MODULE: i64 = 990_169_002;
const E_MODULE: i64 = 990_169;
const HI_SLOT_0: i64 = 27;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    // Row shapes copied from `defense_tests.rs:37-63`; the numbers are invented.
    let rows = format!(
        "{}\
         type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         48=400.0;11=100.0;14=3.0;37=300.0\tInvented Hull 169\n\
         type\t{MODULE}\t1690\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_MODULE}\t12,16,{E_MODULE}\t\
         73=5000.0;6=0.0;50=10.0;30=5.0\tInvented Module 169\n\
         effect\t{E_MODULE}\t1\t0\t73\t0\t0\t0\t0\tinventedModule169\t1\n",
        crate::typedata::FIXTURE
    );
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
            ..crate::model::Character::default()
        }],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content);
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = HULL;
        i.group_id = 25;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_169_100,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.take_pending();
    reg
}

fn raised(e: &ServiceError, key: &str) -> bool {
    format!("{e:?}").contains(&format!("{:?}", key.as_bytes().to_vec()))
}

/// The keys of a `module_info` row's `activeEffects`.
fn active(row: &PyValue) -> Vec<i64> {
    match crate::contracts::attr(row, "activeEffects") {
        Some(PyValue::Dict(d)) => d
            .iter()
            .filter_map(|(_, k)| match k {
                PyValue::Int(id) => Some(*id),
                _ => None,
            })
            .collect(),
        other => panic!("activeEffects is a dict, got {other:?}"),
    }
}

/// Before #169: `SetModuleOnline` answered `EffectAlreadyActive2` for a
/// module godma is never primed with. Now both say "not fitted".
#[test]
fn a_module_stack_in_a_slot_is_not_fitted_to_set_online_either() {
    let mut reg = world();
    let made = reg
        .apply_items(&[crate::item::Edit::Create {
            type_id: MODULE,
            quantity: 1,
            to: crate::item::Slot::new(ALICE, SHIP, HI_SLOT_0),
        }])
        .expect("made");
    let id = made
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("an id");
    assert!(
        !reg.item(id).expect("there").singleton,
        "the shape under test"
    );
    let c = reg.character(ALICE).expect("Alice").clone();
    assert!(
        ship_info_entries(&reg, &c)
            .iter()
            .all(|(_, k)| *k != PyValue::Int(id)),
        "godma is primed without it"
    );
    let now = crate::pyutil::filetime_now();
    let e = set_module_online(&mut reg, Some(ALICE), id, true, now).expect_err("refused");
    assert!(
        !raised(&e, "EffectAlreadyActive2"),
        "no claim it is online: {e:?}"
    );
    assert!(raised(&e, "CustomNotify"), "the not-fitted refusal: {e:?}");
}

/// A singleton charge shares its module's slot flag but is no module: it is
/// not fitted for online either (#178).
#[test]
fn a_singleton_charge_in_a_slot_is_not_fitted_to_set_online() {
    let mut reg = world();
    let id = fit_new(&mut reg, ALICE, SHIP, HI_SLOT_0, MODULE).expect("fitted");
    let mut items = reg.store().items().to_vec();
    items
        .iter_mut()
        .filter(|i| i.item_id == id)
        .for_each(|i| i.category_id = crate::item::CATEGORY_CHARGE);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        ..crate::store::Snapshot::default()
    });
    let item = reg.item(id).expect("there");
    assert!(item.singleton, "the shape under test");
    assert!(crate::item::is_loaded_charge(item));
    let now = crate::pyutil::filetime_now();
    let e = set_module_online(&mut reg, Some(ALICE), id, true, now).expect_err("refused");
    assert!(raised(&e, "CustomNotify"), "the not-fitted refusal: {e:?}");
}

/// `fit_new` gives the shape every production fit gives: godma's read and
/// `set_online` agree, online and then offline.
#[test]
fn a_module_from_fit_new_is_online_to_godma_and_to_set_online() {
    let mut reg = world();
    let id = fit_new(&mut reg, ALICE, SHIP, HI_SLOT_0, MODULE).expect("fitted");
    assert!(reg.item(id).expect("there").singleton);
    let c = reg.character(ALICE).expect("Alice").clone();
    let row = |reg: &Registry| module_info(reg, &c, &reg.item(id).expect("there").clone());
    assert!(
        active(&row(&reg)).contains(&EFFECT_ONLINE),
        "godma reads it online"
    );
    let now = crate::pyutil::filetime_now();
    let e = set_module_online(&mut reg, Some(ALICE), id, true, now).expect_err("already on");
    assert!(raised(&e, "EffectAlreadyActive2"), "{e:?}");
    set_module_online(&mut reg, Some(ALICE), id, false, now).expect("offlined");
    assert!(
        !active(&row(&reg)).contains(&EFFECT_ONLINE),
        "and reads it offline"
    );
}
