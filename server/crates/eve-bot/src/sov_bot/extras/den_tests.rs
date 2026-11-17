//! Tests of the den runner: the plan, the decoders and the site pick. Ids are
//! invented; no `[A]` constant appears as an expected number (the threshold
//! is named through [`extract_at`]).

use super::den::*;
use super::gw::{put_bytes, put_uint};
use crate::site_runner::EscalationRow;

const SYSTEM: i64 = 30_922_001;
const OTHER_SYSTEM: i64 = 30_922_002;
const DEN_TYPE: i64 = 990_221_301;
const INFOMORPH: i64 = 990_221_302;
const DEN_A: i64 = 8_922_401;
const DEN_B: i64 = 8_922_402;
const HOOK_WORK: i64 = 8_922_411;
const HOOK_WORK2: i64 = 8_922_412;
const HOOK_REAGENT: i64 = 8_922_413;
const ITEM: i64 = 100_922_301;
const ROOM: i64 = 100;
const ACT_A: [u8; 16] = [0x22; 16];
const ACT_B: [u8; 16] = [0x23; 16];

fn work(id: i64) -> HookRow {
    HookRow {
        id,
        active: true,
        workforce: true,
        reagent: false,
    }
}

fn reagent_hook(id: i64) -> HookRow {
    HookRow {
        id,
        active: true,
        workforce: false,
        reagent: true,
    }
}

fn den(id: i64, skyhook: i64, infomorphs: u64) -> DenRow {
    DenRow {
        id,
        skyhook,
        system: SYSTEM,
        infomorphs,
    }
}

fn act(id: [u8; 16], den: i64, started: bool) -> ActRow {
    ActRow {
        id,
        den,
        started,
        expiry: 2_000_000_000,
        system: SYSTEM,
    }
}

/// A docked pilot with guns, the skill, a den item and a free workforce
/// skyhook; each test changes what it is about.
struct Fixture {
    max_dens: u64,
    dens: Vec<DenRow>,
    activities: Vec<ActRow>,
    capacity: u64,
    hooks: Vec<HookRow>,
    held: Vec<i64>,
    hold_room: i64,
    infomorphs_held: i64,
    den_item: Option<i64>,
    guns: bool,
    docked: bool,
    refused: Vec<i64>,
    quiet: Vec<[u8; 16]>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            max_dens: 2,
            dens: Vec::new(),
            activities: Vec::new(),
            capacity: 2,
            hooks: vec![work(HOOK_WORK)],
            held: vec![SYSTEM],
            hold_room: ROOM,
            infomorphs_held: 0,
            den_item: Some(ITEM),
            guns: true,
            docked: true,
            refused: Vec::new(),
            quiet: Vec::new(),
        }
    }

    fn plan(&self) -> DenAct {
        den_plan(&DenView {
            max_dens: self.max_dens,
            dens: &self.dens,
            activities: &self.activities,
            capacity: self.capacity,
            hooks: &self.hooks,
            system: SYSTEM,
            held: &self.held,
            hold_room: self.hold_room,
            infomorph: Some(INFOMORPH),
            infomorphs_held: self.infomorphs_held,
            den_item: self.den_item,
            den_item_type: self.den_item.map(|_| DEN_TYPE),
            den_type: Some(DEN_TYPE),
            guns: self.guns,
            docked: self.docked,
            refused: &self.refused,
            quiet: &self.quiet,
        })
    }
}

#[test]
fn a_free_workforce_skyhook_gets_the_den_item_launched() {
    let f = Fixture::new();
    assert_eq!(
        f.plan(),
        DenAct::Launch {
            item: ITEM,
            skyhook: HOOK_WORK
        }
    );
}

