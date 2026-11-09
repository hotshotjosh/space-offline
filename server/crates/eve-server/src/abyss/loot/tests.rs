//! The loot of the abyssal rooms. **Every id here is invented** (types
//! 990,95x to 990,99x, groups 990,195 and 990,196). The client's constants
//! appear only by name ([`GROUP_CACHE`], [`LOOT_LOCATOR`], [`WRECK_GROUPS`],
//! `combat::GROUP_WRECK`); the rows of the test tables name invented groups,
//! and no `[A]` value of the shipped table is asserted.

use super::super::tests::{
    ABY, ALICE, BOB, HOME, NOW, ROOM_A, alive, object, pocket, rat, room, row, seat, ship, stand,
    table, universe, world,
};
use super::*;
use crate::abyss::{Mode, Pocket, TARGETS_LIST, begin, close_run, run_of, slash, tick};
use crate::content::Content;
use crate::dungeon_data::TypeList;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

const CACHE: i64 = 990_960;
const SUBNODE: i64 = 990_961;
const NODE: i64 = 990_962;
const WAVE_NPC: i64 = 990_963;
const WRECK: i64 = 990_970;
const ITEM_A: i64 = 990_981;
const ITEM_B: i64 = 990_982;
const ITEM_HIDDEN: i64 = 990_983;
const PLAN: i64 = 990_984;
const G_ITEM: i64 = 990_195;
const G_PLAN: i64 = 990_196;
const CORP: i64 = WAVE_OWNER;

const SPOT: [f64; 3] = [40.0, 0.0, 70.0];
const NODE_AT: [f64; 3] = [-60.0, 0.0, 10.0];
const SUBNODE_AT: [f64; 3] = [80.0, 5.0, -20.0];

fn entity(id: i64, group: i64, name: &str) -> String {
    format!("type\t{id}\t{group}\t11\t0\t1\t100.0\t0.0\t1000.0\t50.0\t0\t\t9=500.0\t{name}\n")
}

fn thing(id: i64, group: i64, published: i64, name: &str) -> String {
    format!("type\t{id}\t{group}\t17\t{published}\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t\t{name}\n")
}

fn typedata() -> crate::typedata::Installed {
    let wreck = format!(
        "type\t{WRECK}\t{}\t2\t0\t1\t10.0\t0.0\t1.0\t1.0\t0\t\t\tInvented Wreck\n",
        crate::combat::GROUP_WRECK
    );
    let rows = [
        crate::typedata::FIXTURE.to_owned(),
        entity(CACHE, GROUP_CACHE, "Invented Bioadaptive Cache"),
        entity(SUBNODE, GROUP_CACHE, "Invented Extraction SubNode"),
        entity(NODE, GROUP_CACHE, "Invented Extraction Node"),
        entity(WAVE_NPC, WRECK_GROUPS[0], "Invented Swarmer"),
        wreck,
        thing(ITEM_A, G_ITEM, 1, "Invented Alpha Widget"),
        thing(ITEM_B, G_ITEM, 1, "Invented Beta Widget"),
        thing(ITEM_HIDDEN, G_ITEM, 0, "Invented Hidden Widget"),
        thing(PLAN, G_PLAN, 1, "Invented Gamma Plan"),
    ]
    .concat();
    crate::typedata::Installed::build(
        crate::typedata::Table::parse(&rows).expect("the test rows parse"),
    )
}

/// `types/market.tsv` lines: each entity with its group and its wreck.
fn market() -> Vec<String> {
    [
        (CACHE, GROUP_CACHE),
        (SUBNODE, GROUP_CACHE),
        (NODE, GROUP_CACHE),
        (WAVE_NPC, WRECK_GROUPS[0]),
    ]
    .iter()
    .map(|(t, g)| format!("{t}\t{g}\t11\t\t0.0\t\t\t\t\t\t\t{WRECK}\t1\t0"))
    .collect()
}

const HEAD: &str = "tier\tcache\tgroup\tname_has\tchance_ppm\tqty_min\tqty_max\tcopy\twhy\n";

/// Always: a stack of 2 to 4 widgets and one copy of a plan with 3 runs; never:
/// a Beta-only row at no chance.
fn rows() -> String {
    format!(
        "{HEAD}*\tbioadaptive\t{G_ITEM}\t*\t1000000\t2\t4\t0\tinvented\n\
         *\tbioadaptive\t{G_ITEM}\tbeta\t0\t1\t1\t0\tinvented\n\
         *\tbioadaptive\t{G_PLAN}\t*\t1000000\t3\t3\t1\tinvented\n\
         *\tnode\t{G_ITEM}\talpha\t1000000\t1\t1\t0\tinvented\n"
    )
}

