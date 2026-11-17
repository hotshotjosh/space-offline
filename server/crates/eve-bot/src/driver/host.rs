//! **The in-process host library** (#147 FF-1a): the server and the bots in
//! one process, so the one game clock (`crate::clock`) can run them both
//! under a fast-forward mode. This file is the library half only.
//!
//! What it does:
//! - [`parse_ff`] reads the `--ff` value: `1` is the real clock, `K >= 2` is
//!   an Affine clock `K` times faster, anything else is refused; `lockstep`
//!   is refused by name (#148 is not built).
//! - [`server_args`] builds the server's argument list from the caller's
//!   extras: it refuses every flag in [`REFUSED`] (a listener, a second clock
//!   or a control path that would see game time from outside the process) and a
//!   set `EVE_TIME_SCALE`, and puts `--bind 127.0.0.1:0` first.
//! - [`start`] installs the Affine clock (today the install returns the
//!   `SEAM_COMPLETE` refusal and this passes it through unchanged), then
//!   starts the server with `eve_server::cli::spawn_in_process`.
//! - [`mark_state`] and [`refuse_marked_state`]: a bots' `--state DIR` written
//!   under a fast clock holds game days and stamps that a normal run would
//!   read as wall time, so such a directory is marked, and a normal run
//!   refuses a marked one.
//!
//! **Not here yet:** the `eve-bots --host` flag. It needs
//! `eve_offline_server::MODULES`, an edge from eve-bot to the server binary
//! crate in `server/Cargo.lock` (a hot file); it lands with the fast-forward
//! hot batch. Until then nothing calls [`start`] but the tests, and Affine
//! stays refused.
//!
//! The bubble (`--bubble`) is refused with the host: there is one global
//! clock slot, so one origin, and the server stamps `bubble.tsv` in wall time
//! (the bubble reader compares it with the wall, see `bubble::wall_unix_secs`).

use crate::clock;
use crate::clock::Mode;
use eve_server::module::Module;
use std::net::SocketAddr;
use std::path::Path;
use std::thread::JoinHandle;

/// How fast game time runs in a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ff {
    /// The real clock (`--ff 1`).
    Wall,
    /// Game time `k` times faster than the wall, `k >= 2`.
    Affine(u32),
}

/// Read an `--ff` value.
pub fn parse_ff(s: &str) -> Result<Ff, String> {
    if s == "lockstep" {
        return Err("--ff lockstep is refused: lockstep time is #148 and is not built".to_owned());
    }
    match s.parse::<u32>() {
        Ok(1) => Ok(Ff::Wall),
        Ok(k) if k >= 2 => Ok(Ff::Affine(k)),
        _ => Err(format!(
            "--ff {s}: expected 1 (real time) or a whole number of 2 or more"
        )),
    }
}

/// Server flags a host refuses: each opens a way in or out of the process
/// (a listener, a gateway, an admin or control path, a proxy node, chat) or
/// sets a second clock.
pub const REFUSED: &[&str] = &[
    "--bind",
    "--gateway",
    "--gateway-addr",
    "--gateway-port",
    "--admin",
    "--bot-control",
    "--time-scale",
    "--proxy-node",
    "--chat",
    "--chat-advertise",
];

