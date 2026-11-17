//! Tests of the steward's upgrades: the plan over the client's power rule and
//! the wire codecs. Ids and numbers are invented; no `[A]` constant appears as
//! an expected number.

use super::gw::{self, put_bytes, put_uint};
use super::upgrades::*;
use crate::sov_bot::{Duty, Plan};
use eve_server::sov_upgrade::{Def, Power, Slot};
use std::collections::BTreeMap;

const HUB: i64 = 8_922_100;
const SYSTEM: i64 = 30_922_100;
const REAGENT: i64 = 990_221_101;
/// Needs power 5, makes none.
const SMALL: i64 = 990_221_201;
/// Makes power, needs none.
const MAKER: i64 = 990_221_202;
/// Needs power 5 and burns the reagent (startup 3, one a unit of time).
const BURNER: i64 = 990_221_203;
/// Shares `SMALL`'s exclusive group.
const TWIN: i64 = 990_221_204;
/// Burns the reagent too.
const BURNER2: i64 = 990_221_205;

fn def(power: u64, fuel: Option<i64>, per_hour: u32, startup: u32, power_out: u64) -> Def {
    Def {
        power,
        workforce: 0,
        fuel,
        per_hour,
        startup,
        power_out,
        workforce_out: 0,
    }
}

fn defs() -> BTreeMap<i64, Defn> {
    let d = |def: Def, group: &str| Defn {
        def,
        group: group.to_owned(),
    };
    BTreeMap::from([
        (SMALL, d(def(5, None, 0, 0, 0), "ga")),
        (MAKER, d(def(0, None, 0, 0, 9), "gb")),
        (BURNER, d(def(5, Some(REAGENT), 2, 3, 0), "gc")),
        (TWIN, d(def(5, None, 0, 0, 0), "ga")),
        (BURNER2, d(def(0, Some(REAGENT), 2, 3, 0), "gd")),
    ])
}

fn online(type_id: i64) -> Slot {
    Slot {
        type_id,
        online: true,
        prev: Some(Power::Online),
        new: false,
    }
}

fn next(installed: &[Slot], power: u64, stock: u64, candidates: &[i64], held: &[i64]) -> Want {
    let d = defs();
    next_upgrade(
        installed,
        &|t| d.get(&t).map(|x| x.def),
        power,
        0,
        &|r| if r == REAGENT { stock } else { 0 },
        candidates,
        held,
    )
}

#[test]
fn an_upgrade_the_rule_would_leave_low_is_not_wanted() {
    let need = defs()[&SMALL].def.power;
    assert_eq!(next(&[], need - 1, 0, &[SMALL], &[SMALL]), Want::None);
    assert_eq!(next(&[], need, 0, &[SMALL], &[SMALL]), Want::Install(SMALL));
}

#[test]
fn a_producer_comes_before_a_consumer() {
    let d = defs();
    // The consumer has the lower id; the producer is still first.
    assert_eq!(
        preference(&d, &[]),
        vec![MAKER, SMALL, BURNER, TWIN, BURNER2]
    );
    // With no power at all only the producer can come online.
    let order = preference(&d, &[]);
    assert_eq!(
        next(&[], 0, 0, &order, &[MAKER, SMALL]),
        Want::Install(MAKER)
    );
    // The producer's output then carries the consumer, installed after it.
    let out = d[&MAKER].def.power_out;
    assert!(out >= d[&SMALL].def.power, "the invented rows fit");
    assert_eq!(
        next(&[online(MAKER)], 0, 0, &[SMALL], &[SMALL]),
        Want::Install(SMALL)
    );
}

#[test]
fn a_larger_producer_comes_first_and_ties_go_by_type_id() {
    let mut d = defs();
    d.insert(
        TWIN,
        Defn {
            def: def(0, None, 0, 0, 20),
            group: "ga".to_owned(),
        },
    );
    assert_eq!(preference(&d, &[])[0], TWIN);
}

#[test]
fn an_upgrade_that_would_knock_an_online_one_low_is_not_wanted() {
    let d = defs();
    let burn = d[&BURNER].def;
    let b2 = d[&BURNER2].def;
    // The burner is installed and Online; the stock is what its burn needs
    // plus the second burner's startup and burn, less one.
    let installed = [online(BURNER)];
    let stock =
        u64::from(burn.per_hour > 0) + u64::from(b2.startup) + u64::from(b2.per_hour > 0) - 1;
    let candidates = [BURNER2];
    assert_eq!(
        next(&installed, burn.power, stock, &candidates, &[BURNER2]),
        Want::None,
        "the shared reagent stock does not carry both"
    );
    assert_eq!(
        next(&installed, burn.power, stock + 1, &candidates, &[BURNER2]),
        Want::Install(BURNER2)
    );
}

#[test]
fn an_upgrade_with_its_startup_and_one_unit_in_stock_is_wanted() {
    let d = defs();
    let startup = u64::from(d[&BURNER2].def.startup);
    assert_eq!(
        next(&[], 0, startup, &[BURNER2], &[BURNER2]),
        Want::None,
        "the startup alone leaves nothing to burn"
    );
    assert_eq!(
        next(&[], 0, startup + 1, &[BURNER2], &[]),
        Want::Buy(BURNER2),
        "the startup and one unit; not held, so bought"
    );
}

#[test]
fn an_installed_type_or_its_exclusive_group_is_never_wanted_again() {
    let d = defs();
    let order = preference(&d, &[online(SMALL)]);
    assert!(!order.contains(&SMALL), "the installed type");
    assert!(!order.contains(&TWIN), "its exclusive group");
    assert!(order.contains(&MAKER));
    // And the plan itself never picks an installed type, whatever it is told.
    assert_eq!(
        next(&[online(SMALL)], 99, 0, &[SMALL], &[SMALL]),
        Want::None
    );
}

