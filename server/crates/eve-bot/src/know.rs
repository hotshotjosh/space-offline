//! **The acquaintance book** (#209, P2): the memory of other pilots. One
//! book per bot (keyed by the bot's character name): who it has met, when,
//! where, and what passed between them. Regard stays the one number the
//! effects read; the book is the story beside it.
//!
//! The book lives in a process-wide registry. [`attach`] reads
//! `<dir>/<name>.know.tsv` once, [`note`] adds a meeting, [`recall`] reads
//! one back, and [`flush`] writes the file by temp-and-rename when the book
//! changed. The format: `know 1`, one `pilot <who> <first> <last> <system>
//! <kind> <killed_me> <i_killed> <helps>` row each, `end <n>`; a torn file
//! reads as an empty book.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How many pilots a book keeps. **[A]**: falsified by a bot that forgets a
/// pilot who killed it twice within a day of meeting 64 others.
pub const KNOW_KEEP: usize = 64;

/// What happened at the last meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Met {
    KilledMe,
    IKilled,
    HelpedMe,
    IHelped,
    Fleet,
    Theft,
}

impl Met {
    fn word(self) -> &'static str {
        match self {
            Met::KilledMe => "killed_me",
            Met::IKilled => "i_killed",
            Met::HelpedMe => "helped_me",
            Met::IHelped => "i_helped",
            Met::Fleet => "fleet",
            Met::Theft => "theft",
        }
    }

    fn from_word(s: &str) -> Option<Met> {
        Some(match s {
            "killed_me" => Met::KilledMe,
            "i_killed" => Met::IKilled,
            "helped_me" => Met::HelpedMe,
            "i_helped" => Met::IHelped,
            "fleet" => Met::Fleet,
            "theft" => Met::Theft,
            _ => return None,
        })
    }
}

/// One pilot this bot has met: who, when, where, and what between us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acquaintance {
    /// Character id.
    pub who: i64,
    /// Unix seconds of the first meeting.
    pub first: u64,
    /// Unix seconds of the last.
    pub last: u64,
    /// Where `last` happened.
    pub system: i64,
    pub last_kind: Met,
    pub killed_me: u16,
    pub i_killed: u16,
    /// Help, either way.
    pub helps: u16,
}

impl Acquaintance {
    fn weight(&self) -> u32 {
        u32::from(self.killed_me) + u32::from(self.i_killed) + u32::from(self.helps)
    }
}

#[derive(Default)]
struct Book {
    pilots: BTreeMap<i64, Acquaintance>,
    dirty: bool,
    attached: bool,
}

fn books() -> std::sync::MutexGuard<'static, BTreeMap<String, Book>> {
    static B: OnceLock<Mutex<BTreeMap<String, Book>>> = OnceLock::new();
    B.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The book's file for `name` in `dir`: the state file's name with
/// `.know.tsv`.
pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    let mut p = crate::state::path_for(dir, name);
    p.set_extension("know.tsv");
    p
}

/// Remember a meeting with `who` in `system` at `now`.
pub fn note(name: &str, who: i64, kind: Met, system: i64, now: u64) {
    if who <= 0 {
        return;
    }
    let mut all = books();
    let b = all.entry(name.to_owned()).or_default();
    if !b.pilots.contains_key(&who) && b.pilots.len() >= KNOW_KEEP {
        // The stalest of the least eventful goes first.
        let out = b
            .pilots
            .values()
            .min_by_key(|a| (a.weight(), a.last, a.who))
            .map(|a| a.who);
        if let Some(o) = out {
            b.pilots.remove(&o);
        }
    }
    let a = b.pilots.entry(who).or_insert(Acquaintance {
        who,
        first: now,
        last: now,
        system,
        last_kind: kind,
        killed_me: 0,
        i_killed: 0,
        helps: 0,
    });
    a.first = a.first.min(now);
    a.last = a.last.max(now);
    a.system = system;
    a.last_kind = kind;
    match kind {
        Met::KilledMe => a.killed_me = a.killed_me.saturating_add(1),
        Met::IKilled => a.i_killed = a.i_killed.saturating_add(1),
        Met::HelpedMe | Met::IHelped => a.helps = a.helps.saturating_add(1),
        Met::Fleet | Met::Theft => {}
    }
    b.dirty = true;
}

/// What `name` remembers of `who`.
pub fn recall(name: &str, who: i64) -> Option<Acquaintance> {
    books().get(name)?.pilots.get(&who).copied()
}

fn render(b: &Book) -> String {
    let mut o = String::from("know\t1\n");
    for a in b.pilots.values() {
        o.push_str(&format!(
            "pilot\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            a.who,
            a.first,
            a.last,
            a.system,
            a.last_kind.word(),
            a.killed_me,
            a.i_killed,
            a.helps
        ));
    }
    o.push_str(&format!("end\t{}\n", b.pilots.len()));
    o
}

