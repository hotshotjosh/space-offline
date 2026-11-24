//! **The Proving Grounds arena** (issue #65, PG-2b): where a formed match is
//! played, how it is decided and how it ends.
//!
//! `queue::match_round` forms the match, pays the keys and tells the pilots,
//! then [`open`]s an arena (a room and a free void system) and [`launch`]es
//! the pilots into it with the plain drive jump. The stage [`tick`] tells a
//! pilot who landed, watches who is out, decides the match, places the conduit
//! and closes the arena. The wire arm is `service/proving.rs`
//! (`AbyssalPVPEndGateActivation`, [`end_gate`]).
//!
//! ## What the client does **[V]** (out/pyc, byte-identical in 3731714)
//!
//! - `OnAbyssalLocationEntered(client_data)` reads `content_id`,
//!   `nebula_graphic_id`, `difficulty_tier` and `location_id` and always
//!   starts the PvE objective chain (`abyssal_service.py:83-120`); we send
//!   the abyss's own push ([`crate::abyss::Entered`]).
//! - `OnAddExpiryDurationForAbyssalPvpContent(end_time)` sets the PvP timer;
//!   its start is `end_time` minus 15 minutes (`abyssal_service.py:140-163`).
//! - `OnAbyssalContentFinished(content_id, timer_type)` deletes the timer
//!   only for the active content id; the PvP timer type is 14
//!   (`crimewatchTimers.py:814-818`). The abyss's own `Finished` hard-codes 11,
//!   so [`PvpFinished`] is separate.
//! - The ring (type 55768) reads `startTime`, `endTime`, `kill` and
//!   `eventComplete` from its slim: `eventComplete == 1` is the end pose, else
//!   `kill == 1` is an explosion that returns before the countdown, else the
//!   two filetimes drive the countdown (`clientControllerRegistry.py`
//!   `UpdatePVPArena`); a slim change re-applies them (`spaceObject.py:1062`).
//! - The conduit (type 49563) is used through
//!   `pvpFilamentMgr.AbyssalPVPEndGateActivation(itemID)`; the answer is
//!   ignored (`movementFunctions.py:625`).
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **A match lasts 15 minutes and 18 seconds from forming**: the client's
//!   `DEFAULT_PVP_CONTENT_DURATION` plus its `_WITH_JUMP_DELAY` (18 s). The
//!   ring counts the last 15 minutes. Falsified by a client run whose timer
//!   disagrees with the ring.
//! - **A pilot is out** when their non-capsule ship dies in the arena
//!   system, their capsule dies there, or they are absent for 60 s outside a
//!   jump ([`ABSENT_S`]). A team is eliminated when all are out. Falsified by
//!   TQ material.
//! - **One standing team wins; at the clock's end two or more standing teams
//!   draw; if everyone fell, the teams that fell last together draw** (a
//!   mutual kill); eliminated teams lose. Falsified by TQ material.
//! - **Team `i` stands at entry locator `(2i) mod n`** of the room's entry
//!   locators (type 47463, ordered by object id); members 2 km apart along x;
//!   past `n/2` teams the spots are reused 5 km further along y per lap.
//!   Falsified by a client run where pilots land on each other or too near.
//! - **The room is any one-room archetype-51 dungeon with entry and exit
//!   locators and a ring or a fence, and no area-of-effect locator**, drawn
//!   by the world seed and the arena id; the void system is the highest free
//!   one (the Abyss takes the lowest). Falsified by an arena that looks wrong
//!   on the client.
//! - **A loser's capsule goes home 3 s after the ship dies**, never killed by
//!   the server (owner Q2). **At the clock's end everyone inside dies, ship
//!   then pod** (owner Q3), as the Abyss's collapse. Falsified by the owner
//!   changing either ruling.
//! - **No boundary and no stalemate** in this piece (PG-2d); the arena
//!   dbuff beacons are placed by the room's own objects and apply nothing.
//! - **No persistence**: a restart drops the arena; the origin written as
//!   the pilot's logoff spot returns their ship (owner Q1: the key stays
//!   spent).
//! - **The last player damage is not recorded** here (PG-2d).
//! - **The conduit stands at the room's lowest-id exit locator**; a winner
//!   who stays after the clock's end dies with everyone. Falsified by a
//!   client run where the conduit is out of reach.
//!
//! ## Wiring
//!
//! [`SLOT`] (`world_ext::SLOTS`), [`tick`] (`tick::STAGES`, before `site`),
//! [`on_kill`] (`kill_hooks::PLAYER`), [`slim_extras`] (`slim_extra::SOURCES`),
//! [`slash`] (`slash::COMMANDS`, `/pg`), [`in_arena`] (`logoff_spot::record`)
//! and [`systems_taken`] (`abyss::alloc_pocket`) are coordinator lines.

use crate::abyss;
use crate::damage::BlueTime;
use crate::dungeon_data::{Dungeon, Table};
use crate::kill_hooks::KillEvent;
use crate::notify::Push;
use crate::proving::Event;
use crate::proving::matchmaker::Team;
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::site::{self, Sites};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "proving_arena",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(ArenaState::default())
}

const FILETIME_PER_S: i64 = 10_000_000;

