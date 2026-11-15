//! **A persona's decision log** (#271, scale-1000 L7): with `eve-bots --state DIR`
//! each bot appends one line per *decision* to `<character>.log`, beside its
//! `<character>.tsv` ([`crate::state::log_path_for`]): a career switch, a goal
//! pushed or popped, a corp joined or left, a bubble lease, shed or wake, a
//! budget wait, a park and its answer. The kinds are `budget`, `release`,
//! `park`, `career`, `goal`, `corp`, `lease`, `shed`, `wake`.
//!
//! **Why a file.** At 1,000 bots `bots.log` is unreadable and `--quiet` (the
//! production setting) prints no act line at all. This file is written by
//! [`note`] whatever `config.echo` says, so `--quiet` does not silence it.
//!
//! **The format** is one line per decision, `<unix s>\t<kind>\t<text>\n`, the
//! text escaped as in the state file ([`crate::state`]: `\`, tab, newline). A
//! last line with no newline (a crash mid-write) or fewer than three fields is
//! *torn* and [`tail`] skips it.
//!
//! **Bounded.** When a line would take `<character>.log` past [`ROTATE_AT`]
//! bytes the file is renamed over `<character>.log.1` (the one old file) and a
//! new one starts, so a persona holds at most about twice [`ROTATE_AT`].
//!
//! **Cost at 1,000 bots [I].** Decisions are rare (a few an hour per bot), so
//! each [`note`] opens the file in append mode, takes its length from the open
//! handle, writes one `write_all` and closes. No handle is kept: 1,000 held
//! files would spend 1,000 descriptors of a process that already holds a
//! socket per bot. Falsifier: a `[stats]` `log` column that grows with this
//! module on a soak.
//!
//! **Never blocks a turn.** Every error (no directory, disk full, read-only)
//! is swallowed after one stderr line for that bot; the caller sees no `Result`.
//! [`try_note`] is the same write with the error returned, for tests.

use crate::state::{esc, log_path_for, unesc};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The size at which the log rolls to `.1`. **[A]** 256 kB holds about a week
/// of decisions; falsifier: rotation more than daily in a soak.
pub const ROTATE_AT: u64 = 256 * 1024;

/// The lines `/bots inspect` shows.
pub const INSPECT_LINES: usize = 20;

/// A line longer than this many bytes is cut (on a character boundary), so
/// one runaway text cannot fill the file by itself.
const MAX_LINE: usize = 1024;

/// Bots whose first write error was already reported (one stderr line each).
static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The path of the old file, `<character>.log.1`.
pub fn rolled_path(log: &Path) -> PathBuf {
    let mut s = log.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

/// One decision line, with the newline.
pub fn render(at: u64, kind: &str, text: &str) -> String {
    let mut body = esc(text);
    if body.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        // A cut may have split an escape pair; drop a trailing lone `\`.
        let trailing = body.bytes().rev().take_while(|b| *b == b'\\').count();
        if trailing % 2 == 1 {
            body.pop();
        }
    }
    format!("{at}\t{}\t{body}\n", esc(kind))
}

/// Append a decision of `character` to its log in `dir`, rolling it at
/// `limit` bytes. Returns the error instead of swallowing it.
pub fn try_note_in(
    dir: &Path,
    character: &str,
    at: u64,
    kind: &str,
    text: &str,
    limit: u64,
) -> std::io::Result<()> {
    let path = log_path_for(dir, character);
    let line = render(at, kind, text);
    let open = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    };
    let mut f = open()?;
    let len = f.metadata()?.len();
    if len > 0 && len + line.len() as u64 > limit {
        drop(f);
        std::fs::rename(&path, rolled_path(&path))?;
        f = open()?;
    } else if len > 0 && !ends_with_newline(&path, len) {
        // A crash mid-write left a torn last line: start a fresh one so the
        // new entry is not glued onto it.
        f.write_all(b"\n")?;
    }
    f.write_all(line.as_bytes())
}

/// [`try_note_in`] at [`ROTATE_AT`].
pub fn try_note(
    dir: &Path,
    character: &str,
    at: u64,
    kind: &str,
    text: &str,
) -> std::io::Result<()> {
    try_note_in(dir, character, at, kind, text, ROTATE_AT)
}

/// The call sites' one line: note a decision, or do nothing without a
/// `--state` directory. Never fails, never panics, never waits on more than
/// one small append.
pub fn note(dir: Option<&Path>, character: &str, at: u64, kind: &str, text: &str) {
    let Some(dir) = dir else { return };
    if let Err(e) = try_note(dir, character, at, kind, text) {
        let mut w = WARNED.lock().unwrap_or_else(|p| p.into_inner());
        if !w.iter().any(|n| n == character) {
            w.push(character.to_owned());
            eprintln!("persona log: {character}: {e}; decisions are not kept");
        }
    }
}

/// The last `n` whole lines of `character`'s log, oldest first, as
/// `<unix s> <kind> <text>`; the old `.1` file supplies what the current one
/// lacks. A torn line is skipped. An absent log is an empty list.
pub fn tail(dir: &Path, character: &str, n: usize) -> Vec<String> {
    let path = log_path_for(dir, character);
    let mut lines = whole_lines(&path);
    if lines.len() < n {
        let mut older = whole_lines(&rolled_path(&path));
        older.extend(lines);
        lines = older;
    }
    let from = lines.len().saturating_sub(n);
    lines.split_off(from)
}

/// Whether the file's last byte (of `len`) is a newline; unreadable counts as
/// yes, so an error never adds a blank line.
fn ends_with_newline(path: &Path, len: u64) -> bool {
    let mut last = [0u8; 1];
    std::fs::File::open(path)
        .and_then(|mut f| {
            f.seek(SeekFrom::Start(len - 1))?;
            f.read_exact(&mut last)
        })
        .map_or(true, |()| last[0] == b'\n')
}

