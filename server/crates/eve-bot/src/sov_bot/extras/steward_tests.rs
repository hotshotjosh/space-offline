//! Tests of the steward: the plan, the claim board and the decoders. Ids are
//! invented; no `[A]` constant appears as an expected number (the days and
//! the minimum are named through their helpers).

use super::gw::{put_bytes, put_uint};
use super::steward::*;
use crate::clock;
use eve_marshal::PyValue;
use std::time::Duration;

const SYSTEM_HUB: i64 = 8_922_100;
const POWER_PLANET: i64 = 40_922_101;
const LAND_PLANET: i64 = 40_922_102;
const REAGENT_PLANET: i64 = 40_922_103;
const REAGENT: i64 = 990_221_101;
const SKYHOOK_TYPE: i64 = 990_221_001;
const CARGO_SKYHOOK: i64 = 100_922_001;
const HOOK_A: i64 = 8_922_201;
const HOOK_B: i64 = 8_922_202;

fn defs() -> Vec<PlanetDef> {
    vec![
        PlanetDef {
            planet: POWER_PLANET,
            power: 3,
            workforce: 0,
            reagent: None,
        },
        PlanetDef {
            planet: LAND_PLANET,
            power: 0,
            workforce: 2,
            reagent: None,
        },
        PlanetDef {
            planet: REAGENT_PLANET,
            power: 0,
            workforce: 0,
            reagent: Some(REAGENT),
        },
    ]
}

fn hook(id: i64, planet: i64, stock: u64) -> HookView {
    HookView {
        id,
        planet: Some(planet),
        active: true,
        reagent: Some(REAGENT),
        secured: 0,
        unsecured: stock,
        unsecured_cap: Some(400),
    }
}

/// A view with every planet covered, a roomy hold and the role.
struct Fixture {
    planets: Vec<PlanetDef>,
    skipped: Vec<i64>,
    hooks: Vec<HookView>,
    building: Vec<i64>,
    bay: Vec<(i64, u64, u64)>,
    kit_cargo: Option<i64>,
    kit_hangar: bool,
    ask_known: bool,
    cargo_reagents: Vec<(i64, i64)>,
    may_manage: bool,
    hold: f64,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            planets: defs(),
            skipped: vec![],
            hooks: vec![
                hook(HOOK_A, POWER_PLANET, 0),
                hook(HOOK_B, LAND_PLANET, 0),
                hook(8_922_203, REAGENT_PLANET, 0),
            ],
            building: vec![],
            bay: vec![],
            kit_cargo: None,
            kit_hangar: false,
            ask_known: false,
            cargo_reagents: vec![],
            may_manage: true,
            hold: 10_000.0,
        }
    }

    fn plan(&self) -> Vec<StewardAct> {
        steward_plan(&StewardView {
            planets: &self.planets,
            skipped: &self.skipped,
            hooks: &self.hooks,
            building: &self.building,
            bay: &self.bay,
            kit_cargo: self.kit_cargo,
            kit_hangar: self.kit_hangar,
            ask_known: self.ask_known,
            skyhook_type: Some(SKYHOOK_TYPE),
            cargo_reagents: &self.cargo_reagents,
            may_manage: self.may_manage,
            hold_free_m3: self.hold,
            item_m3: &|_| 1.0,
        })
    }

    /// The reagent skyhook (the third), with `stock` units.
    fn stocked(mut self, stock: u64) -> Fixture {
        self.hooks[2].unsecured = stock;
        self
    }
}

#[test]
fn a_yield_planet_without_a_skyhook_wants_one() {
    let mut f = Fixture::new();
    f.hooks.remove(0);
    assert_eq!(f.plan(), vec![StewardAct::Want(SKYHOOK_TYPE)]);
    // A skyhook on sale in reach, or in the hangar, is fetched instead.
    f.ask_known = true;
    assert_eq!(
        f.plan(),
        vec![StewardAct::Fetch {
            item_type: SKYHOOK_TYPE
        }]
    );
    f.ask_known = false;
    f.kit_hangar = true;
    assert_eq!(
        f.plan(),
        vec![StewardAct::Fetch {
            item_type: SKYHOOK_TYPE
        }]
    );
}

