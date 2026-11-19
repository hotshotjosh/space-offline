//! **Admin web** (lane `admin-web`): a small HTTP listener (reads, plus
//! token-guarded actions) for the server's owner. A page with the cluster map and the pilots online, and a
//! JSON API behind a token.
//!
//! Off by default: `--admin ADDR` (e.g. `--admin 127.0.0.1:8090`; plain HTTP, keep it on loopback or behind SSH) turns it on.
//! std sockets and threads only, no dependency.
//!
//! # Auth
//!
//! [`Shared::open`] reads `<dir>/admin.token` (the bot-control directory,
//! which is the world file's directory: `/data` on the server host) and keeps it
//! when it holds a token of at least 32 characters; otherwise it writes a
//! fresh one (32 bytes of OS randomness, hex, mode 0600). Every `/api/`
//! request must carry it in the `X-Admin-Token` header, or it gets 401. The
//! static page itself is served without the token: it holds no data and
//! asks the operator for the token.
//!
//! # World state, without hurting the tick
//!
//! An HTTP thread never takes the world lock. The clock (`push.rs`) calls
//! [`take`] under the lock every [`SNAPSHOT_EVERY`] ticks -- O(pilots online),
//! the same order as the `[live]` line's work -- and publishes the result
//! after the lock with [`Shared::publish`]; each report window it adds the
//! tick health, work samples, lock hold and stage totals with
//! [`Shared::note_window`]. [`system_loads`] runs inside [`take`], under the
//! lock: O(npcs + live systems), plus one `grid::local_balls` build per hot
//! system (its cost is the `snapshot_cost_ms` figure, per report window).
//! A request renders JSON from the last snapshot under the admin's own
//! small mutex.
//!
//! A *minute* in `/api/ticks` is the minute a report window ended (the
//! window's `unix` / 60), not the minute its ticks ran; a window older than
//! the open minute is ignored. The map is built once at
//! bind, from the loaded bundle tables, and kept as a string.
//!
//! # Hardening
//!
//! At most [`MAX_CONNS`] connections at once (the rest are dropped), a
//! request head of at most [`MAX_HEAD`] bytes, 5 s read and write timeouts,
//! `GET` plus `POST /api/action/*` (see [`route_post`] and
//! `admin_actions`), no CORS headers (a page from another origin cannot read the
//! answers), `Cache-Control: no-store`.

use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The token file, in the control directory.
pub const TOKEN_FILE: &str = "admin.token";
/// The request header carrying the token (matched case-insensitively).
pub const TOKEN_HEADER: &str = "x-admin-token";
/// The clock takes a snapshot every this many ticks (2 s at 250 ms).
pub const SNAPSHOT_EVERY: u64 = 8;
/// Connections served at once; more are closed unanswered.
pub const MAX_CONNS: usize = 16;
/// The longest request head read.
pub const MAX_HEAD: usize = 8192;
const IO_WAIT: Duration = Duration::from_secs(5);

const INDEX_HTML: &str = include_str!("../web/index.html");
const ADMIN_JS: &str = include_str!("../web/admin.js");
const ADMIN_CSS: &str = include_str!("../web/admin.css");

/// One pilot online, as the clock saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub character_id: i64,
    pub name: String,
    /// The owning account (`Character::owner_user_id`).
    pub user_id: Option<i64>,
    pub bot: bool,
    /// `Registry::session_system`: in space, the roster's; docked, the
    /// station's.
    pub system_id: Option<i64>,
    /// `space`, `docked` or `jumping`.
    pub state: &'static str,
    /// Docked only.
    pub station_id: Option<i64>,
    pub ship_type_id: i64,
    pub ship: String,
    /// In space only: metres in the system frame, as last written by the
    /// roster (at most one tick old) [A].
    pub position: Option<[f64; 3]>,
}

/// What the clock hands over each snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub taken_unix: u64,
    pub pilots: Vec<Pilot>,
    pub npcs: usize,
    /// One row per warm or hot system ([`system_loads`]).
    pub systems: Vec<SystemLoad>,
}

/// What a live system holds, counted under the world lock.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SystemLoad {
    pub system_id: i64,
    pub hot: bool,
    pub humans: u32,
    pub bots: u32,
    pub npcs: u32,
    /// Characters, NPCs and drones in space (the roster's set).
    pub in_space: u32,
    /// Every local ball; counted for hot systems only.
    pub balls: Option<u32>,
}

/// One closed minute of ticks (`minute` = unix seconds / 60).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Minute {
    pub minute: u64,
    pub ticks: u32,
    pub p50_us: u32,
    pub p95_us: u32,
    pub max_us: u32,
    pub held_max_us: u32,
    pub skipped: u64,
}

/// Minutes the ring keeps.
pub const MINUTES_KEPT: usize = 60;
/// Samples a minute holds at most (240 at a 250 ms tick, room for a faster
/// clock). Extra samples are counted, not kept.
pub const MINUTE_SAMPLES: usize = 1024;

/// The per-minute tick ring: exact samples (no histogram buckets) in a
/// buffer allocated once, and the closed minutes in a deque allocated once.
#[derive(Debug)]
struct TickRing {
    closed: std::collections::VecDeque<Minute>,
    /// The open minute (unix seconds / 60).
    open: Option<u64>,
    /// The open minute's work samples in microseconds; cleared, never shrunk.
    samples: Vec<u32>,
    dropped: u32,
    held_max_us: u32,
    skipped: u64,
    /// Since start, for the `_total` counters.
    ticks_total: u64,
    skipped_total: u64,
}

fn micros(d: Duration) -> u32 {
    d.as_micros().min(u128::from(u32::MAX)) as u32
}

/// Nearest-rank quantile (percent `q`) of a sorted slice, the rule
/// `bot_control::p95` uses.
fn rank(sorted: &[u32], q: usize) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() * q).div_ceil(100).max(1) - 1]
}

impl TickRing {
    fn new() -> TickRing {
        TickRing {
            closed: std::collections::VecDeque::with_capacity(MINUTES_KEPT + 1),
            open: None,
            samples: Vec::with_capacity(MINUTE_SAMPLES),
            dropped: 0,
            held_max_us: 0,
            skipped: 0,
            ticks_total: 0,
            skipped_total: 0,
        }
    }

    /// Fold one report window in; `unix` is its end.
    fn add_window(&mut self, unix: u64, work: &[Duration], held_max: Duration, skipped: u64) {
        let minute = unix / 60;
        if self.open.is_some_and(|o| minute < o) {
            return;
        }
        if self.open != Some(minute) {
            self.close();
            self.open = Some(minute);
        }
        for d in work {
            if self.samples.len() < MINUTE_SAMPLES {
                self.samples.push(micros(*d));
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
        self.held_max_us = self.held_max_us.max(micros(held_max));
        self.skipped += skipped;
        self.ticks_total += work.len() as u64;
        self.skipped_total += skipped;
    }

    /// The open minute as a `Minute`. Sorts the samples in place (their
    /// order does not matter), so nothing is allocated.
    fn open_minute(&mut self) -> Option<Minute> {
        let minute = self.open?;
        self.samples.sort_unstable();
        Some(Minute {
            minute,
            ticks: (self.samples.len() as u32).saturating_add(self.dropped),
            p50_us: rank(&self.samples, 50),
            p95_us: rank(&self.samples, 95),
            max_us: self.samples.last().copied().unwrap_or(0),
            held_max_us: self.held_max_us,
            skipped: self.skipped,
        })
    }

    /// Close the open minute into `closed`; the oldest goes past
    /// [`MINUTES_KEPT`].
    fn close(&mut self) {
        if let Some(m) = self.open_minute() {
            self.closed.push_back(m);
            while self.closed.len() > MINUTES_KEPT {
                self.closed.pop_front();
            }
        }
        self.open = None;
        self.samples.clear();
        self.dropped = 0;
        self.held_max_us = 0;
        self.skipped = 0;
    }
}

/// The last report window's tick health.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Health {
    pub unix: u64,
    pub ticks: u64,
    pub p95_ms: f64,
    pub held_max_ms: f64,
    pub skipped: u64,
}

#[derive(Default)]
struct State {
    snap: Option<Snapshot>,
    health: Option<Health>,
    /// The last window's slowest tick stages: `(name, max ms, mean ms)`.
    stages: Vec<(&'static str, f64, f64)>,
}

/// Tick stages kept for the page.
const STAGES_KEPT: usize = 8;

/// The listener's shared state: the token, the map, the last snapshot.
pub struct Shared {
    started: Instant,
    token: String,
    map_json: String,
    system_names: BTreeMap<i64, String>,
    state: Mutex<State>,
    /// The per-minute tick ring: its own mutex, so a page render never waits
    /// on a window fold.
    ticks: Mutex<TickRing>,
    /// `take`'s cost, nanoseconds: the last, the max of the open report
    /// window, and the max of the last closed window (rolled by
    /// `note_window`; readers never reset anything).
    snap_cost_last_ns: AtomicU64,
    snap_cost_max_ns: AtomicU64,
    snap_cost_window_max_ns: AtomicU64,
    active: AtomicUsize,
    /// The control directory: where `bot-groups.tsv` is read and
    /// `admin-actions.log` appended.
    dir: Option<PathBuf>,
    /// Bot groups by character name, and when the file was last read.
    groups: Mutex<(Option<Instant>, BTreeMap<String, BotGroup>)>,
    actions: crate::admin_actions::Queue,
    /// The connection writers, for `kick` (set once at start-up).
    writers: std::sync::OnceLock<Arc<crate::push::Writers>>,
}

/// One bot's group, side and role (a `bot-groups.tsv` row).
#[derive(Debug, Clone, PartialEq)]
pub struct BotGroup {
    pub group: String,
    pub side: String,
    pub role: String,
}

/// `bot-groups.tsv` (eve-bots writes it every few seconds): `#group` header,
/// then `group<TAB>side<TAB>role<TAB>character name` per bot. Bad rows are
/// skipped.
pub fn parse_groups(text: &str) -> BTreeMap<String, BotGroup> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (group, side, role, name) = (f.next()?, f.next()?, f.next()?, f.next()?);
            (!name.is_empty() && f.next().is_none()).then(|| {
                (
                    name.to_owned(),
                    BotGroup {
                        group: group.to_owned(),
                        side: side.to_owned(),
                        role: role.to_owned(),
                    },
                )
            })
        })
        .collect()
}