/// A room with a loot locator and two nodes of its own.
fn loot_room(id: i64, locator: bool) -> Dungeon {
    let mut d = room(id);
    if locator {
        d.objects.push(object(1, 20, LOOT_LOCATOR, SPOT));
        d.objects.push(object(1, 21, LOOT_LOCATOR, [9_000.0; 3]));
    }
    d.objects.push(object(1, 22, NODE, NODE_AT));
    d.objects.push(object(1, 23, SUBNODE, SUBNODE_AT));
    d
}

fn content(authored: &str, locator: bool) -> Arc<Content> {
    let mut t = table(vec![loot_room(ROOM_A, locator)]);
    t.typelists.insert(
        TARGETS_LIST,
        TypeList {
            inc_groups: vec![crate::npc::FIXTURE_GROUP_FRIGATE],
            ..TypeList::default()
        },
    );
    let mut c = Content::fixture();
    c.insert("universe", universe());
    c.insert("dungeons", t);
    c.insert("typedata", typedata());
    c.insert(
        "abyss_loot",
        Table::parse(authored, &market()).expect("the test table parses"),
    );
    Arc::new(c)
}

/// A world with Alice in room 1 of a tier-2 run with no waves, seated.
fn loot_world(authored: &str, waves: Vec<crate::abyss::WaveRow>) -> Registry {
    let mut reg = world().with_content(content(authored, true));
    reg.ext_mut::<AbyssState>().waves = Some(waves);
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).expect("a run");
    seat(&mut reg, ALICE);
    reg
}

fn npc_at(reg: &Registry, id: i64) -> [f64; 3] {
    reg.roster().get(id).expect("in space").motion.position
}

/// What the store holds in a wreck: `(type, quantity)`, ascending.
fn held(reg: &Registry, wreck_id: i64) -> Vec<(i64, i64)> {
    let mut v: Vec<(i64, i64)> = reg
        .store()
        .contents_of(wreck_id)
        .map(|i| (i.type_id, i.quantity))
        .collect();
    v.sort_unstable();
    v
}

/// Make the cache's wreck as `combat::kill_npc` does (the item, the record,
/// the NPC gone) and run the kill hook, as the economy module's row does.
/// Answers the wreck's id.
fn kill_cache(reg: &mut Registry, npc: i64) -> i64 {
    let p = reg.roster().get(npc).copied().expect("in space");
    let made = reg
        .apply_items_quiet(&[Edit::Create {
            type_id: WRECK,
            quantity: 1,
            to: Slot::new(CORP, p.solar_system_id, 0),
        }])
        .expect("the wreck type is a store item");
    let Change::Created {
        item_id: wreck_id, ..
    } = made[0]
    else {
        panic!("created");
    };
    crate::combat::insert_wreck(
        reg,
        crate::combat::Wreck {
            wreck_id,
            type_id: WRECK,
            solar_system_id: p.solar_system_id,
            position: p.motion.position,
            ship_id: npc,
            owner_id: CORP,
            corp_id: CORP,
            fleet_id: None,
            abandoned: false,
            expires_at: NOW + 1_000_000_000,
        },
    );
    gone(reg, npc);
    on_npc_kill(reg, &event(npc, p.solar_system_id));
    wreck_id
}

fn event(npc: i64, system: i64) -> KillEvent {
    KillEvent {
        victim_character: npc,
        victim_ship: npc,
        ship_type: CACHE,
        killer: Some(ALICE),
        system,
        npc: None,
        when: NOW,
        park: Vec::new(),
    }
}

fn gone(reg: &mut Registry, npc: i64) {
    reg.roster_mut().leave(npc);
    reg.npcs_mut().remove(npc);
}

fn cache_npc(reg: &Registry) -> i64 {
    pocket(reg, ALICE).caches[0].npc
}

// ---- the table and the pure rules ------------------------------------------

