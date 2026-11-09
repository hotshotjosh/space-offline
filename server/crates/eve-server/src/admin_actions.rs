//! **Admin web actions** (lane `admin-web-2`): `POST /api/action/<name>`.
//!
//! The HTTP thread never touches the world. It checks the request
//! ([`crate::admin_web::route_post`]), parses the body, enqueues a [`Job`]
//! and waits for the answer. The clock drains the queue under its own lock
//! ([`drain`], once per tick) and runs each job as the synthetic GM caller
//! (`Caller::Internal`, which `/bots` accepts as a GM) through the same
//! slash path a GM's console line takes ([`crate::bot_panel::issue`]).
//! A `/bots` relay waits on eve-bots over TCP, so it is handed out as a
//! [`crate::bot_control::Deferred`] and run on its own thread after the
//! lock, exactly as the wire path does (TT-B C1).
//!
//! Actions: `bots {words}`, `broadcast {text}`, `save {}`, `kick {pilot}`.
//! Each is limited to one per second ([`Queue::allow`]); every one is logged
//! by the caller.
//!
//! **Kick** (ADMIN-WEB, #14) resolves the pilot under the world lock (the
//! session table is world state) and hands the connection ids out as
//! [`Outcome::Evict`]; [`finish`], after the lock, sends each the
//! `CLIENTDISCONNECTED` close frame through `Writers::evict`, the path a second
//! login's takeover uses (`dispatch.rs` `run_evictions`). The writers come from
//! [`crate::admin_web::Shared::writers`], set once at start-up, so the HTTP
//! thread and this module never need the lock for them (lock order, world >
//! writers > wire > outbox: `docs/reference/threads-and-locks.md` section 0).
//! A refusal answers `error: ...`, which the route turns into a 409.

use crate::bot_control::Deferred;
use crate::push::Writers;
use crate::service::Registry;
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// At most this many jobs wait for the clock; more get 503.
pub const MAX_PENDING: usize = 8;
/// One action per name per this long.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// The longest broadcast text, in characters.
pub const MAX_TEXT: usize = 300;
/// The longest pilot name or id a kick may carry, in characters.
pub const MAX_PILOT: usize = 64;
/// The prefix of a refusal's answer; the route makes it a 409.
pub const REFUSED: &str = "error: ";
/// What the kicked client is told (the text of its `GPSTransportClosed`).
const KICKED: &str = "You were disconnected by the server operator";
/// The most words one `bots` action may carry.
pub const MAX_WORDS: usize = 24;

/// What an action asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `/bots <words>`.
    Bots(Vec<String>),
    /// A toast to every pilot online.
    Broadcast(String),
    /// Save the world now.
    Save,
    /// Disconnect an online pilot: a character name (any case) or id.
    Kick(String),
}

impl Action {
    /// The name in the URL.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Bots(_) => "bots",
            Action::Broadcast(_) => "broadcast",
            Action::Save => "save",
            Action::Kick(_) => "kick",
        }
    }

    /// The arguments, for the log line.
    pub fn args(&self) -> String {
        match self {
            Action::Bots(w) => w.join(" "),
            Action::Broadcast(t) => t.clone(),
            Action::Save => String::new(),
            Action::Kick(p) => p.clone(),
        }
    }

    /// The action `name` with its JSON `body` (a flat object), or why not.
    pub fn parse(name: &str, body: &str) -> Result<Action, String> {
        let fields = parse_flat_json(body).ok_or("the body is not a flat JSON object")?;
        let text = |key: &str| fields.get(key).map(String::as_str).unwrap_or("").trim();
        match name {
            "bots" => {
                let words: Vec<String> = text("words")
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                if words.is_empty() || words.len() > MAX_WORDS {
                    return Err(format!("\"words\" must be 1 to {MAX_WORDS} words"));
                }
                Ok(Action::Bots(words))
            }
            "broadcast" => {
                let t = text("text");
                if t.is_empty() || t.chars().count() > MAX_TEXT {
                    return Err(format!("\"text\" must be 1 to {MAX_TEXT} characters"));
                }
                if t.chars().any(char::is_control) {
                    return Err("\"text\" has a control character".to_owned());
                }
                Ok(Action::Broadcast(t.to_owned()))
            }
            "save" => Ok(Action::Save),
            "kick" => {
                let p = text("pilot");
                if p.is_empty() || p.chars().count() > MAX_PILOT {
                    return Err(format!("\"pilot\" must be 1 to {MAX_PILOT} characters"));
                }
                if p.chars().any(char::is_control) {
                    return Err("\"pilot\" has a control character".to_owned());
                }
                Ok(Action::Kick(p.to_owned()))
            }
            _ => Err("no such action".to_owned()),
        }
    }
}

