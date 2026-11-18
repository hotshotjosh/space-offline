//! Mutated drones in the bay and in space (#224 MUTA-DRONE). Invented ids
//! (the 981_5xx_xxx band) and invented numbers; no client data, no `[A]`
//! value pinned. Expected values come from the fixture constants and the
//! formula `base * (modifier * (max - min) + min)`
//! (`mutator_data::value`), never from `dynamic_item::attributes` itself.

use super::tests::{
    ALICE, ALICE_SHIP, BIG_BAY, DRONE, ONE_BAY, SK_DRONES, SK_IFACE, SYSTEM, undock, world_in,
};
use super::*;

/// The source drone, the dynamic result type and the mutaplasmid.
const SRC_D: i64 = 981_500_001;
const RES_D: i64 = 981_500_002;
const MUT_D: i64 = 981_500_003;

/// Every range is `[RANGE_MIN, RANGE_MAX]`; the mint rolls every modifier at 1.0.
const RANGE_MIN: f64 = 0.5;
const RANGE_MAX: f64 = 1.5;
const ROLLED: [i64; 8] = [9, 37, 54, 64, 158, 160, 263, 265];

/// The source's own numbers (by attribute id), as the rows below carry them.
const SRC_HP: f64 = 111.0;
const SRC_SPEED: f64 = 1000.0;
const SRC_PERIOD: f64 = 3000.0;
const SRC_OPTIMAL: f64 = 2000.0;
const SRC_MULT: f64 = 2.0;
const SRC_KINETIC: f64 = 16.0;
const SRC_FALLOFF: f64 = 1000.0;
const SRC_TRACKING: f64 = 2.0;
const SRC_SHIELD: f64 = 40.0;
const SRC_ARMOR: f64 = 60.0;
const SRC_BANDWIDTH: f64 = 5.0;
const SRC_ARMOR_RES: [f64; 4] = [0.5, 0.625, 0.75, 0.875];
const RES_SIGNATURE: f64 = 30.0;

/// An invented damage amplifier, its result type and mutaplasmid: a passive
/// owner modifier of the drones' damage multiplier by its attribute 292.
const SRC_AMP: i64 = 981_500_021;
const RES_AMP: i64 = 981_500_022;
const MUT_AMP: i64 = 981_500_023;
const EFFECT_AMP: i64 = 981_500_090;
const SRC_AMP_BONUS: f64 = 10.0;
const FLAG_LOW_SLOT: i64 = 11;

/// An invented Drone-Durability-like skill (#265): per level it raises the
/// shield, armour and hull of every drone that requires the fixture Drones
/// skill, by its attribute 327, as three owner post-percent modifiers.
const SK_DUR: i64 = 981_500_031;
const EFFECT_DUR_LEVEL: i64 = 981_500_091;
const EFFECT_DUR: i64 = 981_500_092;
const DUR_BONUS: f64 = 4.0;

/// `base * (1.0 * (max - min) + min)`: the roll the mint (modifier 1.0) gives.
fn rolled(base: f64) -> f64 {
    base * (1.0 * (RANGE_MAX - RANGE_MIN) + RANGE_MIN)
}