/// `typeEntryLocator` **[V]** (`appConst`).
pub const ENTRY_LOCATOR: i64 = 47463;
/// `typeAbyssPVPArenaStructure`: the ring at the centre of a ring room **[V]**.
pub const RING: i64 = 55768;
/// `typeAbyssPvPFence`: the fence at the centre of a fence room **[V]**.
pub const FENCE: i64 = 49562;
/// `TimerType.AbyssalPvpExpiration` **[V]** (`crimewatchTimers.py`).
pub const TIMER_PVP_EXPIRATION: i64 = 14;
/// `DEFAULT_PVP_CONTENT_DURATION`, 15 minutes **[V]** (`pvpFilaments/common/constants.py`).
pub const DURATION_S: i64 = 15 * 60;
/// The jump delay on top of it, 18 seconds **[V]** (`_WITH_JUMP_DELAY`).
pub const JUMP_DELAY_S: i64 = 18;
/// A pilot absent this long outside a jump is out **[A]**.
pub const ABSENT_S: i64 = 60;
/// A losing capsule leaves this long after its ship died **[A]**.
pub const POD_HOME_S: i64 = 3;
/// How long a pilot sent home may take to land before the arena forgets
/// them **[A]**.
const RETURN_GRACE_S: i64 = 120;
/// Where the nth team stands: entry locator `(2n) mod count` **[A]**.
const MEMBER_STEP: f64 = 2_000.0;
/// Team `i` stands at entry locator `(TEAM_STRIDE * i) mod n` **[A]**.
pub const TEAM_STRIDE: usize = 2;
const LAP_STEP: f64 = 5_000.0;

/// Why a pilot is out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    ShipLost,
    PodLost,
    Left,
}

/// One pilot of one team.
#[derive(Debug, Clone, PartialEq)]
pub struct Fighter {
    pub character: i64,
    pub origin_system: i64,
    pub origin_spot: [f64; 3],
    pub spot: [f64; 3],
    /// Told of the arrival (`OnAbyssalLocationEntered` and the timer).
    pub told: bool,
    pub out: Option<Out>,
    /// When the pilot went out (the arena's clock).
    pub out_at: BlueTime,
    /// When the capsule of a pilot whose ship died goes home.
    pub pod_home_at: Option<BlueTime>,
    /// When the pod of a pilot caught by the clock's end dies.
    pub pod_die_at: Option<BlueTime>,
    pub absent_since: Option<BlueTime>,
    /// The clock's end has been dealt with for this pilot.
    pub clocked: bool,
    /// The capsule that is to die at the clock's end (a pilot who boarded
    /// another hull meanwhile loses it first, as the Abyss's collapse does).
    pub pod_ship: i64,
    /// The session the pilot was told on; a new one is told again.
    pub session: Option<i64>,
}

/// How a match ended: a winning team, or the teams that drew.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Win(usize),
    Draw(Vec<usize>),
}

/// One match in play.
#[derive(Debug, Clone, PartialEq)]
pub struct Arena {
    pub id: u64,
    pub match_type_id: i64,
    pub schedule_id: i64,
    pub system: i64,
    pub site_id: i64,
    pub dungeon: i64,
    /// The room's entry locators, by object id, in room coordinates.
    pub entries: Vec<[f64; 3]>,
    pub exit_at: [f64; 3],
    /// The conduit's item id; 0 until the match is decided.
    pub conduit: i64,
    pub formed_at: BlueTime,
    pub ends_at: BlueTime,
    pub teams: Vec<Vec<Fighter>>,
    pub outcome: Option<Outcome>,
    /// The ring shows `kill = 1` (a ship died) until the tick after.
    pub kill_flash: bool,
    pub flash_at: BlueTime,
    /// Reported to [`ON_FINISHED`].
    pub reported: bool,
    /// The pilots are sent in.
    pub launched: bool,
    /// The clock has run out and everyone inside has been killed.
    pub expired: bool,
    /// `[A]` Not recorded in PG-2 (PG-2d): the last player damage.
    pub last_player_damage: BlueTime,
    /// The ring's `startTime`, fixed when the match forms: `/pg expire`
    /// moves only the end.
    pub starts_at: BlueTime,
    /// The room could not be made again, and that was said.
    pub site_warned: bool,
}

impl Arena {
    fn fighters(&self) -> impl Iterator<Item = &Fighter> {
        self.teams.iter().flatten()
    }

    fn fighter(&self, character: i64) -> Option<&Fighter> {
        self.fighters().find(|f| f.character == character)
    }

    /// The ring's `startTime`, when the countdown began.
    pub fn start_time(&self) -> BlueTime {
        self.starts_at
    }
}

/// Where a pilot sent home is going: the system, the spot, and since when.
type Home = (i64, [f64; 3], BlueTime);

/// The slot: the arenas, the next id, the pilots sent home who have not
/// landed (with where, and since when).
#[derive(Debug, Default)]
pub struct ArenaState {
    arenas: BTreeMap<u64, Arena>,
    next_id: u64,
    returning: BTreeMap<i64, Home>,
    /// The stage's clock at its last tick and the instant it was read: a gate
    /// is refused once the clock carried forward from there passes a match's
    /// end ([`now`]), so a test on a fake clock keeps its fake time.
    clock: BlueTime,
    clock_at: Option<Instant>,
}

/// The time on the stage's clock now: its last tick's time plus what has
/// passed since; the wall clock before any tick.
fn now(reg: &Registry) -> BlueTime {
    let s = reg.ext::<ArenaState>();
    match s.clock_at {
        Some(at) => s.clock + (crate::game_clock::clock::since(at).as_nanos() / 100) as BlueTime,
        None => crate::pyutil::filetime_now(),
    }
}

/// The arenas.
pub fn arenas(w: &Registry) -> impl Iterator<Item = &Arena> {
    w.ext::<ArenaState>().arenas.values()
}

