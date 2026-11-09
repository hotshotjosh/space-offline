//! Tests of asset safety. Every id and number is invented; the waits and the
//! fee are set by the tests themselves (the defaults are [A] and not pinned).

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::service::Request;
use crate::space_state::{Motion, Presence};
use eve_marshal::PyValue;

pub(crate) const BOSS: i64 = 7;
pub(crate) const PAL: i64 = 8;
pub(crate) const FOE: i64 = 9;
pub(crate) const SYSTEM: i64 = 31;
pub(crate) const OTHER_SYSTEM: i64 = 32;
/// The NPC station of `SYSTEM`.
pub(crate) const HARBOUR: i64 = 6_001;
/// An NPC station of `OTHER_SYSTEM`.
pub(crate) const FAR: i64 = 6_002;
const ASTRAHUS: i64 = crate::structure_space::ASTRAHUS;
/// Every spare item a test stores is one capsule worth this.
pub(crate) const PRICE: f64 = 1_000.0;
/// The invented waits (seconds) and fee (percent) the tests run on.
pub(crate) const MANUAL: i64 = 100;
pub(crate) const AUTO: i64 = 1_000;
pub(crate) const FEE_PCT: f64 = 10.0;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Hull\n",
        crate::typedata::FIXTURE
    );
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let prices = crate::econ_prices::Table::parse(&[format!(
        "{}\t{PRICE}\trecipe\t{}\t6\t25\t1\t1",
        crate::service::TYPE_CAPSULE,
        PRICE / 2.0
    )])
    .expect("prices parse");
    c.insert("econ_prices", prices);
    c
}

fn config(foe_balance: f64) -> SandboxConfig {
    let ch = |id: i64, balance: f64| Character {
        character_id: id,
        solar_system_id: Some(SYSTEM),
        station_id: Some(HARBOUR),
        ship_id: 1_000 + id,
        ship_type_id: crate::service::TYPE_CAPSULE,
        balance,
        ..Character::default()
    };
    SandboxConfig {
        characters: vec![ch(BOSS, 5_000_000.0), ch(PAL, 100.0), ch(FOE, foe_balance)],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
             station\t{FAR}\t2502\t0.0\t0.0\t0.0\t0.0\t{OTHER_SYSTEM}\t0\t0\n"
        )),
        ..SandboxConfig::default()
    }
}

/// The corporation BOSS founds (he is its CEO); PAL is a member with no
/// roles; FOE has none.
pub(crate) fn world() -> (Registry, i64, i64) {
    world_with(100.0)
}