/// How often the groups file is read again, at most.
const GROUPS_EVERY: Duration = Duration::from_secs(2);
/// The groups file, in the control directory (written by `eve-bots`).
pub const GROUPS_FILE: &str = "bot-groups.tsv";
/// The action log, in the control directory.
pub const ACTION_LOG: &str = "admin-actions.log";
/// The longest request body read.
pub const MAX_BODY: usize = 4096;

impl Shared {
    /// A state with this token and map (tests; [`Shared::open`] is the real
    /// path).
    pub fn new(token: String, map: &MapData) -> Shared {
        Shared {
            started: crate::game_clock::clock::wall(),
            token,
            map_json: map.to_json(),
            system_names: map.systems.iter().map(|s| (s.id, s.name.clone())).collect(),
            state: Mutex::new(State::default()),
            ticks: Mutex::new(TickRing::new()),
            snap_cost_last_ns: AtomicU64::new(0),
            snap_cost_max_ns: AtomicU64::new(0),
            snap_cost_window_max_ns: AtomicU64::new(0),
            active: AtomicUsize::new(0),
            dir: None,
            groups: Mutex::new((None, BTreeMap::new())),
            actions: crate::admin_actions::Queue::default(),
            writers: std::sync::OnceLock::new(),
        }
    }

    /// Hand over the connection writers (`kick` evicts through them).
    pub fn set_writers(&self, w: Arc<crate::push::Writers>) {
        let _ = self.writers.set(w);
    }

    /// The connection writers, once set.
    pub(crate) fn writers(&self) -> Option<Arc<crate::push::Writers>> {
        self.writers.get().cloned()
    }

    /// The action queue the clock drains.
    pub(crate) fn actions(&self) -> &crate::admin_actions::Queue {
        &self.actions
    }

    /// The bot groups, by character name (the file, re-read every 2 s).
    fn bot_groups(&self) -> BTreeMap<String, BotGroup> {
        let Ok(mut g) = self.groups.lock() else {
            return BTreeMap::new();
        };
        let stale =
            g.0.is_none_or(|t| crate::game_clock::clock::wall_since(t) >= GROUPS_EVERY);
        if stale && let Some(dir) = &self.dir {
            g.0 = Some(crate::game_clock::clock::wall());
            g.1 = std::fs::read_to_string(dir.join(GROUPS_FILE))
                .map(|t| parse_groups(&t))
                .unwrap_or_default();
        }
        g.1.clone()
    }

    /// Set the groups directly (tests).
    #[cfg(test)]
    fn set_groups(&self, text: &str) {
        if let Ok(mut g) = self.groups.lock() {
            g.0 = Some(Instant::now());
            g.1 = parse_groups(text);
        }
    }

    /// Read or write the token in `dir`, bind `addr` and start serving.
    pub fn open(addr: SocketAddr, dir: &Path, map: &MapData) -> io::Result<Arc<Shared>> {
        let token = token_in(dir)?;
        let listener = TcpListener::bind(addr)?;
        let mut shared = Shared::new(token, map);
        shared.dir = Some(dir.to_path_buf());
        let shared = Arc::new(shared);
        serve(listener, Arc::clone(&shared))?;
        Ok(shared)
    }

    /// The clock's snapshot, taken under the world lock.
    pub fn publish(&self, snap: Snapshot) {
        if let Ok(mut s) = self.state.lock() {
            s.snap = Some(snap);
        }
    }