#[test]
fn never_launches_without_an_own_skyhook_over_a_workforce_planet() {
    let mut f = Fixture::new();
    f.hooks = vec![reagent_hook(HOOK_REAGENT)];
    assert_eq!(f.plan(), DenAct::None);
    // A workforce skyhook that is switched off is no place either.
    f.hooks = vec![HookRow {
        active: false,
        ..work(HOOK_WORK)
    }];
    assert_eq!(f.plan(), DenAct::None);
    // Nor one that has a den already.
    f.hooks = vec![work(HOOK_WORK)];
    f.dens = vec![den(DEN_A, HOOK_WORK, 0)];
    assert_eq!(f.plan(), DenAct::None);
    // A refused skyhook is left alone; the next one is used.
    f.dens.clear();
    f.hooks = vec![work(HOOK_WORK), work(HOOK_WORK2)];
    f.refused = vec![HOOK_WORK];
    assert_eq!(
        f.plan(),
        DenAct::Launch {
            item: ITEM,
            skyhook: HOOK_WORK2
        }
    );
}

#[test]
fn no_den_item_means_a_want_when_docked_and_nothing_in_space() {
    let mut f = Fixture::new();
    f.den_item = None;
    assert_eq!(f.plan(), DenAct::Want(DEN_TYPE));
    f.docked = false;
    assert_eq!(f.plan(), DenAct::None);
    // A den type refused lately is not wanted again at once.
    f.docked = true;
    f.refused = vec![DEN_TYPE];
    assert_eq!(f.plan(), DenAct::None);
}

#[test]
fn at_the_maximum_no_den_is_wanted() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.max_dens = 1;
    f.dens = vec![den(DEN_A, HOOK_WORK2, 0)];
    assert_eq!(f.plan(), DenAct::None);
    f.den_item = Some(ITEM);
    assert_eq!(f.plan(), DenAct::None);
}

#[test]
fn a_pilot_without_the_skill_does_nothing() {
    let mut f = Fixture::new();
    f.max_dens = 0;
    assert_eq!(f.plan(), DenAct::None);
}

#[test]
fn infomorphs_below_the_threshold_stay_in_the_den() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.docked = false;
    f.dens = vec![den(DEN_A, HOOK_WORK, extract_at(ROOM) - 1)];
    f.hooks.clear();
    assert_eq!(f.plan(), DenAct::None);
    f.dens = vec![den(DEN_A, HOOK_WORK, extract_at(ROOM))];
    assert_eq!(
        f.plan(),
        DenAct::Extract {
            den: DEN_A,
            qty: extract_at(ROOM) as i64
        }
    );
}

#[test]
fn an_extraction_is_clipped_to_the_hold_and_needs_room() {
    let mut f = Fixture::new();
    f.hold_room = 10;
    f.dens = vec![den(DEN_A, HOOK_WORK, 1_000)];
    assert_eq!(
        f.plan(),
        DenAct::Extract {
            den: DEN_A,
            qty: 10
        }
    );
    f.hold_room = 0;
    assert_ne!(f.plan(), DenAct::Extract { den: DEN_A, qty: 0 });
    // A den in another system, or one refused lately, is not flown to.
    f.hold_room = ROOM;
    f.dens = vec![DenRow {
        system: OTHER_SYSTEM,
        ..den(DEN_A, HOOK_WORK, 1_000)
    }];
    assert!(!matches!(f.plan(), DenAct::Extract { .. }));
    f.dens = vec![den(DEN_A, HOOK_WORK, 1_000), den(DEN_B, HOOK_WORK2, 1_000)];
    f.refused = vec![DEN_A];
    assert_eq!(
        f.plan(),
        DenAct::Extract {
            den: DEN_B,
            qty: 1_000.min(ROOM)
        }
    );
}

#[test]
fn infomorphs_on_board_are_sold_when_docked_only() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.hooks.clear();
    f.infomorphs_held = 5;
    assert_eq!(f.plan(), DenAct::Sell(INFOMORPH));
    f.docked = false;
    assert_eq!(f.plan(), DenAct::None);
}

#[test]
fn starts_no_activity_at_capacity() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.hooks.clear();
    f.dens = vec![den(DEN_A, HOOK_WORK, 0), den(DEN_B, HOOK_WORK2, 0)];
    f.capacity = 1;
    f.activities = vec![act(ACT_A, DEN_A, true), act(ACT_B, DEN_B, false)];
    // The started one is run, never a second start.
    f.quiet = vec![ACT_A];
    assert_eq!(f.plan(), DenAct::None);
    f.quiet.clear();
    assert!(matches!(f.plan(), DenAct::Run { .. }));
    f.capacity = 2;
    f.quiet = vec![ACT_A];
    assert_eq!(f.plan(), DenAct::Start { activity: ACT_B });
}

