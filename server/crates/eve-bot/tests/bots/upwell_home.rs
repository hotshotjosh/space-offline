//! BOT-UPWELL-HOME (#212) over a real socket: the director's pass anchors a
//! hull from its hold, waits out the world's anchor time, docks in the
//! structure and takes and releases the controls. Under "nothing from
//! nowhere" the bot itself never calls `/upwell`; the test's setup uses it
//! only as a stand-in for the builder's product (a hull in the cargo) and
//! for a finished structure, as the server's own structure tests do.
//!
//! Invented ids only. The hull type is the server's named constant.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::upwell_home::{Control, Mind, turn};
use eve_marshal::PyValue;
use eve_server::structure_space::RAITARU;
use std::time::Duration;

const ALICE: i64 = 90_000_712;
const MERLIN: i64 = 603;
/// An invented hull with a big hold (the hull's volume is invented).
const HAULER: i64 = 990_000_901;
/// The structure hull's volume, m3: no frigate's hold takes it.
const HULL_VOLUME: f64 = 7_000.0;
/// The shortest anchor time the rules take.
const ANCHOR_SECS: i64 = 60;
/// An invented planet, far out, and its type.
const PLANET: i64 = 40_009_001;
const PLANET_TYPE: i64 = 990_009_001;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// The fixture's tables with the types the pass reads: the hull (group and
/// category the server itself gives structures, an invented radius and name,
/// an invented volume), a hauler, an invented planet type and the sandbox
/// station's type.
fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let station_type = eve_server::model::Character::default().station_type_id;
    let rows = format!(
        "{}type\t{RAITARU}\t{}\t{}\t1\t1\t{HULL_VOLUME}\t0.0\t1000.0\t1000.0\t0\t\t\tInvented Hull Row\n\
         type\t{HAULER}\t513\t6\t1\t1\t1000000.0\t20000.0\t1000.0\t100.0\t0\t\t\tInvented Hauler\n\
         type\t{PLANET_TYPE}\t7\t2\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Planet\n\
         type\t{station_type}\t15\t3\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Station\n",
        eve_server::typedata::FIXTURE,
        eve_server::structure_profile::group_of(RAITARU),
        eve_server::structure_profile::CATEGORY_STRUCTURE,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    c
}

/// The sandbox station where it is (the origin) and one invented planet far
/// from it: a ship beside the station is inside the 1,000 km the server
/// keeps clear round it, and only a planet's warp-in is clear.
fn celestials() -> eve_server::celestial::Catalogue {
    let home = eve_server::model::Character::default();
    let station = home.station_id.expect("the default pilot is docked");
    eve_server::celestial::Catalogue::parse(&format!(
        "station\t{station}\t{}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         planet\t{PLANET}\t{PLANET_TYPE}\t6000000.0\t1500000000.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n",
        home.station_type_id
    ))
}

/// A server with one pilot, a player corporation she is CEO of, and a
/// short anchor time.
fn world(anchor_secs: i64) -> Conn {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_712)];
    config.content = Some(std::sync::Arc::new(content()));
    config.sandbox.celestials = celestials();
    config.sandbox.rules.upwell_anchor_secs = anchor_secs;
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    gm(&mut alice, "/giveisk 2000000000");
    alice
}

fn found_corp(alice: &mut Conn) {
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let a = alice
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp),
            "AddCorporation",
            vec![uni("Home Invented"), uni("HOMI"), uni(""), uni("")],
            vec![],
        )
        .expect("answered");
    assert_eq!(a.refusal(), None, "AddCorporation refused");
}

/// The hangar row of `type_id`.
fn hangar_item(alice: &mut Conn, type_id: i64) -> i64 {
    alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .unwrap_or_else(|| panic!("no {type_id} in the hangar"))
        .item_id
}

/// The builder's product, as the test stands it up: the hull in the hangar
/// (`/upwell give` puts it in the hold of the ship flown, so a frigate carries
/// it for a moment and sets it down), the frigate still the ship flown.
fn hull_in_the_hangar(alice: &mut Conn) {
    gm(alice, "/createitem merlin");
    let merlin = hangar_item(alice, MERLIN);
    alice.board(merlin).expect("boards");
    gm(alice, "/upwell give raitaru");
    let hull = alice
        .cargo()
        .expect("cargo")
        .into_iter()
        .find(|r| r.type_id == RAITARU)
        .expect("a hull in the hold")
        .item_id;
    alice.unload(hull, None).expect("sets the hull down");
    assert!(
        alice
            .hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.type_id == RAITARU),
        "the hull is in the hangar"
    );
}