#[test]
fn a_table_parses_its_tokens_and_the_off_row_turns_it_off() {
    let text = format!(
        "# a comment\n{HEAD}\
         3\tnexus\t{G_ITEM}\t{{tier}}\t5\t1\t2\t0\tw\n\
         *\t*\t{G_ITEM}\t{{grade}}\t0\t1\t1\t1\tw\n\
         *\tSubNode\t{G_PLAN}\tMixed Case\t1000000\t4\t4\t0\tw\n"
    );
    let t = Table::parse(&text, &[]).expect("parses");
    assert!(t.enabled);
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.rows[0].tier, Some(3));
    assert_eq!(t.rows[0].cache, Some(CacheKind::Nexus));
    assert_eq!(t.rows[0].name_has, NameHas::Tier);
    assert_eq!(t.rows[1].name_has, NameHas::Grade);
    assert!(t.rows[1].copy && t.rows[1].tier.is_none() && t.rows[1].cache.is_none());
    assert_eq!(t.rows[2].cache, Some(CacheKind::SubNode));
    assert_eq!(t.rows[2].name_has, NameHas::Word("mixed case".to_owned()));
    let off = format!("{text}off\t*\t1\t*\t0\t1\t1\t0\tw\n");
    assert!(!Table::parse(&off, &[]).expect("parses").enabled);
    // A bad row is an error naming its line; a wrong header is an error.
    for bad in [
        "9\t*\t1\t*\t5\t1\t1\t0\tw",
        "*\twarehouse\t1\t*\t5\t1\t1\t0\tw",
        "*\t*\t1\t*\t2000000\t1\t1\t0\tw",
        "*\t*\t1\t*\t5\t0\t1\t0\tw",
        "*\t*\t1\t*\t5\t3\t2\t0\tw",
        "*\t*\t1\t*\t5\t1\t1\t2\tw",
        "*\t*\t0\t*\t5\t1\t1\t0\tw",
        "*\t*\t1\t\t5\t1\t1\t0\tw",
    ] {
        let e = Table::parse(&format!("{HEAD}{bad}\n"), &[]).expect_err(bad);
        assert!(e.contains("line"), "{e}");
    }
    assert!(Table::parse("tier\tcache\n*\t*\n", &[]).is_err());
}

#[test]
fn the_wreck_map_keeps_only_the_entity_groups_that_have_a_wreck() {
    let lines = vec![
        format!("{CACHE}\t{GROUP_CACHE}\t11\t\t0.0\t\t\t\t\t\t\t{WRECK}\t1\t0"),
        format!(
            "{WAVE_NPC}\t{}\t11\t\t0.0\t\t\t\t\t\t\t\t1\t0",
            WRECK_GROUPS[0]
        ),
        format!("{ITEM_A}\t{G_ITEM}\t17\t\t0.0\t\t\t\t\t\t\t{WRECK}\t1\t1"),
    ];
    let t = Table::parse(HEAD, &lines).expect("parses");
    assert_eq!(t.wreck_of(CACHE), Some(WRECK));
    assert_eq!(t.wreck_of(WAVE_NPC), None, "no wreck column");
    assert_eq!(t.wreck_of(ITEM_A), None, "not an entity group");
    assert!(Table::parse(HEAD, &["x\ty".to_owned()]).is_err());
}

#[test]
fn a_cache_is_known_by_the_words_of_its_name() {
    assert_eq!(
        cache_kind("Some Bioadaptive Cache"),
        Some(CacheKind::Bioadaptive)
    );
    assert_eq!(cache_kind("X Extraction SubNode"), Some(CacheKind::SubNode));
    assert_eq!(cache_kind("X Extraction Node"), Some(CacheKind::Node));
    assert_eq!(cache_kind("X Extraction Nexus"), Some(CacheKind::Nexus));
    assert_eq!(cache_kind("Some Other Cache"), None);
}

fn types() -> Vec<(i64, i64, &'static str, bool)> {
    vec![
        (30, G_ITEM, "Invented Alpha Widget", true),
        (10, G_ITEM, "Invented Beta Widget", true),
        (20, G_ITEM, "Invented Gamma Widget", false),
        (40, G_PLAN, "Invented Alpha Plan", true),
    ]
}

fn a_row(group: i64, name_has: NameHas) -> LootRow {
    LootRow {
        tier: None,
        cache: None,
        group,
        name_has,
        chance_ppm: PPM,
        qty: (1, 1),
        copy: false,
    }
}

#[test]
fn a_row_draws_one_of_its_published_matches_and_never_an_unpublished_one() {
    let words = BTreeMap::new();
    let any = candidates(&a_row(G_ITEM, NameHas::Any), 0, &types(), &words);
    assert_eq!(any, [10, 30], "ascending, the unpublished one left out");
    let alpha = candidates(
        &a_row(G_ITEM, NameHas::Word("alpha".into())),
        0,
        &types(),
        &words,
    );
    assert_eq!(alpha, [30]);
    let none = candidates(
        &a_row(G_ITEM, NameHas::Word("gamma".into())),
        0,
        &types(),
        &words,
    );
    assert!(none.is_empty(), "the only gamma is unpublished");
    // Over many draws a hit lands on a published type only.
    let t = Table::parse(
        &format!("{HEAD}*\t*\t{G_ITEM}\t*\t1000000\t1\t1\t0\tw\n"),
        &[],
    )
    .unwrap();
    let cands = |r: &LootRow| candidates(r, 0, &types(), &words);
    let mut seen = BTreeSet::new();
    for n in 0..200 {
        let mut s = Stream::new(n);
        for x in roll(&t, 0, CacheKind::Node, &cands, &mut s) {
            seen.insert(x.type_id);
        }
    }
    assert_eq!(seen, BTreeSet::from([10, 30]));
}