/// The arena `character` plays in: with more than one (a loser who queued
/// again while the first arena still lists them), the one whose system they
/// stand in, then the one they are on the way to, then the newest.
pub fn arena_of(w: &Registry, character: i64) -> Option<&Arena> {
    arenas(w)
        .filter(|a| a.fighter(character).is_some())
        .max_by_key(|a| {
            let rank = if present(w, a, character) {
                2
            } else {
                u8::from(w.jumps().contains(character))
            };
            (rank, a.id)
        })
}

fn present(w: &Registry, a: &Arena, character: i64) -> bool {
    w.roster()
        .get(character)
        .is_some_and(|p| p.solar_system_id == a.system)
}

/// Whether `character` is in an arena: standing in its system, on the way in
/// or out, or sent home and not landed. `logoff_spot::record` keeps the origin
/// they came from while this holds **[A]**: the arena does not outlive the
/// server.
pub fn in_arena(w: &Registry, character: i64) -> bool {
    w.ext::<ArenaState>().returning.contains_key(&character)
        || arenas(w).any(|a| {
            a.fighter(character).is_some_and(|f| {
                present(w, a, character)
                    // On a jump, and still to land for the match: one who is out
                    // and told is on the way home, which `returning` holds.
                    || (w.jumps().contains(character) && (f.out.is_none() || !f.told))
            })
        })
}

/// The void systems the arenas hold (`abyss::alloc_pocket` leaves them alone).
pub(crate) fn systems_taken(w: &Registry) -> BTreeSet<i64> {
    arenas(w).map(|a| a.system).collect()
}

// ---------------------------------------------------------------------------
// Pure rules
// ---------------------------------------------------------------------------

/// A room an arena can be made in.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub dungeon: i64,
    pub entries: Vec<[f64; 3]>,
    pub exit: [f64; 3],
}

/// The arena room of a dungeon **[A]** (module doc), or `None`.
pub fn room_of(d: &Dungeon) -> Option<Room> {
    if d.archetype != abyss::ARCHETYPE || abyss::is_pve_room(d) {
        return None;
    }
    let rooms: BTreeSet<i64> = d.objects.iter().map(|o| o.room).collect();
    if rooms.len() != 1 {
        return None;
    }
    d.objects
        .iter()
        .any(|o| o.type_id == RING || o.type_id == FENCE)
        .then_some(())?;
    let mut entries: Vec<_> = d
        .objects
        .iter()
        .filter(|o| o.type_id == ENTRY_LOCATOR)
        .collect();
    entries.sort_by_key(|o| o.id);
    if entries.is_empty() {
        return None;
    }
    let exit = d
        .objects
        .iter()
        .filter(|o| o.type_id == abyss::EXIT_LOCATOR)
        .min_by_key(|o| o.id)?;
    Some(Room {
        dungeon: d.id,
        entries: entries.iter().map(|o| o.pos).collect(),
        exit: exit.pos,
    })
}

/// Where member `member` of team `team` lands **[A]** (module doc).
pub fn spot_for(entries: &[[f64; 3]], team: usize, member: usize) -> [f64; 3] {
    let n = entries.len().max(1);
    let at = entries
        .get((team * TEAM_STRIDE) % n)
        .copied()
        .unwrap_or([0.0; 3]);
    let lap = (team * TEAM_STRIDE / n) as f64;
    [
        at[0] + MEMBER_STEP * member as f64,
        at[1] + LAP_STEP * lap,
        at[2],
    ]
}

/// The teams still standing: at least one pilot not out.
fn standing(a: &Arena) -> Vec<usize> {
    (0..a.teams.len())
        .filter(|&i| a.teams[i].iter().any(|f| f.out.is_none()))
        .collect()
}