fn content() -> crate::content::Content {
    let mut ranges: Vec<String> = ROLLED
        .iter()
        .map(|a| format!("{MUT_D}\t{a}\t{RANGE_MIN}\t{RANGE_MAX}\t1"))
        .collect();
    ranges.push(format!("{MUT_AMP}\t292\t{RANGE_MIN}\t{RANGE_MAX}\t1"));
    let rows = format!(
        "type\t{SRC_D}\t100\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t10\t10\t\
9={SRC_HP};37={SRC_SPEED};51={SRC_PERIOD};54={SRC_OPTIMAL};64={SRC_MULT};70=50.0;117={SRC_KINETIC};158={SRC_FALLOFF};160={SRC_TRACKING};182=990581003.0;\
263={SRC_SHIELD};265={SRC_ARMOR};267={};268={};269={};270={};277=1.0;416=500.0;479=100000.0;482=1.0;508=250.0;552=20.0;620=25.0;1272={SRC_BANDWIDTH}\tInvented Source Drone\n\
type\t{RES_D}\t100\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t10\t10\t\
9=1.0;182=990581003.0;263=1.0;265=1.0;277=1.0;552={RES_SIGNATURE}\tInvented Mutated Drone\n\
type\t{MUT_D}\t1964\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Drone Mutaplasmid\n\
type\t{SRC_AMP}\t59\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t{EFFECT_AMP}\t292={SRC_AMP_BONUS}\tInvented Amplifier\n\
type\t{RES_AMP}\t59\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t{EFFECT_AMP}\t\tInvented Mutated Amplifier\n\
type\t{MUT_AMP}\t1964\t17\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\tInvented Amplifier Mutaplasmid\n\
type\t{SK_DUR}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990581100,{EFFECT_DUR_LEVEL},{EFFECT_DUR}\t182=990581003.0;277=1.0;327={DUR_BONUS}\tInvented Durability\n\
effect\t{EFFECT_DUR_LEVEL}\t0\t0\t0\t0\t0\t0\t0\tinventedDurabilityLevel\t\n\
effect\t{EFFECT_DUR}\t0\t0\t0\t0\t0\t0\t0\tinventedDurability\t\n\
modifier\t{EFFECT_DUR_LEVEL}\titemID\tItemModifier\t327\t280\t0\t\t\n\
modifier\t{EFFECT_DUR}\tcharID\tOwnerRequiredSkillModifier\t263\t327\t6\t990581003\t\n\
modifier\t{EFFECT_DUR}\tcharID\tOwnerRequiredSkillModifier\t265\t327\t6\t990581003\t\n\
modifier\t{EFFECT_DUR}\tcharID\tOwnerRequiredSkillModifier\t9\t327\t6\t990581003\t\n\
effect\t{EFFECT_AMP}\t0\t0\t0\t0\t0\t0\t0\tinventedAmplifierEffect\t\n\
modifier\t{EFFECT_AMP}\tcharID\tOwnerRequiredSkillModifier\t64\t292\t6\t990581003\t\n",
        SRC_ARMOR_RES[0], SRC_ARMOR_RES[1], SRC_ARMOR_RES[2], SRC_ARMOR_RES[3]
    );
    let text = format!("{}{}{rows}", typedata::FIXTURE, super::tests::rows());
    let table = typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", typedata::Installed::build(table));
    c.insert(
        "mutators",
        crate::mutator_data::Mutators::parse(
            &ranges,
            &[
                format!("{MUT_D}\t{RES_D}\t{SRC_D}"),
                format!("{MUT_AMP}\t{RES_AMP}\t{SRC_AMP}"),
            ],
        )
        .expect("the test mutators parse"),
    );
    c
}

/// Alice in `hull`, docked, with the skills a drone wants (Interfacing at
/// `iface`), and an empty bay.
fn world(hull: i64, iface: i64) -> Registry {
    world_in(
        content(),
        hull,
        0,
        &[(SKILL_DRONES, 5), (SK_DRONES, 1), (SK_IFACE, iface)],
        DRONE,
    )
}

/// [`world`] with the durability skill trained to `level`.
fn world_dur(level: i64) -> Registry {
    world_in(
        content(),
        BIG_BAY,
        0,
        &[(SKILL_DRONES, 5), (SK_DRONES, 1), (SK_DUR, level)],
        DRONE,
    )
}

/// Mint one mutated drone (every roll at 1.0) into Alice's hangar.
fn mint(reg: &mut Registry) -> i64 {
    let before: Vec<i64> = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id == RES_D)
        .map(|i| i.item_id)
        .collect();
    let answer = crate::dynamic_item::slash(
        reg,
        Some(ALICE),
        &[&SRC_D.to_string(), &MUT_D.to_string(), "1.0"],
    );
    let new = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id == RES_D && !before.contains(&i.item_id))
        .map(|i| i.item_id)
        .max();
    new.unwrap_or_else(|| panic!("a mutated drone was minted: {answer:?}"))
}

fn to_bay(reg: &mut Registry, id: i64) {
    reg.apply_items(&[Edit::Move {
        item_id: id,
        to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
    }])
    .expect("into the bay");
}

/// A plain source drone item in the bay.
fn plain_in_bay(reg: &mut Registry) -> i64 {
    let made = reg
        .apply_items(&[Edit::Create {
            type_id: SRC_D,
            quantity: 1,
            to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
        }])
        .expect("created");
    made[0].item_id()
}

/// Launch one item by id; the drone has the item's id.
fn fly(reg: &mut Registry, id: i64) -> Drone {
    let answer = launch(reg, Some(ALICE), &[(id, 1)]);
    reg.modules()
        .drones()
        .get(id)
        .copied()
        .unwrap_or_else(|| panic!("the drone is out: {answer:?}"))
}

