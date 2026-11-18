//! The fixture every bot test shares: one in-process server, one poll loop,
//! and the plain-connection helpers a non-bot's setup uses.
//!
//! **Synthetic furniture only** (the no-client-data rule): the rocks here
//! have made-up ids, places and a fixture radius law; the one ore type is
//! Veldspar's id [V-data].

use eve_bot::Conn;
use eve_bot::brain::ActKind;
use eve_bot::doctrine_sense::parse_damage;
use eve_bot::driver::{Entry, Handle, Verdict};
use eve_marshal::PyValue;
use eve_server::server::{Config, Server};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The sandbox's home system, `Character::default()`'s.
pub const SYSTEM: i64 = 30_000_001;
/// Veldspar [V-data]: the ore every fixture rock is.
pub const VELDSPAR: i64 = 1230;
/// An invented ship type a test pilot reports to the fleet's `Init` and
/// `AcceptInvite`: the server only stores it on the member row, so no real
/// hull is needed.
pub const FLEET_SHIP: i64 = 900_000_001;
/// The first fixture rock's id; the next ones count up from it.
pub const ROCK: i64 = eve_server::asteroid::MIN_ROCK_ID;

/// A server config on a free loopback port, nothing else set.
pub fn config() -> Config {
    Config::new("127.0.0.1:0".parse().expect("literal"))
}

/// **The server fixture.** Binds `config`, serves every connection on a
/// thread of its own for the rest of the test binary, and returns where.
pub fn serve(config: Config) -> SocketAddr {
    serve_with_chat(config).0
}

/// [`serve`], and the port chat **actually** bound, when `config` turns it
/// on. Give chat port 0 and read it back here: probing a free port and
/// handing it over races every other test binding at once (tracker §83).
pub fn serve_with_chat(config: Config) -> (SocketAddr, Option<u16>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let chat = server.chat_addrs().first().map(SocketAddr::port);
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, chat)
}

/// A sandbox character in her capsule at the home station.
pub fn pilot(id: i64, name: &str, ship: i64) -> eve_server::model::Character {
    eve_server::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ..eve_server::model::Character::default()
    }
}

/// One Veldspar rock in the home system: `ROCK + i`, `quantity` units, at
/// `position`. A fixture law, not a claim about any ore.
pub fn rock(i: usize, quantity: i64, position: [f64; 3]) -> eve_server::asteroid::Rock {
    let law = eve_server::asteroid::Law {
        growth: 0.5,
        unit_size: 90.0,
        max_radius: 16_255.0,
    };
    eve_server::asteroid::Rock {
        id: ROCK + i as i64,
        type_id: VELDSPAR,
        belt_id: 40_000_003,
        solar_system_id: SYSTEM,
        quantity,
        radius: law.radius(quantity),
        position,
        law,
    }
}