pub(crate) fn world_with(foe_balance: f64) -> (Registry, i64, i64) {
    let mut reg = Registry::new(config(foe_balance)).with_content(std::sync::Arc::new(content()));
    let u = |s: &str| PyValue::Unicode(s.to_owned());
    let args = vec![
        u("Safety Test"),
        u("SAFT"),
        u("We keep."),
        u("http://"),
        PyValue::Real(0.0),
        PyValue::Int(419),
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::Bool(true),
        PyValue::Bool(false),
        PyValue::Real(0.0),
    ];
    call_args(&mut reg, BOSS, "corpRegistry", "AddCorporation", &args).expect("founds");
    let corp = reg.character(BOSS).expect("boss").corporation_id;
    reg.corporations_mut().enrol_for_test(PAL, corp, 0);
    for (n, who) in [BOSS, PAL, FOE].into_iter().enumerate() {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            1_000 + who,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    crate::corp_wallet::adjust(
        reg.deltas_mut(),
        corp,
        crate::corp_wallet::MASTER_KEY,
        crate::corp_wallet::to_cents(1_000_000.0),
    )
    .expect("funds the corp");
    let out = crate::service::structures::slash(
        &mut reg,
        Some(BOSS),
        &["spawn", "astrahus", "Invented", "Home"],
    )
    .expect("spawns");
    let PyValue::Bytes(b) = out else {
        panic!("{out:?}")
    };
    assert!(String::from_utf8_lossy(&b).contains("online"));
    let id = crate::structure::all(reg.deltas())
        .last()
        .expect("spawned")
        .id;
    {
        let r = reg.rules_mut();
        r.safety_manual_secs = MANUAL;
        r.safety_auto_secs = AUTO;
        r.safety_fee_pct = FEE_PCT;
    }
    let _ = reg.take_pending();
    let _ = reg.take_session_changes();
    (reg, id, corp)
}

pub(crate) fn call_args(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, crate::service::ServiceError> {
    reg.dispatch(&Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    })
}

/// A spare capsule of `owner` stored at `at`.
pub(crate) fn store_item(reg: &mut Registry, owner: i64, at: i64) -> i64 {
    let changes = reg
        .apply_items(&[crate::item::Edit::Create {
            type_id: crate::service::TYPE_CAPSULE,
            quantity: 1,
            to: Slot::new(owner, at, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("an id")
}

/// Two of BOSS's, one of the corporation's and one of FOE's stored in the
/// structure, then the structure leaves space (the evacuation, with the row
/// still there). Answers the item ids `(boss1, boss2, corp1, foe1)`.
pub(crate) fn unanchor(reg: &mut Registry, id: i64, corp: i64) -> (i64, i64, i64, i64) {
    let items = (
        store_item(reg, BOSS, id),
        store_item(reg, BOSS, id),
        store_item(reg, corp, id),
        store_item(reg, FOE, id),
    );
    let s = crate::structure::get(reg.deltas(), id).expect("the structure");
    crate::structure_dock::evacuate(reg, &s);
    items
}

pub(crate) fn wrap_of(reg: &Registry, owner: i64) -> Wrap {
    let mut w = of_owner(reg.deltas(), owner);
    assert_eq!(w.len(), 1, "one wrap for {owner}: {w:?}");
    w.remove(0)
}

fn pushes(reg: &mut Registry) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    out.extend(
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(5))
            .into_iter()
            .map(|p| p.to_push().0),
    );
    out
}

fn location_of(reg: &Registry, item: i64) -> (i64, i64) {
    let i = reg.store().get(item).expect("the item");
    (i.location_id, i.flag_id)
}

fn spent(reg: &Registry, who: i64) -> Vec<f64> {
    reg.journal()
        .entries()
        .iter()
        .filter(|e| e.entry_type_id == REF_ASSET_SAFETY_TAX && e.owner_id_1 == who)
        .map(|e| e.amount)
        .collect()
}

#[test]
fn fee_is_a_share_of_the_value_in_whole_cents() {
    assert_eq!(fee_of(1_000.0, 10.0), 100.0);
    assert_eq!(fee_of(333.33, 15.0), 50.0);
    assert_eq!(fee_of(0.0, 15.0), 0.0);
    assert_eq!(fee_of(-5.0, 15.0), 0.0);
    assert_eq!(fee_of(1_000.0, 0.0), 0.0);
    assert_eq!(fee_of(f64::NAN, 10.0), 0.0);
}

#[test]
fn wormhole_space_has_no_asset_safety() {
    assert!(applies(SYSTEM));
    assert!(!applies(31_000_005));
}

/// **PV-1e.** A Triglavian system has no asset safety, as wormhole space has
/// none (`assetSafetySvc.py:78-79`); a wrap asked for there is refused.
#[test]
fn a_structure_in_pochven_leaves_no_wrap() {
    let mut c = content();
    c.insert(
        "universe",
        crate::celestial::Universe::parse(
            &[format!(
                "{SYSTEM}\t19070001\t29070001\t{}\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t25\t0\t\t\tInvented",
                crate::pochven::FACTION_TRIGLAVIAN
            )],
            &[],
            &[],
        )
        .expect("invented rows parse"),
    );
    let mut reg = Registry::new(config(100.0)).with_content(std::sync::Arc::new(c));
    assert!(!applies_in(&reg, SYSTEM), "the Triglavian system");
    assert!(applies_in(&reg, OTHER_SYSTEM), "a system the map lacks");
    assert!(!applies_in(&reg, 31_000_005), "wormhole space");
    assert_eq!(
        wrap_place(&mut reg, BOSS, SYSTEM, 1, 0, &|_| false),
        Err(Refusal::NoSafetyHere)
    );
    // The stock fixture has no such system: asset safety applies.
    let (plain, ..) = world();
    assert!(applies_in(&plain, SYSTEM));
}

#[test]
fn a_structure_leaving_makes_one_wrap_per_owner() {
    let (mut reg, id, corp) = world();
    let (b1, b2, c1, f1) = unanchor(&mut reg, id, corp);
    assert_eq!(all(reg.deltas()).len(), 3);
    let (mine, corps, foes) = (wrap_of(&reg, BOSS), wrap_of(&reg, corp), wrap_of(&reg, FOE));
    for w in [&mine, &corps, &foes] {
        assert_eq!((w.system, w.origin), (SYSTEM, id));
        assert_eq!(w.name, "Invented Home Asset Safety");
        // The wrap is an item of its own: type 60, in the system.
        let item = reg.store().get(w.id).expect("the wrap item");
        assert_eq!(item.type_id, TYPE_WRAP);
        assert_eq!((item.owner_id, item.location_id), (w.owner, SYSTEM));
    }
    // Each owner's things are in their own wrap, same owner and flag.
    for (item, w) in [(b1, &mine), (b2, &mine), (c1, &corps), (f1, &foes)] {
        let i = reg.store().get(item).unwrap();
        assert_eq!(
            (i.owner_id, i.location_id, i.flag_id),
            (w.owner, w.id, crate::model::FLAG_HANGAR)
        );
    }
    assert!((value_of(&reg, mine.id) - 2.0 * PRICE).abs() < 1e-9);
    // Nobody was told twice, and the pilots' own ships went to the harbour.
    let ship = reg.character(BOSS).unwrap().ship_id;
    assert_eq!(location_of(&reg, ship).0, HARBOUR);
}

#[test]
fn the_created_push_goes_to_the_owner_and_the_corporation() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let created: Vec<(&'static str, Option<Vec<i64>>)> = reg
        .take_due(std::time::Instant::now() + std::time::Duration::from_secs(5))
        .into_iter()
        .map(|p| (p.to_push().0, p.audience()))
        .filter(|(n, _)| *n == "OnAssetSafetyCreated")
        .collect();
    assert_eq!(created.len(), 3, "{created:?}");
    let for_corp = created
        .iter()
        .filter_map(|(_, a)| a.clone())
        .find(|a| a.contains(&PAL))
        .expect("the corporation's members hear of the corporation's wrap");
    assert!(for_corp.contains(&BOSS) && !for_corp.contains(&FOE));
}

#[test]
fn a_wormhole_system_keeps_the_older_move_to_the_harbour() {
    let (mut reg, id, corp) = world();
    let spare = store_item(&mut reg, BOSS, id);
    let mut s = crate::structure::get(reg.deltas(), id).unwrap();
    s.system = 31_000_005;
    let w = wrap_structure(&mut reg, &s, 1, &|_| false);
    assert!(w.wraps.is_empty());
    assert!(all(reg.deltas()).is_empty());
    assert_eq!(location_of(&reg, spare).0, id, "untouched here");
    let _ = corp;
}

#[test]
fn delivery_is_refused_until_the_manual_wait_has_run_out() {
    let (mut reg, id, corp) = world();
    let (b1, ..) = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let early = w.created + (MANUAL - 1) * SECOND;
    assert_eq!(
        deliver_to(&mut reg, BOSS, w.id, HARBOUR, early),
        Err(Refusal::TooEarly(1))
    );
    assert_eq!(location_of(&reg, b1).0, w.id, "nothing moved");
    assert!(reg.store().get(w.id).is_some());
    let d = deliver_to(&mut reg, BOSS, w.id, HARBOUR, w.created + MANUAL * SECOND)
        .expect("delivered on the second");
    assert_eq!((d.owner, d.to, d.moved), (BOSS, HARBOUR, 2));
    assert_eq!(location_of(&reg, b1), (HARBOUR, crate::model::FLAG_HANGAR));
}

#[test]
fn a_delivery_takes_the_fee_with_the_asset_safety_reference() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let before = reg.character(BOSS).unwrap().balance;
    let d = deliver_to(&mut reg, BOSS, w.id, HARBOUR, w.created + MANUAL * SECOND).unwrap();
    // Two capsules at the invented price, at the invented share.
    let fee = 2.0 * PRICE * FEE_PCT / 100.0;
    assert_eq!(d.fee, fee);
    assert_eq!(reg.character(BOSS).unwrap().balance, before - fee);
    assert_eq!(spent(&reg, BOSS), vec![-fee]);
    let row = reg
        .journal()
        .entries()
        .iter()
        .find(|e| e.entry_type_id == REF_ASSET_SAFETY_TAX)
        .unwrap();
    assert_eq!(row.reference_id, w.id);
    // The wrap is gone, row and item; the owner's pilots hear of it.
    assert!(get(reg.deltas(), w.id).is_none());
    assert!(reg.store().get(w.id).is_none());
    assert!(pushes(&mut reg).contains(&"OnAssetSafetyDelivered"));
}

#[test]
fn a_corporations_fee_comes_from_its_master_wallet() {
    let (mut reg, id, corp) = world();
    let (_, _, c1, _) = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, corp);
    let master = || {
        |reg: &Registry| {
            crate::corp_wallet::to_isk(crate::corp_wallet::balance(
                reg.deltas(),
                corp,
                crate::corp_wallet::MASTER_KEY,
            ))
        }
    };
    let before = master()(&reg);
    // A director acts for it; a plain member may not.
    assert_eq!(
        deliver_to(&mut reg, PAL, w.id, HARBOUR, w.created + MANUAL * SECOND),
        Err(Refusal::NotYours)
    );
    let d = deliver_to(&mut reg, BOSS, w.id, HARBOUR, w.created + MANUAL * SECOND).unwrap();
    let fee = PRICE * FEE_PCT / 100.0;
    assert_eq!(d.fee, fee);
    assert_eq!(master()(&reg), before - fee);
    assert!(
        crate::corp_wallet::all_rows(reg.deltas(), corp)
            .iter()
            .any(|r| r.ref_type == REF_ASSET_SAFETY_TAX && r.reference == w.id)
    );
    let i = reg.store().get(c1).unwrap();
    assert_eq!((i.owner_id, i.location_id), (corp, HARBOUR));
}

#[test]
fn a_wallet_that_cannot_pay_pays_what_it_holds_and_the_delivery_goes_on() {
    let (mut reg, id, corp) = world_with(30.0);
    let (.., f1) = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, FOE);
    let d = deliver_to(&mut reg, FOE, w.id, HARBOUR, w.created + MANUAL * SECOND).unwrap();
    assert_eq!(d.fee, 30.0);
    assert_eq!(reg.character(FOE).unwrap().balance, 0.0);
    assert_eq!(location_of(&reg, f1).0, HARBOUR);
}