#[test]
fn a_skyhook_in_the_hold_is_launched_over_the_planet_that_lacks_one() {
    let mut f = Fixture::new();
    f.hooks.clear();
    f.kit_cargo = Some(CARGO_SKYHOOK);
    let acts = f.plan();
    let launches: Vec<_> = acts
        .iter()
        .filter(|a| matches!(a, StewardAct::Launch { .. }))
        .collect();
    assert_eq!(
        launches,
        vec![&StewardAct::Launch {
            item: CARGO_SKYHOOK,
            planet: POWER_PLANET
        }],
        "one launch per pass, over the first planet that lacks one"
    );
    // The planet that has one is not the one.
    f.hooks.push(hook(HOOK_A, POWER_PLANET, 0));
    assert_eq!(
        f.plan(),
        vec![StewardAct::Launch {
            item: CARGO_SKYHOOK,
            planet: LAND_PLANET
        }]
    );
}

#[test]
fn a_planet_with_another_corps_skyhook_is_left_alone() {
    let mut f = Fixture::new();
    f.hooks.clear();
    f.skipped = vec![POWER_PLANET, LAND_PLANET];
    f.kit_cargo = Some(CARGO_SKYHOOK);
    assert_eq!(
        f.plan(),
        vec![StewardAct::Launch {
            item: CARGO_SKYHOOK,
            planet: REAGENT_PLANET
        }]
    );
    f.skipped.push(REAGENT_PLANET);
    assert_eq!(f.plan(), vec![]);
}

#[test]
fn an_own_skyhook_of_unknown_planet_stands_for_one_planet() {
    let mut f = Fixture::new();
    f.hooks = vec![HookView {
        planet: None,
        ..hook(HOOK_A, 0, 0)
    }];
    f.kit_cargo = Some(CARGO_SKYHOOK);
    // Three yield planets, one unplaced hook: two are still lacking.
    assert_eq!(
        f.plan(),
        vec![StewardAct::Launch {
            item: CARGO_SKYHOOK,
            planet: POWER_PLANET
        }]
    );
    f.hooks.extend([
        HookView {
            planet: None,
            ..hook(HOOK_B, 0, 0)
        },
        HookView {
            planet: None,
            ..hook(8_922_203, 0, 0)
        },
    ]);
    assert_eq!(f.plan(), vec![], "every planet is covered");
}

#[test]
fn without_the_role_nothing_is_launched_or_switched_but_reagents_still_move() {
    let mut f = Fixture::new().stocked(1_000);
    f.may_manage = false;
    f.hooks[0].active = false;
    f.hooks.remove(1);
    f.kit_cargo = Some(CARGO_SKYHOOK);
    let acts = f.plan();
    assert!(
        acts.iter().all(|a| !matches!(
            a,
            StewardAct::Launch { .. }
                | StewardAct::Activate { .. }
                | StewardAct::Want(_)
                | StewardAct::Fetch { .. }
        )),
        "{acts:?}"
    );
    assert!(
        acts.iter().any(|a| matches!(a, StewardAct::Extract { .. })),
        "{acts:?}"
    );
}

#[test]
fn an_inactive_built_skyhook_is_switched_on_but_a_building_one_is_not() {
    let mut f = Fixture::new();
    f.hooks[0].active = false;
    f.hooks[1].active = false;
    f.building = vec![HOOK_B];
    assert_eq!(f.plan(), vec![StewardAct::Activate { skyhook: HOOK_A }]);
}

#[test]
fn reagents_below_the_steward_days_are_extracted_and_above_them_are_not() {
    let per_hour = 5;
    let need = steward_days_units(per_hour);
    let mut f = Fixture::new().stocked(1_000);
    f.bay = vec![(REAGENT, need - 1, per_hour)];
    assert!(
        f.plan()
            .iter()
            .any(|a| matches!(a, StewardAct::Extract { skyhook, .. } if *skyhook == 8_922_203)),
        "below the days: extract"
    );
    f.bay = vec![(REAGENT, need, per_hour)];
    assert_eq!(f.plan(), vec![], "at the days: leave it");
    // Nothing burns it yet: the idle stock decides.
    f.bay = vec![(REAGENT, IDLE_STOCK - 1, 0)];
    assert!(
        f.plan()
            .iter()
            .any(|a| matches!(a, StewardAct::Extract { .. }))
    );
    f.bay = vec![(REAGENT, IDLE_STOCK, 0)];
    assert_eq!(f.plan(), vec![]);
}