#[test]
fn the_tier_token_matches_the_key_word_of_the_played_tier() {
    let words = tier_words([
        (1, "Fast Alpha Filament"),
        (1, "Fast Beta Filament"),
        (1, "placeholder_key"),
        (4, "Slow Alpha Filament"),
        (4, "Slow Beta Filament"),
    ]);
    assert_eq!(words.get(&1).map(String::as_str), Some("fast"));
    assert_eq!(words.get(&4).map(String::as_str), Some("slow"));
    assert_eq!(words.get(&2), None);
    let keys = [
        (11, G_ITEM, "Fast Alpha Filament", true),
        (12, G_ITEM, "Slow Alpha Filament", true),
        (13, G_ITEM, "Fast Beta Filament", true),
    ];
    let row = a_row(G_ITEM, NameHas::Tier);
    assert_eq!(candidates(&row, 1, &keys, &words), [11, 13]);
    assert_eq!(candidates(&row, 4, &keys, &words), [12]);
    assert!(
        candidates(&row, 2, &keys, &words).is_empty(),
        "no key word for tier 2"
    );
}

#[test]
fn the_grade_token_follows_the_tier() {
    let rows = [
        (1, G_ITEM, "Decayed Thing", true),
        (2, G_ITEM, "Gravid Thing", true),
        (3, G_ITEM, "Unstable Thing", true),
    ];
    let row = a_row(G_ITEM, NameHas::Grade);
    let w = BTreeMap::new();
    let by = |tier| candidates(&row, tier, &rows, &w);
    assert_eq!((by(0), by(1)), (vec![1], vec![1]));
    assert_eq!((by(2), by(3)), (vec![2], vec![2]));
    assert_eq!((by(4), by(6)), (vec![3], vec![3]));
}

#[test]
fn a_roll_is_a_function_of_the_seed_and_the_npc() {
    let t = Table::parse(&rows(), &[]).unwrap();
    let w = BTreeMap::new();
    let ty = [
        (ITEM_A, G_ITEM, "Alpha", true),
        (ITEM_B, G_ITEM, "Beta", true),
        (PLAN, G_PLAN, "Plan", true),
    ];
    let go = |npc: u64| {
        let mut s = seed::stream(7, Kind::Loot, &[STREAM_TAG, npc]);
        roll(
            &t,
            2,
            CacheKind::Bioadaptive,
            &|r| candidates(r, 2, &ty, &w),
            &mut s,
        )
    };
    assert_eq!(go(5), go(5));
    let first = go(0);
    assert!(
        (1..200).any(|n| go(n) != first),
        "another npc id gives another result somewhere in 200"
    );
    // The always rows are always there; the never row never is.
    for n in 0..50 {
        let r = go(n);
        assert_eq!(r.iter().filter(|x| x.runs.is_some()).count(), 1);
        assert!(
            r.iter()
                .any(|x| x.type_id == PLAN && x.runs == Some(3) && x.qty == 1)
        );
        assert!(
            r.iter()
                .filter(|x| x.runs.is_none())
                .all(|x| (2..=4).contains(&x.qty))
        );
    }
    // A cache of another kind rolls only its own rows.
    let mut s = Stream::new(1);
    let node = roll(
        &t,
        2,
        CacheKind::Node,
        &|r| candidates(r, 2, &ty, &w),
        &mut s,
    );
    assert!(node.iter().all(|x| x.type_id == ITEM_A));
    assert_eq!(node.len(), 1);
    // An off table drops nothing.
    let mut off = t.clone();
    off.enabled = false;
    let mut s = Stream::new(1);
    assert!(
        roll(
            &off,
            2,
            CacheKind::Bioadaptive,
            &|r| candidates(r, 2, &ty, &w),
            &mut s
        )
        .is_empty()
    );
}

#[test]
fn a_row_does_not_shift_the_draws_of_the_rows_after_it() {
    let w = BTreeMap::new();
    let ty = [
        (ITEM_A, G_ITEM, "Alpha", true),
        (PLAN, G_PLAN, "Plan", true),
    ];
    let run = |first_chance: u64| {
        let text = format!(
            "{HEAD}*\t*\t{G_ITEM}\t*\t{first_chance}\t1\t5\t0\tw\n*\t*\t{G_PLAN}\t*\t1000000\t1\t9\t1\tw\n"
        );
        let t = Table::parse(&text, &[]).unwrap();
        let mut s = Stream::new(99);
        roll(
            &t,
            0,
            CacheKind::Node,
            &|r| candidates(r, 0, &ty, &w),
            &mut s,
        )
        .into_iter()
        .find(|x| x.type_id == PLAN)
    };
    assert_eq!(run(0), run(1_000_000));
}

