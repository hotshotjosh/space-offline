//! **#18 over a real socket: a camping heavy interdictor scripts a focused
//! point on a ship no bubble holds.**
//!
//! Two pilots in a null-sec system on an in-process server. The HIC has the
//! warp disruption field generator fitted and the focused script in its hold;
//! the prey flies a hull whose type carries `warpBubbleImmune`. Everything is
//! invented: type ids 990,164,xxx, a system security of -0.5, the field's
//! numbers. Not invented, and no client value: the bundle's names for the field
//! generator and the script (`camp::FIELD_NAME`, `SCRIPT_NAME`, which the bot
//! looks them up by), the dogma ids the server reads (attribute 1538 on the
//! prey's hull, effect 3380, script group 908 and effect 6849) and the slot
//! flags. A third ship, on a hull a bubble does hold, sits nearer than the
//! prey: the point must go to the prey, not to the nearest.
//!
//! **What runs for real.** The bot's observation is read off the socket the
//! way the driver reads it (the hold, the fitted module's row, the script
//! slot, the park); the bundle reads are `camp::immune_types_in`,
//! `script_type_in` and `field_group_in`; the decisions are
//! `camp::acts`, the camp turn the fight calls; each `Act` it returns is
//! carried out with the verbs the driver's arms call (`Conn::load_ammo`,
//! `Conn::lock_target`, `Conn::activate`). The prey's warp is then asked for
//! on his own socket.
//!
//! **What is skipped, and said.** The whole bot (`driver::spawn`) is not run:
//! it would need a doctrine fit, a dev grant and a gate to camp at, none of
//! which is what this proves. The driver's `kit_act` arm is private, so its
//! three calls are made here by hand, with the same arguments (read from the
//! `Act`). The camp asks for the immune ship's lock itself, because the
//! fight (not run here) locks only the nearest hostile.

use eve_bot::brain::{Act, Ball, Memory, Observation, ParkView, Place, Policy, ShipInfo};
use eve_bot::camp;
use eve_bot::doctrine::Role;
use eve_bot::hunt::{Ground, Pilot};
use eve_bot::pvp_kit::{FieldScript, Kit, KitAct, KitModule};
use eve_bot::{Conn, REPEAT_UNTIL_STOPPED};
use eve_server::group_contacts::Affiliation;
use eve_server::model::Character;
use eve_server::server::{Config, Server};
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const HIC: i64 = 90_164_001;
const PREY: i64 = 90_164_002;
const HIC_SHIP: i64 = 100_164_001;
const PREY_SHIP: i64 = 100_164_002;
/// A friend on the grid: a lone HIC is a feed and the camp will not run.
const FRIEND_BALL: i64 = 100_164_003;

/// Invented types.
const HIC_HULL: i64 = 990_164_001;
const PREY_HULL: i64 = 990_164_002;
const FIELD_GEN: i64 = 990_164_003;
const SCRIPT: i64 = 990_164_004;
const GROUP_FIELD: i64 = 990_164;
const GROUP_SCRIPT: i64 = camp::GROUP_SCRIPT;
const DECOY_HULL: i64 = 990_164_005;
const DECOY_BALL: i64 = 100_164_004;

/// The key the server refuses a warp with under a point (`tackle::WARP_SCRAMBLED`;
/// a bubble's own are `WarpDisrupted` and `WarpScrambledByWarpDisruptionFieldGenerator`).
const POINT_REFUSAL: &str = "WarpScrambled";

/// Invented numbers: the field's radius, metres, and its cycle, ms.
const RADIUS: f64 = 20_000.0;
const CYCLE_MS: i64 = 30_000;
/// A belt a long warp from the station, for the prey's warp.
const BELT: i64 = 40_164_001;
const BELT_TYPE: i64 = 15;
const NULL_SEC: f64 = -0.5;

/// `flagHiSlot0` and `flagCargo` **[V]**.
const HI_SLOT_0: i64 = 27;
const CARGO: i64 = 5;