#[test]
fn a_skyhook_below_the_haul_minimum_is_not_worth_the_trip() {
    let mut f = Fixture::new();
    let min = haul_min(&f.hooks[2]);
    f.hooks[2].unsecured = min - 1;
    assert_eq!(f.plan(), vec![]);
    f.hooks[2].unsecured = min;
    assert!(
        f.plan()
            .iter()
            .any(|a| matches!(a, StewardAct::Extract { .. }))
    );
}

#[test]
fn reagent_cargo_is_deposited_first() {
    let mut f = Fixture::new().stocked(1_000);
    f.cargo_reagents = vec![(REAGENT, 30)];
    let acts = f.plan();
    assert_eq!(
        acts,
        vec![StewardAct::Deposit {
            reagent: REAGENT,
            qty: 30
        }],
        "the same reagent is not taken again until the hold is empty"
    );
    // Another reagent in the hold: deposit before extract.
    f.cargo_reagents = vec![(REAGENT + 1, 30)];
    let acts = f.plan();
    let dep = acts
        .iter()
        .position(|a| matches!(a, StewardAct::Deposit { .. }));
    let ext = acts
        .iter()
        .position(|a| matches!(a, StewardAct::Extract { .. }));
    assert!(dep.is_some() && ext.is_some() && dep < ext, "{acts:?}");
}

#[test]
fn the_extract_is_clipped_to_the_free_hold() {
    let mut f = Fixture::new().stocked(1_000);
    f.hold = 40.0;
    assert_eq!(
        f.plan(),
        vec![StewardAct::Extract {
            skyhook: 8_922_203,
            qty: 40
        }]
    );
    f.hold = 0.0;
    assert_eq!(f.plan(), vec![], "no room, no trip");
}

#[test]
fn one_steward_per_hub() {
    let now = clock::instant();
    let hub = SYSTEM_HUB + 1;
    assert!(claim(hub, 1, now), "the first takes it");
    assert!(
        !claim(hub, 2, now + Duration::from_secs(1)),
        "a second waits"
    );
    // The holder's renewal keeps it past the first TTL.
    let renewed = now + STEWARD_TTL - Duration::from_secs(1);
    assert!(claim(hub, 1, renewed));
    assert!(!claim(hub, 2, now + STEWARD_TTL + Duration::from_secs(1)));
    // Unrenewed, it lapses.
    assert!(claim(hub, 2, renewed + STEWARD_TTL));
    // Another hub is its own.
    assert!(claim(hub + 1, 1, now));
}

// ---- the decoders, against hand-built bytes in the server's layouts ------------------

fn ident(id: u64) -> Vec<u8> {
    super::gw::id(id)
}

#[test]
fn planet_definitions_decode_with_an_absent_reagent_and_elided_zeros() {
    let mut payload = Vec::new();
    // A power planet: no workforce (elided zero), no reagent.
    let mut d = Vec::new();
    put_bytes(&mut d, 1, &ident(POWER_PLANET as u64));
    put_uint(&mut d, 2, 3);
    put_bytes(&mut payload, 1, &d);
    // A reagent planet.
    let mut d = Vec::new();
    put_bytes(&mut d, 1, &ident(REAGENT_PLANET as u64));
    let mut r = Vec::new();
    put_bytes(&mut r, 2, &[]);
    put_bytes(&mut r, 3, &ident(REAGENT as u64));
    put_bytes(&mut d, 4, &r);
    put_bytes(&mut payload, 1, &d);
    // The version trailer is another field and is ignored.
    put_bytes(&mut payload, 2, &[8, 1]);
    assert_eq!(
        decode_defs(&payload),
        vec![
            PlanetDef {
                planet: POWER_PLANET,
                power: 3,
                workforce: 0,
                reagent: None
            },
            PlanetDef {
                planet: REAGENT_PLANET,
                power: 0,
                workforce: 0,
                reagent: Some(REAGENT)
            },
        ]
    );
}