#[test]
fn the_cache_stands_on_the_loot_locator_or_between_entry_and_exit() {
    let sp = crate::abyss::spots(&loot_room(ROOM_A, true)).expect("a room");
    assert_eq!(
        cache_spot(&loot_room(ROOM_A, true), sp),
        SPOT,
        "the lowest-id locator"
    );
    let plain = loot_room(ROOM_A, false);
    let sp = crate::abyss::spots(&plain).expect("a room");
    let at = cache_spot(&plain, sp);
    for (i, c) in at.iter().enumerate() {
        assert!((c - (sp.entry[i] + sp.exit[i]) / 2.0).abs() < 1e-9);
    }
}

#[test]
fn the_nodes_of_a_room_are_its_objects_of_the_cache_group_in_order() {
    let d = loot_room(ROOM_A, true);
    let group = |t: i64| match t {
        NODE | SUBNODE => Some(GROUP_CACHE),
        _ => Some(1),
    };
    assert_eq!(nodes(&d, &group), [(NODE, NODE_AT), (SUBNODE, SUBNODE_AT)]);
    assert!(nodes(&d, &|_| None).is_empty());
}

#[test]
fn the_shipped_table_parses_and_is_all_authored_groups_and_words() {
    let t = Table::parse(SHIPPED, &[]).expect("the shipped table parses");
    assert!(t.enabled && !t.rows.is_empty());
    assert!(t.rows.iter().all(|r| r.group > 0 && r.chance_ppm <= PPM));
    // Every played tier has something to draw from a bioadaptive cache.
    for tier in crate::abyss::TIERS {
        assert!(
            t.rows.iter().any(|r| {
                r.tier.is_none_or(|x| x == tier)
                    && r.cache.is_none_or(|c| c == CacheKind::Bioadaptive)
            }),
            "tier {tier}"
        );
    }
    // The text is the file the header comment says: the lint's own header.
    assert!(SHIPPED.lines().any(|l| l == HEADER));
}

// ---- the world --------------------------------------------------------------

#[test]
fn a_room_spawns_one_cache_and_its_nodes_and_they_do_not_hold_the_gate() {
    let reg = loot_world(&rows(), vec![]);
    let p = pocket(&reg, ALICE);
    assert_eq!(p.caches.len(), 3, "the cache and the two nodes");
    assert_eq!(p.caches[0].type_id, CACHE);
    assert_eq!(p.caches[0].kind, CacheKind::Bioadaptive);
    assert_eq!(
        (p.caches[1].type_id, p.caches[2].type_id),
        (NODE, SUBNODE),
        "dungeon order"
    );
    assert_eq!(npc_at(&reg, p.caches[0].npc), SPOT);
    assert_eq!(npc_at(&reg, p.caches[1].npc), NODE_AT);
    assert_eq!(npc_at(&reg, p.caches[2].npc), SUBNODE_AT);
    let live = alive(&reg, p.site_id);
    assert_eq!(live.len(), 3);
    for id in live {
        let n = reg.npcs().get(id).expect("an npc");
        assert_eq!(n.kind.group_id, GROUP_CACHE);
        assert_eq!(n.kind.owner_corp_id, WAVE_OWNER);
        assert_eq!(n.solar_system_id, p.system);
        assert!(reg.ext::<AbyssState>().cache_of.contains_key(&id));
    }
    assert!(
        crate::abyss::room_clear(&reg, p.site_id),
        "none is a target"
    );
    assert!(p.cleared && p.gate != 0, "the gate stands with them alive");
    // The static layout does not also place the nodes.
    let inst = reg
        .ext::<crate::site::Sites>()
        .get(p.site_id)
        .expect("an instance");
    assert!(
        inst.objects
            .iter()
            .all(|o| o.type_id != NODE && o.type_id != SUBNODE)
    );
    // Their kind carries the bundle's wreck.
    assert_eq!(
        reg.npcs().get(p.caches[0].npc).unwrap().kind.wreck_type_id,
        Some(WRECK)
    );
}

#[test]
fn a_room_without_a_locator_puts_its_cache_between_entry_and_exit() {
    let mut reg = world().with_content(content(&rows(), false));
    reg.ext_mut::<AbyssState>().waves = Some(vec![]);
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).expect("a run");
    let p = pocket(&reg, ALICE);
    let mid = [
        (p.entry[0] + p.exit_at[0]) / 2.0,
        (p.entry[1] + p.exit_at[1]) / 2.0,
        (p.entry[2] + p.exit_at[2]) / 2.0,
    ];
    assert_eq!(npc_at(&reg, p.caches[0].npc), mid);
}