/// One queued action and where its answer goes.
pub struct Job {
    pub action: Action,
    pub reply: Sender<String>,
}

/// What running a job under the lock left.
pub enum Outcome {
    /// The answer, ready.
    Now(Sender<String>, String),
    /// A `/bots` relay to run after the lock.
    Relay(Sender<String>, Deferred),
    /// The clock saves, then answers.
    Save(Sender<String>),
    /// A kick, resolved: evict these connections after the lock.
    Evict(Evict),
}

/// A resolved kick.
pub struct Evict {
    pub reply: Sender<String>,
    pub writers: Arc<Writers>,
    pub clients: Vec<i64>,
    pub name: String,
    pub character_id: i64,
}

/// `pilot` as an online character: an id, or a name in any case.
fn find_online(w: &Registry, pilot: &str) -> Result<i64, String> {
    let online = w.sessions().online_characters();
    if let Ok(id) = pilot.parse::<i64>() {
        if online.contains(&id) {
            return Ok(id);
        }
        return Err(if w.character(id).is_some() {
            format!("{REFUSED}pilot {id} is offline")
        } else {
            format!("{REFUSED}no pilot with id {id}")
        });
    }
    online
        .into_iter()
        .find(|id| {
            w.character(*id)
                .is_some_and(|c| c.name.eq_ignore_ascii_case(pilot))
        })
        .ok_or_else(|| format!("{REFUSED}no pilot online called \"{pilot}\" (unknown or offline)"))
}

/// The jobs waiting for the clock, and the rate limit.
#[derive(Default)]
pub struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    last: Mutex<BTreeMap<&'static str, Instant>>,
}

impl Queue {
    /// Whether `name` may run at `now`; records it when so.
    pub fn allow(&self, name: &'static str, now: Instant) -> bool {
        let Ok(mut last) = self.last.lock() else {
            return false;
        };
        if last
            .get(name)
            .is_some_and(|t| now.saturating_duration_since(*t) < MIN_GAP)
        {
            return false;
        }
        last.insert(name, now);
        true
    }

    /// Queue a job; `false` when the queue is full.
    pub fn push(&self, job: Job) -> bool {
        match self.jobs.lock() {
            Ok(mut q) if q.len() < MAX_PENDING => {
                q.push_back(job);
                true
            }
            _ => false,
        }
    }

    /// The clock's drain, for a test that plays the clock.
    #[cfg(test)]
    pub(crate) fn take_all_for_test(&self) -> Vec<Job> {
        self.take_all()
    }