#[test]
fn a_director_boards_a_hauler_warps_to_a_planet_anchors_waits_docks_and_controls() {
    let mut alice = world(ANCHOR_SECS);
    found_corp(&mut alice);
    hull_in_the_hangar(&mut alice);
    // A ship whose hold takes the invented hull volume, in the hangar beside the frigate.
    gm(&mut alice, &format!("/createitem {HAULER}"));
    let hauler = hangar_item(&mut alice, HAULER);
    let content = content();
    let mut mind = Mind::default();
    let mut notes = Vec::new();
    let mut anchored = false;
    for _ in 0..240 {
        notes.push(turn(&mut alice, Some(&content), &mut mind).expect("a turn"));
        if !alice.corp_structures().expect("lists").is_empty() {
            anchored = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let joined = notes.join("\n");
    assert!(anchored, "no structure after the turns: {joined}");
    for want in [
        "boarded",
        "is in the hold",
        "undocked",
        "warping to planet",
        "anchored hull",
    ] {
        assert!(joined.contains(want), "no {want:?}: {joined}");
    }
    assert_eq!(alice.session().ship_id(), Some(hauler), "{joined}");
    let rows = alice.corp_structures().expect("lists");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].type_id, RAITARU);
    // The hull left the hold.
    let cargo = alice.cargo().expect("cargo");
    assert!(cargo.iter().all(|r| r.type_id != RAITARU), "{cargo:?}");
    // Still anchoring: not a place to dock yet.
    let id = rows[0].structure_id;
    let at = rows[0].system;
    assert_eq!(at, SYSTEM);
    assert!(
        !alice
            .dockable_structures(SYSTEM)
            .expect("lists")
            .contains(&id),
        "dockable before the anchor time"
    );
    let n = turn(&mut alice, Some(&content), &mut mind).expect("a turn");
    assert!(n.contains("waiting"), "{n}");
    assert!(!mind.active, "the bot goes back to its role while it waits");
    // After the anchor time it is.
    let online = until(Duration::from_secs(120), || {
        alice
            .dockable_structures(SYSTEM)
            .map(|d| d.contains(&id))
            .unwrap_or(false)
    });
    assert!(online, "never dockable: {:?}", alice.corp_structures());
    // Then the pass docks, takes and releases the controls.
    let mut notes = Vec::new();
    for _ in 0..60 {
        let n = turn(&mut alice, Some(&content), &mut mind).expect("a turn");
        let done = n.contains("done:");
        notes.push(n);
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let joined = notes.join("\n");
    assert!(joined.contains("done:"), "{joined}");
    assert_eq!(mind.control, Control::Done);
    assert!(mind.finished);
    // Released: the session's ship is the hauler again, not the structure.
    assert_eq!(alice.session().ship_id(), Some(hauler), "{joined}");
}

/// Review finding 3: the hull does not go in a frigate's hold, and the pass
/// neither forces it nor gives up: it waits for a ship that can carry it.
#[test]
fn a_hull_with_no_hauler_waits_and_does_not_give_up() {
    let mut alice = world(ANCHOR_SECS);
    found_corp(&mut alice);
    hull_in_the_hangar(&mut alice);
    let content = content();
    let mut mind = Mind::default();
    for _ in 0..3 {
        let n = turn(&mut alice, Some(&content), &mut mind).expect("a turn");
        assert!(n.contains("no hauler for the hull"), "{n}");
    }
    assert_eq!(mind.gave_up, None);
    assert!(
        alice
            .hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.type_id == RAITARU),
        "the hull stays in the hangar"
    );
}

#[test]
fn a_pilot_docks_in_the_home_and_takes_and_releases_the_controls() {
    let mut alice = world(ANCHOR_SECS);
    found_corp(&mut alice);
    // A finished structure at her station: the stand-in for one a corp
    // anchored earlier.
    gm(&mut alice, "/upwell spawn raitaru Invented Keep");
    let mut mind = Mind::default();
    let mut notes = Vec::new();
    for _ in 0..60 {
        let n = turn(&mut alice, None, &mut mind).expect("a turn");
        let done = n.contains("done:");
        notes.push(n);
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let joined = notes.join("\n");
    assert!(joined.contains("docked in home"), "{joined}");
    assert!(joined.contains("took the controls"), "{joined}");
    assert!(joined.contains("released the controls"), "{joined}");
    assert!(joined.contains("done:"), "{joined}");
    assert_eq!(mind.control, Control::Done);
    // Finding C: after the controls it leaves the structure instead of
    // staying docked in it.
    assert!(joined.contains("left the structure"), "{joined}");
    assert!(alice.session().get_i64("structureid").is_none(), "{joined}");
}

/// Review finding 1: a spot beside the station is refused for where it is,
/// and the refusal's words reach the pass (`CustomNotify` is every refusal's
/// key), so it can try the next planet instead of retrying the same spot.
#[test]
fn a_refused_anchor_carries_the_servers_words() {
    let mut alice = world(ANCHOR_SECS);
    found_corp(&mut alice);
    gm(&mut alice, "/createitem merlin");
    let merlin = hangar_item(&mut alice, MERLIN);
    alice.board(merlin).expect("boards");
    gm(&mut alice, "/upwell give raitaru");
    let hull = alice
        .cargo()
        .expect("cargo")
        .into_iter()
        .find(|r| r.type_id == RAITARU)
        .expect("a hull in the hold")
        .item_id;
    alice.undock().expect("undocks");
    alice.enter_space(SYSTEM).expect("binds");
    let why = alice
        .anchor_structure(hull, 0.0, 0.0, 0.5, "Home Invented", 3, 12)
        .expect_err("a spot beside the station is refused")
        .to_string();
    assert!(why.contains("cannot be anchored within"), "{why}");
    assert_eq!(
        eve_bot::upwell_home::classify(&why),
        eve_bot::upwell_home::Refusal::Placement
    );
}