#[test]
fn only_the_owner_delivers_and_only_to_a_place_in_the_system() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let ripe = w.created + MANUAL * SECOND;
    assert_eq!(
        deliver_to(&mut reg, FOE, w.id, HARBOUR, ripe),
        Err(Refusal::NotYours)
    );
    assert_eq!(
        deliver_to(&mut reg, BOSS, w.id, FAR, ripe),
        Err(Refusal::BadDestination),
        "a station in another system"
    );
    assert_eq!(
        deliver_to(&mut reg, BOSS, 424_242, HARBOUR, ripe),
        Err(Refusal::NoWrap)
    );
    assert!(get(reg.deltas(), w.id).is_some(), "still there");
}

#[test]
fn a_structure_is_a_destination_for_those_who_may_dock() {
    let (mut reg, id, corp) = world();
    // A second structure: the destination (the first one leaves).
    let out = crate::service::structures::slash(
        &mut reg,
        Some(BOSS),
        &["spawn", "astrahus", "Invented", "Other"],
    )
    .unwrap();
    let _ = out;
    let other = crate::structure::all(reg.deltas())
        .last()
        .map(|s| s.id)
        .unwrap();
    assert_ne!(other, id);
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    assert!(
        destinations(&reg, BOSS, SYSTEM)
            .iter()
            .any(|&(d, _)| d == other),
        "the corporation's own structure"
    );
    let d = deliver_to(&mut reg, BOSS, w.id, other, w.created + MANUAL * SECOND).unwrap();
    assert_eq!(d.to, other);
}