    pub(crate) fn take_all(&self) -> Vec<Job> {
        self.jobs
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Run one job in the world (under the lock).
pub fn execute(w: &mut Registry, job: Job) -> Outcome {
    let Job { action, reply } = job;
    match action {
        Action::Bots(words) => {
            let line = format!("/bots {}", words.join(" "));
            crate::bot_control::allow_deferral(w, true);
            let text = crate::bot_panel::issue(w, None, &line);
            crate::bot_control::allow_deferral(w, false);
            match crate::bot_control::take_deferred(w) {
                Some(job) => Outcome::Relay(reply, job),
                None => Outcome::Now(reply, text),
            }
        }
        Action::Broadcast(text) => {
            let to = w.sessions().online_characters();
            let n = to.len();
            crate::world_events::notice(w, to, "Server announcement", "", &text);
            Outcome::Now(reply, format!("sent to {n} pilot(s)"))
        }
        Action::Save => Outcome::Save(reply),
        Action::Kick(pilot) => {
            let id = match find_online(w, &pilot) {
                Ok(id) => id,
                Err(e) => return Outcome::Now(reply, e),
            };
            let Some(writers) = crate::admin_web::shared_of(w).and_then(|s| s.writers()) else {
                return Outcome::Now(
                    reply,
                    format!("{REFUSED}the connection writers are not attached"),
                );
            };
            Outcome::Evict(Evict {
                reply,
                writers,
                clients: w
                    .sessions()
                    .of_character(id)
                    .iter()
                    .map(|e| e.client_id)
                    .collect(),
                name: w.character(id).map(|c| c.name.clone()).unwrap_or_default(),
                character_id: id,
            })
        }
    }
}

/// Every queued job, run (the clock, under the world lock, each tick).
pub fn drain(w: &mut Registry) -> Vec<Outcome> {
    let Some(shared) = crate::admin_web::shared_of(w) else {
        return Vec::new();
    };
    let jobs = shared.actions().take_all();
    jobs.into_iter().map(|j| execute(w, j)).collect()
}

/// Send the answers (after the lock); a relay waits on its own thread.
pub fn finish(outcomes: Vec<Outcome>) {
    for o in outcomes {
        match o {
            Outcome::Now(tx, text) => {
                let _ = tx.send(text);
            }
            Outcome::Relay(tx, job) => {
                let _ = std::thread::Builder::new()
                    .name("admin-relay".to_owned())
                    .spawn(move || {
                        let _ = tx.send(job.run());
                    });
            }
            Outcome::Save(tx) => {
                let _ = tx.send("save was not run".to_owned());
            }
            Outcome::Evict(e) => {
                let _ = e.reply.send(evict_now(&e));
            }
        }
    }
}

/// Hang up each connection of a kicked pilot (after the world lock), and say
/// how many were still there.
fn evict_now(e: &Evict) -> String {
    let payload = crate::wire::dumps(&crate::account_book::transport_closed(
        KICKED,
        Some("CLIENTDISCONNECTED"),
    ));
    let sent = e
        .clients
        .iter()
        .filter(|c| {
            e.writers
                .evict(**c, payload.clone(), crate::dispatch::EVICT_GRACE)
        })
        .count();
    if sent == 0 {
        return format!(
            "{REFUSED}{} ({}) was already disconnected",
            e.name, e.character_id
        );
    }
    format!(
        "kicked {} ({}): {sent} connection(s)",
        e.name, e.character_id
    )
}

/// A flat JSON object (`{"k":"v","n":3,"b":true,"z":null}`) as text values.
/// No nesting: an object or array value is refused.
pub fn parse_flat_json(s: &str) -> Option<BTreeMap<String, String>> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let skip = |i: &mut usize| {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1;
        }
    };
    let string = |i: &mut usize| -> Option<String> {
        if b.get(*i) != Some(&'"') {
            return None;
        }
        *i += 1;
        let mut out = String::new();
        loop {
            let c = *b.get(*i)?;
            *i += 1;
            match c {
                '"' => return Some(out),
                '\\' => {
                    let e = *b.get(*i)?;
                    *i += 1;
                    match e {
                        '"' | '\\' | '/' => out.push(e),
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' | 'f' => out.push(' '),
                        'u' => {
                            let hex: String = b.get(*i..*i + 4)?.iter().collect();
                            *i += 4;
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                        }
                        _ => return None,
                    }
                }
                c => out.push(c),
            }
        }
    };
    skip(&mut i);
    if b.get(i) != Some(&'{') {
        return None;
    }
    i += 1;
    let mut map = BTreeMap::new();
    skip(&mut i);
    if b.get(i) == Some(&'}') {
        i += 1;
    } else {
        loop {
            skip(&mut i);
            let key = string(&mut i)?;
            skip(&mut i);
            if b.get(i) != Some(&':') {
                return None;
            }
            i += 1;
            skip(&mut i);
            let value = if b.get(i) == Some(&'"') {
                string(&mut i)?
            } else {
                let start = i;
                while i < b.len() && !matches!(b[i], ',' | '}') && !b[i].is_whitespace() {
                    if matches!(b[i], '{' | '[') {
                        return None;
                    }
                    i += 1;
                }
                let lit: String = b[start..i].iter().collect();
                let ok =
                    matches!(lit.as_str(), "true" | "false" | "null") || lit.parse::<f64>().is_ok();
                if !ok {
                    return None;
                }
                lit
            };
            map.insert(key, value);
            skip(&mut i);
            match b.get(i) {
                Some(',') => i += 1,
                Some('}') => {
                    i += 1;
                    break;
                }
                _ => return None,
            }
        }
    }
    skip(&mut i);
    (i == b.len()).then_some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_control::Link;
    use crate::model::SandboxConfig;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::sync::mpsc::channel;