#[test]
fn a_started_activity_is_run_before_anything_else() {
    let mut f = Fixture::new();
    f.dens = vec![den(DEN_A, HOOK_WORK, 1_000)];
    f.infomorphs_held = 9;
    f.activities = vec![act(ACT_B, DEN_A, false), act(ACT_A, DEN_A, true)];
    assert_eq!(
        f.plan(),
        DenAct::Run {
            site_system: SYSTEM,
            activity: ACT_A
        }
    );
    // A site outside the alliance's hubs is not run, nor started.
    f.held = vec![OTHER_SYSTEM];
    assert!(!matches!(
        f.plan(),
        DenAct::Run { .. } | DenAct::Start { .. }
    ));
}

#[test]
fn a_pilot_without_guns_starts_no_activity() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.hooks.clear();
    f.dens = vec![den(DEN_A, HOOK_WORK, 0)];
    f.activities = vec![act(ACT_A, DEN_A, false)];
    assert_eq!(f.plan(), DenAct::Start { activity: ACT_A });
    f.guns = false;
    assert_eq!(f.plan(), DenAct::None);
    // Nor does it run a started one.
    f.activities = vec![act(ACT_A, DEN_A, true)];
    assert_eq!(f.plan(), DenAct::None);
}

#[test]
fn an_activity_of_no_den_of_the_pilot_is_left_alone() {
    let mut f = Fixture::new();
    f.den_item = None;
    f.hooks.clear();
    f.activities = vec![act(ACT_A, DEN_A, false)];
    assert_eq!(f.plan(), DenAct::None);
}

// ---- the site pick -----------------------------------------------------------------------

fn row(id: i64, system: i64, expiry: i64) -> EscalationRow {
    EscalationRow {
        instance_id: id,
        system,
        difficulty: None,
        expiry,
    }
}

const NOW: i64 = 1_000_000_000;
const S: i64 = 10_000_000;

#[test]
fn the_site_is_the_row_whose_expiry_matches_the_activity() {
    let rows = [
        row(1, SYSTEM, NOW + 500 * S),
        row(2, SYSTEM, NOW + 1_000 * S + S),
        row(3, OTHER_SYSTEM, NOW + 1_000 * S),
    ];
    assert_eq!(pick_site(&rows, SYSTEM, NOW + 1_000 * S, NOW), Some(2));
}

#[test]
fn with_no_matching_expiry_there_is_no_site() {
    let rows = [row(7, SYSTEM, NOW + 500 * S), row(5, SYSTEM, NOW + 600 * S)];
    assert_eq!(pick_site(&rows, SYSTEM, NOW + 9_000 * S, NOW), None);
    assert_eq!(pick_site(&rows, OTHER_SYSTEM, NOW + 500 * S, NOW), None);
    // A row that has run out is no site.
    let old = [row(1, SYSTEM, NOW - S)];
    assert_eq!(pick_site(&old, SYSTEM, NOW - S, NOW), None);
}

#[test]
fn a_refused_den_type_is_neither_launched_nor_wanted_and_a_refused_infomorph_is_not_sold() {
    let mut f = Fixture::new();
    f.refused = vec![DEN_TYPE];
    assert_eq!(f.plan(), DenAct::None);
    let mut f = Fixture::new();
    f.den_item = None;
    f.hooks.clear();
    f.infomorphs_held = 5;
    f.refused = vec![INFOMORPH];
    assert_eq!(f.plan(), DenAct::None);
}

// ---- the decoders ------------------------------------------------------------------------

fn id_msg(n: u64) -> Vec<u8> {
    super::gw::id(n)
}