#[test]
fn the_gm_seam_moves_the_wrap_back_in_time() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    assert_eq!(
        move_eject_time(&mut reg, FOE, w.id, 5),
        Err(Refusal::NotYours)
    );
    move_eject_time(&mut reg, BOSS, w.id, 5).unwrap();
    let moved = wrap_of(&reg, BOSS);
    assert_eq!(moved.created, w.created - 5 * DAY);
    // Now, with no clock moved, it can be delivered.
    let now = crate::pyutil::filetime_now();
    assert!(deliver_to(&mut reg, BOSS, w.id, HARBOUR, now).is_ok());
}

#[test]
fn the_tick_moves_a_wrap_to_the_nearest_station_after_the_long_wait() {
    let (mut reg, id, corp) = world();
    let (b1, _, c1, _) = unanchor(&mut reg, id, corp);
    let before = reg.character(BOSS).unwrap().balance;
    // Not yet: the wait counts from now.
    tick(
        &mut reg,
        std::time::Instant::now(),
        crate::pyutil::filetime_now(),
    );
    assert_eq!(all(reg.deltas()).len(), 3);
    // A day older is past the invented thousand seconds.
    let mine = wrap_of(&reg, BOSS);
    let corps = wrap_of(&reg, corp);
    move_eject_time(&mut reg, BOSS, mine.id, 1).unwrap();
    move_eject_time(&mut reg, BOSS, corps.id, 1).unwrap();
    tick(
        &mut reg,
        std::time::Instant::now(),
        crate::pyutil::filetime_now(),
    );
    assert_eq!(all(reg.deltas()).len(), 1, "FOE's, not yet old");
    assert_eq!(location_of(&reg, b1), (HARBOUR, crate::model::FLAG_HANGAR));
    assert_eq!(location_of(&reg, c1).0, HARBOUR);
    assert_eq!(reg.character(BOSS).unwrap().balance, before - 200.0);
}