fn item(reg: &Registry, id: i64) -> crate::item::Item {
    reg.store().get(id).expect("the item").clone()
}

/// T1: the record read off the roll.
#[test]
fn a_mutated_drone_reads_its_roll() {
    let mut reg = world(BIG_BAY, 0);
    let id = mint(&mut reg);
    let (k, o) = kind_of(&reg, &item(&reg, id)).expect("a mutated drone flies");
    assert!(o.is_some());
    assert_eq!(k.type_id, RES_D);
    assert_eq!(k.optimal, rolled(SRC_OPTIMAL));
    assert_eq!(k.hull.max_velocity, rolled(SRC_SPEED) as f32);
    assert_eq!(k.hull.hp, rolled(SRC_HP));
    assert_eq!(k.hull.shield_capacity, rolled(SRC_SHIELD));
    assert_eq!(k.hull.armor_hp, rolled(SRC_ARMOR));
    assert_eq!(k.tracking, rolled(SRC_TRACKING));
    assert_eq!(k.falloff, rolled(SRC_FALLOFF));
    assert_eq!(k.damage_multiplier, rolled(SRC_MULT));
    assert_eq!(k.damage.kinetic, SRC_KINETIC, "unrolled: the source's");
    assert_eq!(k.weapon_period_ms, SRC_PERIOD as i64);
    assert_eq!(k.bandwidth, SRC_BANDWIDTH, "the source's, not 0");
    assert_eq!(k.hull.signature_radius, RES_SIGNATURE, "layer 2");
}

/// T2: a plain drone is its type's.
#[test]
fn a_plain_drone_reads_its_type() {
    let mut reg = world(BIG_BAY, 0);
    let id = plain_in_bay(&mut reg);
    let (k, o) = kind_of(&reg, &item(&reg, id)).expect("a plain drone");
    assert!(o.is_none());
    assert_eq!(k, find_in(reg.content(), SRC_D).expect("flies"));
}

/// T3: why `kind_of` exists.
#[test]
fn the_dynamic_type_alone_does_not_fly() {
    let c = content();
    let k = find_in(&c, RES_D).expect("accepted by the gate");
    assert_eq!(k.hull.max_velocity, 0.0);
    assert_eq!(k.bandwidth, 0.0);
}

/// T4: launched, it flies its roll and the client is told the same.
#[test]
fn a_launched_mutated_drone_flies_its_roll() {
    let mut reg = world(BIG_BAY, 0);
    let id = mint(&mut reg);
    to_bay(&mut reg, id);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, id);
    assert_eq!(d.kind.optimal, rolled(SRC_OPTIMAL));
    let h = crate::dogma::hull(&reg, id).expect("a drone hull");
    assert_eq!(h.max_velocity, rolled(SRC_SPEED) as f32);
    assert!(h.max_velocity > 0.0);
    let motion = reg.roster().get(id).expect("in space").motion;
    let p = ball(&reg, &d, &motion).physics.expect("a free ball");
    assert_eq!(p.max_velocity, d.kind.hull.max_velocity);
}

/// R3's risk, pinned: an unskilled plain drone's ball is what its type's
/// hull said before the change (speed, mass, radius, agility).
#[test]
fn a_plain_drones_ball_is_unchanged() {
    let mut reg = world(BIG_BAY, 0);
    let id = plain_in_bay(&mut reg);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, id);
    let motion = reg.roster().get(id).expect("in space").motion;
    let now = ball(&reg, &d, &motion);
    let was = destiny::ego_ball_in(reg.content(), id, SRC_D, -1, -1, &motion);
    let (n, w) = (now.physics.expect("free"), was.physics.expect("free"));
    assert_eq!(n.max_velocity, w.max_velocity);
    assert_eq!(n.agility, w.agility);
    assert_eq!((now.mass, now.radius), (was.mass, was.radius));
}