/// The whole lines of one file; only its last 2 × [`ROTATE_AT`] are read.
fn whole_lines(path: &Path) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut raw = Vec::new();
    let from = f
        .metadata()
        .map(|m| m.len().saturating_sub(2 * ROTATE_AT))
        .unwrap_or(0);
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut raw).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&raw);
    // Whatever follows the last newline is torn; so is a first line cut by
    // the seek.
    let Some(end) = text.rfind('\n') else {
        return Vec::new();
    };
    let mut it = text[..end].split('\n');
    if from > 0 {
        it.next();
    }
    it.filter_map(|l| {
        let mut f = l.splitn(3, '\t');
        let (at, kind, body) = (f.next()?, f.next()?, f.next()?);
        at.parse::<u64>().ok()?;
        Some(format!("{at} {} {}", unesc(kind).ok()?, unesc(body).ok()?))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eve-bot-plog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_decision_is_appended_and_read_back_escaped() {
        let d = scratch("rt");
        try_note(
            &d,
            "Inv Pilot",
            100,
            "career",
            "miner -> ratter\twith\nnewline \\",
        )
        .unwrap();
        try_note(&d, "Inv Pilot", 200, "park", "ok").unwrap();
        assert!(d.join("Inv_Pilot.log").exists());
        let t = tail(&d, "Inv Pilot", 20);
        assert_eq!(
            t,
            vec![
                "100 career miner -> ratter\twith\nnewline \\".to_owned(),
                "200 park ok".to_owned()
            ]
        );
        assert_eq!(tail(&d, "Inv Pilot", 1), vec!["200 park ok".to_owned()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_log_rolls_to_one_old_file_and_stays_bounded() {
        let d = scratch("rot");
        for i in 0..200u64 {
            try_note_in(&d, "Inv", i, "goal", "push Skill 1 2", 500).unwrap();
        }
        let log = log_path_for(&d, "Inv");
        let (a, b) = (
            std::fs::metadata(&log).unwrap().len(),
            std::fs::metadata(rolled_path(&log)).unwrap().len(),
        );
        assert!(a <= 500 && b <= 500, "{a} {b}");
        assert!(a > 0 && b > 0);
        // Only one old file ever exists.
        let n = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(n, 2);
        // The newest line is last, and the tail reaches across the roll.
        let t = tail(&d, "Inv", 20);
        assert_eq!(t.len(), 20);
        assert!(t.last().unwrap().starts_with("199 goal"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_torn_last_line_is_ignored() {
        let d = scratch("torn");
        try_note(&d, "Inv", 1, "park", "ok").unwrap();
        let log = log_path_for(&d, "Inv");
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(b"2\tpark\tha").unwrap();
        assert_eq!(tail(&d, "Inv", 20), vec!["1 park ok".to_owned()]);
        // A complete but malformed line is skipped too; the line the torn
        // write began is whole once its newline arrives.
        f.write_all(b"\nnot a line\n").unwrap();
        assert_eq!(
            tail(&d, "Inv", 20),
            vec!["1 park ok".to_owned(), "2 park ha".to_owned()]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_entry_after_a_torn_line_is_not_glued_onto_it() {
        let d = scratch("glue");
        try_note(&d, "Inv", 1, "park", "ok").unwrap();
        let log = log_path_for(&d, "Inv");
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        // A crash mid-write: no newline after the torn fragment.
        f.write_all(b"2\tpark\tha").unwrap();
        drop(f);
        try_note(&d, "Inv", 3, "career", "miner").unwrap();
        assert_eq!(
            tail(&d, "Inv", 20),
            vec![
                "1 park ok".to_owned(),
                "2 park ha".to_owned(),
                "3 career miner".to_owned()
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_a_state_directory_nothing_is_written() {
        // No directory: a no-op that cannot fail.
        note(None, "Inv", 1, "park", "ok");
        let d = scratch("none");
        assert!(tail(&d, "Inv", 20).is_empty());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_write_failure_returns_an_error_and_never_panics() {
        // The directory is a file, so the open fails.
        let d = scratch("fail");
        let not_a_dir = d.join("file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        assert!(try_note(&not_a_dir, "Inv", 1, "park", "ok").is_err());
        note(Some(&not_a_dir), "Inv", 1, "park", "ok");
        note(Some(&d.join("absent/deeper")), "Inv", 1, "park", "ok");
        // And a log path that is a directory.
        std::fs::create_dir(log_path_for(&d, "Dir")).unwrap();
        note(Some(&d), "Dir", 1, "park", "ok");
        assert!(tail(&d, "Dir", 5).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_long_text_is_cut_and_a_cut_never_leaves_a_lone_backslash() {
        let long = "\\".repeat(MAX_LINE * 2);
        let line = render(5, "k", &long);
        let body = line.trim_end_matches('\n').splitn(3, '\t').nth(2).unwrap();
        assert!(body.len() <= MAX_LINE);
        assert!(unesc(body).is_ok(), "the cut text still unescapes");
        let odd = format!("{}\\", "a".repeat(MAX_LINE - 1));
        let body = render(5, "k", &odd);
        let body = body
            .trim_end_matches('\n')
            .splitn(3, '\t')
            .nth(2)
            .unwrap()
            .to_owned();
        assert!(
            unesc(&body).is_ok(),
            "a cut inside an escape pair is dropped"
        );
        let multi = "é".repeat(MAX_LINE);
        assert!(render(5, "k", &multi).len() <= MAX_LINE + 16);
    }
}