#[test]
fn the_waits_are_world_settings_read_when_asked() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let w = wrap_of(&reg, BOSS);
    let at = w.created + MANUAL * SECOND;
    reg.rules_mut().safety_manual_secs = MANUAL * 10;
    assert!(matches!(
        deliver_to(&mut reg, BOSS, w.id, HARBOUR, at),
        Err(Refusal::TooEarly(_))
    ));
    reg.rules_mut().safety_manual_secs = 0;
    assert!(deliver_to(&mut reg, BOSS, w.id, HARBOUR, w.created).is_ok());
}

#[test]
fn a_restart_keeps_the_wraps_and_their_clocks() {
    let (mut reg, id, corp) = world();
    let (b1, ..) = unanchor(&mut reg, id, corp);
    assert!(crate::delta_kinds::is_registered(KIND));
    let before = all(reg.deltas());
    let snap = reg.to_snapshot();
    let mut again = Registry::new(config(100.0)).with_content(std::sync::Arc::new(content()));
    again.apply_snapshot(snap);
    assert_eq!(all(again.deltas()), before);
    assert_eq!(
        again.store().get(b1).unwrap().location_id,
        wrap_of(&again, BOSS).id
    );
    again.rules_mut().safety_manual_secs = MANUAL;
    let w = wrap_of(&again, BOSS);
    assert!(deliver_to(&mut again, BOSS, w.id, HARBOUR, w.created + MANUAL * SECOND).is_ok());
}

#[test]
fn things_can_be_put_in_safety_from_a_structure_one_is_not_docked_in() {
    let (mut reg, id, _corp) = world();
    let spare = store_item(&mut reg, BOSS, id);
    let other = store_item(&mut reg, PAL, id);
    let ship = reg.character(BOSS).unwrap().ship_id;
    let now = crate::pyutil::filetime_now();
    assert_eq!(
        wrap_place(&mut reg, FOE, SYSTEM, id, now, &|_| false),
        Err(Refusal::NothingThere)
    );
    assert_eq!(
        wrap_place(&mut reg, BOSS, 31_000_005, id, now, &|_| false),
        Err(Refusal::NoSafetyHere)
    );
    let wrap = wrap_place(&mut reg, BOSS, SYSTEM, id, now, &|i| i == ship).unwrap();
    assert_eq!(reg.store().get(spare).unwrap().location_id, wrap);
    assert_eq!(reg.store().get(other).unwrap().location_id, id, "not his");
    assert_eq!(wrap_of(&reg, BOSS).origin, id);
}