#[test]
fn a_bundle_without_the_cache_types_places_none() {
    let reg = {
        let mut reg = world();
        reg.ext_mut::<AbyssState>().waves = Some(vec![]);
        stand(&mut reg, ALICE, HOME, [0.0; 3]);
        begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).expect("a run");
        reg
    };
    assert!(pocket(&reg, ALICE).caches.is_empty());
    assert!(reg.ext::<AbyssState>().cache_of.is_empty());
}

#[test]
fn a_killed_cache_leaves_a_wreck_with_its_rolled_loot() {
    let mut reg = loot_world(&rows(), vec![]);
    let npc = cache_npc(&reg);
    // What `roll` draws for this seed and this NPC, worked out from the
    // test's own type list.
    let expect = {
        let t = content(&rows(), true);
        let table = t.get::<Table>();
        let run = run_of(&reg, ALICE).expect("a run").id;
        let ty = [
            (ITEM_A, G_ITEM, "Invented Alpha Widget", true),
            (ITEM_B, G_ITEM, "Invented Beta Widget", true),
            (ITEM_HIDDEN, G_ITEM, "Invented Hidden Widget", false),
            (PLAN, G_PLAN, "Invented Gamma Plan", true),
        ];
        let w = BTreeMap::new();
        let mut s = seed::stream(
            reg.world().seed,
            Kind::Loot,
            &[STREAM_TAG, npc as u64, run, 1],
        );
        roll(
            table,
            2,
            CacheKind::Bioadaptive,
            &|r| candidates(r, 2, &ty, &w),
            &mut s,
        )
    };
    assert_eq!(expect.len(), 2, "a stack and a copy");
    let wreck = kill_cache(&mut reg, npc);
    let mut want: Vec<(i64, i64)> = expect.iter().map(|x| (x.type_id, x.qty)).collect();
    want.sort_unstable();
    assert_eq!(held(&reg, wreck), want);
    assert!(
        held(&reg, wreck).iter().all(|(t, _)| *t != ITEM_HIDDEN),
        "never the unpublished one"
    );
    // The cache is spent: the slot says so and the hook forgot it.
    let p = pocket(&reg, ALICE);
    assert!(p.caches[0].killed && !p.caches[1].killed);
    assert!(!reg.ext::<AbyssState>().cache_of.contains_key(&npc));
    // The loot is owned as the wreck is.
    let owner = reg
        .combat()
        .wrecks()
        .find(|w| w.wreck_id == wreck)
        .expect("the record")
        .owner_id;
    assert!(reg.store().contents_of(wreck).all(|i| i.owner_id == owner));
}

#[test]
fn a_node_leaves_the_loot_of_its_own_kind() {
    let mut reg = loot_world(&rows(), vec![]);
    let node = pocket(&reg, ALICE).caches[1].npc;
    let wreck = kill_cache(&mut reg, node);
    assert_eq!(held(&reg, wreck), [(ITEM_A, 1)]);
    // The subnode has no row: an empty wreck, and no complaint.
    let sub = pocket(&reg, ALICE).caches[2].npc;
    let wreck = kill_cache(&mut reg, sub);
    assert!(held(&reg, wreck).is_empty());
}

#[test]
fn a_copy_row_gives_one_blueprint_copy_with_its_runs() {
    let mut reg = loot_world(&rows(), vec![]);
    let npc = cache_npc(&reg);
    let wreck = kill_cache(&mut reg, npc);
    let copies: Vec<_> = reg
        .store()
        .contents_of(wreck)
        .filter(|i| i.type_id == PLAN)
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].quantity, 1);
    let state = reg
        .jobs()
        .blueprints()
        .get(copies[0].item_id)
        .expect("the copy has a blueprint state");
    assert_eq!(state.runs_remaining, 3);
}

#[test]
fn a_record_only_wreck_gets_no_loot_and_nothing_panics() {
    let mut reg = loot_world(&rows(), vec![]);
    let run = run_of(&reg, ALICE).expect("a run").id;
    let sub = pocket(&reg, ALICE).caches[1].npc;
    let node = pocket(&reg, ALICE).caches[2].npc;
    for npc in [sub, node] {
        let p = reg.roster().get(npc).copied().expect("in space");
        crate::combat::insert_wreck(
            &mut reg,
            crate::combat::Wreck {
                wreck_id: 77_000_000 + npc,
                type_id: WRECK,
                solar_system_id: p.solar_system_id,
                position: p.motion.position,
                ship_id: npc,
                owner_id: CORP,
                corp_id: CORP,
                fleet_id: None,
                abandoned: false,
                expires_at: NOW,
            },
        );
        gone(&mut reg, npc);
        on_npc_kill(&mut reg, &event(npc, p.solar_system_id));
        assert!(
            reg.store().contents_of(77_000_000 + npc).next().is_none(),
            "nothing in a wreck the store does not hold"
        );
    }
    assert!(reg.ext::<AbyssState>().warned.contains(&run), "logged once");
}