    /// The report window's numbers, its work samples, the clock's longest
    /// lock hold and its stage totals `(name, max, total)` (`push.rs`, off
    /// the world lock).
    pub fn note_window(
        &self,
        h: Health,
        work: &[Duration],
        held_max: Duration,
        stages: &[(&'static str, Duration, Duration)],
    ) {
        if let Ok(mut s) = self.state.lock() {
            s.health = Some(h);
            let ticks = h.ticks.max(1) as f64;
            s.stages.clear();
            s.stages.extend(stages.iter().map(|(n, max, total)| {
                (
                    *n,
                    max.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0 / ticks,
                )
            }));
            s.stages.sort_by(|a, b| b.1.total_cmp(&a.1));
            s.stages.truncate(STAGES_KEPT);
        }
        self.snap_cost_window_max_ns.store(
            self.snap_cost_max_ns.swap(0, Ordering::Relaxed),
            Ordering::Relaxed,
        );
        if let Ok(mut t) = self.ticks.lock() {
            t.add_window(h.unix, work, held_max, h.skipped);
        }
    }

    fn token_ok(&self, given: Option<&str>) -> bool {
        given.is_some_and(|t| same(t.trim(), &self.token))
    }
}

/// The token in `<dir>/admin.token`, kept when it is long enough, else a
/// fresh one written there (0600).
fn token_in(dir: &Path) -> io::Result<String> {
    let path = dir.join(TOKEN_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let t = text.trim();
        if t.len() >= 32 && t.bytes().all(|b| b.is_ascii_graphic()) {
            return Ok(t.to_owned());
        }
    }
    std::fs::create_dir_all(dir)?;
    let token: String = crate::accounts::urandom32()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// Equal without stopping at the first different byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ---------------------------------------------------------------- world slot

/// The world slot: the listener's state, when `--admin` is on.
#[derive(Default)]
pub struct AdminSlot {
    shared: Option<Arc<Shared>>,
}

/// The slot, for `world_ext::SLOTS`.
pub const SLOT: SlotSpec = SlotSpec {
    name: "admin_web",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(AdminSlot::default())
}

/// Hand the world the listener (`Server::bind`).
pub fn attach(reg: &mut Registry, shared: Arc<Shared>) {
    reg.ext_mut::<AdminSlot>().shared = Some(shared);
}

/// The listener's state, when `--admin` is on (the clock's action drain).
pub(crate) fn shared_of(w: &Registry) -> Option<Arc<Shared>> {
    w.ext::<AdminSlot>().shared.clone()
}

/// The clock's snapshot, under the lock; `None` without `--admin`.
pub fn take(w: &Registry) -> Option<(Arc<Shared>, Snapshot)> {
    let shared = Arc::clone(w.ext::<AdminSlot>().shared.as_ref()?);
    let t = crate::game_clock::clock::wall();
    let snap = snapshot(w);
    let ns = crate::game_clock::clock::wall_since(t)
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64;
    shared.snap_cost_last_ns.store(ns, Ordering::Relaxed);
    shared.snap_cost_max_ns.fetch_max(ns, Ordering::Relaxed);
    Some((shared, snap))
}

/// One row per warm or hot system. Under the world lock: O(live systems +
/// NPCs), plus `grid::local_balls` for hot systems only.
pub fn system_loads(w: &Registry) -> Vec<SystemLoad> {
    use crate::liveness::{Liveness, Tier};
    let mut npcs: BTreeMap<i64, u32> = BTreeMap::new();
    for npc in w.npcs().iter() {
        *npcs.entry(npc.solar_system_id).or_default() += 1;
    }
    let in_space: BTreeMap<i64, u32> = w
        .roster()
        .systems()
        .map(|(s, ids)| (s, ids.len() as u32))
        .collect();
    w.ext::<Liveness>()
        .iter()
        .map(|(system_id, live)| {
            let hot = live.tier == Tier::Hot;
            SystemLoad {
                system_id,
                hot,
                humans: live.humans,
                bots: live.bots,
                npcs: npcs.get(&system_id).copied().unwrap_or(0),
                in_space: in_space.get(&system_id).copied().unwrap_or(0),
                balls: hot.then(|| crate::grid::local_balls(w, system_id).len() as u32),
            }
        })
        .collect()
}

/// Every pilot online, in connection order.
pub fn snapshot(w: &Registry) -> Snapshot {
    let roster = w.roster();
    let mut pilots = Vec::new();
    for id in w.sessions().online_characters() {
        let Some(c) = w.character(id) else { continue };
        let presence = roster.get(id);
        let state = if presence.is_some() {
            "space"
        } else if w.jumps().contains(id) {
            "jumping"
        } else {
            "docked"
        };
        pilots.push(Pilot {
            character_id: id,
            name: c.name.clone(),
            user_id: c.owner_user_id,
            bot: c.is_bot,
            system_id: w.session_system(id),
            state,
            station_id: if state == "docked" {
                c.station_id
            } else {
                None
            },
            ship_type_id: c.ship_type_id,
            ship: crate::item::find_in(w.content(), c.ship_type_id)
                .map(|t| t.name.to_owned())
                .unwrap_or_default(),
            position: presence.map(|p| p.motion.position),
        });
    }
    Snapshot {
        taken_unix: unix_now(),
        pilots,
        npcs: w.npcs().iter().count(),
        systems: system_loads(w),
    }
}

fn unix_now() -> u64 {
    crate::game_clock::clock::wall_system()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------- map

/// One system on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSystem {
    pub id: i64,
    pub name: String,
    pub region_id: i64,
    pub security: f64,
    /// Galaxy-frame centre, metres.
    pub center: [f64; 3],
}

/// The cluster for the canvas: systems and gate links.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapData {
    pub systems: Vec<MapSystem>,
    /// `(a, b)` with `a < b`, each link once.
    pub links: Vec<(i64, i64)>,
}

impl MapData {
    /// From rows: `(id, name, region, security)` for each system, the
    /// centres, and gate pairs `(from_system, to_system)`. A system without
    /// a centre is left out. With any gate, only systems a gate touches are
    /// kept (known space; wormhole and hidden systems have no gates);
    /// without gates (no celestials loaded) every system with a centre in
    /// known space is.
    pub fn build(
        systems: impl IntoIterator<Item = (i64, String, i64, f64)>,
        centers: &crate::celestial::Centers,
        gates: impl IntoIterator<Item = (i64, i64)>,
    ) -> MapData {
        let links: BTreeSet<(i64, i64)> = gates
            .into_iter()
            .filter(|(a, b)| a != b && *a != 0 && *b != 0)
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        let gated: BTreeSet<i64> = links.iter().flat_map(|(a, b)| [*a, *b]).collect();
        let mut out: Vec<MapSystem> = systems
            .into_iter()
            .filter(|(id, ..)| {
                if gated.is_empty() {
                    crate::celestial::KNOWN_SPACE.contains(id)
                } else {
                    gated.contains(id)
                }
            })
            .filter_map(|(id, name, region_id, security)| {
                Some(MapSystem {
                    id,
                    name,
                    region_id,
                    security,
                    center: centers.system(id)?,
                })
            })
            .collect();
        out.sort_by_key(|s| s.id);
        let kept: BTreeSet<i64> = out.iter().map(|s| s.id).collect();
        MapData {
            systems: out,
            links: links
                .into_iter()
                .filter(|(a, b)| kept.contains(a) && kept.contains(b))
                .collect(),
        }
    }

    /// From the world's loaded tables (once, at bind).
    pub fn from_registry(reg: &Registry) -> MapData {
        let content = reg.content();
        let empty_c = crate::celestial::Centers::default();
        let centers = content
            .try_get::<crate::celestial::Centers>()
            .unwrap_or(&empty_c);
        let systems: Vec<(i64, String, i64, f64)> = content
            .try_get::<crate::celestial::Universe>()
            .map(|u| {
                u.systems()
                    .map(|s| (s.id, s.name.clone(), s.region_id, s.security))
                    .collect()
            })
            .unwrap_or_default();
        let catalogue = reg.celestials();
        let gates: Vec<(i64, i64)> = catalogue
            .served()
            .filter(|c| c.kind == crate::celestial::Kind::Gate)
            .filter_map(|c| Some((catalogue.system_of(c.id)?, catalogue.jump_of(c.id)?.1)))
            .collect();
        MapData::build(systems, centers, gates)
    }

    /// `{"systems":[[id,"name",region,sec,x,z],...],"links":[[a,b],...]}`,
    /// x and z in light years to three places. The page draws x right and
    /// z up (`-z` down the screen) [A].
    pub fn to_json(&self) -> String {
        let ly = crate::celestial::LIGHTYEAR;
        let mut s = String::from("{\"systems\":[");
        for (i, sys) in self.systems.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "[{},{},{},{},{},{}]",
                sys.id,
                json_str(&sys.name),
                sys.region_id,
                num(round(sys.security, 2)),
                num(round(sys.center[0] / ly, 3)),
                num(round(sys.center[2] / ly, 3)),
            );
        }
        s.push_str("],\"links\":[");
        for (i, (a, b)) in self.links.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(s, "[{a},{b}]");
        }
        s.push_str("]}");
        s
    }
}

fn round(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

// --------------------------------------------------------------------- JSON

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `<` too, so a name can never close a script tag.
            c if (c as u32) < 0x20 || c == '<' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A finite number, or `null`.
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".to_owned()
    }
}

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

/// `/api/status`.
pub fn status_json(shared: &Shared) -> String {
    let st = shared.state.lock().map(|s| (s.snap.clone(), s.health));
    let (snap, health) = st.unwrap_or((None, None));
    let now = unix_now();
    let mut s = String::new();
    let _ = write!(
        s,
        "{{\"uptime_s\":{},\"now_unix\":{now}",
        crate::game_clock::clock::wall_since(shared.started).as_secs()
    );
    match &snap {
        Some(snap) => {
            let humans = snap.pilots.iter().filter(|p| !p.bot).count();
            let in_space = snap.pilots.iter().filter(|p| p.state == "space").count();
            let docked = snap.pilots.iter().filter(|p| p.state == "docked").count();
            let systems: BTreeSet<i64> = snap.pilots.iter().filter_map(|p| p.system_id).collect();
            let _ = write!(
                s,
                ",\"snapshot_age_s\":{},\"pilots\":{},\"humans\":{humans},\"bots\":{},\
                 \"in_space\":{in_space},\"docked\":{docked},\"systems\":{},\"npcs\":{}",
                now.saturating_sub(snap.taken_unix),
                snap.pilots.len(),
                snap.pilots.len() - humans,
                systems.len(),
                snap.npcs
            );
        }
        None => s.push_str(",\"snapshot_age_s\":null"),
    }
    match health {
        Some(h) => {
            let _ = write!(
                s,
                ",\"tick\":{{\"window_unix\":{},\"ticks\":{},\"p95_ms\":{},\"held_max_ms\":{},\
                 \"skipped\":{}}}",
                h.unix,
                h.ticks,
                num(round(h.p95_ms, 2)),
                num(round(h.held_max_ms, 2)),
                h.skipped
            );
        }
        None => s.push_str(",\"tick\":null"),
    }
    let _ = write!(
        s,
        ",\"snapshot_cost_ms\":{{\"last\":{},\"max\":{}}}",
        num(round(ns_ms(&shared.snap_cost_last_ns), 3)),
        num(round(ns_ms(&shared.snap_cost_window_max_ns), 3)),
    );
    s.push('}');
    s
}

fn ns_ms(a: &AtomicU64) -> f64 {
    a.load(Ordering::Relaxed) as f64 / 1.0e6
}

/// `/api/pilots`: `{"taken_unix":N,"pilots":[{...}]}`.
pub fn pilots_json(shared: &Shared) -> String {
    let snap = shared
        .state
        .lock()
        .ok()
        .and_then(|s| s.snap.clone())
        .unwrap_or_default();
    let groups = shared.bot_groups();
    let mut s = format!("{{\"taken_unix\":{},\"pilots\":[", snap.taken_unix);
    for (i, p) in snap.pilots.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let system_name = p
            .system_id
            .and_then(|id| shared.system_names.get(&id))
            .map_or_else(|| "null".to_owned(), |n| json_str(n));
        let pos = p.position.map_or_else(
            || "null".to_owned(),
            |[x, y, z]| format!("[{},{},{}]", num(x.round()), num(y.round()), num(z.round())),
        );
        let bg = groups.get(&p.name).filter(|_| p.bot);
        let field =
            |f: fn(&BotGroup) -> &String| bg.map_or_else(|| "null".to_owned(), |g| json_str(f(g)));
        let (group, side, role) = (field(|g| &g.group), field(|g| &g.side), field(|g| &g.role));
        let _ = write!(
            s,
            "{{\"character_id\":{},\"name\":{},\"user_id\":{},\"bot\":{},\"system_id\":{},\
             \"system\":{system_name},\"state\":\"{}\",\"station_id\":{},\"ship_type_id\":{},\
             \"ship\":{},\"position\":{pos},\"group\":{group},\"side\":{side},\"role\":{role}}}",
            p.character_id,
            json_str(&p.name),
            opt(p.user_id),
            p.bot,
            opt(p.system_id),
            p.state,
            opt(p.station_id),
            p.ship_type_id,
            json_str(&p.ship),
        );
    }
    s.push_str("]}");
    s
}