/// T5: skills bonus the rolled value, and the sheet does not put the
/// dynamic row's own unrolled numbers back.
#[test]
fn skills_bonus_the_rolled_value() {
    let mut reg = world(BIG_BAY, 3);
    let m = mint(&mut reg);
    to_bay(&mut reg, m);
    let p = plain_in_bay(&mut reg);
    undock(&mut reg, ALICE);
    let (dm, dp) = (fly(&mut reg, m), fly(&mut reg, p));
    let skill_factor = dp.kind.damage_multiplier / SRC_MULT;
    assert!(
        (skill_factor - 1.0).abs() > 1e-6,
        "precondition: the skills move the multiplier"
    );
    let want = rolled(SRC_MULT) * skill_factor;
    assert!(
        (dm.kind.damage_multiplier - want).abs() < 1e-9,
        "{} vs {want}",
        dm.kind.damage_multiplier
    );
    assert_eq!(dm.kind.hull.hp, rolled(SRC_HP), "not the dynamic row's 1.0");
    assert_eq!(dm.kind.hull.shield_capacity, rolled(SRC_SHIELD));
    assert_eq!(dm.kind.hull.armor_hp, rolled(SRC_ARMOR));
    assert_eq!(dm.kind.optimal, rolled(SRC_OPTIMAL));
}

/// T6: the bandwidth check reads the roll's bandwidth.
#[test]
fn the_bandwidth_check_reads_the_roll() {
    let mut reg = world(ONE_BAY, 0);
    let (a, b) = (mint(&mut reg), mint(&mut reg));
    to_bay(&mut reg, a);
    to_bay(&mut reg, b);
    undock(&mut reg, ALICE);
    let _ = launch(&mut reg, Some(ALICE), &[(a, 1), (b, 1)]);
    let out = reg.modules().drones().controlled_by(ALICE_SHIP).count();
    assert_eq!(out, 1, "one drone's bandwidth fits the hull, two do not");
}

/// T7: it resists as its source does.
#[test]
fn a_mutated_drone_resists_as_its_source() {
    let mut reg = world(BIG_BAY, 0);
    let id = mint(&mut reg);
    to_bay(&mut reg, id);
    undock(&mut reg, ALICE);
    let _ = fly(&mut reg, id);
    let p = profile_of(&reg, id).expect("a profile");
    let [em, ex, ki, th] = SRC_ARMOR_RES;
    assert_eq!(
        (
            p.armor_resonance.em,
            p.armor_resonance.explosive,
            p.armor_resonance.kinetic,
            p.armor_resonance.thermal
        ),
        (em, ex, ki, th)
    );
    assert_eq!(p.armor_hp, rolled(SRC_ARMOR));
}

/// T8: scooping and relaunching keep the item and its roll.
#[test]
fn scoop_and_relaunch_keep_the_item_and_the_roll() {
    let mut reg = world(BIG_BAY, 0);
    let id = mint(&mut reg);
    to_bay(&mut reg, id);
    undock(&mut reg, ALICE);
    let first = fly(&mut reg, id);
    let refused = scoop_now(&mut reg, Some(ALICE), &[id]);
    assert_eq!(refused, PyValue::Dict(vec![]), "home");
    let back = item(&reg, id);
    assert_eq!(
        (back.location_id, back.flag_id),
        (ALICE_SHIP, FLAG_DRONE_BAY)
    );
    assert!(crate::dynamic_item::row(&reg, id).is_some());
    let second = fly(&mut reg, id);
    assert_eq!(second.kind, first.kind);
    assert_eq!(item(&reg, id).location_id, SYSTEM);
}

/// T9: a destroyed drone's row waits for the daily sweep.
#[test]
fn a_destroyed_mutated_drone_leaves_its_row_to_the_sweep() {
    let mut reg = world(BIG_BAY, 0);
    let id = mint(&mut reg);
    to_bay(&mut reg, id);
    undock(&mut reg, ALICE);
    let _ = fly(&mut reg, id);
    destroyed(&mut reg, id, vec![]);
    assert!(reg.store().get(id).is_none(), "the item is gone");
    assert!(
        crate::dynamic_item::row(&reg, id).is_some(),
        "the row stays"
    );
    let day = crate::rollover::Day {
        day: 1,
        days: 1,
        now: 0,
    };
    assert_eq!(crate::dynamic_item::on_rollover(&mut reg, &day), 1);
    assert!(crate::dynamic_item::row(&reg, id).is_none());
}