fn parse(text: &str) -> Option<BTreeMap<i64, Acquaintance>> {
    let mut lines = text.lines();
    if lines.next()? != "know\t1" {
        return None;
    }
    let mut out = BTreeMap::new();
    let mut rows = 0usize;
    for l in lines.by_ref() {
        let f: Vec<&str> = l.split('\t').collect();
        match f.as_slice() {
            ["pilot", who, first, last, system, kind, km, ik, h] => {
                let a = Acquaintance {
                    who: who.parse().ok()?,
                    first: first.parse().ok()?,
                    last: last.parse().ok()?,
                    system: system.parse().ok()?,
                    last_kind: Met::from_word(kind)?,
                    killed_me: km.parse().ok()?,
                    i_killed: ik.parse().ok()?,
                    helps: h.parse().ok()?,
                };
                out.insert(a.who, a);
                rows += 1;
            }
            ["end", n] => {
                let ok = n.parse::<usize>().ok()? == rows
                    && rows == out.len()
                    && out.len() <= KNOW_KEEP
                    && lines.next().is_none();
                return ok.then_some(out);
            }
            _ => return None,
        }
    }
    None
}

/// Read `<dir>/<name>.know.tsv` into the book, once per process. A missing
/// or torn file is an empty book.
pub fn attach(dir: &Path, name: &str) {
    let mut all = books();
    let b = all.entry(name.to_owned()).or_default();
    if b.attached {
        return;
    }
    b.attached = true;
    let loaded = std::fs::read_to_string(path_for(dir, name))
        .ok()
        .and_then(|t| parse(&t));
    if let Some(p) = loaded {
        b.pilots = p;
    }
}

/// Write the book by temp-and-rename when it changed since the last write.
pub fn flush(dir: &Path, name: &str) -> std::io::Result<()> {
    let mut all = books();
    let Some(b) = all.get_mut(name) else {
        return Ok(());
    };
    if !b.dirty {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let path = path_for(dir, name);
    let tmp = path.with_extension("tsv.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(render(b).as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    b.dirty = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eve-bot-know-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_kill_is_remembered_with_where_and_when() {
        let me = "know-t1";
        note(me, 9001, Met::KilledMe, 30001, 500);
        let a = recall(me, 9001).unwrap();
        assert_eq!(
            (a.first, a.last, a.system, a.last_kind),
            (500, 500, 30001, Met::KilledMe)
        );
        assert_eq!((a.killed_me, a.i_killed, a.helps), (1, 0, 0));
        assert_eq!(recall(me, 9002), None);
        assert_eq!(recall("know-t1-other", 9001), None);
    }

    #[test]
    fn a_second_kill_counts_and_moves_last() {
        let me = "know-t2";
        note(me, 7, Met::KilledMe, 1, 100);
        note(me, 7, Met::KilledMe, 2, 200);
        note(me, 7, Met::IHelped, 3, 300);
        let a = recall(me, 7).unwrap();
        assert_eq!((a.first, a.last, a.system), (100, 300, 3));
        assert_eq!((a.killed_me, a.helps, a.last_kind), (2, 1, Met::IHelped));
    }

    #[test]
    fn the_book_keeps_keep_and_drops_the_stalest_trivial_entry() {
        let me = "know-t3";
        // A killer met first, then KNOW_KEEP trivial meetings.
        note(me, 1, Met::KilledMe, 1, 10);
        for i in 0..KNOW_KEEP as i64 {
            note(me, 100 + i, Met::Fleet, 1, 20 + i as u64);
        }
        assert_eq!(books().get(me).unwrap().pilots.len(), KNOW_KEEP);
        assert!(recall(me, 1).is_some(), "the killer outlives trivial ones");
        assert_eq!(recall(me, 100), None, "the stalest trivial entry went");
        assert!(recall(me, 101).is_some());
    }

    #[test]
    fn a_torn_file_reads_as_an_empty_book() {
        let dir = scratch("torn");
        std::fs::create_dir_all(&dir).unwrap();
        let me = "know-t4";
        let ok = "know\t1\npilot\t5\t1\t2\t3\ti_killed\t0\t1\t0\nend\t1\n";
        let torn = [
            "know\t1\npilot\t5\t1\t2\t3\ti_killed\t0\t1\t0\n",
            "know\t1\npilot\t5\t1\t2\t3\ti_killed\t0\t1\t0\nend\t2\n",
            "know\t1\npilot\t5\t1\tx\t3\ti_killed\t0\t1\t0\nend\t1\n",
            "know\t2\nend\t0\n",
            "",
        ];
        for (i, t) in torn.iter().enumerate() {
            let n = format!("{me}-{i}");
            std::fs::write(path_for(&dir, &n), t).unwrap();
            attach(&dir, &n);
            assert_eq!(recall(&n, 5), None, "{t:?}");
        }
        std::fs::write(path_for(&dir, me), ok).unwrap();
        attach(&dir, me);
        assert_eq!(recall(me, 5).unwrap().i_killed, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_writes_only_when_dirty() {
        let dir = scratch("dirty");
        let me = "know-t5";
        flush(&dir, me).unwrap();
        assert!(!path_for(&dir, me).exists(), "nothing to write");
        note(me, 3, Met::HelpedMe, 4, 50);
        flush(&dir, me).unwrap();
        let p = path_for(&dir, me);
        assert!(p.exists());
        assert!(!p.with_extension("tsv.tmp").exists());
        // A clean flush writes nothing: a deleted file stays deleted.
        std::fs::remove_file(&p).unwrap();
        flush(&dir, me).unwrap();
        assert!(!p.exists());
        note(me, 3, Met::HelpedMe, 4, 60);
        flush(&dir, me).unwrap();
        let back = parse(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back.get(&3).unwrap().helps, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