    #[test]
    fn flat_json_reads_strings_numbers_and_refuses_nesting() {
        let m = parse_flat_json(" {\"words\":\"spawn a\\\"b\", \"n\": 3, \"t\":true} ").unwrap();
        assert_eq!(m["words"], "spawn a\"b");
        assert_eq!(m["n"], "3");
        assert_eq!(m["t"], "true");
        assert_eq!(parse_flat_json("{}").unwrap().len(), 0);
        for bad in [
            "",
            "[]",
            "{\"a\":{}}",
            "{\"a\":[1]}",
            "{\"a\":1",
            "{\"a\":1} x",
            "{a:1}",
        ] {
            assert!(parse_flat_json(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn actions_validate_their_fields() {
        assert_eq!(
            Action::parse("bots", "{\"words\":\" list \"}"),
            Ok(Action::Bots(vec!["list".to_owned()]))
        );
        assert!(Action::parse("bots", "{\"words\":\"\"}").is_err());
        assert!(Action::parse("bots", "{}").is_err());
        let long = format!("{{\"words\":\"{}\"}}", "w ".repeat(MAX_WORDS + 1));
        assert!(Action::parse("bots", &long).is_err());
        assert!(Action::parse("broadcast", "{\"text\":\"hi\\nthere\"}").is_err());
        assert_eq!(
            Action::parse("broadcast", "{\"text\":\"hello\"}"),
            Ok(Action::Broadcast("hello".to_owned()))
        );
        assert_eq!(Action::parse("save", "{}"), Ok(Action::Save));
        assert!(Action::parse("stop", "{}").is_err());
    }

    #[test]
    fn rate_limit_is_one_per_second_per_action() {
        let q = Queue::default();
        let t0 = Instant::now();
        assert!(q.allow("bots", t0));
        assert!(!q.allow("bots", t0 + Duration::from_millis(900)));
        assert!(q.allow("save", t0 + Duration::from_millis(900)));
        assert!(q.allow("bots", t0 + MIN_GAP));
    }

    #[test]
    fn the_queue_is_bounded() {
        let q = Queue::default();
        for _ in 0..MAX_PENDING {
            let (tx, _rx) = channel();
            assert!(q.push(Job {
                action: Action::Save,
                reply: tx
            }));
        }
        let (tx, _rx) = channel();
        assert!(!q.push(Job {
            action: Action::Save,
            reply: tx
        }));
        assert_eq!(q.take_all().len(), MAX_PENDING);
    }

    /// The bots action reaches the bot-control path: a fake eve-bots peer
    /// gets the command, and the relay is handed out past the lock.
    #[test]
    fn the_bots_action_reaches_the_control_link() {
        let link = Link::open(None).unwrap();
        let mut s = TcpStream::connect(link.addr()).unwrap();
        s.write_all(format!("HELLO {}\n", link.token()).as_bytes())
            .unwrap();
        let mut first = String::new();
        let mut r = BufReader::new(s.try_clone().unwrap());
        r.read_line(&mut first).unwrap();
        assert_eq!(first.trim(), "OK");
        let fake = std::thread::spawn(move || {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let rest = line.trim_end().strip_prefix("CMD ").unwrap().to_owned();
            let (id, cmd) = rest.split_once(' ').unwrap();
            s.write_all(format!("REPLY {id} fake saw {cmd}\n").as_bytes())
                .unwrap();
        });
        let mut w = Registry::new(SandboxConfig::default());
        crate::bot_control::attach(&mut w, true, Some(Arc::clone(&link)));
        let (tx, rx) = channel();
        let out = execute(
            &mut w,
            Job {
                action: Action::Bots(vec!["list".to_owned()]),
                reply: tx,
            },
        );
        assert!(matches!(out, Outcome::Relay(..)));
        finish(vec![out]);
        let answer = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(answer, "fake saw list");
        fake.join().unwrap();
    }

    #[test]
    fn bots_without_a_dev_server_says_so_and_broadcast_counts_pilots() {
        let mut w = Registry::new(SandboxConfig::default());
        let (tx, _rx) = channel();
        let Outcome::Now(_, text) = execute(
            &mut w,
            Job {
                action: Action::Bots(vec!["list".to_owned()]),
                reply: tx,
            },
        ) else {
            panic!("answered inline");
        };
        assert!(text.contains("dev server"), "{text}");
        let (tx, _rx) = channel();
        let Outcome::Now(_, text) = execute(
            &mut w,
            Job {
                action: Action::Broadcast("hi".to_owned()),
                reply: tx,
            },
        ) else {
            panic!("answered inline");
        };
        assert_eq!(text, "sent to 0 pilot(s)");
    }

    type WirePair = (Arc<Mutex<crate::push::Wire>>, TcpStream);

    /// A world with two invented pilots online (one connection each, wires
    /// registered with the writers), the admin state attached.
    fn kick_world() -> (Registry, Arc<Writers>, Vec<WirePair>) {
        let pilot = |id: i64, name: &str| crate::model::Character {
            character_id: id,
            ship_id: id + 5,
            name: name.to_owned(),
            ..crate::model::Character::default()
        };
        let mut w = Registry::new(SandboxConfig {
            characters: vec![
                pilot(91_000_001, "Test Alpha"),
                pilot(91_000_002, "Test Beta"),
            ],
            ..SandboxConfig::default()
        });
        let shared = Arc::new(crate::admin_web::Shared::new(
            "tok".to_owned(),
            &crate::admin_web::MapData::default(),
        ));
        let writers = Arc::new(Writers::default());
        shared.set_writers(Arc::clone(&writers));
        crate::admin_web::attach(&mut w, shared);
        let mut wires = Vec::new();
        for (client, ch) in [(501, 91_000_001), (502, 91_000_002)] {
            w.session_opened(crate::session_table::SessionEntry {
                client_id: client,
                session_id: client,
                user_id: 1,
                character_id: None,
            });
            w.session_selected_character(client, ch);
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let ours = TcpStream::connect(l.local_addr().unwrap()).unwrap();
            let (peer, _) = l.accept().unwrap();
            let wire = Arc::new(Mutex::new(crate::push::Wire::new(ours, None)));
            writers.register(client, &wire);
            wires.push((wire, peer));
        }
        (w, writers, wires)
    }

    fn run(w: &mut Registry, a: Action) -> Outcome {
        let (tx, _rx) = channel();
        execute(
            w,
            Job {
                action: a,
                reply: tx,
            },
        )
    }

    #[test]
    fn kick_parses_and_validates() {
        assert_eq!(
            Action::parse("kick", "{\"pilot\":\" Test Alpha \"}"),
            Ok(Action::Kick("Test Alpha".to_owned()))
        );
        assert!(Action::parse("kick", "{}").is_err());
        assert!(Action::parse("kick", "{\"pilot\":\"a\\nb\"}").is_err());
        let long = format!("{{\"pilot\":\"{}\"}}", "x".repeat(MAX_PILOT + 1));
        assert!(Action::parse("kick", &long).is_err());
        assert_eq!(Action::Kick("p".into()).name(), "kick");
    }

    #[test]
    fn kick_refuses_unknown_and_offline_pilots_and_a_missing_writer_set() {
        let (mut w, _writers, _wires) = kick_world();
        for (who, want) in [
            ("Nobody Here", "unknown or offline"),
            ("90000000000", "no pilot with id"),
        ] {
            let Outcome::Now(_, t) = run(&mut w, Action::Kick(who.to_owned())) else {
                panic!("refused inline");
            };
            assert!(t.starts_with(REFUSED) && t.contains(want), "{t}");
        }
        // Known character, no session: offline.
        w.session_closed(502);
        let Outcome::Now(_, t) = run(&mut w, Action::Kick("91000002".to_owned())) else {
            panic!("refused inline");
        };
        assert!(t.contains("offline"), "{t}");
        // No writers attached.
        let mut bare = Registry::new(SandboxConfig::default());
        let Outcome::Now(_, t) = run(&mut bare, Action::Kick("x".to_owned())) else {
            panic!("refused inline");
        };
        assert!(t.starts_with(REFUSED), "{t}");
    }

    /// The kicked pilot's wire gets the CLIENTDISCONNECTED close frame; the
    /// other pilot's wire gets nothing. A second kick of a connection that
    /// has gone says so.
    #[test]
    fn kick_sends_the_close_frame_to_that_pilot_only() {
        use std::io::Read;
        let (mut w, _writers, mut wires) = kick_world();
        let out = run(&mut w, Action::Kick("test alpha".to_owned()));
        let Outcome::Evict(e) = &out else {
            panic!("a kick is resolved for eviction");
        };
        assert_eq!(e.clients, vec![501]);
        assert_eq!(e.character_id, 91_000_001);
        let answer = evict_now(e);
        assert_eq!(answer, "kicked Test Alpha (91000001): 1 connection(s)");
        let now = Instant::now();
        for (wire, _) in &wires {
            wire.lock().unwrap().flush_due(now).unwrap();
        }
        let mut got = vec![0u8; 4096];
        wires[0]
            .1
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let n = wires[0].1.read(&mut got).unwrap();
        assert!(
            got[..n].windows(18).any(|x| x == b"CLIENTDISCONNECTED"),
            "{n} bytes"
        );
        wires[1]
            .1
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(
            wires[1].1.read(&mut got).is_err(),
            "the other pilot hears nothing"
        );
        // The connection goes away; kicking again finds nothing to evict.
        let gone = Evict {
            reply: channel().0,
            writers: Arc::new(Writers::default()),
            clients: e.clients.clone(),
            name: e.name.clone(),
            character_id: e.character_id,
        };
        assert!(evict_now(&gone).starts_with(REFUSED));
    }
}