/// The server's arguments for a host: `extra` checked against [`REFUSED`]
/// (bare or `flag=value`) and the time-scale variable (`env` reads it), with
/// `--bind 127.0.0.1:0` put first.
pub fn server_args(
    extra: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<String>, String> {
    for a in extra {
        let flag = a.split_once('=').map_or(a.as_str(), |(f, _)| f);
        if REFUSED.contains(&flag) {
            return Err(format!(
                "{flag} is refused with the in-process host (one loopback listener, one clock)"
            ));
        }
    }
    if let Some(v) = env(eve_server::game_clock::ENV)
        && !v.is_empty()
    {
        return Err(format!(
            "{} is set ({v}): a host has one clock, unset it",
            eve_server::game_clock::ENV
        ));
    }
    let mut args = vec!["--bind".to_owned(), "127.0.0.1:0".to_owned()];
    args.extend(extra.iter().cloned());
    Ok(args)
}

/// Install the clock mode (`ff`), then start the server in this process.
/// Returns the loopback address and the server thread's handle (stop it with
/// `--stop-file`). Affine is refused by the clock until its seam is complete;
/// that text comes back unchanged.
#[allow(clippy::type_complexity)]
pub fn start(
    mods: &[&Module],
    ff: Ff,
    extra: &[String],
) -> Result<(SocketAddr, JoinHandle<Result<(), String>>), String> {
    let args = server_args(extra, |k| std::env::var(k).ok())?;
    if let Ff::Affine(k) = ff {
        clock::install(Mode::Affine { k })?;
    }
    eve_server::cli::spawn_in_process(mods, args)
}

/// The marker file's name inside a state directory.
pub const MARK_FILE: &str = "ffclock";

/// Mark a bots' `--state DIR` as written under `ff`: mode, k and the game
/// clock's unix second at the mark (the epoch). A real-time host writes no
/// mark, its stamps are the wall's.
pub fn mark_state(dir: &Path, ff: Ff) -> Result<(), String> {
    let Ff::Affine(k) = ff else {
        return Ok(());
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = format!("mode\taffine\nk\t{k}\nepoch\t{}\n", clock::unix_secs());
    let file = dir.join(MARK_FILE);
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))
}

/// For a normal (non-host) run: `Err` when `dir` carries the marker.
pub fn refuse_marked_state(dir: &Path) -> Result<(), String> {
    let file = dir.join(MARK_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => Err(format!(
            "--state {}: written by a fast-forward host ({}); its stamps are game time, \
             not wall time. Use a fresh directory, or run it under --host",
            dir.display(),
            text.lines()
                .collect::<Vec<_>>()
                .join(", ")
                .replace('\t', " ")
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn parse_ff_accepts_k_and_refuses_zero_and_lockstep() {
        assert_eq!(parse_ff("1"), Ok(Ff::Wall));
        assert_eq!(parse_ff("2"), Ok(Ff::Affine(2)));
        assert_eq!(parse_ff("60"), Ok(Ff::Affine(60)));
        assert!(parse_ff("0").is_err());
        assert!(parse_ff("x").is_err());
        assert!(parse_ff("-3").is_err());
        assert!(parse_ff("").is_err());
        let e = parse_ff("lockstep").unwrap_err();
        assert!(e.contains("#148"), "{e}");
    }

    #[test]
    fn server_args_refuses_each_listener_and_the_time_scale() {
        for flag in REFUSED {
            let bare = server_args(&[(*flag).to_owned()], no_env);
            assert!(bare.is_err(), "{flag} bare");
            let eq = server_args(&[format!("{flag}=x")], no_env);
            assert!(eq.is_err(), "{flag}=value");
            let spaced = server_args(&[(*flag).to_owned(), "x".to_owned()], no_env);
            assert!(spaced.is_err(), "{flag} value");
        }
        let set = |k: &str| (k == eve_server::game_clock::ENV).then(|| "60".to_owned());
        assert!(server_args(&[], set).is_err());
    }

    #[test]
    fn server_args_binds_loopback_port_0() {
        let extra = vec!["--world".to_owned(), "w.dat".to_owned()];
        let args = server_args(&extra, no_env).unwrap();
        assert_eq!(&args[..2], ["--bind", "127.0.0.1:0"]);
        assert_eq!(&args[2..], &extra[..]);
    }

    #[test]
    fn a_marked_state_dir_is_refused_by_a_normal_run() {
        let dir = std::env::temp_dir().join(format!("ff-host-mark-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(refuse_marked_state(&dir), Ok(()));
        mark_state(&dir, Ff::Wall).unwrap();
        assert_eq!(
            refuse_marked_state(&dir),
            Ok(()),
            "a real-time host marks nothing"
        );
        mark_state(&dir, Ff::Affine(30)).unwrap();
        let e = refuse_marked_state(&dir).unwrap_err();
        assert!(e.contains("fast-forward") && e.contains("k 30"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