/// T10: two mutated drones never stack.
#[test]
fn two_mutated_drones_never_stack() {
    let mut reg = world(BIG_BAY, 0);
    let (a, b) = (mint(&mut reg), mint(&mut reg));
    to_bay(&mut reg, a);
    to_bay(&mut reg, b);
    assert!(item(&reg, a).singleton && item(&reg, b).singleton);
    let merged = reg.apply_items(&[Edit::Merge { from: a, into: b }]);
    assert!(
        matches!(merged, Err(crate::item::EditError::NotMergeable { .. })),
        "{merged:?}"
    );
    assert!(reg.store().get(a).is_some() && reg.store().get(b).is_some());
}

/// T15: a fitted mutated damage amplifier counts for the drones, at its
/// rolled bonus (the MUTA-3 gap: `stats` used `evaluate`, not
/// `evaluate_dynamic`).
#[test]
fn a_mutated_damage_amplifier_counts_for_drones() {
    let launched = |amp: bool| {
        let mut reg = world(BIG_BAY, 0);
        let m = mint(&mut reg);
        to_bay(&mut reg, m);
        if amp {
            let _ = crate::dynamic_item::slash(
                &mut reg,
                Some(ALICE),
                &[&SRC_AMP.to_string(), &MUT_AMP.to_string(), "1.0"],
            );
            let a = reg
                .store()
                .items()
                .iter()
                .find(|i| i.type_id == RES_AMP)
                .map(|i| i.item_id)
                .expect("an amplifier was minted");
            reg.apply_items(&[Edit::Move {
                item_id: a,
                to: Slot::new(ALICE, ALICE_SHIP, FLAG_LOW_SLOT),
            }])
            .expect("fitted");
        }
        undock(&mut reg, ALICE);
        fly(&mut reg, m).kind.damage_multiplier
    };
    let (without, with) = (launched(false), launched(true));
    let want = 1.0 + rolled(SRC_AMP_BONUS) / 100.0;
    assert!(
        (with / without - want).abs() < 1e-9,
        "{with} / {without} vs {want}"
    );
}

/// #265: a durability-like skill raises a launched drone's shield, armour
/// and hull by the fixture bonus per level, through the existing owner
/// modifier path; untrained leaves the type's own numbers.
#[test]
fn a_durability_skill_raises_a_plain_drones_hit_points() {
    let level = 3;
    let f = 1.0 + DUR_BONUS * level as f64 / 100.0;
    let mut reg = world_dur(level);
    let p = plain_in_bay(&mut reg);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, p);
    assert!(
        (d.kind.hull.hp - SRC_HP * f).abs() < 1e-9,
        "{}",
        d.kind.hull.hp
    );
    assert!((d.kind.hull.shield_capacity - SRC_SHIELD * f).abs() < 1e-9);
    assert!((d.kind.hull.armor_hp - SRC_ARMOR * f).abs() < 1e-9);
    // No skill moves a resonance: the profile keeps the type's.
    let pr = profile_of(&reg, p).expect("a profile");
    assert_eq!(pr.armor_resonance.em, SRC_ARMOR_RES[0]);
    assert!((pr.armor_hp - SRC_ARMOR * f).abs() < 1e-9);

    let mut reg = world_dur(0);
    let p = plain_in_bay(&mut reg);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, p);
    assert_eq!(d.kind.hull.hp, SRC_HP);
    assert_eq!(d.kind.hull.shield_capacity, SRC_SHIELD);
    assert_eq!(d.kind.hull.armor_hp, SRC_ARMOR);
}

/// #265: a mutated drone gets the skill over its roll.
#[test]
fn a_durability_skill_applies_over_a_mutated_drones_roll() {
    let level = 5;
    let f = 1.0 + DUR_BONUS * level as f64 / 100.0;
    let mut reg = world_dur(level);
    let m = mint(&mut reg);
    to_bay(&mut reg, m);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, m);
    assert!(
        (d.kind.hull.hp - rolled(SRC_HP) * f).abs() < 1e-9,
        "{}",
        d.kind.hull.hp
    );
    assert!((d.kind.hull.shield_capacity - rolled(SRC_SHIELD) * f).abs() < 1e-9);
    assert!((d.kind.hull.armor_hp - rolled(SRC_ARMOR) * f).abs() < 1e-9);

    let mut reg = world_dur(0);
    let m = mint(&mut reg);
    to_bay(&mut reg, m);
    undock(&mut reg, ALICE);
    let d = fly(&mut reg, m);
    assert_eq!(d.kind.hull.hp, rolled(SRC_HP));
    assert_eq!(d.kind.hull.armor_hp, rolled(SRC_ARMOR));
}