/// `/api/systems`: the occupied and the live systems with counts. `humans`,
/// `bots` and `in_space` come from the pilot snapshot; `npcs`,
/// `in_space_all` (characters, NPCs and drones), `balls` (hot systems
/// only) and `hot` from the clock's per-system loads. The list holds every
/// live (warm or hot) system, including ones with no pilots, plus any system
/// a pilot is in.
pub fn systems_json(shared: &Shared) -> String {
    let snap = shared
        .state
        .lock()
        .ok()
        .and_then(|s| s.snap.clone())
        .unwrap_or_default();
    // system -> (humans, bots, in space)
    let mut by: BTreeMap<i64, (u32, u32, u32)> = BTreeMap::new();
    for p in &snap.pilots {
        let Some(id) = p.system_id else { continue };
        let e = by.entry(id).or_default();
        if p.bot {
            e.1 += 1;
        } else {
            e.0 += 1;
        }
        if p.state == "space" {
            e.2 += 1;
        }
    }
    let loads: BTreeMap<i64, &SystemLoad> = snap.systems.iter().map(|l| (l.system_id, l)).collect();
    for l in &snap.systems {
        by.entry(l.system_id).or_insert((l.humans, l.bots, 0));
    }
    let mut s = String::from("{\"systems\":[");
    for (i, (id, (h, b, sp))) in by.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let name = shared
            .system_names
            .get(id)
            .map_or_else(|| "null".to_owned(), |n| json_str(n));
        let load = loads.get(id).copied().copied().unwrap_or_default();
        let balls = load
            .balls
            .map_or_else(|| "null".to_owned(), |n| n.to_string());
        let _ = write!(
            s,
            "{{\"system_id\":{id},\"name\":{name},\"humans\":{h},\"bots\":{b},\"in_space\":{sp},\
             \"npcs\":{},\"in_space_all\":{},\"balls\":{balls},\"hot\":{}}}",
            load.npcs, load.in_space, load.hot
        );
    }
    s.push_str("]}");
    s
}

fn ms_of_us(us: u32) -> String {
    num(round(f64::from(us) / 1000.0, 2))
}

fn minute_json(m: &Minute) -> String {
    format!(
        "{{\"minute_unix\":{},\"ticks\":{},\"p50_ms\":{},\"p95_ms\":{},\"max_ms\":{},\
         \"held_max_ms\":{},\"skipped\":{}}}",
        m.minute * 60,
        m.ticks,
        ms_of_us(m.p50_us),
        ms_of_us(m.p95_us),
        ms_of_us(m.max_us),
        ms_of_us(m.held_max_us),
        m.skipped
    )
}

/// `/api/ticks`: the closed minutes, oldest first, the open minute and the
/// last window's slowest stages.
pub fn ticks_json(shared: &Shared) -> String {
    let (closed, open) = shared.ticks.lock().map_or((Vec::new(), None), |mut t| {
        (
            t.closed.iter().copied().collect::<Vec<_>>(),
            t.open_minute(),
        )
    });
    let stages = shared
        .state
        .lock()
        .map(|s| s.stages.clone())
        .unwrap_or_default();
    let mut s = String::from("{\"minutes\":[");
    for (i, m) in closed.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&minute_json(m));
    }
    s.push_str("],\"open\":");
    match open {
        Some(m) => s.push_str(&minute_json(&m)),
        None => s.push_str("null"),
    }
    s.push_str(",\"stages\":[");
    for (i, (name, max, mean)) in stages.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"stage\":{},\"max_ms\":{},\"mean_ms\":{}}}",
            json_str(name),
            num(round(*max, 2)),
            num(round(*mean, 2))
        );
    }
    s.push_str("]}");
    s
}

fn dur_ms(d: Duration) -> String {
    num(round(d.as_secs_f64() * 1000.0, 2))
}

/// `/api/methods`: the last window's slowest calls.
pub fn methods_json(w: &crate::live_status::prof::MethodsWindow) -> String {
    let mut s = String::from("{\"window_unix\":");
    if w.unix == 0 {
        s.push_str("null");
    } else {
        let _ = write!(s, "{}", w.unix);
    }
    let _ = write!(s, ",\"secs\":{},\"methods\":[", num(round(w.secs, 1)));
    for (i, r) in w.rows.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            "{{\"label\":{},\"count\":{},\"hold_ms\":{},\"max_ms\":{},\"mode\":\"{}\"}}",
            json_str(&r.label),
            r.count,
            dur_ms(r.hold),
            dur_ms(r.max),
            r.mode
        );
    }
    s.push_str("]}");
    s
}

/// The content type of `/api/metrics`.
const METRICS_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// A label value: `\` -> `\\`, `"` -> `\"`, newline -> `\n` (the exposition
/// format's three escapes).
fn label_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// `/api/metrics`: the Prometheus text format (version 0.0.4), by hand.
/// Reading it resets the snapshot-cost max.
pub fn metrics_text(shared: &Shared) -> String {
    let methods = crate::live_status::prof::last_methods().unwrap_or_default();
    render_metrics(shared, &methods)
}

fn render_metrics(shared: &Shared, methods: &crate::live_status::prof::MethodsWindow) -> String {
    let (snap, stages) = shared
        .state
        .lock()
        .map(|s| (s.snap.clone().unwrap_or_default(), s.stages.clone()))
        .unwrap_or_default();
    let (ticks_total, skipped_total, minute) = shared.ticks.lock().map_or((0, 0, None), |mut t| {
        let m = t.closed.back().copied().or_else(|| t.open_minute());
        (t.ticks_total, t.skipped_total, m)
    });
    let sec = |us: u32| f64::from(us) / 1.0e6;
    let mut o = String::new();
    fn fam(o: &mut String, name: &str, kind: &str, help: &str) {
        let _ = write!(o, "# HELP {name} {help}\n# TYPE {name} {kind}\n");
    }
    fam(
        &mut o,
        "eve_uptime_seconds",
        "gauge",
        "Seconds since the admin listener started.",
    );
    let _ = writeln!(
        o,
        "eve_uptime_seconds {}",
        crate::game_clock::clock::wall_since(shared.started).as_secs()
    );
    fam(
        &mut o,
        "eve_ticks_total",
        "counter",
        "Clock ticks since start.",
    );
    let _ = writeln!(o, "eve_ticks_total {ticks_total}");
    fam(
        &mut o,
        "eve_tick_skipped_total",
        "counter",
        "Clock ticks skipped since start.",
    );
    let _ = writeln!(o, "eve_tick_skipped_total {skipped_total}");
    if let Some(m) = minute {
        fam(
            &mut o,
            "eve_tick_work_seconds",
            "gauge",
            "Tick work time over the last minute.",
        );
        for (q, us) in [("0.5", m.p50_us), ("0.95", m.p95_us), ("1", m.max_us)] {
            let _ = writeln!(
                o,
                "eve_tick_work_seconds{{quantile=\"{q}\"}} {:.6}",
                sec(us)
            );
        }
        fam(
            &mut o,
            "eve_tick_lock_held_max_seconds",
            "gauge",
            "Longest world-lock hold of the clock over the last minute.",
        );
        let _ = writeln!(
            o,
            "eve_tick_lock_held_max_seconds {:.6}",
            sec(m.held_max_us)
        );
    }
    fam(
        &mut o,
        "eve_tick_stage_max_seconds",
        "gauge",
        "Longest tick stage, last window.",
    );
    for (name, max, _) in &stages {
        let _ = writeln!(
            o,
            "eve_tick_stage_max_seconds{{stage=\"{}\"}} {:.6}",
            label_value(name),
            max / 1000.0
        );
    }
    fam(&mut o, "eve_pilots", "gauge", "Pilots online.");
    for bot in [false, true] {
        for state in ["space", "docked", "jumping"] {
            let n = snap
                .pilots
                .iter()
                .filter(|p| p.bot == bot && p.state == state)
                .count();
            let kind = if bot { "bot" } else { "human" };
            let _ = writeln!(o, "eve_pilots{{kind=\"{kind}\",state=\"{state}\"}} {n}");
        }
    }
    fam(&mut o, "eve_npcs", "gauge", "NPCs in the world.");
    let _ = writeln!(o, "eve_npcs {}", snap.npcs);
    fam(
        &mut o,
        "eve_system_pilots",
        "gauge",
        "Pilots in a live system.",
    );
    for l in &snap.systems {
        let _ = writeln!(
            o,
            "eve_system_pilots{{system=\"{0}\",kind=\"human\"}} {1}\n\
             eve_system_pilots{{system=\"{0}\",kind=\"bot\"}} {2}",
            l.system_id, l.humans, l.bots
        );
    }
    fam(&mut o, "eve_system_npcs", "gauge", "NPCs in a live system.");
    for l in &snap.systems {
        let _ = writeln!(
            o,
            "eve_system_npcs{{system=\"{}\"}} {}",
            l.system_id, l.npcs
        );
    }
    fam(
        &mut o,
        "eve_system_balls",
        "gauge",
        "Balls in a hot system.",
    );
    for l in &snap.systems {
        if let Some(b) = l.balls {
            let _ = writeln!(o, "eve_system_balls{{system=\"{}\"}} {b}", l.system_id);
        }
    }
    fam(
        &mut o,
        "eve_method_calls",
        "gauge",
        "Calls of a handler, last window.",
    );
    for r in &methods.rows {
        let _ = writeln!(
            o,
            "eve_method_calls{{method=\"{}\"}} {}",
            label_value(&r.label),
            r.count
        );
    }
    fam(
        &mut o,
        "eve_method_hold_max_seconds",
        "gauge",
        "Longest hold of a handler, last window.",
    );
    for r in &methods.rows {
        let _ = writeln!(
            o,
            "eve_method_hold_max_seconds{{method=\"{}\"}} {:.6}",
            label_value(&r.label),
            r.max.as_secs_f64()
        );
    }
    fam(
        &mut o,
        "eve_method_hold_seconds",
        "gauge",
        "Summed hold of a handler, last window.",
    );
    for r in &methods.rows {
        let _ = writeln!(
            o,
            "eve_method_hold_seconds{{method=\"{}\"}} {:.6}",
            label_value(&r.label),
            r.hold.as_secs_f64()
        );
    }
    fam(
        &mut o,
        "eve_admin_snapshot_seconds",
        "gauge",
        "Cost of the admin snapshot under the world lock.",
    );
    let last = shared.snap_cost_last_ns.load(Ordering::Relaxed);
    let max = shared.snap_cost_window_max_ns.load(Ordering::Relaxed);
    let _ = writeln!(
        o,
        "eve_admin_snapshot_seconds{{stat=\"last\"}} {:.6}",
        last as f64 / 1.0e9
    );
    let _ = writeln!(
        o,
        "eve_admin_snapshot_seconds{{stat=\"max\"}} {:.6}",
        max as f64 / 1.0e9
    );
    o
}