/// The match's outcome now, or `None` while it goes on **[A]** (module doc):
/// one standing team wins; with none, the teams that fell last together
/// draw; with two or more, they draw at the clock's end.
pub fn decide(a: &Arena, clock_over: bool) -> Option<Outcome> {
    let alive = standing(a);
    match alive.len() {
        1 => Some(Outcome::Win(alive[0])),
        0 => {
            let fell = |i: usize| a.teams[i].iter().map(|f| f.out_at).max().unwrap_or(0);
            let last = (0..a.teams.len()).map(fell).max()?;
            Some(Outcome::Draw(
                (0..a.teams.len()).filter(|&i| fell(i) == last).collect(),
            ))
        }
        _ if clock_over => Some(Outcome::Draw(alive)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnAddExpiryDurationForAbyssalPvpContent(end_time)` **[V]**: the PvP timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PvpExpiry {
    pub to: i64,
    pub end_time: BlueTime,
}

impl Push for PvpExpiry {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAddExpiryDurationForAbyssalPvpContent",
            vec![PyValue::Int(self.end_time)],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnAbyssalContentFinished(content_id, 14)` **[V]**: the PvP timer ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PvpFinished {
    pub to: i64,
    pub content_id: i64,
}

impl Push for PvpFinished {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAbyssalContentFinished",
            vec![
                PyValue::Int(self.content_id),
                PyValue::Int(TIMER_PVP_EXPIRATION),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The hand-off to the ranking (PG-3)
// ---------------------------------------------------------------------------

/// `MatchResult` **[V]** (`provinggrounds_pb2`): loss 0, win 1, draw 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    Loss = 0,
    Win = 1,
    Draw = 2,
}

/// A match that ended: each team's characters and its result.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchFinished {
    pub match_type_id: i64,
    pub schedule_id: i64,
    pub results: Vec<(Vec<i64>, MatchResult)>,
}

/// Every consumer of a finished match, fired once per match. PG-3 adds the
/// leaderboard and the statistics here (coordinator).
pub const ON_FINISHED: &[fn(&mut Registry, &MatchFinished)] = &[log_only];

#[cfg(test)]
thread_local! {
    /// The matches `log_only` was handed, for the tests.
    pub(super) static FIRED: std::cell::RefCell<Vec<MatchFinished>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn log_only(_: &mut Registry, m: &MatchFinished) {
    #[cfg(test)]
    FIRED.with(|f| f.borrow_mut().push(m.clone()));
    let of = |r: MatchResult| -> Vec<&Vec<i64>> {
        m.results
            .iter()
            .filter(|(_, x)| *x == r)
            .map(|(c, _)| c)
            .collect()
    };
    let (w, d) = (of(MatchResult::Win), of(MatchResult::Draw));
    if let Some(w) = w.first() {
        crate::outln!(
            "[proving] match {} (schedule {}): win {w:?}, loss {:?}",
            m.match_type_id,
            m.schedule_id,
            of(MatchResult::Loss)
        );
    } else {
        crate::outln!(
            "[proving] match {} (schedule {}): draw {d:?}, loss {:?}",
            m.match_type_id,
            m.schedule_id,
            of(MatchResult::Loss)
        );
    }
}

/// What a decided arena reports: each team's characters and result.
pub fn finished(a: &Arena) -> Option<MatchFinished> {
    let o = a.outcome.as_ref()?;
    let results = (0..a.teams.len())
        .map(|i| {
            let result = match o {
                Outcome::Win(w) if *w == i => MatchResult::Win,
                Outcome::Draw(d) if d.contains(&i) => MatchResult::Draw,
                _ => MatchResult::Loss,
            };
            (a.teams[i].iter().map(|f| f.character).collect(), result)
        })
        .collect();
    Some(MatchFinished {
        match_type_id: a.match_type_id,
        schedule_id: a.schedule_id,
        results,
    })
}

fn report(reg: &mut Registry, a: &Arena) {
    let Some(m) = finished(a) else { return };
    for hook in ON_FINISHED {
        hook(reg, &m);
    }
}

// ---------------------------------------------------------------------------
// The world side: opening and launching
// ---------------------------------------------------------------------------

fn get(reg: &Registry, id: u64) -> Option<Arena> {
    reg.ext::<ArenaState>().arenas.get(&id).cloned()
}

fn put(reg: &mut Registry, a: Arena) {
    reg.ext_mut::<ArenaState>().arenas.insert(a.id, a);
}

/// The void systems nobody may be put in: the Abyss's pockets, the arenas,
/// and any with a wreck or an abandoned ship in it (its leavings stay where
/// they fell).
fn taken(reg: &Registry) -> BTreeSet<i64> {
    let mut t = abyss::systems_taken(reg);
    t.extend(systems_taken(reg));
    t.extend(reg.combat().wrecks().map(|w| w.solar_system_id));
    t.extend(
        reg.ext::<crate::pilotless::State>()
            .iter()
            .map(|a| a.system),
    );
    t
}

/// The highest free void system: the Abyss takes the lowest.
fn free_system(reg: &Registry) -> Option<i64> {
    let taken = taken(reg);
    reg.content()
        .try_get::<crate::celestial::Universe>()?
        .systems()
        .map(|s| s.id)
        .filter(|id| crate::celestial::VOID_SYSTEMS.contains(id) && !taken.contains(id))
        .max()
}

/// **Open an arena** for the formed `teams` of `ev`: a room, a free void
/// system and a site id from the Abyss's band (so `site::tick` leaves it
/// alone). `Err` is for the log; nothing has changed then but the id
/// counters. The pilots are not moved until [`launch`].
pub fn open(reg: &mut Registry, ev: &Event, teams: &[Team], ft: BlueTime) -> Result<u64, String> {
    let id = {
        let s = reg.ext_mut::<ArenaState>();
        s.next_id += 1;
        s.next_id
    };
    let seed = crate::seed::mix(reg.world().seed, &[id]);
    let rooms: Vec<Room> = reg
        .content()
        .try_get::<Table>()
        .map(|t| t.dungeons.values().filter_map(room_of).collect())
        .unwrap_or_default();
    if rooms.is_empty() {
        return Err("the bundle has no arena room (dungeons, archetype 51)".to_owned());
    }
    let room = rooms[crate::seed::Stream::new(seed).range(0, rooms.len() as u64) as usize].clone();
    let Some(system) = free_system(reg) else {
        return Err("no free arena system".to_owned());
    };
    let site_id = abyss::next_site(reg);
    if !site::instantiate(reg, &abyss::site_of(system, site_id, room.dungeon, ft)) {
        return Err("the arena room could not be placed".to_owned());
    }
    let fighter = |team: usize, member: usize, character: i64| Fighter {
        character,
        origin_system: 0,
        origin_spot: [0.0; 3],
        spot: spot_for(&room.entries, team, member),
        told: false,
        out: None,
        out_at: 0,
        pod_home_at: None,
        pod_die_at: None,
        absent_since: None,
        clocked: false,
        pod_ship: 0,
        session: None,
    };
    let fighters = teams
        .iter()
        .enumerate()
        .map(|(i, t)| {
            t.members
                .iter()
                .enumerate()
                .map(|(j, e)| fighter(i, j, e.char_id))
                .collect()
        })
        .collect();
    put(
        reg,
        Arena {
            id,
            match_type_id: ev.match_type.id,
            schedule_id: ev.schedule.id,
            system,
            site_id,
            dungeon: room.dungeon,
            entries: room.entries,
            exit_at: room.exit,
            conduit: 0,
            formed_at: ft,
            ends_at: ft + (DURATION_S + JUMP_DELAY_S) * FILETIME_PER_S,
            teams: fighters,
            outcome: None,
            kill_flash: false,
            flash_at: 0,
            reported: false,
            launched: false,
            expired: false,
            last_player_damage: 0,
            starts_at: ft + JUMP_DELAY_S * FILETIME_PER_S,
            site_warned: false,
        },
    );
    crate::outln!(
        "[proving] arena {id}: system {system}, room {}",
        room.dungeon
    );
    Ok(id)
}

/// **Send the pilots in**: each one's origin is written as their logoff spot
/// (a restart or a log-off puts the ship back), then the plain drive jump to
/// their spot.
pub fn launch(reg: &mut Registry, id: u64, ft: BlueTime) {
    let Some(mut a) = get(reg, id) else { return };
    let system = a.system;
    for f in a.teams.iter_mut().flatten() {
        reg.advance_character(f.character);
        let Some(here) = reg.roster().get(f.character).copied() else {
            crate::errln!(
                "[proving] arena {id}: {} is not in space to jump",
                f.character
            );
            f.out = Some(Out::Left);
            f.out_at = ft;
            continue;
        };
        f.origin_system = here.solar_system_id;
        f.origin_spot = here.motion.position;
        crate::logoff_spot::set(
            reg,
            f.character,
            crate::logoff_spot::Spot {
                ship_id: here.ship_id,
                system: here.solar_system_id,
                position: here.motion.position,
            },
        );
        crate::jump_drive::depart(
            reg,
            f.character,
            here.ship_id,
            here.solar_system_id,
            system,
            f.spot,
        );
    }
    a.launched = true;
    put(reg, a);
}

// ---------------------------------------------------------------------------
// The world side: the ring, the conduit
// ---------------------------------------------------------------------------

/// The ring's item id in the arena's room, if the room has one.
fn ring_item(reg: &Registry, a: &Arena) -> Option<i64> {
    reg.ext::<Sites>()
        .get(a.site_id)?
        .objects
        .iter()
        .find(|o| o.type_id == RING)
        .map(|o| o.item_id)
}

fn push_ring(reg: &mut Registry, a: &Arena) {
    if let Some(item) = ring_item(reg, a) {
        crate::fw_capture::push_slim(reg, item);
    }
}

/// The S9 source (`slim_extra::SOURCES`): the ring's `startTime`, `endTime`,
/// `kill` and `eventComplete` **[V]** (module doc).
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if reg.ext::<ArenaState>().arenas.is_empty() {
        return Vec::new();
    }
    arenas(reg)
        .filter_map(|a| {
            let item = ring_item(reg, a)?;
            ids.contains(&item).then(|| {
                (
                    item,
                    vec![
                        ("startTime", PyValue::Int(a.start_time())),
                        ("endTime", PyValue::Int(a.ends_at)),
                        ("kill", PyValue::Int(i64::from(a.kill_flash))),
                        (
                            "eventComplete",
                            PyValue::Int(i64::from(a.outcome.is_some())),
                        ),
                    ],
                )
            })
        })
        .collect()
}

/// Make sure the room is in space, and the conduit if the match is decided.
/// `site::tick` vacates an instance whose system has gone cold, so this is
/// also how a room comes back. `false` when it could not be made.
fn ensure_site(reg: &mut Registry, a: &mut Arena, ft: BlueTime) -> bool {
    if reg.ext::<Sites>().get(a.site_id).is_none() {
        if !site::instantiate(reg, &abyss::site_of(a.system, a.site_id, a.dungeon, ft)) {
            return false;
        }
        a.conduit = 0;
    }
    if a.outcome.is_some()
        && !a.expired
        && (a.conduit == 0 || site::object(reg, a.conduit).is_none())
    {
        match site::add_object(reg, a.site_id, abyss::PVP_GATE, a.exit_at) {
            Some(g) => a.conduit = g.item_id,
            None => return false,
        }
    }
    true
}

// ---------------------------------------------------------------------------
// The world side: events
// ---------------------------------------------------------------------------

/// `kill_hooks::PLAYER` row (coordinator): a fighter's ship or capsule died
/// in the arena's system **[A]**. A ship's death sends the capsule home in
/// [`POD_HOME_S`] (unless the clock has run out) and makes the ring flash.
pub fn on_kill(reg: &mut Registry, ev: &KillEvent) {
    if ev.npc.is_some() || reg.ext::<ArenaState>().arenas.is_empty() {
        return;
    }
    let Some(mut a) = arenas(reg)
        .find(|a| a.system == ev.system && a.fighter(ev.victim_character).is_some())
        .cloned()
    else {
        return;
    };
    let is_pod = reg
        .store()
        .get(ev.victim_ship)
        .is_some_and(|i| i.group_id == crate::fitting::GROUP_CAPSULE);
    let expired = a.expired;
    let Some(f) = a
        .teams
        .iter_mut()
        .flatten()
        .find(|f| f.character == ev.victim_character)
    else {
        return;
    };
    if is_pod {
        if f.out.is_none() {
            f.out_at = ev.when;
        }
        f.out = Some(Out::PodLost);
        f.pod_home_at = None;
        f.pod_die_at = None;
    } else {
        if f.out.is_none() {
            f.out_at = ev.when;
            f.out = Some(Out::ShipLost);
        }
        if !expired {
            f.pod_home_at = Some(ev.when + POD_HOME_S * FILETIME_PER_S);
        }
        a.kill_flash = true;
        a.flash_at = ev.when;
    }
    push_ring_after(reg, a);
}

fn push_ring_after(reg: &mut Registry, a: Arena) {
    put(reg, a.clone());
    push_ring(reg, &a);
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

fn sessions_gone(reg: &Registry, character: i64) -> bool {
    reg.sessions().of_character(character).is_empty()
}

/// `tick::STAGES` row (coordinator, before `site`): tell the pilots who
/// landed, drop the absent, send the capsules home, decide, end the clock
/// and close the arenas that are done.
pub fn tick(reg: &mut Registry, at: Instant, ft: BlueTime) {
    let s = reg.ext_mut::<ArenaState>();
    s.clock = ft;
    s.clock_at = Some(at);
    if !reg.ext::<ArenaState>().returning.is_empty() {
        land_returning(reg, ft);
    }
    let ids: Vec<u64> = reg.ext::<ArenaState>().arenas.keys().copied().collect();
    for id in ids {
        tick_arena(reg, id, ft);
    }
}

/// Send a pilot home: the PvP timer ends on their client first, then the jump.
fn depart_home(
    reg: &mut Registry,
    f: &Fighter,
    ship_id: i64,
    from: i64,
    content_id: i64,
    ft: BlueTime,
) {
    reg.push(PvpFinished {
        to: f.character,
        content_id,
    });
    crate::jump_drive::depart(
        reg,
        f.character,
        ship_id,
        from,
        f.origin_system,
        f.origin_spot,
    );
    reg.ext_mut::<ArenaState>()
        .returning
        .insert(f.character, (f.origin_system, f.origin_spot, ft));
}

/// Send home the pilots who were sent and have not landed, again if their
/// jump left them in the void, and clear their origin row once they have.
fn land_returning(reg: &mut Registry, ft: BlueTime) {
    let ids: Vec<(i64, Home)> = reg
        .ext::<ArenaState>()
        .returning
        .iter()
        .map(|(c, v)| (*c, *v))
        .collect();
    for (c, (system, spot, since)) in ids {
        if reg.jumps().contains(c) {
            continue;
        }
        let gave_up = ft - since >= RETURN_GRACE_S * FILETIME_PER_S;
        match reg.roster().get(c).copied() {
            Some(p) if crate::celestial::VOID_SYSTEMS.contains(&p.solar_system_id) => {
                if !gave_up {
                    crate::jump_drive::depart(reg, c, p.ship_id, p.solar_system_id, system, spot);
                    continue;
                }
                // The Abyss's rescue needs the origin row to name this very
                // ship, which a capsule's never does: say so, keep the row (the
                // next login restores the origin) and let go.
                crate::errln!(
                    "[proving] {c}: still in void system {} after {RETURN_GRACE_S} s; the origin row is kept",
                    p.solar_system_id
                );
            }
            // Landed: the row has done its work.
            Some(_) => crate::logoff_spot::clear(reg, c),
            None if !gave_up && !sessions_gone(reg, c) => continue,
            // Given up with a session: they are docked or gone, nothing to return to.
            None if !sessions_gone(reg, c) => crate::logoff_spot::clear(reg, c),
            None => {}
        }
        reg.ext_mut::<ArenaState>().returning.remove(&c);
    }
}

fn tick_arena(reg: &mut Registry, id: u64, ft: BlueTime) {
    let Some(mut a) = get(reg, id) else { return };
    if !a.launched {
        return;
    }
    let here: Vec<bool> = a
        .fighters()
        .map(|f| present(reg, &a, f.character))
        .collect();
    if here.iter().any(|h| *h) {
        if ensure_site(reg, &mut a, ft) {
            a.site_warned = false;
        } else if !a.site_warned {
            a.site_warned = true;
            crate::errln!("[proving] arena {id}: the room could not be made again");
        }
    }
    let sids: Vec<Option<i64>> = a
        .fighters()
        .map(|f| {
            reg.sessions()
                .of_character(f.character)
                .first()
                .map(|s| s.session_id)
        })
        .collect();
    let mut ring = false;
    // The news: a pilot who has landed is told once.
    let (cid, ends_at) = (a.site_id, a.ends_at);
    for (k, f) in a.teams.iter_mut().flatten().enumerate() {
        // A new session in the arena's system knows nothing: told again.
        if here[k] && f.told && sids[k].is_some() && f.session.is_some() && sids[k] != f.session {
            f.told = false;
        }
        if here[k] && !f.told {
            f.told = true;
            f.session = sids[k];
            reg.push(abyss::Entered {
                to: f.character,
                content_id: cid,
                nebula_graphic_id: abyss::nebula(0),
                tier: 0,
                room: 1,
            });
            reg.push(PvpExpiry {
                to: f.character,
                end_time: ends_at,
            });
            // Absent for a minute and out, and only now landed: the timer ends.
            if f.out == Some(Out::Left) {
                reg.push(PvpFinished {
                    to: f.character,
                    content_id: cid,
                });
            }
        }
        // Absent: not in the arena's system and not in a jump.
        if f.out.is_none() {
            if here[k] || reg.jumps().contains(f.character) {
                f.absent_since = None;
            } else {
                let since = *f.absent_since.get_or_insert(ft);
                if ft - since >= ABSENT_S * FILETIME_PER_S {
                    f.out = Some(Out::Left);
                    f.out_at = ft;
                }
            }
        }
    }
    if a.kill_flash && ft > a.flash_at {
        a.kill_flash = false;
        ring = true;
    }
    // The capsules of the ships that died go home.
    let due: Vec<(usize, usize)> = a
        .teams
        .iter()
        .enumerate()
        .flat_map(|(i, t)| t.iter().enumerate().map(move |(j, f)| (i, j, f)))
        .filter(|(_, _, f)| f.pod_home_at.is_some_and(|at| ft >= at))
        .map(|(i, j, _)| (i, j))
        .collect();
    for (i, j) in due {
        let f = a.teams[i][j].clone();
        a.teams[i][j].pod_home_at = None;
        if reg.jumps().contains(f.character) {
            continue;
        }
        if let Some(p) = reg
            .roster()
            .get(f.character)
            .copied()
            .filter(|p| p.solar_system_id == a.system)
        {
            depart_home(reg, &f, p.ship_id, p.solar_system_id, a.site_id, ft);
        }
    }
    // The clock.
    if !a.expired && ft >= a.ends_at {
        a.expired = true;
    }
    // Once out of time, anyone found inside who has not been dealt with (one
    // who landed late) is killed at once.
    let kill_all = a.expired
        && a.fighters()
            .any(|f| !f.clocked && present(reg, &a, f.character));
    // The decision.
    if a.outcome.is_none()
        && let Some(o) = decide(&a, a.expired)
    {
        a.outcome = Some(o);
        ring = true;
        if !a.expired && !ensure_site(reg, &mut a, ft) {
            crate::errln!("[proving] arena {id}: the conduit could not be placed");
        }
    }
    if a.outcome.is_some() && !a.reported {
        a.reported = true;
        report(reg, &a);
    }
    // A pilot who is out, has left and has landed is no longer the arena's,
    // once the match is decided (the result is kept above).
    if a.outcome.is_some() {
        let home: BTreeSet<i64> = reg.ext::<ArenaState>().returning.keys().copied().collect();
        let system = a.system;
        for team in &mut a.teams {
            team.retain(|f| {
                f.out.is_none()
                    || f.pod_home_at.is_some()
                    || f.pod_die_at.is_some()
                    || home.contains(&f.character)
                    || reg.jumps().contains(f.character)
                    || reg
                        .roster()
                        .get(f.character)
                        .is_some_and(|p| p.solar_system_id == system)
            });
        }
    }
    put(reg, a.clone());
    if ring {
        push_ring(reg, &a);
    }
    if kill_all {
        clock_out(reg, id, ft);
    }
    pods_die(reg, id, ft);
    close_if_done(reg, id, ft);
}

/// **The clock's end**: everyone inside gets the timer's end and dies, ship
/// then pod, as the Abyss's collapse does (`abyss::kill_pilot`; owner Q3). The
/// pilot is the cause **[A]** (the same kill as a self-destruct). The pod of a
/// pilot who is watching dies after a moment, and until then the pilot's
/// logoff row names the pod in this void system, so a restart sweeps it.
fn clock_out(reg: &mut Registry, id: u64, ft: BlueTime) {
    let Some(mut a) = get(reg, id) else { return };
    let system = a.system;
    let victims: Vec<i64> = a
        .teams
        .iter_mut()
        .flatten()
        .filter(|f| {
            !f.clocked
                && reg
                    .roster()
                    .get(f.character)
                    .is_some_and(|p| p.solar_system_id == system)
        })
        .map(|f| {
            f.clocked = true;
            f.pod_home_at = None;
            f.character
        })
        .collect();
    let cid = a.site_id;
    put(reg, a);
    for c in &victims {
        reg.push(PvpFinished {
            to: *c,
            content_id: cid,
        });
        crate::outln!("[proving] arena {id}: {c}: the clock has run out");
        crate::combat::self_destruct_kill(reg, *c, ft);
    }
    for c in victims {
        let Some(p) = reg.roster().get(c).copied() else {
            crate::logoff_spot::clear(reg, c);
            continue;
        };
        if sessions_gone(reg, c) {
            // Nobody is there to see it, and a delay would let the log-off
            // hold take the pod out of space alive: it dies now.
            crate::combat::self_destruct_kill(reg, c, ft);
            crate::logoff_spot::clear(reg, c);
            continue;
        }
        let pod = reg.character(c).map_or(0, |ch| ch.ship_id);
        if let Some(mut a) = get(reg, id) {
            if let Some(f) = a.teams.iter_mut().flatten().find(|f| f.character == c) {
                f.pod_die_at = Some(ft + POD_HOME_S * FILETIME_PER_S);
                f.pod_ship = pod;
            }
            put(reg, a);
        }
        crate::logoff_spot::set(
            reg,
            c,
            crate::logoff_spot::Spot {
                ship_id: pod,
                system: p.solar_system_id,
                position: p.motion.position,
            },
        );
    }
}

/// The pods whose time has come die. A pilot who flies something else by then
/// loses that hull first, then the pod; one who has left the system is let
/// off. The origin row goes either way (`abyss::kill_due_pods`).
fn pods_die(reg: &mut Registry, id: u64, ft: BlueTime) {
    let Some(mut a) = get(reg, id) else { return };
    let due: Vec<(i64, i64)> = a
        .fighters()
        .filter(|f| f.pod_die_at.is_some_and(|at| ft >= at))
        .map(|f| (f.character, f.pod_ship))
        .collect();
    if due.is_empty() {
        return;
    }
    for f in a.teams.iter_mut().flatten() {
        if due.iter().any(|(c, _)| *c == f.character) {
            f.pod_die_at = None;
        }
    }
    let system = a.system;
    put(reg, a);
    for (c, pod) in due {
        let here = reg.roster().get(c).copied();
        if here.is_some_and(|p| p.solar_system_id == system) {
            let other_hull = reg.character(c).is_some_and(|ch| ch.ship_id != pod);
            crate::combat::self_destruct_kill(reg, c, ft);
            if other_hull && reg.roster().get(c).is_some() {
                crate::combat::self_destruct_kill(reg, c, ft);
            }
        }
        crate::logoff_spot::clear(reg, c);
    }
}

/// Take the arena out of space once it is over and empty: the room leaves,
/// and the wrecks with it (a closed arena's system is not reused, so its
/// leavings go) **[A]**.
fn close_if_done(reg: &mut Registry, id: u64, ft: BlueTime) {
    let Some(a) = get(reg, id) else { return };
    let over = a.outcome.is_some() || a.expired || ft >= a.ends_at;
    let anyone = a.fighters().any(|f| {
        present(reg, &a, f.character)
            || (f.out.is_none() && !f.told && reg.jumps().contains(f.character))
    });
    if !over || anyone {
        return;
    }
    let wrecks: Vec<i64> = reg
        .combat()
        .wrecks()
        .filter(|w| w.solar_system_id == a.system)
        .map(|w| w.wreck_id)
        .collect();
    crate::outln!(
        "[proving] arena {id} closed: {} wreck(s) removed with it",
        wrecks.len()
    );
    for w in wrecks {
        crate::combat::remove_wreck(reg, w);
    }
    site::vacate(reg, a.site_id);
    reg.ext_mut::<ArenaState>().arenas.remove(&id);
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

fn notify(text: &str) -> ServiceError {
    crate::modules::user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// What an end-gate activation is refused for, if anything: the text for the
/// pilot **[A]** (the wording is ours).
pub fn gate_refusal(
    a: Option<&Arena>,
    actor: i64,
    item: i64,
    ft: BlueTime,
) -> Option<&'static str> {
    let Some(a) = a else {
        return Some("You are not in a Proving Grounds arena.");
    };
    if a.outcome.is_none() {
        return Some("The match is not decided yet.");
    }
    if a.conduit == 0 || item != a.conduit {
        return Some("That is not the conduit of your arena.");
    }
    if ft >= a.ends_at {
        return Some("The arena is collapsing.");
    }
    a.fighter(actor)
        .is_none()
        .then_some("You are not in a Proving Grounds arena.")
}

/// `pvpFilamentMgr.AbyssalPVPEndGateActivation(itemID)`: the conduit sends its
/// pilot home. The client ignores the answer **[V]**.
pub fn end_gate(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(item) = Args::new(req).int(0) else {
        return Err(notify("AbyssalPVPEndGateActivation needs (itemID,)."));
    };
    let ft = now(reg);
    reg.advance_character(actor);
    let a = arena_of(reg, actor).cloned();
    if let Some(why) = gate_refusal(a.as_ref(), actor, item, ft) {
        return Err(notify(why));
    }
    let Some(mut a) = a else {
        return Ok(PyValue::None);
    };
    let Some(p) = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.solar_system_id == a.system)
        .filter(|_| !reg.jumps().contains(actor))
    else {
        return Err(notify("Your ship is not in the arena."));
    };
    let cid = a.site_id;
    let Some(f) = a.teams.iter_mut().flatten().find(|f| f.character == actor) else {
        return Ok(PyValue::None);
    };
    if f.out.is_none() {
        f.out = Some(Out::Left);
        f.out_at = ft;
    }
    let f = f.clone();
    put(reg, a);
    depart_home(reg, &f, p.ship_id, p.solar_system_id, cid, ft);
    crate::outln!("[proving] {actor}: home from arena (content {cid})");
    Ok(PyValue::None)
}

/// `/pg status | expire <seconds> | decide`: a GM door to the match for the
/// scenarios and testers.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |t: &str| Ok(crate::pyutil::bstr(t));
    let usage = "usage: /pg status | expire <seconds> | decide";
    let Some(actor) = actor else {
        return say("no character");
    };
    let Some(id) = arena_of(reg, actor).map(|a| a.id) else {
        return say("you are not in an arena");
    };
    let ft = now(reg);
    match args {
        ["status"] => {
            let a = arena_of(reg, actor).cloned().expect("looked up above");
            let left = (a.ends_at - ft) / FILETIME_PER_S;
            let standing = standing(&a).len();
            say(&format!(
                "arena {} system {} room {}: {} team(s), {standing} standing, {left} s left, {}, conduit {}",
                a.id,
                a.system,
                a.dungeon,
                a.teams.len(),
                match &a.outcome {
                    None => "undecided".to_owned(),
                    Some(Outcome::Win(i)) => format!("team {i} won"),
                    Some(Outcome::Draw(d)) => format!("teams {d:?} drew"),
                },
                a.conduit
            ))
        }
        ["expire", secs] => match secs
            .parse::<i64>()
            .ok()
            .filter(|s| (0..=86_400).contains(s))
        {
            None => say(usage),
            Some(s) => {
                let Some(mut a) = get(reg, id) else {
                    return say(usage);
                };
                a.ends_at = ft + s * FILETIME_PER_S;
                let to: Vec<i64> = a.fighters().map(|f| f.character).collect();
                let end_time = a.ends_at;
                put(reg, a.clone());
                for to in to {
                    reg.push(PvpExpiry { to, end_time });
                }
                push_ring(reg, &a);
                say(&format!("the arena collapses in {s} s"))
            }
        },
        ["decide"] => {
            let Some(mut a) = get(reg, id) else {
                return say(usage);
            };
            match decide(&a, true) {
                None => say("nobody to decide"),
                Some(o) => {
                    if a.outcome.is_none() {
                        a.outcome = Some(o);
                        if !ensure_site(reg, &mut a, ft) {
                            crate::errln!("[proving] arena {id}: the conduit could not be placed");
                        }
                        if !a.reported {
                            a.reported = true;
                            report(reg, &a);
                        }
                        put(reg, a.clone());
                        push_ring(reg, &a);
                    }
                    say("decided")
                }
            }
        }
        _ => say(usage),
    }
}

#[cfg(test)]
mod tests;
