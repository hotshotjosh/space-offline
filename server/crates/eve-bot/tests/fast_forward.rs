//! **The in-process host (#147 FF-1a) over a real socket.** Its own test
//! binary: the process has ONE clock slot, and any read of it (a bot's, a
//! server's) makes a later `install` fail (`clock.rs`), so these tests cannot
//! share a process with `tests/bots`. Both tests call [`install_once`] first.
//!
//! Everything here is invented: the pilot's name, the temp paths.

use eve_bot::Conn;
use eve_bot::clock::{self, Mode};
use eve_bot::driver::host::{self, Ff};
use std::sync::OnceLock;
use std::time::Duration;

/// The one clock install, before anything reads the clock. Wall is the only
/// mode that can be installed today.
fn install_once() {
    static ONCE: OnceLock<Result<(), String>> = OnceLock::new();
    ONCE.get_or_init(|| clock::install(Mode::Wall))
        .clone()
        .expect("install the wall clock");
}

#[test]
fn a_wall_host_serves_a_bot_login() {
    install_once();
    let dir = std::env::temp_dir().join(format!("ff-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let world = dir.join("world.dat");
    let stop = dir.join("stop");
    let extra: Vec<String> = [
        "--world",
        world.to_str().unwrap(),
        "--character",
        "FfPilotOne",
        "--stop-file",
        stop.to_str().unwrap(),
    ]
    .iter()
    .map(|a| (*a).to_owned())
    .collect();
    let (addr, handle) = host::start(&[], Ff::Wall, &extra).expect("start the host");
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
    let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    c.handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    c.select_character_named("FfPilotOne")
        .expect("selects by name");
    drop(c);
    std::fs::write(&stop, b"stop").unwrap();
    let ended = handle.join().expect("server thread");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(ended, Ok(()));
}

#[test]
fn affine_is_refused_until_the_seam_is_complete() {
    install_once();
    let e = host::start(&[], Ff::Affine(60), &[]).unwrap_err();
    assert!(
        e.contains("not every time source reads the clock seam yet"),
        "{e}"
    );
}