/// Poll until `done` says so or `within` runs out; `false` on the timeout,
/// so the assertion at the call site can say what was waited for.
pub fn until(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// Read `c`'s pushes until `done` holds or `within` runs out.
pub fn pump_until(c: &mut Conn, within: Duration, mut done: impl FnMut(&mut Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        let _ = c.pump(Duration::from_millis(100));
        if done(c) {
            return true;
        }
    }
    done(c)
}

/// **Warp to `item` and wait until the pilot has landed**, not merely until
/// the warp was commanded: no warp in flight and within `reach` of the
/// item's surface. `false` on the timeout.
pub fn warp_and_arrive(c: &mut Conn, item: i64, reach: f64, within: Duration) -> bool {
    c.warp_to_item(item, 0.0).expect("warps");
    pump_until(c, within, |c| {
        let w = c.world_now();
        let warping = w.ego_ball().is_some_and(|b| b.warp_commanded());
        !warping
            && w.ego
                .and_then(|e| w.surface_distance(e, item))
                .is_some_and(|g| g < reach)
    })
}

pub fn ok(e: &Entry, kind: ActKind) -> bool {
    e.kind == kind && e.verdict == Verdict::Ok
}

/// For a failing assertion: a bot's act log, one line per entry.
pub fn dump(h: &Handle) -> String {
    h.log()
        .iter()
        .map(|e| format!("  {:?} {:?} {} {:?}", e.at, e.kind, e.what, e.verdict))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Logged in and selected **by name**, the way the bots do it, as a plain
/// connection: a human stand-in. The token is the bots' default, never the
/// name (ruling 2, IP41).
pub fn login(addr: SocketAddr, name: &str) -> Conn {
    let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    c.handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    c.select_character_named(name).expect("selects by name");
    c
}

/// A non-bot's setup: a GM command, named at the call site.
pub fn gm(c: &mut Conn, line: &str) {
    c.call(
        "slash",
        "SlashCmd",
        vec![PyValue::Bytes(line.as_bytes().to_vec())],
    )
    .expect("the slash is answered");
}

/// **T9, as a property every bot test checks**: each `slash` in a bot's act
/// log comes before that bot's first `Undock`.
pub fn gm_only_in_setup(h: &Handle) {
    let log = h.log();
    if let Some(at) = log.iter().position(|e| e.kind == ActKind::Undock) {
        assert!(
            !log[at..].iter().any(|e| e.kind == ActKind::Slash),
            "a slash after the first undock:\n{}",
            dump(h)
        );
    }
}

/// Wait until `victim` has been hit by the ship `by_ship`: one incoming
/// `OnDamageMessage` that landed (`doctrine_sense::parse_damage`), read from
/// the victim's own connection (`keep_pushes` must be on). The shooter's
/// volley is booked once this holds.
pub fn wait_hit(victim: &mut Conn, by_ship: i64) {
    let mut lines: Vec<Vec<PyValue>> = Vec::new();
    let got = until(Duration::from_secs(60), || {
        let _ = victim.collect_pushes(Duration::from_millis(100));
        for (name, args) in victim.take_pushes() {
            if name == "OnDamageMessage" {
                lines.push(args);
            }
        }
        lines.iter().any(|a| {
            parse_damage(a)
                .is_some_and(|h| h.incoming && h.quality > 0 && h.attacker == Some(by_ship))
        })
    });
    assert!(got, "ship {by_ship}'s volley lands on the victim");
}

/// Rows for a world whose pilots fly drones: the Drones skill under its id
/// with the effects that give the character `maxActiveDrones` (352) one per
/// level, and the character's base `droneControlDistance` (458). The drone
/// count and range are read off the dogma sheet's character node, so a
/// test world that launches drones needs the skill's own effect in its
/// table, as a real bundle has. **Invented effect and attribute ids**, the
/// attributes 352 and 458 being the ones the server reads.
pub fn drone_skill_rows() -> String {
    let skill = eve_server::drones::SKILL_DRONES;
    let range = eve_server::drones::CONTROL_RANGE;
    format!(
        "attribute\t280\t0.0\tskillLevel\n\
attribute\t352\t0.0\tmaxActiveDrones\n\
attribute\t458\t{range}\tdroneControlDistance\n\
attribute\t990790001\t0.0\tinventedDroneLevel\n\
effect\t990790001\t0\t0\t0\t0\t0\t0\t0\tinventedDroneLevel\t\n\
effect\t990790002\t0\t0\t0\t0\t0\t0\t0\tinventedDroneCount\t\n\
modifier\t990790001\titemID\tItemModifier\t990790001\t280\t2\t\t\n\
modifier\t990790002\tcharID\tItemModifier\t352\t990790001\t2\t\t\n\
type\t{skill}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990790001,990790002\t\tFixture Drones Skill\n"
    )
}

#[cfg(test)]
mod scan {
    //! **Process-global switches stay off in this binary.** eve-server is
    //! built without `cfg(test)` here, so its test seams do not exist, and a
    //! test that turned one of these on would change every other test in the
    //! process. A source scan, like the one in `gateway_out.rs`.

    /// What no test file may contain, as split pairs so that this file
    /// never holds a pattern in full, with each one's reason.
    const FORBIDDEN: [(&str, &str, &str); 11] = [
        ("coalesce_destiny", " = true", "push::set_coalesce_destiny"),
        ("coalesce_destiny", ": true", "push::set_coalesce_destiny"),
        ("set_coalesce_", "destiny(", "push::set_coalesce_destiny"),
        (".gateway", " = Some", "gateway_out::install_hub"),
        ("gateway", ": Some(", "gateway_out::install_hub"),
        ("gateway_out::", "install_hub(", "gateway_out::install_hub"),
        ("--tick", "-threads", "par::set_threads"),
        ("par::", "set_threads(", "par::set_threads"),
        ("--turn", "-sched", "sched_stat::set_enabled"),
        ("sched_stat::", "set_enabled(", "sched_stat::set_enabled"),
        ("sched_stat::", "set_enabled (", "sched_stat::set_enabled"),
    ];

    #[test]
    fn no_test_turns_on_a_process_global_switch() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut files = Vec::new();
        let mut todo = vec![dir];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).expect("the tests dir reads") {
                let p = e.expect("an entry").path();
                if p.is_dir() {
                    todo.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 10, "the scan sees the test files");
        let mut hits = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).expect("reads");
            for (a, b, why) in FORBIDDEN {
                let pat = format!("{a}{b}");
                if text.contains(&pat) {
                    hits.push(format!("{}: `{pat}` ({why} is process-wide)", f.display()));
                }
            }
        }
        assert!(hits.is_empty(), "{}", hits.join("\n"));
    }
}