#[test]
fn a_place_that_is_not_a_structure_or_in_another_system_is_not_wrapped() {
    let (mut reg, id, _corp) = world();
    // BOSS's things in a far NPC station's hangar and in his own ship.
    let far = store_item(&mut reg, BOSS, FAR);
    let ship = reg.character(BOSS).unwrap().ship_id;
    let aboard = store_item(&mut reg, BOSS, ship);
    let spare = store_item(&mut reg, BOSS, id);
    let now = crate::pyutil::filetime_now();
    for (system, place) in [(SYSTEM, FAR), (OTHER_SYSTEM, FAR), (SYSTEM, ship)] {
        assert_eq!(
            wrap_place(&mut reg, BOSS, system, place, now, &|_| false),
            Err(Refusal::NothingThere),
            "{place}"
        );
    }
    // The real structure, named in the wrong system: refused, nothing moved.
    assert_eq!(
        wrap_place(&mut reg, BOSS, OTHER_SYSTEM, id, now, &|_| false),
        Err(Refusal::BadDestination)
    );
    assert!(of_owner(reg.deltas(), BOSS).is_empty());
    assert_eq!(reg.store().get(far).unwrap().location_id, FAR);
    assert_eq!(reg.store().get(aboard).unwrap().location_id, ship);
    assert_eq!(reg.store().get(spare).unwrap().location_id, id);
    // Named right, it is wrapped in the structure's own system.
    let wrap = wrap_place(&mut reg, BOSS, SYSTEM, id, now, &|_| false).unwrap();
    assert_eq!(wrap_of(&reg, BOSS).system, SYSTEM);
    assert_eq!(reg.store().get(spare).unwrap().location_id, wrap);
}

#[test]
fn the_tick_runs_on_the_time_the_stage_gives_it() {
    let (mut reg, id, corp) = world();
    let _ = unanchor(&mut reg, id, corp);
    let mine = wrap_of(&reg, BOSS);
    move_eject_time(&mut reg, BOSS, mine.id, 1).unwrap();
    let now = crate::pyutil::filetime_now();
    // A tick far in the future moves it; nothing is left to retry.
    tick(&mut reg, std::time::Instant::now(), now);
    assert!(of_owner(reg.deltas(), BOSS).is_empty());
}

#[test]
fn a_failed_delivery_in_one_world_does_not_delay_the_same_wrap_id_in_another() {
    let (mut a, id, corp) = world();
    let _ = unanchor(&mut a, id, corp);
    let (mut b, id_b, corp_b) = world();
    let _ = unanchor(&mut b, id_b, corp_b);
    // Two worlds in one process, born at invented, different instants.
    for (reg, birth) in [(&mut a, 1_000_001), (&mut b, 1_000_002)] {
        let mut w = *reg.deltas().world().expect("a world");
        w.birth = birth;
        reg.deltas_mut().set_world(w);
    }
    let wa = wrap_of(&a, BOSS);
    let wb = wrap_of(&b, BOSS);
    assert_eq!(wa.id, wb.id, "the same wrap id in both worlds");
    move_eject_time(&mut a, BOSS, wa.id, 1).unwrap();
    move_eject_time(&mut b, BOSS, wb.id, 1).unwrap();
    let now = crate::pyutil::filetime_now();
    // World A's delivery of that id failed a moment ago and is held back.
    NEXT_TRY
        .lock()
        .unwrap()
        .insert(try_key(&a, wa.id), now.saturating_add(RETRY));
    tick(&mut b, std::time::Instant::now(), now);
    assert!(of_owner(b.deltas(), BOSS).is_empty(), "B delivered");
    tick(&mut a, std::time::Instant::now(), now);
    assert_eq!(of_owner(a.deltas(), BOSS).len(), 1, "A still waits");
    // After its own retry time, A delivers too.
    tick(
        &mut a,
        std::time::Instant::now(),
        now.saturating_add(2 * RETRY),
    );
    assert!(of_owner(a.deltas(), BOSS).is_empty());
}