// --------------------------------------------------------------------- HTTP

/// An answer: status line, content type, body.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: String) -> Response {
        Response {
            status,
            content_type,
            body,
        }
    }

    fn error(status: u16, msg: &str) -> Response {
        Response::new(
            status,
            "application/json",
            format!("{{\"error\":{}}}", json_str(msg)),
        )
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            429 => "Too Many Requests",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Error",
        }
    }

    /// The whole HTTP/1.1 answer, connection closed after it.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
             X-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
             Content-Security-Policy: default-src 'self'; style-src 'self'; script-src 'self'\r\n\
             Connection: close\r\n",
            self.status,
            self.reason(),
            self.content_type,
            self.body.len()
        );
        if self.status == 204 {
            // RFC 9110 §8.6: no Content-Length on a 204.
            head = head.replace("Content-Length: 0\r\n", "");
        }
        if self.status == 401 {
            head.push_str("WWW-Authenticate: X-Admin-Token\r\n");
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }
}

/// Answer one request. `token` is the `X-Admin-Token` header's value.
pub fn route(shared: &Shared, method: &str, target: &str, token: Option<&str>) -> Response {
    if method != "GET" {
        return Response::error(405, "GET only");
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    match path {
        "/" | "/index.html" => {
            return Response::new(200, "text/html; charset=utf-8", INDEX_HTML.to_owned());
        }
        "/admin.js" => {
            return Response::new(200, "text/javascript; charset=utf-8", ADMIN_JS.to_owned());
        }
        "/admin.css" => {
            return Response::new(200, "text/css; charset=utf-8", ADMIN_CSS.to_owned());
        }
        // No icon: answered empty, so a browser does not log a 404.
        "/favicon.ico" => return Response::new(204, "image/x-icon", String::new()),
        _ => {}
    }
    if !path.starts_with("/api/") {
        return Response::error(404, "not found");
    }
    if !shared.token_ok(token) {
        return Response::error(401, "token required");
    }
    let json = |body| Response::new(200, "application/json", body);
    match path {
        "/api/status" => json(status_json(shared)),
        "/api/pilots" => json(pilots_json(shared)),
        "/api/systems" => json(systems_json(shared)),
        "/api/ticks" => json(ticks_json(shared)),
        "/api/methods" => json(methods_json(
            &crate::live_status::prof::last_methods().unwrap_or_default(),
        )),
        "/api/metrics" => Response::new(200, METRICS_TYPE, metrics_text(shared)),
        "/api/map" => json(shared.map_json.clone()),
        _ => Response::error(404, "no such API"),
    }
}

/// A request head, as far as the listener reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub token: Option<String>,
    /// `Authorization: Bearer X`: accepted for GET only.
    pub bearer: Option<String>,
    pub content_type: Option<String>,
    pub origin: Option<String>,
    pub host: Option<String>,
    pub content_length: Option<usize>,
}

/// The token a GET carries: `X-Admin-Token`, else `Authorization: Bearer`.
/// (A POST uses `Head::token` only.)
pub fn get_token(h: &Head) -> Option<&str> {
    h.token.as_deref().or(h.bearer.as_deref())
}