#[test]
fn the_owned_list_and_one_den_decode() {
    let mut owned = Vec::new();
    put_bytes(&mut owned, 1, &id_msg(DEN_A as u64));
    put_bytes(&mut owned, 1, &id_msg(DEN_B as u64));
    assert_eq!(decode_owned(&owned), vec![DEN_A, DEN_B]);

    let mut attrs = Vec::new();
    put_bytes(&mut attrs, 1, &id_msg(90_000_922));
    put_bytes(&mut attrs, 2, &id_msg(HOOK_WORK as u64));
    put_bytes(&mut attrs, 3, &id_msg(SYSTEM as u64));
    let mut contents = Vec::new();
    put_uint(&mut contents, 1, 17);
    let mut inf = Vec::new();
    put_bytes(&mut inf, 2, &contents);
    let mut body = Vec::new();
    put_bytes(&mut body, 1, &attrs);
    put_bytes(&mut body, 4, &inf);
    assert_eq!(
        decode_den(DEN_A, &body),
        Some(DenRow {
            id: DEN_A,
            skyhook: HOOK_WORK,
            system: SYSTEM,
            infomorphs: 17
        })
    );
    // An empty den elides the count.
    let mut empty = Vec::new();
    put_bytes(&mut empty, 1, &attrs);
    assert_eq!(decode_den(DEN_A, &empty).map(|d| d.infomorphs), Some(0));
    assert_eq!(decode_den(DEN_A, &[]), None);
}

#[test]
fn an_activity_list_decodes_with_its_uuid_state_and_system() {
    let mut uuid = Vec::new();
    put_bytes(&mut uuid, 1, &ACT_A);
    let mut expiry = Vec::new();
    put_uint(&mut expiry, 1, 2_000_000_123);
    let mut attrs = Vec::new();
    put_bytes(&mut attrs, 2, &id_msg(DEN_A as u64));
    put_uint(&mut attrs, 3, 1);
    put_bytes(&mut attrs, 4, &expiry);
    put_bytes(&mut attrs, 5, &id_msg(SYSTEM as u64));
    let mut entry = Vec::new();
    put_bytes(&mut entry, 1, &uuid);
    put_bytes(&mut entry, 2, &attrs);
    let mut all = Vec::new();
    put_bytes(&mut all, 1, &entry);
    assert_eq!(
        decode_activities(&all),
        vec![ActRow {
            id: ACT_A,
            den: DEN_A,
            started: true,
            expiry: 2_000_000_123,
            system: SYSTEM
        }]
    );
    // The start body carries the same uuid.
    let body = start_body(&ACT_A);
    let inner = super::gw::bytes(&body, 1).expect("the message");
    assert_eq!(super::gw::bytes(inner, 1), Some(&ACT_A[..]));
}

#[test]
fn a_skyhook_record_says_workforce_and_reagent() {
    let rec = |id: u64, active: bool, workforce: Option<u64>, reagent: bool| {
        let mut r = Vec::new();
        put_bytes(&mut r, 1, &id_msg(id));
        if reagent {
            let mut sim = Vec::new();
            put_bytes(&mut sim, 1, &id_msg(990_221_303));
            put_bytes(&mut r, 2, &sim);
        }
        if active {
            put_uint(&mut r, 3, 1);
        }
        if let Some(w) = workforce {
            let mut wf = Vec::new();
            put_uint(&mut wf, 1, w);
            put_bytes(&mut r, 7, &wf);
        }
        r
    };
    let mut all = Vec::new();
    put_bytes(&mut all, 1, &rec(HOOK_WORK as u64, true, Some(3), false));
    put_bytes(&mut all, 1, &rec(HOOK_REAGENT as u64, true, None, true));
    put_bytes(&mut all, 1, &rec(HOOK_WORK2 as u64, false, Some(3), false));
    assert_eq!(
        decode_hooks(&all),
        vec![
            work(HOOK_WORK),
            reagent_hook(HOOK_REAGENT),
            HookRow {
                active: false,
                ..work(HOOK_WORK2)
            }
        ]
    );
}

#[test]
fn the_den_runner_sends_no_slash_line() {
    let slash = ["Slash", "Cmd"].concat();
    let qa = ["public", "QaToolsServer"].concat();
    let src = include_str!("den.rs");
    assert!(!src.contains(&slash));
    assert!(!src.contains(&qa));
}