#[test]
fn a_by_corp_record_decodes_stock_activity_and_cap_and_tolerates_none() {
    let mut rec = Vec::new();
    put_bytes(&mut rec, 1, &ident(HOOK_A as u64));
    let mut sim = Vec::new();
    put_uint(&mut sim, 1, 4);
    put_uint(&mut sim, 2, 9);
    let mut e = Vec::new();
    put_bytes(&mut e, 1, &ident(REAGENT as u64));
    put_bytes(&mut e, 4, &sim);
    put_bytes(&mut rec, 2, &e);
    put_uint(&mut rec, 3, 1);
    let mut def = Vec::new();
    put_uint(&mut def, 5, 80);
    let mut de = Vec::new();
    put_bytes(&mut de, 1, &ident(REAGENT as u64));
    put_bytes(&mut de, 2, &def);
    put_bytes(&mut rec, 4, &de);
    let mut payload = Vec::new();
    put_bytes(&mut payload, 1, &rec);
    // A skyhook with no reagent and inactive (the elided false).
    let mut bare = Vec::new();
    put_bytes(&mut bare, 1, &ident(HOOK_B as u64));
    put_bytes(&mut payload, 1, &bare);
    let got = decode_hooks(&payload);
    assert_eq!(
        got,
        vec![
            HookView {
                id: HOOK_A,
                planet: None,
                active: true,
                reagent: Some(REAGENT),
                secured: 4,
                unsecured: 9,
                unsecured_cap: Some(80),
            },
            HookView {
                id: HOOK_B,
                planet: None,
                active: false,
                reagent: None,
                secured: 0,
                unsecured: 0,
                unsecured_cap: None,
            },
        ]
    );
    assert_eq!(got[0].stock(), 13);
}

#[test]
fn the_bay_and_the_add_body_use_the_servers_layouts() {
    let mut payload = Vec::new();
    let mut e = Vec::new();
    put_bytes(&mut e, 1, &ident(REAGENT as u64));
    put_uint(&mut e, 2, 70);
    put_uint(&mut e, 3, 2);
    put_bytes(&mut payload, 1, &e);
    // An empty entry reads zeros; the stamp (field 2) is not an entry.
    let mut idle = Vec::new();
    put_bytes(&mut idle, 1, &ident(REAGENT as u64 + 1));
    put_bytes(&mut payload, 1, &idle);
    put_bytes(&mut payload, 2, &[8, 1]);
    assert_eq!(
        decode_bay(&payload),
        vec![(REAGENT, 70, 2), (REAGENT + 1, 0, 0)]
    );
    let body = add_body(CARGO_SKYHOOK, 12, SYSTEM_HUB);
    assert_eq!(super::gw::read_id(&body, 1), Some(CARGO_SKYHOOK as u64));
    assert_eq!(super::gw::uint(&body, 2), Some(12));
    assert_eq!(super::gw::read_id(&body, 3), Some(SYSTEM_HUB as u64));
    assert_eq!(super::gw::read_id(&id_body(HOOK_A), 1), Some(HOOK_A as u64));
}

#[test]
fn a_launch_refusal_reads_as_its_notify_text() {
    let refusal = |text: &str| {
        PyValue::Tuple(vec![
            PyValue::Bytes(b"CustomNotify".to_vec()),
            PyValue::Dict(vec![(
                PyValue::Bytes(text.as_bytes().to_vec()),
                PyValue::Bytes(b"notify".to_vec()),
            )]),
        ])
    };
    let answer = PyValue::Dict(vec![(
        PyValue::List(vec![refusal("no role here")]),
        PyValue::Int(CARGO_SKYHOOK),
    )]);
    assert_eq!(
        super::gw::launch_result(&answer, CARGO_SKYHOOK),
        Err("no role here".to_owned())
    );
    let ok = PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Int(8_922_300)]),
        PyValue::Int(CARGO_SKYHOOK),
    )]);
    assert_eq!(super::gw::launch_result(&ok, CARGO_SKYHOOK), Ok(8_922_300));
}