#[test]
fn another_npc_dying_is_not_a_cache() {
    let mut reg = loot_world(&rows(), vec![]);
    let before = reg.store().items().len();
    let stray = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, ABY[0], [0.0; 3]).unwrap();
    on_npc_kill(&mut reg, &event(stray, ABY[0]));
    assert_eq!(reg.store().items().len(), before);
    assert_eq!(
        pocket(&reg, ALICE)
            .caches
            .iter()
            .filter(|c| c.killed)
            .count(),
        0
    );
}

#[test]
fn a_cache_never_fires_and_never_moves() {
    let mut reg = loot_world(&rows(), vec![]);
    let p = pocket(&reg, ALICE);
    // Not in a capsule, which no NPC attacks.
    reg.character_mut(ALICE).expect("a character").ship_type_id = 990_965;
    // The control: an armed rat beside the cache does acquire the pilot in
    // this very world, so a cache that does not is not just out of view.
    let control = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, p.system, SPOT).unwrap();
    crate::grid::reconcile_in(&mut reg, p.system);
    let before: Vec<[f64; 3]> = p.caches.iter().map(|c| npc_at(&reg, c.npc)).collect();
    let t0 = crate::pyutil::filetime_now();
    for i in 0..10 {
        crate::npc::tick(&mut reg, t0 + i * 20_000_000);
    }
    assert_eq!(
        reg.npcs().get(control).and_then(|n| n.target),
        Some(ship(ALICE)),
        "the control rat locks the pilot"
    );
    for (c, at) in p.caches.iter().zip(before) {
        let n = reg.npcs().get(c.npc).copied().expect("still there");
        assert_eq!(n.target, None, "never locks a pilot");
        assert!(!n.firing);
        assert_eq!(npc_at(&reg, c.npc), at, "never moves");
    }
}

#[test]
fn abyss_clear_leaves_the_cache() {
    let waves = vec![row(2, 1, 1, rat(), 2)];
    let mut reg = loot_world(&rows(), waves);
    let p = pocket(&reg, ALICE);
    assert_eq!(
        alive(&reg, p.site_id).len(),
        5,
        "two rats, a cache, two nodes"
    );
    let said = slash(&mut reg, Some(ALICE), &["clear"]).unwrap();
    let said = String::from_utf8_lossy(crate::pyutil::as_bytes(&said).unwrap()).into_owned();
    assert!(said.contains("2 NPC(s)"), "{said}");
    assert_eq!(alive(&reg, p.site_id).len(), 3, "the cache and nodes stand");
    tick(&mut reg, Instant::now(), NOW + 1);
    let p = pocket(&reg, ALICE);
    assert!(
        p.cleared && p.gate != 0,
        "the gate comes with the cache alive"
    );
    assert_eq!(alive(&reg, p.site_id).len(), 3);
}

#[test]
fn a_room_made_again_respawns_only_the_caches_not_killed() {
    let waves = vec![row(2, 1, 1, rat(), 1), row(2, 1, 2, rat(), 1)];
    let mut reg = loot_world(&rows(), waves);
    let old = pocket(&reg, ALICE);
    // The node is shot (with the hook); the subnode is gone without it.
    kill_cache(&mut reg, old.caches[1].npc);
    gone(&mut reg, old.caches[2].npc);
    assert!(crate::site::vacate(&mut reg, old.site_id));
    tick(&mut reg, Instant::now(), NOW + 1);
    let p = pocket(&reg, ALICE);
    assert!(!p.cleared, "wave one is still to be killed");
    let flags: Vec<bool> = p.caches.iter().map(|c| c.killed).collect();
    assert_eq!(flags, [false, true, true]);
    let mut types: Vec<i64> = alive(&reg, p.site_id)
        .into_iter()
        .map(|id| reg.npcs().get(id).unwrap().kind.type_id)
        .filter(|t| [CACHE, NODE, SUBNODE].contains(t))
        .collect();
    types.sort_unstable();
    assert_eq!(types, [CACHE], "the cache only, and once");
    assert_eq!(reg.ext::<AbyssState>().cache_of.len(), 1);
    assert_eq!(npc_at(&reg, p.caches[0].npc), SPOT);
}