fn content() -> Arc<eve_server::content::Content> {
    let home = Character::default();
    let system = home.solar_system_id.expect("a home system");
    let universe = eve_server::celestial::Universe::parse(
        &[format!(
            "{system}\t{}\t{}\t\t\t{NULL_SEC}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {system}",
            home.region_id, home.constellation_id
        )],
        &[format!("{}\t\t\t\t\tFixture region", home.region_id)],
        &[format!(
            "{}\t{}\t\t\t\tFixture constellation",
            home.constellation_id, home.region_id
        )],
    )
    .expect("the invented rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("universe", universe);
    let rows = format!(
        "{}type\t{HIC_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=300.0;70=3.1;76=21000.0;263=420.0;265=440.0;552=100.0;600=5.0\tInvented Hull\n\
         type\t{PREY_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=300.0;70=3.1;76=21000.0;263=420.0;265=440.0;552=100.0;600=5.0;1538=1.0\t\
         Invented Bubble Immune Hull\n\
         type\t{DECOY_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=300.0;70=3.1;76=21000.0;263=420.0;265=440.0;552=100.0;600=5.0\tInvented Bubbleable Hull\n\
         type\t{FIELD_GEN}\t{GROUP_FIELD}\t7\t1\t1\t50.0\t1.0\t0.0\t1.0\t3380\t12,16,3380\t\
         73={CYCLE_MS}.0;103={RADIUS};105=2.0;604={GROUP_SCRIPT}.0\t{}\n\
         type\t{SCRIPT}\t{GROUP_SCRIPT}\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t6849\t6849\t\t{}\n\
         attribute\t1974\t1.0\tgateScrambleStrength\n\
         effect\t3380\t1\t1\t73\t0\t103\t0\t0\twarpDisruptSphere\teffects.WarpDisruptFieldGenerating\t0\n\
         effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t\n",
        eve_server::typedata::FIXTURE,
        camp::FIELD_NAME,
        camp::SCRIPT_NAME,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(c)
}

fn start() -> (SocketAddr, Arc<RwLock<Registry>>) {
    let home = Character::default();
    let system = home.solar_system_id.expect("a home system");
    let station = home.station_id.expect("docked");
    let pilot = |id: i64, name: &str, ship: i64, hull: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ship_type_id: hull,
        ..Character::default()
    };
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.sandbox.characters = vec![
        pilot(HIC, "Hic164", HIC_SHIP, HIC_HULL),
        pilot(PREY, "Prey164", PREY_SHIP, PREY_HULL),
    ];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{system}\t{}\t{}\n\
         station\t{station}\t{}\t5000.0\t0.0\t0.0\t0.0\t{system}\t0\t0\n\
         belt\t{BELT}\t{BELT_TYPE}\t0.0\t30000000.0\t0.0\t0.0\t{system}\t0\t0\n",
        home.constellation_id, home.region_id, home.station_type_id
    ));
    config.content = Some(content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn login(addr: SocketAddr, name: &str) -> Conn {
    let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    c.handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    c.select_character_named(name).expect("selects by name");
    c
}

/// A ship's ball as the server holds it (its roster entry).
fn ball_at(world: &RwLock<Registry>, who: i64, ship: i64) -> Ball {
    let w = world.read().expect("the world");
    let p = w.roster().get(who).expect("the ship is in space");
    Ball {
        id: ship,
        position: p.motion.position,
        radius: 50.0,
    }
}

/// One camp turn's observation, read off the socket as the driver reads it.
/// `locked` is what the bot has locked.
fn observe(
    conn: &mut Conn,
    m: &Memory,
    now: Instant,
    ego: Ball,
    hostiles: &[(Ball, i64)],
    locked: Vec<i64>,
) -> Observation {
    let content = content();
    let ship_id = conn.session().ship_id().expect("a ship");
    let mut o = Observation::empty(now);
    o.place = Place::InSpace {
        system: Character::default().solar_system_id.expect("a system"),
        bound: true,
    };
    o.ship = Some(ShipInfo {
        ship_id,
        type_id: Some(HIC_HULL),
    });
    o.cargo = conn.cargo().expect("the hold");
    o.locked = locked;
    o.lock_range = 100_000.0;
    o.park = Some(ParkView {
        ego,
        ego_warping: false,
        rocks: Vec::new(),
        belts: Vec::new(),
        station: None,
        boss: None,
        taken: Default::default(),
        hostiles: Vec::new(),
        wrecks: Vec::new(),
        ego_tether: None,
    });
    // The fitted field generator and its script slot (driver.rs, BP-KIT).
    let rows = conn.ship_items(ship_id, None).expect("the ship's items");
    let field_row = rows
        .iter()
        .find(|r| r.is_fitted() && camp::is_field(m, r))
        .expect("the field generator is fitted and known by its group");
    let effect = eve_bot::modules::find(Some(&content), field_row.type_id)
        .and_then(|(t, _)| t.effect)
        .expect("the field's default effect");
    let info = conn.module_row(field_row.item_id).expect("ItemGetInfo");
    let from_type = eve_bot::modules::find(Some(&content), field_row.type_id).map(|(t, _)| t);
    let mut kit = Kit::default();
    kit.push(KitModule::read(
        field_row.item_id,
        field_row.type_id,
        &effect,
        info.effects.contains(&effect.effect_id),
        info.effects.contains(&eve_bot::EFFECT_ONLINE),
        |a| {
            info.attribute(a)
                .or_else(|| from_type.and_then(|t| t.attribute(a)))
        },
    ));
    let loaded = m.camp.script_type.and_then(|t| {
        (conn
            .loaded_rounds(ship_id, field_row.flag_id, t)
            .unwrap_or(0)
            > 0)
        .then_some(t)
    });
    kit.script = Some(FieldScript {
        flag_id: field_row.flag_id,
        loaded,
    });
    o.kit = kit;
    // The grid: a friend, and the prey with the hull the park names.
    let friend = Pilot {
        ball: Ball {
            id: FRIEND_BALL,
            position: [ego.position[0] + 500.0, ego.position[1], ego.position[2]],
            radius: 50.0,
        },
        char_id: 1,
        type_id: None,
        who: Affiliation {
            character_id: 1,
            corporation_id: 1,
            alliance_id: None,
        },
        hostile: false,
        relation: 0.0,
        war: false,
    };
    let mut pilots = vec![friend];
    for (ball, hull) in hostiles {
        pilots.push(Pilot {
            ball: *ball,
            char_id: ball.id,
            type_id: Some(*hull),
            who: Affiliation {
                character_id: ball.id,
                corporation_id: 2,
                alliance_id: None,
            },
            hostile: true,
            relation: 0.0,
            war: false,
        });
    }
    o.hunt.pilots = pilots;
    o
}

fn memory(content: &eve_server::content::Content) -> Memory {
    let system = Character::default().solar_system_id.expect("a system");
    let mut policy = Policy::fast();
    policy.pvp = Some(Arc::new(Ground::new().with_security(system, NULL_SEC)));
    let mut m = Memory::new("Hic164", policy);
    m.combat.role = Some(Role::Interdict);
    // The bundle's reads, at run time.
    m.camp.field_group = camp::field_group_in(content);
    m.camp.script_type = camp::script_type_in(content);
    m.camp.immune = camp::immune_types_in(content);
    m
}

/// The act the server will be asked for, with the driver's own verbs.
fn carry_out(conn: &mut Conn, act: &Act) -> eve_bot::Result<()> {
    match act {
        Act::LoadAmmo {
            guns,
            charges,
            location,
        } => {
            // The type being loaded is the charge's row's (driver.rs).
            let round = conn
                .cargo()?
                .iter()
                .find(|r| Some(&r.item_id) == charges.first())
                .map(|r| r.type_id)
                .expect("the charge is in the hold");
            conn.load_ammo(guns, charges, round, *location).map(|_| ())
        }
        Act::Kit(KitAct::FieldPoint(m, target)) => conn
            .activate(m.item_id, m.effect, Some(*target), REPEAT_UNTIL_STOPPED)
            .map(|_| ()),
        Act::Lock(t) => conn.lock_target(*t).map(|_| ()),
        other => panic!("an act this test does not carry out: {other:?}"),
    }
}

#[test]
fn a_camping_hic_scripts_a_focused_point_on_a_ship_no_bubble_holds() {
    let (addr, world) = start();
    let system = Character::default().solar_system_id.expect("a system");
    let content = content();

    // Both undock and enter space.
    let mut hic = login(addr, "Hic164");
    let mut prey = login(addr, "Prey164");
    // The HIC's kit: the field generator fitted, the script in the hold; the
    // items a fit and a stock would have made.
    {
        let mut w = world.write().expect("the world");
        // The hulls: the seeded ships take the invented types.
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut() {
            let hull = match i.item_id {
                HIC_SHIP => HIC_HULL,
                PREY_SHIP => PREY_HULL,
                _ => continue,
            };
            i.type_id = hull;
            i.group_id = 25;
        }
        w.apply_snapshot(snap);
        eve_server::modules::fit_new(&mut w, HIC, HIC_SHIP, HI_SLOT_0, FIELD_GEN).expect("fitted");
        w.apply_items(&[eve_server::item::Edit::Create {
            type_id: SCRIPT,
            quantity: 1,
            to: eve_server::item::Slot::new(HIC, HIC_SHIP, CARGO),
        }])
        .expect("the items are made");
    }
    hic.undock().expect("the HIC undocks");
    hic.enter_space(system).expect("the HIC's state");
    prey.undock().expect("the prey undocks");
    prey.enter_space(system).expect("the prey's state");
    // Each sees the other.
    let seen = |c: &mut Conn, id: i64| {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            let _ = c.pump(Duration::from_millis(100));
            if c.world().get(id).is_some() {
                return true;
            }
        }
        false
    };
    assert!(seen(&mut hic, PREY_SHIP), "the HIC sees the prey");
    assert!(seen(&mut prey, HIC_SHIP), "the prey sees the HIC");

    // PRECONDITIONS, read from the server: the prey's hull is one no bubble
    // holds, the generator is idle and nothing is loaded.
    let field_id = {
        let w = world.read().expect("the world");
        assert!(
            eve_server::bubble::immune(&w, PREY_SHIP),
            "the prey's hull carries warpBubbleImmune"
        );
        assert!(
            !eve_server::bubble::immune(&w, HIC_SHIP),
            "the HIC's own hull does not"
        );
        let id = w
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == FIELD_GEN && i.owner_id == HIC)
            .expect("the generator")
            .item_id;
        assert!(w.modules().running(id).is_none(), "the generator is idle");
        assert!(
            eve_server::modules::loaded(w.store(), HIC_SHIP, HI_SLOT_0).is_none(),
            "no script is loaded yet"
        );
        id
    };

    let mut m = memory(&content);
    assert!(
        m.camp.immune.contains(&PREY_HULL) && !m.camp.immune.contains(&HIC_HULL),
        "the bundle read marks the prey's hull only"
    );
    let t0 = Instant::now();
    // Place the ships: the HIC at rest, the prey 3 km off, and a third ship
    // (a hull a bubble holds, known to the camp turn only, not a pilot on
    // the server) 1 km off: nearer than the prey.
    {
        let mut w = world.write().expect("the world");
        let at = w.roster().get(HIC).expect("HIC").motion.position;
        for (who, dx) in [(HIC, 0.0), (PREY, 3_000.0)] {
            let mo = &mut w.roster_mut().get_mut(who).expect("in space").motion;
            mo.position = [at[0] + dx, at[1], at[2]];
            mo.velocity = [0.0; 3];
        }
    }
    let ego = ball_at(&world, HIC, HIC_SHIP);
    let prey_ball = ball_at(&world, PREY, PREY_SHIP);
    let decoy = Ball {
        id: DECOY_BALL,
        position: [ego.position[0] + 1_000.0, ego.position[1], ego.position[2]],
        radius: 50.0,
    };
    let seen_prey = (prey_ball, PREY_HULL);
    let seen_decoy = (decoy, DECOY_HULL);
    assert!(
        !m.camp.immune.contains(&DECOY_HULL),
        "the nearer ship's hull is one a bubble holds"
    );

    // Only the bubbleable ship on the grid: the camp runs the bubble and
    // loads no script (a separate memory, so no timer is shared).
    {
        let mut quiet = memory(&content);
        let o = observe(&mut hic, &quiet, t0, ego, &[seen_decoy], vec![]);
        let acts = camp::acts(&mut quiet, &o, &decoy);
        assert!(
            !acts.iter().any(|a| matches!(a, Act::LoadAmmo { .. })),
            "no script is loaded for a ship a bubble holds: {acts:?}"
        );
        assert!(
            matches!(acts[..], [Act::Kit(KitAct::FieldOn(_))]),
            "the bubble runs: {acts:?}"
        );
    }

    // Turn 1: the script is in the hold, none in the generator. The camp
    // loads it (the generator is idle, so nothing is stopped first).
    // `prey` is the nearest hostile, the bubbleable one; the target is not.
    let o = observe(&mut hic, &m, t0, ego, &[seen_prey, seen_decoy], vec![]);
    let acts = camp::acts(&mut m, &o, &decoy);
    // The fight locks only the nearest hostile (the decoy), so the camp
    // asks for the lock of the ship it will point.
    let [Act::LoadAmmo { guns, charges, .. }, Act::Lock(lock)] = &acts[..] else {
        panic!("turn 1 should load the script and lock the immune ship: {acts:?}");
    };
    assert_eq!(*lock, PREY_SHIP, "the lock asked for is the immune ship");
    assert_eq!(guns[0].0, field_id, "the script goes into the generator");
    assert_eq!(charges.len(), 1);
    carry_out(&mut hic, &acts[0]).expect("the script loads");
    carry_out(&mut hic, &acts[1]).expect("the lock is asked for");
    {
        let w = world.read().expect("the world");
        assert_eq!(
            eve_server::modules::loaded(w.store(), HIC_SHIP, HI_SLOT_0).map(|i| i.type_id),
            Some(SCRIPT),
            "the focused disruption script is loaded in the generator"
        );
        assert!(
            w.modules().running(field_id).is_none(),
            "loading a script does not start the generator"
        );
    }

    let end = Instant::now() + Duration::from_secs(30);
    let mut locked = false;
    while Instant::now() < end && !locked {
        let _ = hic.pump(Duration::from_millis(100));
        locked = world
            .read()
            .expect("the world")
            .locks()
            .targets_of(HIC_SHIP)
            .contains(&PREY_SHIP);
    }
    assert!(locked, "the prey is locked");

    // Turn 2 (a later turn, past the reload lockout): the observation now
    // shows the script in the slot and the prey locked; the camp points.
    let t1 = t0 + Duration::from_secs(60);
    let o = observe(
        &mut hic,
        &m,
        t1,
        ego,
        &[seen_prey, seen_decoy],
        vec![PREY_SHIP],
    );
    assert_eq!(
        o.kit.script.and_then(|s| s.loaded),
        m.camp.script_type,
        "the driver's read of the slot sees the script"
    );
    let acts = camp::acts(&mut m, &o, &decoy);
    let [Act::Kit(KitAct::FieldPoint(module, target))] = &acts[..] else {
        panic!("turn 2 should aim the generator and do nothing else: {acts:?}");
    };
    assert_eq!(module.item_id, field_id);
    assert_eq!(
        *target, PREY_SHIP,
        "the point goes to the ship no bubble holds, not the nearer one"
    );
    carry_out(&mut hic, &acts[0]).expect("the generator is activated on the prey");

    // The server's state: running, as a point, on the prey.
    {
        let w = world.read().expect("the world");
        let r = w
            .modules()
            .running(field_id)
            .copied()
            .expect("the generator is active");
        assert_eq!(r.target_id, Some(PREY_SHIP), "its target is the prey");
        assert_eq!(r.effect.effect_id, eve_server::bubble::EFFECT_FIELD);
        assert_eq!(
            r.effect.category,
            eve_server::modules::CATEGORY_TARGET,
            "the scripted generator is a targeted effect, a point"
        );
        assert!(
            eve_server::bubble::zones(&w, system).is_empty(),
            "no sphere stands: this is a point, not a bubble"
        );
    }

    // The prey's own warp, on his socket, is refused with the point's key.
    let _ = prey.pump(Duration::from_millis(300));
    let said = prey
        .warp_to_item(BELT, 0.0)
        .expect("the call is answered")
        .refusal();
    assert_eq!(
        said.as_deref(),
        Some(POINT_REFUSAL),
        "the prey's warp is refused as scrambled by the point"
    );
}