#[test]
fn nothing_is_wanted_without_a_hub() {
    let plan = Plan::default();
    assert_eq!(hub_of(&plan, Duty::Haul(HUB)), None);
    let plan = Plan {
        fuel: vec![(SYSTEM, HUB)],
        ..Plan::default()
    };
    assert_eq!(hub_of(&plan, Duty::Haul(HUB)), Some((SYSTEM, HUB)));
    assert_eq!(hub_of(&plan, Duty::Haul(HUB + 1)), None, "not this hub");
    assert_eq!(hub_of(&plan, Duty::Idle), None, "not a hauler");
}

fn ident(id: u64) -> Vec<u8> {
    gw::id(id)
}

#[test]
fn the_configuration_names_installed_plus_new_in_order() {
    let body = configuration_body(100_922_001, &[(SMALL, true), (MAKER, false)], BURNER, HUB);
    let items = gw::fields(&body, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(gw::uint(items[0], 1), Some(100_922_001), "the item id");
    let config: Vec<(u64, bool)> = gw::fields(&body, 2)
        .into_iter()
        .map(|e| {
            (
                gw::read_id(e, 1).expect("a type"),
                gw::uint(e, 2).unwrap_or(0) != 0,
            )
        })
        .collect();
    assert_eq!(
        config,
        vec![
            (SMALL as u64, true),
            (MAKER as u64, false),
            (BURNER as u64, true)
        ],
        "installed in their order, the new one last and online"
    );
    assert_eq!(gw::read_id(&body, 3), Some(HUB as u64));
}

/// One `Definition` as the server writes it (`definition_bytes`).
fn definition(type_id: i64, d: &Defn) -> Vec<u8> {
    let mut b = Vec::new();
    put_bytes(&mut b, 1, &ident(type_id as u64));
    put_uint(&mut b, 2, d.def.power);
    put_uint(&mut b, 3, d.def.workforce);
    if let Some(f) = d.def.fuel {
        put_bytes(&mut b, 4, &ident(f as u64));
    }
    put_uint(&mut b, 5, u64::from(d.def.per_hour));
    put_uint(&mut b, 6, u64::from(d.def.startup));
    put_bytes(&mut b, 7, d.group.as_bytes());
    put_uint(&mut b, 8, d.def.power_out);
    put_uint(&mut b, 9, d.def.workforce_out);
    b
}

#[test]
fn definitions_decode_from_the_servers_layout() {
    let d = defs();
    let mut payload = Vec::new();
    for (t, x) in &d {
        put_bytes(&mut payload, 1, &definition(*t, x));
    }
    assert_eq!(decode_definitions(&payload), d);
    assert!(decode_definitions(&[]).is_empty());
}

fn hub_list(rows: &[(i64, u64)]) -> Vec<u8> {
    let mut hub = Vec::new();
    put_bytes(&mut hub, 1, &ident(HUB as u64));
    for (t, state) in rows {
        let mut id = Vec::new();
        put_bytes(&mut id, 1, &ident(HUB as u64));
        put_bytes(&mut id, 2, &ident(*t as u64));
        put_bytes(&mut id, 3, &ident(*t as u64));
        let mut attrs = Vec::new();
        put_bytes(&mut attrs, 1, &definition(*t, &defs()[t]));
        put_uint(&mut attrs, 2, *state);
        let mut up = Vec::new();
        put_bytes(&mut up, 1, &id);
        put_bytes(&mut up, 2, &attrs);
        put_bytes(&mut hub, 2, &up);
    }
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &hub);
    out
}

#[test]
fn the_hub_list_decodes_in_configured_order_with_its_states() {
    let payload = hub_list(&[(BURNER, 3), (MAKER, 2), (SMALL, 1), (TWIN, 4)]);
    assert_eq!(
        decode_hub_upgrades(&payload),
        vec![
            (BURNER, Power::Low),
            (MAKER, Power::Online),
            (SMALL, Power::Offline),
            (TWIN, Power::Pending)
        ]
    );
    assert!(decode_hub_upgrades(&[]).is_empty());
    let slots = slots(&decode_hub_upgrades(&payload));
    assert_eq!(
        slots.iter().map(|s| s.online).collect::<Vec<_>>(),
        vec![true, true, false, true],
        "only an Offline slot is configured off"
    );
    assert!(slots.iter().all(|s| !s.new && s.prev.is_some()));
}

#[test]
fn resources_decode_and_the_dens_share_comes_off_the_workforce() {
    let triple = |a: u64, b: u64| {
        let mut t = Vec::new();
        put_uint(&mut t, 1, a);
        put_uint(&mut t, 2, b);
        t
    };
    let mut payload = Vec::new();
    put_bytes(&mut payload, 1, &triple(30, 4));
    put_bytes(&mut payload, 2, &triple(20, 7));
    assert_eq!(decode_resources(&payload), Some((30, 20, 7)));
    assert_eq!(decode_resources(&[]), None);
    // A workforce-using upgrade is Online (it accounts for part of the
    // allocation); the rest is the dens'.
    let mut d = defs();
    d.get_mut(&SMALL).expect("a row").def.workforce = 4;
    let installed = [(SMALL, Power::Online), (MAKER, Power::Low)];
    assert_eq!(budget((30, 20), 7, &installed, &d), (30, 17));
    assert_eq!(budget((30, 2), 7, &installed, &d), (30, 0), "never below 0");
}