#[test]
fn a_cleared_room_made_again_keeps_its_cache_and_tags_it_again() {
    let mut reg = loot_world(&rows(), vec![row(2, 1, 1, rat(), 1)]);
    let site_id = pocket(&reg, ALICE).site_id;
    for id in alive(&reg, site_id) {
        if reg.npcs().get(id).is_some_and(|n| n.kind.type_id == rat()) {
            reg.npcs_mut().remove(id);
        }
    }
    tick(&mut reg, Instant::now(), NOW + 1);
    let old = pocket(&reg, ALICE);
    assert!(old.cleared);
    assert!(crate::site::vacate(&mut reg, old.site_id));
    assert!(
        alive(&reg, old.site_id).is_empty(),
        "the cold sweep dropped the tags"
    );
    tick(&mut reg, Instant::now(), NOW + 2);
    let p = pocket(&reg, ALICE);
    assert_eq!(alive(&reg, p.site_id).len(), 3, "tagged again");
    assert_eq!(
        p.caches[0].npc, old.caches[0].npc,
        "the same cache, not a second"
    );
    assert_eq!(reg.ext::<AbyssState>().cache_of.len(), 3);
}

#[test]
fn a_room_with_no_waves_made_again_stands_its_caches_once() {
    let mut reg = loot_world(&rows(), vec![]);
    let old = pocket(&reg, ALICE);
    assert!(crate::site::vacate(&mut reg, old.site_id));
    tick(&mut reg, Instant::now(), NOW + 1);
    let p = pocket(&reg, ALICE);
    assert_eq!(alive(&reg, p.site_id).len(), 3);
    assert_eq!(reg.ext::<AbyssState>().cache_of.len(), 3);
    let in_system = reg
        .npcs()
        .iter()
        .filter(|n| n.solar_system_id == p.system)
        .count();
    assert_eq!(in_system, 3, "no second set");
}

#[test]
fn a_closed_pocket_takes_its_wrecks_and_the_system_is_free_again() {
    let mut reg = loot_world(&rows(), vec![]);
    let p = pocket(&reg, ALICE);
    let wreck = kill_cache(&mut reg, p.caches[0].npc);
    assert!(reg.combat().wrecks().any(|w| w.wreck_id == wreck));
    let run = run_of(&reg, ALICE).expect("a run").clone();
    close_run(&mut reg, run);
    assert!(!reg.combat().wrecks().any(|w| w.solar_system_id == p.system));
    assert!(reg.store().get(wreck).is_none(), "the wreck item went too");
    assert!(
        reg.ext::<AbyssState>().cache_of.is_empty(),
        "no cache is remembered"
    );
    // The system is the lowest free one again: the next run gets it.
    stand(&mut reg, BOB, HOME, [0.0; 3]);
    begin(&mut reg, BOB, Mode::Solo, 2, 0, 0, 1, NOW).expect("a run");
    assert_eq!(pocket(&reg, BOB).system, p.system);
}

#[test]
fn a_wave_type_with_a_wreck_in_the_map_keeps_it_and_the_rest_falls_back() {
    let mut reg = loot_world(&rows(), vec![]);
    let k = kind_for(&mut reg, WAVE_NPC).expect("derived from the type data");
    assert_eq!(k.wreck_type_id, Some(WRECK));
    assert_eq!(k.owner_corp_id, WAVE_OWNER);
    let again = kind_for(&mut reg, WAVE_NPC).expect("kept");
    assert!(std::ptr::eq(k, again), "derived once");
    // A hand type, and one the type data lacks.
    let hand = kind_for(&mut reg, rat()).expect("the fixture rat");
    assert!(std::ptr::eq(
        hand,
        crate::npc::resolve_law(&mut reg, rat(), WAVE_OWNER).unwrap()
    ));
    assert_eq!(hand.wreck_type_id, crate::npc::FIXTURE_RAT.wreck_type_id);
    assert!(kind_for(&mut reg, 990_999).is_none());
}

#[test]
fn a_pocket_for_waves_is_unchanged_when_there_are_no_cache_types() {
    // The wave tests' world: no cache in the type data, so nothing is added.
    let mut reg = world();
    reg.ext_mut::<AbyssState>().waves = Some(vec![row(2, 1, 1, rat(), 2)]);
    stand(&mut reg, ALICE, HOME, [0.0; 3]);
    begin(&mut reg, ALICE, Mode::Solo, 2, 0, 0, 1, NOW).unwrap();
    let p: Pocket = pocket(&reg, ALICE);
    assert!(p.caches.is_empty());
    assert_eq!(alive(&reg, p.site_id).len(), 2);
}