/// The head, or `None` when it is not one.
pub fn parse_head_full(head: &str) -> Option<Head> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    if !first.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let mut h = Head {
        method,
        target,
        ..Head::default()
    };
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case(TOKEN_HEADER) {
            h.token = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("authorization") {
            let mut it = v.split_whitespace();
            if it.next().is_some_and(|s| s.eq_ignore_ascii_case("bearer")) {
                h.bearer = it.next().map(str::to_owned);
            }
        } else if k.eq_ignore_ascii_case("content-type") {
            h.content_type = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("origin") {
            h.origin = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("host") {
            h.host = Some(v.to_owned());
        } else if k.eq_ignore_ascii_case("content-length") {
            h.content_length = v.parse().ok();
        }
    }
    Some(h)
}

/// `(method, target, token)` from a request head, or `None` when it is
/// not one.
pub fn parse_head(head: &str) -> Option<(String, String, Option<String>)> {
    let h = parse_head_full(head)?;
    Some((h.method, h.target, h.token))
}

/// How long a `POST` waits for the clock to run its action.
const ACTION_WAIT: Duration = Duration::from_secs(10);

/// `POST /api/action/<name>`: the token, a JSON content type, no foreign
/// `Origin`, then the rate limit; the action runs on the clock and its text
/// comes back as `{"ok":true,"text":"..."}`. `peer` is for the log.
pub fn route_post(shared: &Shared, h: &Head, body: &str, peer: &str) -> Response {
    let path = h.target.split(['?', '#']).next().unwrap_or("");
    let Some(name) = path.strip_prefix("/api/action/") else {
        return Response::error(404, "not found");
    };
    if !shared.token_ok(h.token.as_deref()) {
        return Response::error(401, "token required");
    }
    let json = h
        .content_type
        .as_deref()
        .and_then(|c| c.split(';').next())
        .is_some_and(|c| c.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Response::error(415, "Content-Type must be application/json");
    }
    if let Some(origin) = &h.origin {
        let own = h.host.as_deref().is_some_and(|host| {
            origin == &format!("http://{host}") || origin == &format!("https://{host}")
        });
        if !own {
            return Response::error(403, "foreign Origin");
        }
    }
    let action = match crate::admin_actions::Action::parse(name, body) {
        Ok(a) => a,
        Err(why) if why == "no such action" => return Response::error(404, &why),
        Err(why) => return Response::error(400, &why),
    };
    if !shared
        .actions
        .allow(action.name(), crate::game_clock::clock::wall())
    {
        return Response::error(429, "one per second per action");
    }
    let (name, args) = (action.name(), action.args());
    let (tx, rx) = std::sync::mpsc::channel();
    if !shared
        .actions
        .push(crate::admin_actions::Job { action, reply: tx })
    {
        return Response::error(503, "too many actions waiting");
    }
    let Ok(text) = rx.recv_timeout(ACTION_WAIT) else {
        return Response::error(504, "no answer in time");
    };
    shared.log_action(peer, name, &args, &text);
    if let Some(why) = text.strip_prefix(crate::admin_actions::REFUSED) {
        return Response::error(409, why);
    }
    Response::new(
        200,
        "application/json",
        format!("{{\"ok\":true,\"text\":{}}}", json_str(&text)),
    )
}

impl Shared {
    /// `[admin] <unix> <peer> <action> <args> -> <result>` on stdout and
    /// appended to `<dir>/admin-actions.log`, one line each.
    fn log_action(&self, peer: &str, action: &str, args: &str, result: &str) {
        let one = |t: &str| t.replace(['\n', '\r'], " / ");
        let line = format!(
            "[admin] {} {peer} {action} {} -> {}",
            unix_now(),
            one(args),
            one(result)
        );
        println!("{line}");
        if let Some(dir) = &self.dir
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(ACTION_LOG))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Accept connections on `listener`, one short-lived thread each.
pub fn serve(listener: TcpListener, shared: Arc<Shared>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("admin-web".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if shared.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
                    shared.active.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let s = Arc::clone(&shared);
                let spawned = std::thread::Builder::new()
                    .name("admin-conn".to_owned())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        answer(stream, &s);
                        s.active.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    shared.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
    Ok(())
}

fn answer(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(IO_WAIT));
    let _ = stream.set_write_timeout(Some(IO_WAIT));
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break Some(i);
        }
        if buf.len() >= MAX_HEAD {
            break None;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = head_end
        .and_then(|i| std::str::from_utf8(&buf[..i]).ok())
        .and_then(parse_head_full);
    let resp = match (head, head_end) {
        (Some(h), Some(end)) if h.method == "POST" => {
            let want = h.content_length.unwrap_or(0);
            if want > MAX_BODY {
                Response::error(413, "body too large")
            } else {
                let mut body = buf[(end + 4).min(buf.len())..].to_vec();
                while body.len() < want {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                    }
                }
                body.truncate(want);
                match String::from_utf8(body) {
                    Ok(body) if body.len() == want => {
                        let peer = stream
                            .peer_addr()
                            .map_or_else(|_| "?".to_owned(), |a| a.to_string());
                        route_post(shared, &h, &body, &peer)
                    }
                    _ => Response::error(400, "bad body"),
                }
            }
        }
        (Some(h), _) => route(shared, &h.method, &h.target, get_token(&h)),
        (None, _) => Response::error(400, "bad request"),
    };
    let _ = stream.write_all(&resp.to_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centers() -> crate::celestial::Centers {
        let ly = crate::celestial::LIGHTYEAR;
        let lines: Vec<String> = [
            format!("s\t39000001\t{}\t0\t{}", ly, -2.0 * ly),
            format!("s\t39000002\t{}\t0\t0", 3.0 * ly),
            "s\t39000003\t0\t0\t0".to_owned(),
            "c\t29000001\t0\t0\t0".to_owned(),
        ]
        .into();
        crate::celestial::Centers::parse(&lines).expect("invented centres parse")
    }

    fn rows() -> Vec<(i64, String, i64, f64)> {
        vec![
            (39_000_001, "Alpha \"A\"".to_owned(), 19_990_001, 0.95),
            (39_000_002, "Beta".to_owned(), 19_990_001, -0.2),
            (39_000_003, "Gamma".to_owned(), 19_000_002, 0.4),
            // No centre: left out.
            (39_000_004, "Delta".to_owned(), 19_000_002, 0.4),
        ]
    }

    fn map() -> MapData {
        MapData::build(
            rows(),
            &centers(),
            vec![
                (39_000_001, 39_000_002),
                (39_000_002, 39_000_001),
                (39_000_002, 39_000_004),
            ],
        )
    }

    fn shared_with(pilots: Vec<Pilot>) -> Shared {
        let s = Shared::new("t0k3n".repeat(8), &map());
        s.publish(Snapshot {
            taken_unix: 1,
            pilots,
            npcs: 3,
            systems: Vec::new(),
        });
        s
    }

    fn pilot(id: i64, bot: bool, state: &'static str, system: i64) -> Pilot {
        Pilot {
            character_id: id,
            name: format!("Pilot {id}"),
            user_id: Some(7),
            bot,
            system_id: Some(system),
            state,
            station_id: (state == "docked").then_some(69_000_001),
            ship_type_id: 99_000_001,
            ship: "Invented Hull".to_owned(),
            position: (state == "space").then_some([1.4, -2.6, 3.0]),
        }
    }

    #[test]
    fn map_keeps_gated_systems_with_centres_and_each_link_once() {
        let m = map();
        let ids: Vec<i64> = m.systems.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![39_000_001, 39_000_002]);
        assert_eq!(m.links, vec![(39_000_001, 39_000_002)]);
        let json = m.to_json();
        assert_eq!(
            json,
            "{\"systems\":[[39000001,\"Alpha \\\"A\\\"\",19990001,0.95,1,-2],\
             [39000002,\"Beta\",19990001,-0.2,3,0]],\"links\":[[39000001,39000002]]}"
        );
    }

    #[test]
    fn map_without_gates_keeps_known_space_only() {
        let ly = crate::celestial::LIGHTYEAR;
        let lines = vec![
            format!("s\t30999990\t{ly}\t0\t0"),
            "s\t39000001\t0\t0\t0".to_owned(),
        ];
        let c = crate::celestial::Centers::parse(&lines).expect("parse");
        let m = MapData::build(
            vec![
                (30_999_990, "K".to_owned(), 1, 0.5),
                (39_000_001, "J".to_owned(), 1, -1.0),
            ],
            &c,
            Vec::new(),
        );
        assert_eq!(m.systems.len(), 1);
        assert_eq!(m.systems[0].id, 30_999_990);
    }

    #[test]
    fn api_refuses_without_or_with_a_wrong_token() {
        let s = shared_with(Vec::new());
        for path in [
            "/api/status",
            "/api/pilots",
            "/api/map",
            "/api/systems",
            "/api/ticks",
            "/api/methods",
            "/api/metrics",
            "/api/x",
        ] {
            assert_eq!(route(&s, "GET", path, None).status, 401, "{path}");
            assert_eq!(route(&s, "GET", path, Some("nope")).status, 401, "{path}");
        }
        assert_eq!(
            route(&s, "GET", "/api/status", Some(&"t0k3n".repeat(8))).status,
            200
        );
        assert_eq!(route(&s, "POST", "/api/status", None).status, 405);
        assert_eq!(route(&s, "GET", "/", None).status, 200);
        assert_eq!(route(&s, "GET", "/favicon.ico", None).status, 204);
        assert_eq!(route(&s, "GET", "/etc/passwd", None).status, 404);
    }

    #[test]
    fn answers_carry_no_cors_header() {
        let s = shared_with(Vec::new());
        let bytes = route(&s, "GET", "/api/status", Some(&"t0k3n".repeat(8))).to_bytes();
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(!text.to_ascii_lowercase().contains("access-control"));
        assert!(text.contains("Cache-Control: no-store"));
    }

    #[test]
    fn pilots_json_shape_on_an_invented_world() {
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_002),
        ]);
        let json = pilots_json(&s);
        assert_eq!(
            json,
            "{\"taken_unix\":1,\"pilots\":[\
             {\"character_id\":90000001,\"name\":\"Pilot 90000001\",\"user_id\":7,\"bot\":false,\
             \"system_id\":39000001,\"system\":\"Alpha \\\"A\\\"\",\"state\":\"space\",\
             \"station_id\":null,\"ship_type_id\":99000001,\"ship\":\"Invented Hull\",\
             \"position\":[1,-3,3],\"group\":null,\"side\":null,\"role\":null},\
             {\"character_id\":90000002,\"name\":\"Pilot 90000002\",\"user_id\":7,\"bot\":true,\
             \"system_id\":39000002,\"system\":\"Beta\",\"state\":\"docked\",\
             \"station_id\":69000001,\"ship_type_id\":99000001,\"ship\":\"Invented Hull\",\
             \"position\":null,\"group\":null,\"side\":null,\"role\":null}]}"
        );
    }

    #[test]
    fn status_and_systems_count_the_snapshot() {
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_001),
            pilot(90_000_003, true, "space", 39_000_002),
        ]);
        s.note_window(
            Health {
                unix: 5,
                ticks: 40,
                p95_ms: 1.234,
                held_max_ms: 2.0,
                skipped: 0,
            },
            &[],
            Duration::ZERO,
            &[],
        );
        let st = status_json(&s);
        for part in [
            "\"pilots\":3",
            "\"humans\":1",
            "\"bots\":2",
            "\"in_space\":2",
            "\"docked\":1",
            "\"systems\":2",
            "\"npcs\":3",
            "\"tick\":{\"window_unix\":5,\"ticks\":40,\"p95_ms\":1.23,\"held_max_ms\":2,\"skipped\":0}",
        ] {
            assert!(st.contains(part), "{part} in {st}");
        }
        assert_eq!(
            systems_json(&s),
            "{\"systems\":[\
             {\"system_id\":39000001,\"name\":\"Alpha \\\"A\\\"\",\"humans\":1,\"bots\":1,\"in_space\":1,\
             \"npcs\":0,\"in_space_all\":0,\"balls\":null,\"hot\":false},\
             {\"system_id\":39000002,\"name\":\"Beta\",\"humans\":0,\"bots\":1,\"in_space\":1,\
             \"npcs\":0,\"in_space_all\":0,\"balls\":null,\"hot\":false}]}"
        );
    }

    #[test]
    fn json_str_escapes_quotes_controls_and_angle() {
        assert_eq!(
            json_str("a\"b\\c\n</script>"),
            "\"a\\\"b\\\\c\\n\\u003c/script>\""
        );
    }

    #[test]
    fn parse_head_reads_the_token_case_insensitively() {
        let (m, t, tok) =
            parse_head("GET /api/status?x=1 HTTP/1.1\r\nHost: a\r\nx-ADMIN-token:  abc \r\n")
                .expect("head");
        assert_eq!(
            (m.as_str(), t.as_str(), tok.as_deref()),
            ("GET", "/api/status?x=1", Some("abc"))
        );
        assert!(parse_head("garbage").is_none());
    }

    /// An invented world: one human docked, one bot in space; `take` is
    /// `None` until a listener is attached.
    #[test]
    fn snapshot_reads_docked_and_in_space_pilots() {
        use crate::space_state::{Motion, Presence};
        let pilot = |id, name: &str, ship, is_bot| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            is_bot,
            solar_system_id: Some(39_000_001),
            station_id: Some(69_000_001),
            ship_type_id: 99_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(91_000_001, "Human", 92_000_001, false),
                pilot(91_000_002, "Bot", 92_000_002, true),
                // Offline: not in the snapshot.
                pilot(91_000_003, "Away", 92_000_003, false),
            ],
            ..crate::model::SandboxConfig::default()
        });
        for (n, id) in [(1, 91_000_001), (2, 91_000_002)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        reg.roster_mut().arrive(Presence::arriving(
            91_000_002,
            92_000_002,
            39_000_002,
            Motion::arrived([10.0, 20.0, 30.0], [0.0; 3]),
        ));
        assert!(take(&reg).is_none());
        attach(&mut reg, Arc::new(Shared::new("x".repeat(32), &map())));
        type Row = (
            i64,
            bool,
            Option<i64>,
            &'static str,
            Option<i64>,
            Option<[f64; 3]>,
        );
        let (_, snap) = take(&reg).expect("attached");
        let got: Vec<Row> = snap
            .pilots
            .iter()
            .map(|p| {
                (
                    p.character_id,
                    p.bot,
                    p.system_id,
                    p.state,
                    p.station_id,
                    p.position,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    91_000_001,
                    false,
                    Some(39_000_001),
                    "docked",
                    Some(69_000_001),
                    None
                ),
                (
                    91_000_002,
                    true,
                    Some(39_000_002),
                    "space",
                    None,
                    Some([10.0, 20.0, 30.0])
                ),
            ]
        );
    }

    fn post_head(path: &str, token: Option<&str>) -> Head {
        Head {
            method: "POST".to_owned(),
            target: path.to_owned(),
            token: token.map(str::to_owned),
            bearer: None,
            content_type: Some("application/json".to_owned()),
            origin: None,
            host: Some("admin.example:8090".to_owned()),
            content_length: None,
        }
    }

    #[test]
    fn actions_refuse_without_token_wrong_type_or_foreign_origin() {
        let s = shared_with(Vec::new());
        let tok = "t0k3n".repeat(8);
        let body = "{\"words\":\"list\"}";
        let post = |h: &Head| route_post(&s, h, body, "peer").status;
        assert_eq!(post(&post_head("/api/action/bots", None)), 401);
        assert_eq!(post(&post_head("/api/action/bots", Some("nope"))), 401);
        let mut h = post_head("/api/action/bots", Some(&tok));
        h.content_type = Some("text/plain".to_owned());
        assert_eq!(post(&h), 415);
        h.content_type = None;
        assert_eq!(post(&h), 415);
        let mut h = post_head("/api/action/bots", Some(&tok));
        h.origin = Some("http://evil.example".to_owned());
        assert_eq!(post(&h), 403);
        h.origin = Some("http://admin.example:8090.evil.example".to_owned());
        assert_eq!(post(&h), 403);
        // Nothing reached the queue.
        assert!(s.actions().allow("bots", Instant::now()));
        assert_eq!(
            route_post(&s, &post_head("/api/action/nope", Some(&tok)), "{}", "p").status,
            404
        );
        assert_eq!(
            route_post(&s, &post_head("/api/action/bots", Some(&tok)), "x", "p").status,
            400
        );
    }

    /// Over a socket: a POST with the token and the listener's own Origin is
    /// queued; a thread playing the clock answers it; the second one inside a
    /// second is 429.
    #[test]
    fn an_action_is_queued_answered_and_rate_limited() {
        let dir = std::env::temp_dir().join(format!("admin-act-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let tok = "t0k3n".repeat(8);
        let mut shared = Shared::new(tok.clone(), &map());
        shared.dir = Some(dir.clone());
        let shared = Arc::new(shared);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        serve(listener, Arc::clone(&shared)).expect("serve");
        let clock = Arc::clone(&shared);
        let fake_clock = std::thread::spawn(move || {
            for _ in 0..200 {
                for job in clock.actions().take_all_for_test() {
                    let _ =
                        job.reply
                            .send(format!("ran {} {}", job.action.name(), job.action.args()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let ask = |origin: &str| {
            let body = "{\"words\":\"list now\"}";
            let mut c = TcpStream::connect(addr).expect("connect");
            c.set_read_timeout(Some(Duration::from_secs(8))).expect("t");
            write!(
                c,
                "POST /api/action/bots HTTP/1.1\r\nHost: {addr}\r\n{origin}\
                 Content-Type: application/json\r\nX-Admin-Token: {tok}\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .expect("write");
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        let foreign = ask("Origin: http://evil.example\r\n");
        assert!(foreign.starts_with("HTTP/1.1 403 "), "{foreign}");
        let own = ask(&format!("Origin: http://{addr}\r\n"));
        assert!(own.starts_with("HTTP/1.1 200 "), "{own}");
        assert!(
            own.contains("{\"ok\":true,\"text\":\"ran bots list now\"}"),
            "{own}"
        );
        let again = ask("");
        assert!(again.starts_with("HTTP/1.1 429 "), "{again}");
        let log = std::fs::read_to_string(dir.join(ACTION_LOG)).expect("log");
        assert!(log.contains("bots list now -> ran bots list now"), "{log}");
        assert!(log.starts_with("[admin] "), "{log}");
        let _ = fake_clock.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bot_groups_join_the_pilots_by_name_and_only_for_bots() {
        let text = "#group\tside\trole\tname\n\
                    g1\tred\tmine\tPilot 90000002\n\
                    bad row\n\
                    g2\tblue\tescort\tPilot 90000001\n";
        let got = parse_groups(text);
        assert_eq!(got.len(), 2, "{got:?}");
        let s = shared_with(vec![
            pilot(90_000_001, false, "space", 39_000_001),
            pilot(90_000_002, true, "docked", 39_000_002),
        ]);
        s.set_groups(text);
        let json = pilots_json(&s);
        // The human named in the file gets nothing; the bot gets its row.
        assert!(
            json.contains("\"position\":[1,-3,3],\"group\":null,\"side\":null,\"role\":null}"),
            "{json}"
        );
        assert!(
            json.contains("\"group\":\"g1\",\"side\":\"red\",\"role\":\"mine\""),
            "{json}"
        );
    }

    /// Over a real socket on loopback: no token, 401; the token, 200.
    #[test]
    fn listener_refuses_without_the_token() {
        let dir = std::env::temp_dir().join(format!("admin-web-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Shared::open("127.0.0.1:0".parse().expect("addr"), &dir, &map());
        // `open` binds an ephemeral port we cannot read back; bind our own.
        drop(shared);
        let token = std::fs::read_to_string(dir.join(TOKEN_FILE)).expect("token file");
        let token = token.trim().to_owned();
        assert_eq!(token.len(), 64);
        // A second open keeps the token.
        assert_eq!(token_in(&dir).expect("again"), token);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let s = Arc::new(Shared::new(token.clone(), &map()));
        serve(listener, s).expect("serve");
        let ask = |extra: &str| {
            let mut c = TcpStream::connect(addr).expect("connect");
            c.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            write!(c, "GET /api/status HTTP/1.1\r\nHost: x\r\n{extra}\r\n").expect("write");
            let mut out = String::new();
            let _ = c.read_to_string(&mut out);
            out
        };
        assert!(ask("").starts_with("HTTP/1.1 401 "));
        assert!(ask(&format!("X-Admin-Token: {token}\r\n")).starts_with("HTTP/1.1 200 "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ms_list(a: u64, b: u64) -> Vec<Duration> {
        (a..=b).map(Duration::from_millis).collect()
    }

    #[test]
    fn tick_ring_closes_a_minute_with_exact_quantiles() {
        let mut r = TickRing::new();
        r.add_window(10, &ms_list(1, 100), Duration::from_millis(7), 2);
        assert!(r.closed.is_empty());
        r.add_window(70, &ms_list(1, 1), Duration::ZERO, 0);
        let m = r.closed.back().copied().expect("a closed minute");
        assert_eq!(m.minute, 0);
        assert_eq!(m.ticks, 100);
        assert_eq!(m.p50_us, 50_000);
        assert_eq!(m.p95_us, 95_000);
        assert_eq!(m.max_us, 100_000);
        assert_eq!(m.held_max_us, 7_000);
        assert_eq!(m.skipped, 2);
        assert_eq!(r.ticks_total, 101);
    }

    #[test]
    fn tick_ring_keeps_sixty_minutes_and_reuses_its_buffer() {
        let mut r = TickRing::new();
        let (cap, qcap) = (r.samples.capacity(), r.closed.capacity());
        assert!(cap >= MINUTE_SAMPLES);
        for minute in 0..70u64 {
            r.add_window(minute * 60 + 5, &ms_list(1, 40), Duration::ZERO, 0);
        }
        r.close();
        assert_eq!(r.closed.len(), MINUTES_KEPT);
        assert_eq!(r.closed.front().map(|m| m.minute), Some(10));
        assert_eq!(r.closed.back().map(|m| m.minute), Some(69));
        assert_eq!(r.samples.capacity(), cap, "the sample buffer is reused");
        assert_eq!(r.closed.capacity(), qcap, "the ring is allocated once");
    }

    #[test]
    fn tick_ring_ignores_a_window_older_than_the_open_minute() {
        let mut r = TickRing::new();
        r.add_window(180, &[Duration::from_millis(1)], Duration::ZERO, 0);
        r.add_window(60, &[Duration::from_millis(9)], Duration::ZERO, 5);
        assert_eq!(r.open, Some(3));
        assert_eq!(r.samples.len(), 1);
        assert_eq!(r.skipped, 0);
        assert!(r.closed.is_empty());
    }

    #[test]
    fn tick_ring_counts_samples_past_the_cap() {
        let mut r = TickRing::new();
        let cap = r.samples.capacity();
        let many = vec![Duration::from_millis(1); MINUTE_SAMPLES + 3];
        r.add_window(0, &many, Duration::ZERO, 0);
        assert_eq!(r.samples.len(), MINUTE_SAMPLES);
        assert_eq!(r.samples.capacity(), cap, "no growth past the cap");
        r.close();
        assert_eq!(
            r.closed.back().map(|m| m.ticks),
            Some(MINUTE_SAMPLES as u32 + 3)
        );
    }

    #[test]
    fn ticks_json_lists_minutes_oldest_first() {
        let s = shared_with(Vec::new());
        s.note_window(
            Health {
                unix: 30,
                ticks: 2,
                ..Health::default()
            },
            &[Duration::from_micros(1_234), Duration::from_micros(2_000)],
            Duration::from_millis(3),
            &[(
                "invented_stage",
                Duration::from_millis(4),
                Duration::from_millis(2),
            )],
        );
        s.note_window(
            Health {
                unix: 90,
                ticks: 1,
                ..Health::default()
            },
            &[Duration::from_millis(5)],
            Duration::ZERO,
            &[],
        );
        s.note_window(
            Health {
                unix: 150,
                ticks: 1,
                ..Health::default()
            },
            &[Duration::from_millis(6)],
            Duration::ZERO,
            &[],
        );
        let j = ticks_json(&s);
        let first = j.find("\"minute_unix\":0,").expect("minute 0");
        let second = j.find("\"minute_unix\":60,").expect("minute 1");
        assert!(first < second, "{j}");
        assert!(
            j.contains(
                "{\"minute_unix\":0,\"ticks\":2,\"p50_ms\":1.23,\"p95_ms\":2,\"max_ms\":2,\
                 \"held_max_ms\":3,\"skipped\":0}"
            ),
            "{j}"
        );
        assert!(j.contains("\"open\":{\"minute_unix\":120,"), "{j}");
        assert!(j.contains("\"stages\":[]"), "the last window had none: {j}");
        s.note_window(
            Health {
                unix: 151,
                ticks: 4,
                ..Health::default()
            },
            &[],
            Duration::ZERO,
            &[(
                "invented_stage",
                Duration::from_millis(4),
                Duration::from_millis(2),
            )],
        );
        assert!(
            ticks_json(&s).contains("{\"stage\":\"invented_stage\",\"max_ms\":4,\"mean_ms\":0.5}"),
            "{}",
            ticks_json(&s)
        );
    }

    #[test]
    fn system_loads_count_npcs_and_balls_for_hot_systems_only() {
        use crate::space_state::{Motion, Presence};
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        let who = w.config().characters[0].character_id;
        let system = w.session_system(who).expect("a system");
        let ship = w.character(who).map(|c| c.ship_id).expect("a ship");
        w.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        crate::npc::spawn(&mut w, &crate::npc::TYPES[0], system, [9_000.0, 0.0, 0.0])
            .expect("an npc");
        crate::liveness::refresh(&mut w, Instant::now(), 1_000 * 86_400 * 10_000_000);
        let loads = system_loads(&w);
        let here = loads
            .iter()
            .find(|l| l.system_id == system)
            .expect("the pilot's system");
        assert!(here.hot);
        assert_eq!(here.npcs, 1);
        assert!(here.in_space >= 2, "pilot and npc: {here:?}");
        assert!(here.balls.is_some_and(|n| n >= 1), "{here:?}");
        for l in loads.iter().filter(|l| !l.hot) {
            assert_eq!(l.balls, None, "{l:?}");
        }
        let json = {
            let s = Shared::new("t0k3n".repeat(8), &map());
            s.publish(Snapshot {
                systems: loads.clone(),
                ..Snapshot::default()
            });
            systems_json(&s)
        };
        assert!(json.contains("\"npcs\":1"), "{json}");
        assert!(json.contains("\"hot\":true"), "{json}");
    }

    #[test]
    fn metrics_text_escapes_method_labels() {
        use crate::live_status::prof::{MethodRow, MethodsWindow};
        let s = shared_with(vec![pilot(90_000_001, false, "space", 39_000_001)]);
        s.note_window(
            Health {
                unix: 5,
                ticks: 1,
                ..Health::default()
            },
            &[Duration::from_millis(2)],
            Duration::from_millis(1),
            &[(
                "invented_stage",
                Duration::from_millis(1),
                Duration::from_millis(1),
            )],
        );
        let w = MethodsWindow {
            unix: 5,
            secs: 10.0,
            rows: vec![MethodRow {
                label: "a\"b\\c\n".to_owned(),
                count: 3,
                hold: Duration::from_millis(9),
                max: Duration::from_millis(4),
                mode: "rw",
            }],
        };
        let text = render_metrics(&s, &w);
        assert!(text.contains("method=\"a\\\"b\\\\c\\n\""), "{text}");
        assert!(text.contains("eve_uptime_seconds "), "{text}");
        assert!(
            text.contains("eve_tick_work_seconds{quantile=\"0.95\"} 0.002000"),
            "{text}"
        );
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (name, value) = line.rsplit_once(' ').expect("name and value");
            let bare = name.split('{').next().unwrap_or("");
            assert!(
                !bare.is_empty() && bare.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{line}"
            );
            assert!(
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b".e+-".contains(&b)),
                "{line}"
            );
            assert_eq!(
                name.matches('{').count(),
                name.matches('}').count(),
                "{line}"
            );
        }
        // Reading changes nothing; the max is rolled by the report window.
        s.snap_cost_max_ns.store(5_000, Ordering::Relaxed);
        let before = render_metrics(&s, &w);
        assert_eq!(render_metrics(&s, &w), before, "a GET has no side effects");
        assert_eq!(s.snap_cost_max_ns.load(Ordering::Relaxed), 5_000);
        assert!(
            before.contains("eve_admin_snapshot_seconds{stat=\"max\"} 0.000000"),
            "the open window's max is not reported yet: {before}"
        );
        s.note_window(
            Health {
                unix: 6,
                ticks: 1,
                ..Health::default()
            },
            &[],
            Duration::ZERO,
            &[],
        );
        let after = render_metrics(&s, &w);
        assert!(
            after.contains("eve_admin_snapshot_seconds{stat=\"max\"} 0.000005"),
            "{after}"
        );
        assert_eq!(render_metrics(&s, &w), after, "still there on the next GET");
    }

    #[test]
    fn methods_json_is_a_pure_view() {
        use crate::live_status::prof::{MethodRow, MethodsWindow};
        assert_eq!(
            methods_json(&MethodsWindow::default()),
            "{\"window_unix\":null,\"secs\":0,\"methods\":[]}"
        );
        let w = MethodsWindow {
            unix: 9,
            secs: 10.0,
            rows: vec![MethodRow {
                label: "svcA.Call".to_owned(),
                count: 2,
                hold: Duration::from_millis(3),
                max: Duration::from_millis(2),
                mode: "ro?",
            }],
        };
        assert_eq!(
            methods_json(&w),
            "{\"window_unix\":9,\"secs\":10,\"methods\":[{\"label\":\"svcA.Call\",\"count\":2,\
             \"hold_ms\":3,\"max_ms\":2,\"mode\":\"ro?\"}]}"
        );
    }

    #[test]
    fn bearer_header_is_accepted_for_get_only() {
        let token = "t0k3n".repeat(8);
        let get =
            format!("GET /api/status HTTP/1.1\r\nHost: a\r\nauthorization: bearer {token}\r\n");
        let h = parse_head_full(&get).expect("head");
        assert_eq!(h.bearer.as_deref(), Some(token.as_str()));
        assert_eq!(h.token, None);
        let s = shared_with(Vec::new());
        assert_eq!(route(&s, &h.method, &h.target, get_token(&h)).status, 200);
        let post = format!(
            "POST /api/action/kick HTTP/1.1\r\nHost: a\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: 2\r\n"
        );
        let h = parse_head_full(&post).expect("head");
        assert_eq!(route_post(&s, &h, "{}", "t").status, 401);
        let other =
            parse_head_full("GET /x HTTP/1.1\r\nAuthorization: Basic abc\r\n").expect("head");
        assert_eq!(other.bearer, None);
    }
}
