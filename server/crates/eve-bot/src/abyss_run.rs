//! **The abyss runners** (BOT-ABYSS, issue #238, `parity-abyss-depth.md`
//! AD-7): bots that run the Abyss as a gang, under "nothing from nowhere".
//!
//! The server side is whole (`eve_server::abyss`: three modes, the entrance
//! gate, three rooms, the clock, the loot). This module is the pilot's half.
//!
//! # The role ([`ROLE`], career `abyss_runner`)
//!
//! A site runner ([`crate::site_runner`]) whose hull is past the rookie
//! frigate and who holds an abyssal filament:
//!
//! - **Docked** the runner offers itself on the [`Board`] (who is here, in
//!   what hull, holding how many keys). The **leader** is the lowest
//!   character id that holds a key; it reads the others' offers, picks the
//!   biggest gang its keys pay for ([`plan`]: three list-92 hulls and three
//!   keys, else two list-233 hulls and two, else one list-57 hull and one),
//!   puts the key in its hold, forms a fleet and invites the mates
//!   ([`Op::Gather`]). The mates hold their undock while a key-holder is
//!   docked beside them ([`MATE_PATIENCE`]) and accept the invite as any
//!   fleet member does.
//! - **In space** the leader deploys ([`Op::Deploy`]): at a belt or a planet
//!   of the system, the next one when the server refuses the spot for its
//!   neighbours ([`next_spot`], [`read_refusal`]). A solo run jumps at once;
//!   a gang's entrance gate opens, each mate takes it ([`Op::Enter`]) and
//!   the leader last.
//! - **Inside** every pilot plays the site runner's fight, room by room: the
//!   NPCs, then the cache and nodes, then the wrecks looted, then the room's
//!   gate ([`Op::Gate`], the encounter gate, or the exit gate in the last
//!   room). The room is shared, so every pilot sees it clear together.
//! - **The clock** ([`pace`]): the run's end comes from
//!   `OnAddExpiryDurationForAbyssalContent`. With less than [`ROOM_RESERVE`]
//!   a room ahead, the runner leaves the cache and the nodes alone and never
//!   stops shooting the targets (the gates are the only way out).
//! - **After** it docks and sells the abyssal goods ([`Op::Sell`]); a gang
//!   that died in a pocket drops its tier cap for the next run
//!   ([`State::losses`], [`lowered`]).
//!
//! *Nothing from nowhere*: a key is one the bot holds (a cache's, a rat's or
//! the market's); nothing here makes one, and a lost ship is lost.
//!
//! Every number below is **[A] policy** with its falsifier next to it; no
//! test asserts one.

use crate::brain::{self, Act, ActKind, GIVE_UP_AFTER, Memory, Observation, Place};
use crate::clock;
use crate::hull_choice::{Class, HULLS};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::site_runner;
use crate::{Conn, Error, Event, Result};
use eve_marshal::PyValue;
use eve_server::abyss::weather::WEATHERS;
use eve_server::abyss::{self, Mode, err};
use eve_server::celestial::VOID_SYSTEMS;
use eve_server::dungeon_data::TypeList;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

type BlueTime = i64;

// ---- the numbers ------------------------------------------------------------

/// **[A]** The time a room ahead needs: with less than this per room still to
/// come, the cache and nodes are skipped. Falsified by gangs that collapse
/// with rooms unvisited (raise), or that leave loot with half the clock
/// unspent (lower).
pub const ROOM_RESERVE: Duration = Duration::from_secs(4 * 60);
/// **[A]** Spots (belts, planets) tried for one deploy. Falsified by deploys
/// refused for proximity at every spot of a system (raise) or a runner that
/// wanders a system for minutes (lower).
pub const MAX_SPOTS: usize = 6;
/// **[A]** How long the leader waits for its mates to join the fleet.
/// Falsified by mates that join after the leader has left (raise).
pub const GATHER_WAIT: Duration = Duration::from_secs(45);
/// **[A]** How long the leader waits at the open entrance for its mates
/// before it goes in. Falsified by mates who arrive after the leader is
/// in (raise) or leaders idle at a gate with nobody coming (lower).
pub const ENTRANCE_WAIT: Duration = Duration::from_secs(180);
/// **[A]** How long a docked key-holder waits for mates to offer, and how long
/// a mate waits to be invited, before each goes about its day. Falsified by
/// gangs that never form because one side gave up first (raise) or bots idle
/// at the dock (lower).
pub const MATE_PATIENCE: Duration = Duration::from_secs(150);
/// **[A]** An offer on the board is stale after this. Falsified by a leader
/// that invites a bot already gone.
pub const OFFER_TTL: Duration = Duration::from_secs(600);
/// **[A]** A gang record on the board is purged after this: a run's full
/// clock plus the entrance and company waits, with a minute to spare. A
/// leader that died or logged off never closes its record; this is what does.
/// Falsified by a live gang's record purged under it (a mate then goes on
/// without the leader: raise).
pub const GANG_TTL: Duration = Duration::from_secs(
    abyss::DURATION_S as u64 + ENTRANCE_WAIT.as_secs() + COMPANY_WAIT.as_secs() + 60,
);
/// **[A]** How long a room must stand clear before the gate is tried: the
/// next wave spawns on a tick, a little after the last NPC dies. Falsified by
/// gate tries that find no gate (raise) or runners idle at an open gate
/// (lower).
pub const CLEAR_SETTLE: Duration = Duration::from_secs(5);
/// **[A]** The pause between gate tries in one room.
pub const GATE_RETRY: Duration = Duration::from_secs(4);
/// **[A]** How long an act waits for a gate to show on the grid.
pub const GATE_WAIT: Duration = Duration::from_secs(6);
/// **[A]** How long a jump (deploy, entrance, gate) is given to land. The
/// server's jump delay is seconds. Falsified by `no session change` refusals
/// of jumps that did land later.
pub const JUMP_PATIENCE: Duration = Duration::from_secs(90);
/// **[A]** How near a gate the runner closes before it calls it; inside
/// the server's own reach (`abyss::MAX_GATE_DISTANCE`). Falsified by
/// `PROXIMITY_ENTRANCE` refusals.
pub const GATE_REACH: f64 = 1_500.0;
/// **[A]** How near a spot the runner stands to deploy. Falsified by deploy
/// refusals for proximity that a closer or farther spot clears.
pub const SPOT_REACH: f64 = 50_000.0;
/// **[A]** The rest after a run (win or lose) before the next deploy.
/// Falsified by runners that never use a second key (raise the rate) or run
/// back to back with a stock of one (lower it).
pub const RUN_REST: Duration = Duration::from_secs(10 * 60);
/// **[A]** The rest after a refused deploy. Falsified by a runner that
/// retries a dead system every turn (lower) or never again (raise).
pub const DEPLOY_RETRY: Duration = Duration::from_secs(20 * 60);
/// **[A]** How long the leader may stay in the `Gather` phase (from the plan
/// to the deploy) before the run is dropped and the keys kept. Falsified by
/// leaders that drop runs they would have deployed a minute later (raise) or
/// that sit in `Gather` for a quarter of an hour (lower).
pub const GATHER_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// **[A]** The rest after a gang that did not assemble (a mate did not join
/// the fleet), the keys kept. Falsified by leaders that re-gather the same
/// absent mates at once (raise) or that idle with mates ready (lower).
pub const GANG_RETRY: Duration = Duration::from_secs(5 * 60);
/// **[A]** How long a pilot waits at a cleared room's gate for its company
/// (mates for the leader, the leader for a mate) before it goes on alone.
/// Falsified by pilots who leave a mate behind in a room (raise) or idle at
/// a gate with nobody coming (lower).
pub const COMPANY_WAIT: Duration = Duration::from_secs(90);
/// **[A]** A roster entry is stale after this: the abyss runners of the world
/// are the ones seen within it. Falsified by builders that bid for the
/// condensate in a world whose runners are all gone (lower).
pub const ROSTER_TTL: Duration = Duration::from_secs(3 * 3600);
/// Group of a planet and of an asteroid belt **[V-data]** (type data groups 7
/// and 9): the spots a deploy tries.
pub const GROUP_PLANET: i64 = 7;
pub const GROUP_BELT: i64 = 9;

// ---- hulls, lists, keys (pure) ----------------------------------------------

/// A pilot's ship, as the lists read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hull {
    pub type_id: i64,
    /// 0: not known (no bundle).
    pub group_id: i64,
    pub category_id: i64,
}

impl Hull {
    /// The hull of `type_id`, group and category from the bundle the driver
    /// handed this thread (zero without one).
    pub fn of(type_id: i64) -> Hull {
        let (group_id, category_id) = crate::acquire::with_content(|c| {
            let t = c.try_get::<eve_server::typedata::Installed>()?;
            let r = t.table().get(type_id)?;
            Some((r.group_id, r.category_id))
        })
        .unwrap_or((0, 0));
        Hull {
            type_id,
            group_id,
            category_id,
        }
    }

    fn class(&self) -> Option<Class> {
        HULLS
            .iter()
            .find(|h| h.type_id == self.type_id)
            .map(|h| h.class)
    }
}

/// The ship lists of the three modes, from the bundle's typelists (the ids are
/// `Mode::ship_list`'s, never written here).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lists {
    lists: BTreeMap<i64, TypeList>,
}

impl Lists {
    /// `typelists` by their ids, as the bundle holds them.
    pub fn new(lists: impl IntoIterator<Item = (i64, TypeList)>) -> Lists {
        Lists {
            lists: lists.into_iter().collect(),
        }
    }

    /// The lists of the content the driver handed this thread; empty without.
    pub fn read() -> Lists {
        crate::acquire::with_content(|c| {
            let t = c.try_get::<eve_server::dungeon_data::Table>()?;
            Some(Lists::new(
                [Mode::Solo, Mode::CoOp, Mode::TwoPlayer]
                    .into_iter()
                    .filter_map(|m| {
                        Some((m.ship_list(), t.typelists.get(&m.ship_list())?.clone()))
                    }),
            ))
        })
        .unwrap_or_default()
    }

    /// Whether `mode` takes `hull`. The bundle's list when it has one and the
    /// hull's group is known; else by size class **[A]**: cruisers solo,
    /// frigates co-op, destroyers in a pair. Falsified by a deploy refused
    /// `INVALID_SHIP_TYPE*` for a hull this said was fine.
    pub fn allows(&self, mode: Mode, hull: &Hull) -> bool {
        match self.lists.get(&mode.ship_list()) {
            Some(l) if hull.group_id != 0 => {
                l.contains(hull.type_id, hull.group_id, hull.category_id)
            }
            _ => matches!(
                (mode, hull.class()),
                (Mode::Solo, Some(Class::Cruiser | Class::Battlecruiser))
                    | (Mode::CoOp, Some(Class::Frigate))
                    | (Mode::TwoPlayer, Some(Class::Destroyer))
            ),
        }
    }
}

/// **The mode a gang flies** (module header): by its size, the lists, and the
/// key stack it holds. `None` when no mode fits.
pub fn mode_for(gang: &[Hull], keys: i64, lists: &Lists) -> Option<Mode> {
    let mode = match gang.len() {
        1 => Mode::Solo,
        2 => Mode::TwoPlayer,
        3 => Mode::CoOp,
        _ => return None,
    };
    (keys >= mode.key_stack() && gang.iter().all(|h| lists.allows(mode, h))).then_some(mode)
}

/// **The highest tier a gang runs** by its weakest hull **[A]**: a frigate
/// gang tiers 0-1, a destroyer pair 0-2, a cruiser solo 0-1; a hull the ladder
/// does not know 0. Falsified by gangs lost to a tier this said they could
/// clear (lower), or that never use a key they could (raise). (The heavy
/// assault cruiser, whose cap the spec raises to 3, is not in the hull
/// ladder: `hull_choice::HULLS` has none.)
pub fn tier_cap(gang: &[Hull]) -> i64 {
    gang.iter()
        .map(|h| match h.class() {
            Some(Class::Frigate) => 1,
            Some(Class::Destroyer) => 2,
            Some(Class::Cruiser | Class::Battlecruiser) => 1,
            _ => 0,
        })
        .min()
        .unwrap_or(0)
}

/// The cap after `losses` gangs died in a pocket **[A]**: one tier per loss.
/// Falsified by a runner that dies at tier 0 over and over (then it should
/// stop) or one that stays at 0 for ever after a single loss (then forgive
/// faster, [`State::win`]).
pub fn lowered(cap: i64, losses: u8) -> i64 {
    (cap - i64::from(losses)).max(0)
}

/// The weakest side of a gang's tank, for the weather it avoids **[A]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tank {
    /// Shield: the weak resists are kinetic and explosive.
    pub shield: bool,
    /// Turrets: the dark weather cuts their range.
    pub turrets: bool,
}

/// What the bots fly: projectile turrets on shield-tanked T1 hulls **[A]**.
/// Falsified by an armour fit becoming the bots' norm.
pub const BOT_TANK: Tank = Tank {
    shield: true,
    turrets: true,
};

/// The dbuff penalty collections a tank avoids (`abyss::weather::WEATHERS`
/// pairs each weather's key type with its penalty collection **[V]**:
/// 99 kinetic, 93 explosive, 90 EM, 95 thermal, 97 turret optimal).
fn avoided(tank: Tank) -> Vec<i64> {
    let mut v = Vec::new();
    if tank.shield {
        v.extend([99, 93]);
    } else {
        v.extend([90, 95]);
    }
    if tank.turrets {
        v.push(97);
    }
    v
}

/// A weather key's penalty collection.
fn penalty_of(weather: i64) -> Option<i64> {
    WEATHERS
        .iter()
        .find(|w| w.beacon == weather)
        .map(|w| w.penalty)
}

/// One stack of keys the bot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRow {
    pub item_id: i64,
    pub type_id: i64,
    /// `difficultyTier`, raw (1001 and 1002 are the special ones).
    pub tier: i64,
    /// `weatherID`: the weather type the key names.
    pub weather: i64,
    pub qty: i64,
}

/// **Which key** (module header): of the stacks with `need` or more, the ones
/// the gang may play (its `cap`, the system's `max_tier`, unless the persona
/// is a pirate's and the system lets any key in), preferring a weather that
/// does not hit the `tank`, then the lowest tier, then the lowest type.
pub fn pick_key(
    held: &[KeyRow],
    need: i64,
    cap: i64,
    max_tier: i64,
    pirate: bool,
    tank: Tank,
) -> Option<KeyRow> {
    let bad = avoided(tank);
    held.iter()
        .filter(|k| k.qty >= need)
        .filter(|k| abyss::play_tier(k.tier) <= cap)
        .filter(|k| {
            let demo = k.tier == abyss::TIER_0_DEMO;
            if max_tier == 0 {
                // The server refuses any tier above 0 in the very secure.
                return demo || k.tier == 0;
            }
            demo || pirate || k.tier <= max_tier
        })
        .min_by_key(|k| {
            let hit = penalty_of(k.weather).is_some_and(|p| bad.contains(&p));
            (hit, abyss::play_tier(k.tier), k.type_id)
        })
        .copied()
}

/// The first of `spots` not in `tried`.
pub fn next_spot(tried: &BTreeSet<i64>, spots: &[i64]) -> Option<i64> {
    spots.iter().copied().find(|s| !tried.contains(s))
}

/// How the clock sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Fight,
    /// Targets only: the cache and nodes are left.
    SkipLoot,
}

/// **The clock** (module header): `end` and `now` in filetime ticks (100 ns),
/// `rooms_left` the rooms after this one.
pub fn pace(now: BlueTime, end: BlueTime, rooms_left: u8) -> Pace {
    let reserve = ROOM_RESERVE.as_secs() as i64 * 10_000_000 * i64::from(rooms_left.max(1));
    if end - now < reserve {
        Pace::SkipLoot
    } else {
        Pace::Fight
    }
}

// ---- the server's refusals --------------------------------------------------

/// The `(code, args)` pairs of an `AbyssError` raised value
/// (`UserError('AbyssError', {'errors': [(code, args)]})` **[V]**), empty for
/// any other.
pub fn refusal_pairs(v: &PyValue) -> Vec<(i64, Vec<i64>)> {
    let PyValue::ObjectEx { header, .. } = v else {
        return Vec::new();
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        return Vec::new();
    };
    let Some(PyValue::Tuple(args)) = h.get(1) else {
        return Vec::new();
    };
    let Some(PyValue::Dict(dict)) = args.get(1) else {
        return Vec::new();
    };
    let named = |v: &PyValue| matches!(v, PyValue::Bytes(b) if b.as_slice() == b"errors");
    let Some(PyValue::List(list)) = dict.iter().find_map(|(a, b)| {
        if named(b) {
            Some(a)
        } else if named(a) {
            Some(b)
        } else {
            None
        }
    }) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| {
            let PyValue::Tuple(t) = e else { return None };
            let PyValue::Int(code) = t.first()? else {
                return None;
            };
            let args = match t.get(1) {
                Some(PyValue::Tuple(a) | PyValue::List(a)) => a
                    .iter()
                    .filter_map(|x| {
                        if let PyValue::Int(n) = x {
                            Some(*n)
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Some((*code, args))
        })
        .collect()
}

/// [`refusal_pairs`] of a call's whole reply (`CallOutcome::result`): the
/// pairs of a refused call, empty for an answered one.
pub fn read_refusal(reply: &PyValue) -> Vec<(i64, Vec<i64>)> {
    let Ok(packet) = eve_proto::MachoPacket::from_value(reply) else {
        return Vec::new();
    };
    if packet.command != eve_marshal::MachoNetMsg::ErrorResponse {
        return Vec::new();
    }
    packet
        .body
        .iter()
        .find_map(|part| {
            let PyValue::Tuple(wrapped) = part else {
                return None;
            };
            let PyValue::Bytes(pickle) = wrapped.first()? else {
                return None;
            };
            let v = crate::loads(pickle).ok()?;
            let pairs = refusal_pairs(&v);
            (!pairs.is_empty()).then_some(pairs)
        })
        .unwrap_or_default()
}

// ---- the board --------------------------------------------------------------

/// One bot docked and ready to run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub station: i64,
    pub hull: Hull,
    /// The biggest stack of keys it holds.
    pub holds: i64,
    pub at: Instant,
}

/// A gang's entrance, as the leader and the mates see it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gang {
    pub mates: Vec<i64>,
    pub entered: BTreeSet<i64>,
    /// When the entrance opened; `None` once the leader is in (the gate is
    /// then shut to the mates) .
    pub opened: Option<Instant>,
    /// When the record was made: [`Board::purge`] drops it after
    /// [`GANG_TTL`] (a leader that died or went offline never closes it).
    pub since: Option<Instant>,
    /// The room each pilot inside is in (the leader's too).
    pub rooms: BTreeMap<i64, u8>,
    /// The room the leader is in; 0 before it entered.
    pub leader_room: u8,
}

/// **Whether a pilot takes the room's gate now** (pure): the leader waits
/// until every mate still in the run is in `room`; a mate waits until the
/// leader has gone past `room`. A short clock (`short`), a wait of
/// [`COMPANY_WAIT`] or no record (a solo run, a closed gang) lets it go.
pub fn gate_open_for(
    gang: Option<&Gang>,
    me: i64,
    leader: i64,
    room: u8,
    waited: Duration,
    short: bool,
    now: Instant,
) -> bool {
    let stale = gang.is_some_and(|g| {
        g.leader_room == 0
            && g.opened
                .is_some_and(|t| now.saturating_duration_since(t) > ENTRANCE_WAIT + COMPANY_WAIT)
    });
    may_take_gate(
        if stale { None } else { gang },
        me,
        leader,
        room,
        waited,
        short,
    )
}

/// [`gate_open_for`]'s rule on the gang record alone (pure).
pub fn may_take_gate(
    gang: Option<&Gang>,
    me: i64,
    leader: i64,
    room: u8,
    waited: Duration,
    short: bool,
) -> bool {
    if short {
        return true;
    }
    let Some(g) = gang else { return true };
    // The leader is still outside with the entrance open: the mates inside
    // wait for it, unbounded here. The leader enters or gives up within the
    // entrance wait and either seals or closes the gang; a leader that
    // does neither (offline, say) is passed over by [`gate_open_for`],
    // which drops a record open longer than the entrance wait and the
    // company wait.
    if me != leader && g.leader_room == 0 && g.opened.is_some() {
        return false;
    }
    if waited >= COMPANY_WAIT {
        return true;
    }
    if me == leader {
        g.rooms
            .iter()
            .filter(|(who, _)| **who != leader)
            .all(|(_, r)| *r >= room)
    } else {
        g.leader_room > room
    }
}

#[derive(Debug, Default)]
struct Inner {
    offers: BTreeMap<(SocketAddr, i64), Offer>,
    gangs: BTreeMap<(SocketAddr, i64), Gang>,
    /// The abyss runners this process runs: last seen, per bot.
    roster: BTreeMap<(SocketAddr, i64), Instant>,
}

impl Inner {
    fn purge(&mut self, now: Instant) {
        self.offers
            .retain(|_, o| now.saturating_duration_since(o.at) < OFFER_TTL);
        self.gangs.retain(|_, g| {
            g.since
                .is_none_or(|t| now.saturating_duration_since(t) < GANG_TTL)
        });
    }
}

/// **Who is ready, and whose entrance is open** (per process, like the
/// raiders' board: the bots' own etiquette; a real player is invited by the
/// fleet window as ever).
#[derive(Debug)]
pub struct Board {
    inner: Mutex<Option<Inner>>,
}

impl Board {
    pub const fn new() -> Board {
        Board {
            inner: Mutex::new(None),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(Inner::default))
    }

    pub fn offer(&self, server: SocketAddr, who: i64, offer: Offer) {
        self.with(|i| {
            i.purge(offer.at);
            i.offers.insert((server, who), offer);
        });
    }

    /// Drop the offers older than [`OFFER_TTL`] and the gangs older than
    /// [`GANG_TTL`] (bots that went offline never withdraw or close).
    pub fn purge(&self, now: Instant) {
        self.with(|i| i.purge(now));
    }

    /// How many offers and gangs the board holds (stale ones included).
    pub fn sizes(&self) -> (usize, usize) {
        self.with(|i| (i.offers.len(), i.gangs.len()))
    }

    pub fn withdraw(&self, server: SocketAddr, who: i64) {
        self.with(|i| {
            i.offers.remove(&(server, who));
        });
    }

    /// The fresh offers of others docked at `station`, oldest first.
    pub fn others_at(
        &self,
        server: SocketAddr,
        station: i64,
        me: i64,
        now: Instant,
    ) -> Vec<(i64, Offer)> {
        self.with(|i| {
            let mut v: Vec<(i64, Offer)> = i
                .offers
                .iter()
                .filter(|((s, who), o)| {
                    *s == server
                        && *who != me
                        && o.station == station
                        && now.saturating_duration_since(o.at) < OFFER_TTL
                })
                .map(|((_, who), o)| (*who, *o))
                .collect();
            v.sort_by_key(|(who, o)| (o.at, *who));
            v
        })
    }

    pub fn open(&self, server: SocketAddr, leader: i64, mates: Vec<i64>, now: Instant) {
        self.with(|i| {
            i.purge(now);
            i.gangs.insert(
                (server, leader),
                Gang {
                    mates,
                    opened: Some(now),
                    since: Some(now),
                    ..Gang::default()
                },
            );
        });
    }

    pub fn entered(&self, server: SocketAddr, leader: i64, who: i64) {
        self.with(|i| {
            if let Some(g) = i.gangs.get_mut(&(server, leader)) {
                g.since = Some(clock::instant());
                g.entered.insert(who);
            }
        });
    }

    /// The leader is in: the entrance is shut to the mates (the record stays
    /// for the rooms).
    pub fn seal(&self, server: SocketAddr, leader: i64) {
        self.with(|i| {
            if let Some(g) = i.gangs.get_mut(&(server, leader)) {
                g.since = Some(clock::instant());
                g.opened = None;
            }
        });
    }

    /// `who` is in `room` of `leader`'s run.
    pub fn at_room(&self, server: SocketAddr, leader: i64, who: i64, room: u8) {
        self.with(|i| {
            if let Some(g) = i.gangs.get_mut(&(server, leader)) {
                g.since = Some(clock::instant());
                g.rooms.insert(who, room);
                if who == leader {
                    g.leader_room = room;
                }
            }
        });
    }

    /// `who` is out of `leader`'s run (it died, or took the exit).
    pub fn leave(&self, server: SocketAddr, leader: i64, who: i64) {
        self.with(|i| {
            if let Some(g) = i.gangs.get_mut(&(server, leader)) {
                g.rooms.remove(&who);
            }
        });
    }

    /// The bot finished a run, or holds abyssal goods: somebody here really
    /// brings the condensate in.
    pub fn enroll(&self, server: SocketAddr, who: i64, now: Instant) {
        self.with(|i| {
            i.roster.insert((server, who), now);
        });
    }

    /// Whether any abyss runner has been seen within [`ROSTER_TTL`] of `now`.
    /// On `server` only, when one is named.
    pub fn has_runners(&self, server: Option<SocketAddr>, now: Instant) -> bool {
        self.with(|i| {
            i.roster.iter().any(|((s, _), t)| {
                server.is_none_or(|x| x == *s) && now.saturating_duration_since(*t) < ROSTER_TTL
            })
        })
    }

    pub fn gang(&self, server: SocketAddr, leader: i64) -> Option<Gang> {
        self.with(|i| i.gangs.get(&(server, leader)).cloned())
    }

    pub fn close(&self, server: SocketAddr, leader: i64) {
        self.with(|i| {
            i.gangs.remove(&(server, leader));
        });
    }
}

impl Default for Board {
    fn default() -> Board {
        Board::new()
    }
}

/// The board every abyss runner of this process shares.
pub static BOARD: Board = Board::new();

// ---- the plan (pure) ---------------------------------------------------------

/// What a leader means to fly.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub mode: Mode,
    pub mates: Vec<i64>,
    pub key: KeyRow,
}

/// What a plan reads besides the bot's own hull.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub max_tier: i64,
    pub pirate: bool,
    pub tank: Tank,
    pub losses: u8,
}

/// Whether `me` leads the offers at its station: it holds a key and no other
/// key-holder has a lower character id.
pub fn leads(me: i64, holds: i64, others: &[(i64, Offer)]) -> bool {
    holds > 0 && !others.iter().any(|(who, o)| o.holds > 0 && *who < me)
}

/// **The biggest gang the keys pay for** (module header): co-op with two
/// mates, else a pair with one, else solo. `others` are the offers at the
/// station, oldest first.
pub fn plan(
    me: &Hull,
    keys: &[KeyRow],
    others: &[(i64, Offer)],
    lists: &Lists,
    ctx: Ctx,
) -> Option<Plan> {
    for mode in [Mode::CoOp, Mode::TwoPlayer, Mode::Solo] {
        let want = mode.max_players() - 1;
        let mates: Vec<&(i64, Offer)> = others
            .iter()
            .filter(|(_, o)| lists.allows(mode, &o.hull))
            .take(want)
            .collect();
        if mates.len() < want {
            continue;
        }
        let mut gang = vec![*me];
        gang.extend(mates.iter().map(|(_, o)| o.hull));
        let best = keys.iter().map(|k| k.qty).max().unwrap_or(0);
        if mode_for(&gang, best, lists) != Some(mode) {
            continue;
        }
        let cap = lowered(tier_cap(&gang), ctx.losses);
        let Some(key) = pick_key(
            keys,
            mode.key_stack(),
            cap,
            ctx.max_tier,
            ctx.pirate,
            ctx.tank,
        ) else {
            continue;
        };
        return Some(Plan {
            mode,
            mates: mates.iter().map(|(who, _)| *who).collect(),
            key,
        });
    }
    None
}

// ---- what the bot remembers ---------------------------------------------------

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Docked, or about to be: nothing under way.
    #[default]
    Docked,
    /// The key is in the hold and the fleet formed: off to deploy.
    Gather,
    /// The entrance gate is open.
    Deploy,
    /// Entering (a mate's phase between the gate and the pocket).
    Enter,
    /// In a pocket, playing a room.
    Room,
    /// Taking a room's gate.
    Gate,
    /// Out the exit gate: home and sell.
    Home,
}

/// [`Memory::abyss`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub phase: Phase,
    /// The server, once an offer told it.
    pub server: Option<SocketAddr>,
    /// The key type and the mode (wire) of the run in hand.
    pub key: Option<i64>,
    pub mode: Option<i64>,
    pub mates: Vec<i64>,
    /// `OnAddExpiryDurationForAbyssalContent`'s end time.
    pub run_end: Option<BlueTime>,
    /// The room being played, 1-based; 0 outside a pocket.
    pub room: u8,
    /// Gangs lost in a pocket, net of the wins since.
    pub losses: u8,
    /// What the last offer said, and when the bot first offered at this dock.
    pub offered: Option<(i64, i64)>,
    pub offered_at: Option<Instant>,
    pub retry_after: Option<Instant>,
    /// The room stands clear since; the next gate try not before.
    pub clear_since: Option<Instant>,
    pub gate_after: Option<Instant>,
    /// Goods to sell at the next dock.
    pub sell_due: bool,
    /// The leader of the gang this pilot runs with (itself for the leader);
    /// `None` for a solo run.
    pub boss: Option<i64>,
    /// The leader's deadline in `Gather`.
    pub gather_by: Option<Instant>,
    /// The tier of the key in hand.
    pub key_tier: i64,
    /// The leader went in with mates missing and the loss is booked
    /// ([`State::book_short_gang`]).
    pub short_booked: bool,
    /// The hangar's keys were stacked at this dock (once a dock).
    pub stack_tried: bool,
    /// The last act's line for the act log.
    pub note: Option<String>,
}

impl State {
    /// A run ended well: a loss is forgiven **[A]**.
    pub fn win(&mut self, now: Instant) {
        *self = State {
            phase: Phase::Home,
            server: self.server,
            losses: self.losses.saturating_sub(1),
            retry_after: Some(now + RUN_REST),
            sell_due: true,
            ..State::default()
        };
    }

    /// A run ended in a death or an ejection. A run already booked short
    /// ([`State::book_short_gang`]) is one loss, not two.
    pub fn lose(&mut self, now: Instant) {
        let booked = u8::from(!self.short_booked);
        *self = State {
            phase: Phase::Docked,
            server: self.server,
            losses: self.losses.saturating_add(booked),
            retry_after: Some(now + RUN_REST),
            ..State::default()
        };
    }

    fn forget_run(&mut self) {
        self.key = None;
        self.mode = None;
        self.mates.clear();
        self.run_end = None;
        self.room = 0;
        self.clear_since = None;
        self.gate_after = None;
        self.boss = None;
        self.gather_by = None;
        self.short_booked = false;
    }

    /// The leader goes in with a mate missing: the run counts as a loss for
    /// the tier cap (a win later forgives it), once per run. `true` when
    /// booked now.
    pub fn book_short_gang(&mut self) -> bool {
        if self.short_booked {
            return false;
        }
        self.short_booked = true;
        self.losses = self.losses.saturating_add(1);
        true
    }
}

/// One step of a run, as an [`Act`].
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Docked: put this pilot on the board.
    Offer {
        station: i64,
        hull: Hull,
        holds: i64,
    },
    /// Docked, the leader: form the fleet and invite the mates.
    Gather { ship_type: i64, mates: Vec<i64> },
    /// In space: deploy the key (its type) in `mode` (wire id).
    Deploy { key: i64, mode: i64 },
    /// Take the entrance gate of `leader`'s run (the bot's own character id
    /// for the leader).
    Enter { leader: i64 },
    /// Take the room's gate (the exit gate in the last room).
    Gate,
    /// Docked: sell the abyssal goods.
    Sell,
    /// Docked: stack the hangar (`StackAll`), so looted keys form one stack.
    Stack,
}

impl Op {
    pub fn describe(&self) -> String {
        match self {
            Op::Offer { station, holds, .. } => {
                format!("abyss: offer at {station}, {holds} keys held")
            }
            Op::Gather { mates, .. } => format!("abyss: form the fleet, invite {mates:?}"),
            Op::Deploy { key, mode } => format!("abyss: deploy key type {key}, mode {mode}"),
            Op::Enter { leader } => format!("abyss: take {leader}'s entrance gate"),
            Op::Gate => "abyss: take the room's gate".to_owned(),
            Op::Sell => "abyss: sell the abyssal goods".to_owned(),
            Op::Stack => "abyss: stack the hangar".to_owned(),
        }
    }
}

// ---- the role ---------------------------------------------------------------

/// Runs the anomalies, and the Abyss with a key and a gang.
pub static ROLE: RoleSpec = RoleSpec {
    name: "abyss",
    greeting: "o7 -- running the abyss with you",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

fn setup(lacks: Lacks) -> Vec<String> {
    (site_runner::ROLE.setup)(lacks)
}

/// The keys in `rows` (a type's largest single stack), with their tier and weather read from the bundle
/// (a key of an unknown tier is not listed: nothing is guessed).
pub fn key_rows(rows: &[crate::ItemRow]) -> Vec<KeyRow> {
    let mut out: Vec<KeyRow> = Vec::new();
    for r in rows.iter().filter(|r| r.group_id == abyss::KEY_GROUP) {
        let attrs = crate::acquire::with_content(|c| {
            let t = c.try_get::<eve_server::typedata::Installed>()?;
            let row = t.table().get(r.type_id)?;
            Some((
                row.attribute(abyss::ATTR_TIER)? as i64,
                row.attribute(abyss::ATTR_WEATHER).map_or(0, |w| w as i64),
            ))
        });
        let Some((tier, weather)) = attrs else {
            continue;
        };
        // One stack is what a deploy takes (the server reads a single row):
        // the largest row of a type stands for it.
        match out.iter_mut().find(|k| k.type_id == r.type_id) {
            Some(k) => {
                if r.quantity > k.qty {
                    k.item_id = r.item_id;
                    k.qty = r.quantity;
                }
            }
            None => out.push(KeyRow {
                item_id: r.item_id,
                type_id: r.type_id,
                tier,
                weather,
                qty: r.quantity,
            }),
        }
    }
    out
}

/// Whether the hangar holds two rows of one key type: `StackAll` would
/// merge them (singletons never merge).
pub fn needs_stack(rows: &[crate::ItemRow]) -> bool {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    rows.iter()
        .filter(|r| r.group_id == abyss::KEY_GROUP && !r.singleton)
        .any(|r| !seen.insert(r.type_id))
}

/// The security of the system the bot is in, from the bundle; `None` unknown.
fn security_of(system: i64) -> Option<f64> {
    crate::acquire::with_content(|c| {
        c.try_get::<eve_server::celestial::Universe>()?
            .system(system)
            .map(|s| s.security)
    })
}

/// The system a bot docked at `station` is in.
fn system_of_station(m: &Memory, station: i64) -> Option<i64> {
    m.policy.travel.as_ref()?.system_of(station)
}

fn in_pocket(o: &Observation) -> bool {
    matches!(o.place, Place::InSpace { system, .. } if VOID_SYSTEMS.contains(&system))
}

/// Bookkeeping that runs wherever the bot is: a run that is over without the
/// bot having taken the exit gate (it died, or was thrown out).
fn settle_run(m: &mut Memory, o: &Observation) {
    let a = &mut m.abyss;
    let docked = matches!(o.place, Place::Docked { .. });
    match a.phase {
        Phase::Room | Phase::Gate if !in_pocket(o) => {
            // The run is over for this pilot: its gang is closed (the
            // leader's) or loses it (a mate's), so nobody waits on it.
            if let (Some(server), Some(boss)) = (a.server, a.boss) {
                if boss == o.me {
                    BOARD.close(server, boss);
                } else {
                    BOARD.leave(server, boss, o.me);
                }
            }
            a.lose(o.now);
        }
        Phase::Home if docked => a.phase = Phase::Docked,
        // Docked with a run half made (the key was unloaded, or the deploy
        // never came): start over, keeping the keys.
        Phase::Gather | Phase::Deploy if docked => {
            if let (Some(server), Some(boss)) = (a.server, a.boss)
                && boss == o.me
            {
                BOARD.close(server, boss);
            }
            a.forget_run();
            a.phase = Phase::Docked;
        }
        _ => {}
    }
}

/// **Docked** (module header): the site runner's rules, with the offer, the
/// sale, the key and the fleet put before its undock.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    settle_run(m, o);
    let mut acts = (site_runner::ROLE.docked)(m, o);
    let Place::Docked { station } = o.place else {
        return acts;
    };
    let Some(ship) = o.ship.and_then(|s| s.type_id) else {
        return acts;
    };
    let Some(at) = acts.iter().position(|a| matches!(a, Act::Undock)) else {
        return acts;
    };
    // Looted keys sit in rows of one: the server does not merge them, so no
    // row holds a co-op stack. Stack the hangar before planning on it.
    if !m.abyss.stack_tried && needs_stack(&o.hangar) {
        m.abyss.stack_tried = true;
        acts.retain(|a| !matches!(a, Act::Undock));
        acts.insert(0, Act::Abyss(Op::Stack));
        acts.push(Act::Wait(brain::SHORT_WAIT));
        return acts;
    }
    let hull = Hull::of(ship);
    let all: Vec<crate::ItemRow> = o.hangar.iter().chain(&o.cargo).cloned().collect();
    let keys = key_rows(&all);
    let holds = keys.iter().map(|k| k.qty).max().unwrap_or(0);
    let mut front: Vec<Act> = Vec::new();
    let rested = m.abyss.retry_after.is_none_or(|t| o.now >= t);
    let fit = o.failed(ActKind::Abyss) < GIVE_UP_AFTER && rested && m.abyss.phase == Phase::Docked;
    // A resting key-holder offers no keys, or it would keep the others from
    // leading.
    let offered_holds = if fit { holds } else { 0 };
    if m.abyss.offered != Some((station, offered_holds)) {
        m.abyss.offered = Some((station, offered_holds));
        m.abyss.offered_at.get_or_insert(o.now);
        front.push(Act::Abyss(Op::Offer {
            station,
            hull,
            holds: offered_holds,
        }));
    }
    let offered_for = m
        .abyss
        .offered_at
        .map_or(Duration::ZERO, |t| o.now.saturating_duration_since(t));
    let Some(server) = m.abyss.server else {
        // The offer has not run yet: it will, in this turn's front acts. The
        // undock waits a turn, or the offer would outlive the dock.
        if acts.iter().any(|a| matches!(a, Act::Undock)) {
            acts.retain(|a| !matches!(a, Act::Undock));
            acts.push(Act::Wait(brain::SHORT_WAIT));
        }
        acts.splice(0..0, front);
        return acts;
    };
    if m.abyss.sell_due && o.hangar.iter().any(is_abyssal_good) {
        m.abyss.sell_due = false;
        front.push(Act::Abyss(Op::Sell));
    }
    let others = BOARD.others_at(server, station, o.me, o.now);
    let leader = fit && leads(o.me, holds, &others);
    if leader {
        let system = system_of_station(m, station).or(match o.place {
            Place::InSpace { system, .. } => Some(system),
            _ => None,
        });
        let ctx = Ctx {
            // A system the bot cannot place is the strictest one.
            max_tier: system.and_then(security_of).map_or(0, abyss::max_tier),
            pirate: false,
            tank: BOT_TANK,
            losses: m.abyss.losses,
        };
        match plan(&hull, &keys, &others, &Lists::read(), ctx) {
            Some(p) => {
                let a = &mut m.abyss;
                a.key = Some(p.key.type_id);
                a.mode = Some(p.mode.wire());
                a.key_tier = abyss::play_tier(p.key.tier);
                a.mates = p.mates.clone();
                a.phase = Phase::Gather;
                a.boss = Some(o.me);
                a.gather_by = Some(o.now + GATHER_DEADLINE);
                let need = p.mode.key_stack();
                // A deploy takes one row: the hold's largest stack counts.
                let in_hold: i64 = o
                    .cargo
                    .iter()
                    .filter(|r| r.type_id == p.key.type_id)
                    .map(|r| r.quantity)
                    .max()
                    .unwrap_or(0);
                // The key goes into the hold only now: the escort's docked
                // rules unload anything of the sort at the next dock.
                // A full stack from one row, never a top-up beside a part row.
                if in_hold < need
                    && let Some(row) = o
                        .hangar
                        .iter()
                        .find(|r| r.type_id == p.key.type_id && r.quantity >= need)
                {
                    front.push(Act::Stock {
                        item: row.item_id,
                        quantity: need,
                    });
                }
                if p.mode.fleet_required() {
                    front.push(Act::Abyss(Op::Gather {
                        ship_type: ship,
                        mates: p.mates,
                    }));
                }
            }
            None if holds > 0 && offered_for < MATE_PATIENCE => {
                // A key and no gang yet: the others may still be offering.
                acts.remove(at);
                acts.push(Act::Wait(brain::SHORT_WAIT));
            }
            None => {}
        }
    } else if fit
        && o.fleet.is_none()
        && offered_for < MATE_PATIENCE
        && others.iter().any(|(who, x)| x.holds > 0 && *who < o.me)
    {
        // Somebody here holds a key and leads: wait to be invited.
        acts.remove(at);
        acts.push(Act::Wait(brain::SHORT_WAIT));
    }
    // Undocking ends the offer: the next dock offers again.
    if acts.iter().any(|a| matches!(a, Act::Undock)) {
        m.abyss.offered = None;
        m.abyss.offered_at = None;
        m.abyss.stack_tried = false;
        BOARD.withdraw(server, o.me);
    }
    acts.splice(0..0, front);
    acts
}

/// Whether a hangar row is something the pockets yield that the bots sell:
/// Zero-Point Condensate by its group, the others by name **[V-data]**
/// (`Survey Database`, `Crystalline Isogen-10`).
pub fn is_abyssal_good(r: &crate::ItemRow) -> bool {
    if r.singleton {
        return false;
    }
    if r.group_id == crate::hulls::GROUP_ABYSSAL {
        return true;
    }
    crate::acquire::with_content(|c| {
        let t = c.try_get::<eve_server::typedata::Installed>()?;
        let n = &t.table().get(r.type_id)?.name;
        Some(n.contains("Survey Database") || n.starts_with("Crystalline Isogen"))
    })
    .unwrap_or(false)
}

/// Rule 5: what to do in space.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    settle_run(m, o);
    if in_pocket(o) {
        return inside(m, o);
    }
    if let Some(acts) = k_space(m, o, park) {
        return acts;
    }
    (site_runner::ROLE.in_space)(m, o)
}

/// What the leader does at its open entrance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrance {
    /// Keep waiting for the mates.
    Wait,
    /// Go in (`all_in`: every mate is inside already).
    Enter { all_in: bool },
    /// Close the gang and book a loss: nobody is inside to go in with.
    Abandon,
}

/// **The leader's turn at its entrance** (pure). It goes in when every mate
/// is in or [`ENTRANCE_WAIT`] has run out; a `threatened` leader (hostiles shooting
/// the leader, or a homing) does not wait: it goes in at once if a mate is
/// inside, else abandons the gang. Nobody inside when the wait runs out is an
/// abandon too, with a key tier known (`key_known`); a threatened leader
/// abandons whatever the key.
pub fn entrance_verdict(
    gang: &Gang,
    waited: Duration,
    threatened: bool,
    key_known: bool,
) -> Entrance {
    let all_in = gang.mates.iter().all(|x| gang.entered.contains(x));
    if !(all_in || threatened || waited >= ENTRANCE_WAIT) {
        return Entrance::Wait;
    }
    if !all_in && gang.entered.is_empty() && (threatened || key_known) {
        return Entrance::Abandon;
    }
    Entrance::Enter { all_in }
}

/// In known space: take an open entrance, or deploy the key in hand.
fn k_space(m: &mut Memory, o: &Observation, park: &brain::ParkView) -> Option<Vec<Act>> {
    let server = m.abyss.server?;
    // A mate: the leader's entrance is open and the leader is on this grid.
    // (The phase is set by `enter`, once the jump has landed: a try that
    // found no gate or was refused is no loss.)
    if let Some(boss) = o.fleet.and_then(|f| f.boss)
        && boss != o.me
        && park.boss.is_some()
        && m.abyss.phase == Phase::Docked
        && m.abyss.retry_after.is_none_or(|t| o.now >= t)
        && !o.hold_full
        && BOARD.gang(server, boss).is_some_and(|g| {
            g.mates.contains(&o.me)
                && !g.entered.contains(&o.me)
                && g.opened.is_some_and(|t| {
                    o.now.saturating_duration_since(t).as_secs()
                        < abyss::UNENTERED_GATE_LIFETIME_S as u64
                })
        })
        && o.failed(ActKind::Abyss) < GIVE_UP_AFTER
    {
        return Some(vec![Act::Abyss(Op::Enter { leader: boss })]);
    }
    match m.abyss.phase {
        Phase::Deploy => {
            let gang = BOARD.gang(server, o.me)?;
            let waited = gang
                .opened
                .map_or(Duration::ZERO, |t| o.now.saturating_duration_since(t));
            // The leader waits here with the escort's rules off: attacked, it
            // would neither shoot back nor flee. Hostiles shooting it, or a
            // homing, end the wait. Only an NPC with its gun on this ship
            // counts: a belt's passive rats do not make a leader give up.
            let threatened = m.homing.is_some()
                || park
                    .hostiles
                    .iter()
                    .any(|h| h.shooting == Some(park.ego.id));
            match entrance_verdict(&gang, waited, threatened, m.abyss.key_tier > 0) {
                Entrance::Wait => Some(vec![Act::Wait(brain::SHORT_WAIT)]),
                Entrance::Abandon => {
                    // Nobody made it in: the leader leaves the gate (the
                    // keys are lost, the ship kept), the cap learns, and the
                    // site runner's rules (fight, flee) have the turn.
                    BOARD.close(server, o.me);
                    m.abyss.lose(o.now);
                    None
                }
                Entrance::Enter { all_in } => {
                    if !all_in && m.abyss.book_short_gang() {
                        m.abyss.note = Some("[abyss] entering with mates missing".to_owned());
                    }
                    if o.failed(ActKind::Abyss) >= GIVE_UP_AFTER {
                        BOARD.close(server, o.me);
                        m.abyss.forget_run();
                        m.abyss.phase = Phase::Docked;
                        return None;
                    }
                    Some(vec![Act::Abyss(Op::Enter { leader: o.me })])
                }
            }
        }
        Phase::Gather => {
            let key = m.abyss.key?;
            let mode = Mode::from_wire(m.abyss.mode?)?;
            if m.abyss.gather_by.is_some_and(|t| o.now >= t) {
                // Stuck in the gather: start over, the keys kept.
                if m.abyss.boss == Some(o.me) {
                    BOARD.close(server, o.me);
                }
                m.abyss.forget_run();
                m.abyss.phase = Phase::Docked;
                m.abyss.retry_after = Some(o.now + DEPLOY_RETRY);
                return None;
            }
            // A deploy takes one row: the largest stack counts.
            let held: i64 = o
                .cargo
                .iter()
                .filter(|r| r.type_id == key)
                .map(|r| r.quantity)
                .max()
                .unwrap_or(0);
            let ready = m.homing.is_none()
                && !o.hold_full
                && !o.guns.is_empty()
                && held >= mode.key_stack()
                && o.failed(ActKind::Abyss) < GIVE_UP_AFTER;
            if !ready {
                if o.failed(ActKind::Abyss) >= GIVE_UP_AFTER {
                    m.abyss.forget_run();
                    m.abyss.phase = Phase::Docked;
                    m.abyss.retry_after = Some(o.now + DEPLOY_RETRY);
                }
                return None;
            }
            Some(vec![Act::Abyss(Op::Deploy {
                key,
                mode: mode.wire(),
            })])
        }
        _ => None,
    }
}

/// **In a pocket** (module header): the site runner's fight as a solo
/// runner, whatever the fleet; the gate when the room stands clear.
fn inside(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    if m.abyss.phase != Phase::Gate {
        m.abyss.phase = Phase::Room;
    }
    m.abyss.room = m.abyss.room.max(1);
    // The pocket has one way out: no retreat reasons count.
    let mut seen = o.clone();
    seen.fleet = None;
    // Refused gate tries are a fact about the room (the gate is not up yet),
    // not a sign the bot is broken: three of them would set the escort's
    // `Homing::Recovery`, which keeps only `Say` below and silences the guns
    // until the pocket clock ends the run. The other kinds still count.
    seen.failures.remove(&ActKind::Abyss);
    if let Some(p) = &mut seen.park {
        p.boss = None;
    }
    let skip = m.abyss.run_end.is_some_and(|end| {
        let rooms_left = (abyss::ROOMS as u8).saturating_sub(m.abyss.room);
        pace(eve_server::pyutil::filetime_now(), end, rooms_left) == Pace::SkipLoot
    });
    if skip && let Some(p) = &mut seen.park {
        p.hostiles.retain(|h| !is_cache(h.type_id));
    }
    // A full hold or a short clock ends the looting: the room's wrecks count
    // as done, or the gate would never be tried.
    let wrecks_done = skip || o.hold_full;
    m.homing = None;
    let kept = m.looted.clone();
    let mut acts = (site_runner::ROLE.in_space)(m, &seen);
    // The runner's settle ends in "next site" and forgets its looted wrecks;
    // here the room stays as it is, or the same wreck would be opened again
    // and again and the room never read clear.
    if acts.iter().any(|a| matches!(a, Act::Sites)) {
        m.looted = kept;
    }
    // The Agency has nothing in a pocket: the site runner's "next site" is
    // never sent (its refusals would count toward a recovery).
    acts.retain(|a| !matches!(a, Act::Sites));
    if m.homing.take().is_some() {
        acts.retain(|a| matches!(a, Act::Say(_)));
    }
    let view = seen.park.as_ref().unwrap_or(park);
    // The site runner marks a wreck looted the turn it sends the Loot, so a
    // turn that carries a Loot is never a clear one: the act must go out
    // before the gate is taken.
    if wrecks_done {
        acts.retain(|a| !matches!(a, Act::Loot(_)));
    }
    let looting = acts.iter().any(|a| matches!(a, Act::Loot(_)));
    let clear = !looting
        && view.hostiles.is_empty()
        && (wrecks_done || view.wrecks.iter().all(|w| m.looted.contains(&w.id)));
    if !clear {
        m.abyss.clear_since = None;
        if acts.is_empty() {
            acts.push(Act::Wait(brain::SHORT_WAIT));
        }
        return acts;
    }
    let since = *m.abyss.clear_since.get_or_insert(o.now);
    let mut out: Vec<Act> = acts
        .into_iter()
        .filter(|a| matches!(a, Act::Say(_)))
        .collect();
    let waited = o.now.saturating_duration_since(since);
    // The gang keeps together: the mates follow the leader through a gate,
    // and the leader waits for its mates (bounded).
    let company = match (m.abyss.server, m.abyss.boss) {
        (Some(server), Some(boss)) => gate_open_for(
            BOARD.gang(server, boss).as_ref(),
            o.me,
            boss,
            m.abyss.room,
            waited,
            skip,
            o.now,
        ),
        _ => true,
    };
    let due = waited >= CLEAR_SETTLE
        && company
        && m.abyss.gate_after.is_none_or(|t| o.now >= t)
        && !park.ego_warping;
    if due {
        m.abyss.gate_after = Some(o.now + GATE_RETRY);
        out.push(Act::Abyss(Op::Gate));
    } else {
        out.push(Act::Wait(brain::SHORT_WAIT));
    }
    out
}

/// Whether a type is a cache or a node (the bundle's group, **[V-data]**).
fn is_cache(type_id: i64) -> bool {
    crate::acquire::with_content(|c| {
        Some(
            c.try_get::<eve_server::typedata::Installed>()?
                .table()
                .get(type_id)?
                .group_id,
        )
    }) == Some(abyss::NODE_GROUP)
}

// ---- on the wire --------------------------------------------------------------

pub(crate) fn server_of(c: &mut Conn) -> Result<SocketAddr> {
    Ok(c.socket()?.peer_addr()?)
}

fn me(c: &Conn) -> i64 {
    c.session().char_id().unwrap_or_default()
}

/// **One abyss step** over the wire (the driver's `Act::Abyss` arm).
pub fn act(c: &mut Conn, m: &mut Memory, op: Op) -> Result<()> {
    match op {
        Op::Offer {
            station,
            hull,
            holds,
        } => {
            let server = server_of(c)?;
            m.abyss.server = Some(server);
            BOARD.offer(
                server,
                me(c),
                Offer {
                    station,
                    hull,
                    holds,
                    at: clock::instant(),
                },
            );
            Ok(())
        }
        Op::Gather { ship_type, mates } => gather(c, m, ship_type, &mates),
        Op::Deploy { key, mode } => deploy(c, m, key, mode),
        Op::Enter { leader } => enter(c, m, leader),
        Op::Gate => gate(c, m),
        Op::Sell => sell(c),
        Op::Stack => stack(c),
    }
}

/// Form a fleet if there is none, and invite the mates; wait for them.
fn gather(c: &mut Conn, m: &mut Memory, ship_type: i64, mates: &[i64]) -> Result<()> {
    let fleet = match c.session().fleet_id() {
        Some(f) => f,
        None => c.create_fleet(ship_type)?,
    };
    for (seat, mate) in mates.iter().enumerate() {
        let squad = c.fleet_seat(fleet, crate::fleet::squad_of(seat))?;
        // A refused invite is the mate's business (it may be in a fleet).
        let _ = c.fleet_invite(fleet, *mate, squad);
    }
    let deadline = clock::instant() + GATHER_WAIT;
    let mut joined = false;
    while clock::instant() < deadline {
        if let Ok(rows) = c.fleet_composition()
            && mates.iter().all(|x| rows.iter().any(|r| r.char_id == *x))
        {
            joined = true;
            break;
        }
        c.pump(Duration::from_millis(500))?;
    }
    if !joined {
        // A key is not spent on a gang that did not assemble: the run is
        // dropped, the keys stay in the hold, the rest is a short one.
        m.abyss.forget_run();
        m.abyss.phase = Phase::Docked;
        m.abyss.retry_after = Some(clock::instant() + GANG_RETRY);
        m.abyss.note = Some(format!("[abyss] fleet {fleet}: {mates:?} did not join"));
        return Ok(());
    }
    m.abyss.note = Some(format!("[abyss] fleet {fleet} formed, invited {mates:?}"));
    Ok(())
}

/// The celestials of the grid a deploy may stand at: planets and belts,
/// nearest first.
fn spots(c: &mut Conn) -> Vec<i64> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    let mut v: Vec<(f64, i64)> = world
        .balls()
        .filter_map(|b| {
            let t = b.slim.and_then(|s| s.type_id)?;
            let g = crate::acquire::with_content(|c| {
                Some(
                    c.try_get::<eve_server::typedata::Installed>()?
                        .table()
                        .get(t)?
                        .group_id,
                )
            })?;
            (g == GROUP_PLANET || g == GROUP_BELT).then(|| {
                (
                    world.surface_distance(ego, b.ball_id).unwrap_or(f64::MAX),
                    b.ball_id,
                )
            })
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, id)| id).collect()
}

/// The end time out of the pushes read while the run was entered.
fn expiry_in(events: &[Event]) -> Option<BlueTime> {
    events.iter().find_map(|e| match e {
        Event::Push(name, args) if name == "OnAddExpiryDurationForAbyssalContent" => {
            match args.first() {
                Some(PyValue::Int(t)) => Some(*t),
                _ => None,
            }
        }
        _ => None,
    })
}

fn expiry_of_frames(frames: &[PyValue]) -> Option<BlueTime> {
    let events: Vec<Event> = frames
        .iter()
        .filter_map(crate::notification)
        .map(|(n, a)| Event::Push(n, a))
        .collect();
    expiry_in(&events)
}

/// A jump took: the session's system changed. Bind there, and look for the
/// pocket's clock in the pushes that follow.
fn landed(c: &mut Conn, m: &mut Memory, from: Option<i64>, frames: &[PyValue]) -> Result<()> {
    let now = c.session().solar_system_id();
    if now.is_none() || now == from {
        return Err(Error::Refused(
            "abyss: no solarsystemid session change".to_owned(),
        ));
    }
    crate::probe::bound(c)?;
    let mut end = expiry_of_frames(frames);
    for _ in 0..3 {
        if end.is_some() {
            break;
        }
        end = expiry_in(&c.pump(Duration::from_millis(400))?);
    }
    m.abyss.run_end = run_end_after(end, m.abyss.run_end, eve_server::pyutil::filetime_now());
    Ok(())
}

/// **The run's end** after a jump: the pushed one, else the one already
/// known (a room gate never resets the clock), else the full length from
/// `now` **[A]** (falsified by a pocket whose clock differs from
/// `abyss::DURATION_S`).
pub fn run_end_after(
    pushed: Option<BlueTime>,
    known: Option<BlueTime>,
    now: BlueTime,
) -> Option<BlueTime> {
    pushed
        .or(known)
        .or_else(|| Some(now + abyss::DURATION_S * 10_000_000))
}

fn deploy(c: &mut Conn, m: &mut Memory, key_type: i64, mode_wire: i64) -> Result<()> {
    let mode = Mode::from_wire(mode_wire)
        .ok_or_else(|| Error::NotSent(format!("abyss: no mode {mode_wire}")))?;
    let item = c
        .cargo()?
        .into_iter()
        .find(|r| r.type_id == key_type && r.quantity >= mode.key_stack())
        .ok_or_else(|| Error::NotSent("abyss: no key in the hold".to_owned()))?;
    let server = server_of(c)?;
    let from = c.session().solar_system_id();
    let all = spots(c);
    let mut tried: BTreeSet<i64> = BTreeSet::new();
    for _ in 0..MAX_SPOTS {
        let spot = next_spot(&tried, &all);
        if let Some(s) = spot {
            tried.insert(s);
            crate::probe::reach(c, s, SPOT_REACH)?;
        }
        let mut outcome = c.call(
            "abyssalMgr",
            "AbyssalEntranceDeployment",
            vec![PyValue::Int(item.item_id), PyValue::Int(mode.wire())],
        )?;
        if outcome.refusal().is_some() {
            let pairs = read_refusal(&outcome.result);
            let proximity = pairs
                .iter()
                .any(|(code, _)| *code == err::PROXIMITY_RESTRICTION);
            if proximity && spot.is_some() && next_spot(&tried, &all).is_some() {
                continue;
            }
            m.abyss.retry_after = Some(clock::instant() + DEPLOY_RETRY);
            m.abyss.forget_run();
            m.abyss.phase = Phase::Docked;
            return Err(Error::Refused(format!(
                "abyss: deploy refused {pairs:?} ({})",
                outcome.refusal().unwrap_or_default()
            )));
        }
        if mode.jumps_on_deploy() {
            c.await_session_change(&mut outcome, JUMP_PATIENCE)?;
            landed(c, m, from, &outcome.notifications)?;
            m.abyss.room = 1;
            m.abyss.phase = Phase::Room;
            m.abyss.note = Some("[abyss] solo run begun".to_owned());
        } else {
            BOARD.open(server, me(c), m.abyss.mates.clone(), clock::instant());
            m.abyss.boss = Some(me(c));
            m.abyss.phase = Phase::Deploy;
            m.abyss.note = Some("[abyss] entrance open".to_owned());
        }
        return Ok(());
    }
    Err(Error::Refused(
        "abyss: no spot left to deploy at".to_owned(),
    ))
}

/// The nearest ball of one of `types` on the grid, waiting for it to show.
fn gate_ball(c: &mut Conn, types: &[i64]) -> Option<(i64, i64)> {
    let deadline = clock::instant() + GATE_WAIT;
    loop {
        let world = c.world_now();
        if let Some(ego) = world.ego
            && let Some(found) = world
                .balls()
                .filter_map(|b| {
                    let t = b.slim.and_then(|s| s.type_id)?;
                    types.contains(&t).then(|| {
                        (
                            world.surface_distance(ego, b.ball_id).unwrap_or(f64::MAX),
                            b.ball_id,
                            t,
                        )
                    })
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        {
            return Some((found.1, found.2));
        }
        if clock::instant() >= deadline || c.pump(Duration::from_millis(250)).is_err() {
            return None;
        }
    }
}

/// Call a gate method and land in what it opens.
fn through(c: &mut Conn, m: &mut Memory, method: &str, gate: i64) -> Result<()> {
    crate::probe::reach(c, gate, GATE_REACH)?;
    let from = c.session().solar_system_id();
    let mut outcome = c.call("abyssalMgr", method, vec![PyValue::Int(gate)])?;
    if let Some(key) = outcome.refusal() {
        let pairs = read_refusal(&outcome.result);
        return Err(Error::Refused(format!("abyss: {method}: {key} {pairs:?}")));
    }
    c.await_session_change(&mut outcome, JUMP_PATIENCE)?;
    landed(c, m, from, &outcome.notifications)
}

fn enter(c: &mut Conn, m: &mut Memory, leader: i64) -> Result<()> {
    let server = server_of(c)?;
    let rest = |m: &mut Memory| m.abyss.retry_after = Some(clock::instant() + GATE_RETRY);
    let Some((gate, _)) = gate_ball(c, &[abyss::ENTRANCE_GATE]) else {
        // Not on this grid yet: a failed try, after a short rest.
        rest(m);
        return Err(Error::NotSent(
            "abyss: no entrance gate on the grid".to_owned(),
        ));
    };
    if let Err(e) = through(c, m, "AbyssalEntranceGateActivation", gate) {
        rest(m);
        return Err(e);
    }
    let who = me(c);
    BOARD.entered(server, leader, who);
    BOARD.at_room(server, leader, who, 1);
    if leader == who {
        // The leader goes in last: the entrance is shut behind it.
        BOARD.seal(server, leader);
    }
    m.abyss.boss = Some(leader);
    m.abyss.room = 1;
    m.abyss.phase = Phase::Room;
    m.abyss.note = Some("[abyss] in the pocket".to_owned());
    Ok(())
}

/// What a gate try says when no room gate shows on the grid.
pub const NO_GATE: &str = "abyss: no room gate on the grid";

/// Whether a `NotSent` line is the no-gate try: the driver does not count it
/// in `failures[Abyss]` (it is a fact about the room, not about the bot; three
/// would send the escort into `Homing::Recovery`, which drops its fight).
pub fn is_no_gate(why: &str) -> bool {
    why == NO_GATE
}

fn gate(c: &mut Conn, m: &mut Memory) -> Result<()> {
    let Some((gate, kind)) = gate_ball(c, &[abyss::ENCOUNTER_GATE, abyss::EXIT_GATE]) else {
        // A try that found no gate is a failed try, not a done one: the
        // retry rests on `gate_after`, and the log says what happened.
        return Err(Error::NotSent(NO_GATE.to_owned()));
    };
    let last = kind == abyss::EXIT_GATE;
    m.abyss.phase = Phase::Gate;
    let method = if last {
        "AbyssalEndGateActivation"
    } else {
        "AbyssalGateActivation"
    };
    let done = through(c, m, method, gate);
    if done.is_err() {
        m.abyss.phase = Phase::Room;
        return done;
    }
    m.abyss.clear_since = None;
    m.abyss.gate_after = None;
    let server = server_of(c)?;
    let boss = m.abyss.boss;
    if last {
        BOARD.enroll(server, me(c), clock::instant());
        match boss {
            Some(b) if b == me(c) => BOARD.close(server, b),
            Some(b) => BOARD.leave(server, b, me(c)),
            None => {}
        }
        m.abyss.win(clock::instant());
        if m.homing.is_none() {
            m.homing = Some(brain::Homing::HoldFull);
        }
        m.abyss.note = Some("[abyss] out of the last room".to_owned());
    } else {
        m.abyss.room = m.abyss.room.saturating_add(1);
        if let Some(b) = boss {
            BOARD.at_room(server, b, me(c), m.abyss.room);
        }
        m.abyss.phase = Phase::Room;
        m.abyss.note = Some(format!("[abyss] room {}", m.abyss.room));
    }
    Ok(())
}

/// **Stack the hangar**: `invCacheContainer.StackAll` on the station hangar,
/// the "Stack All" button. The server does not merge stacks on its own, so
/// keys looted one at a time stay as rows of one key each, and no row holds
/// a co-op stack.
fn stack(c: &mut Conn) -> Result<()> {
    let hangar = c.open_hangar()?;
    c.call_on(&hangar, "StackAll", vec![], vec![])?;
    Ok(())
}

/// Sell the abyssal goods into the bids the builders' floor allows.
fn sell(c: &mut Conn) -> Result<()> {
    let station = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("abyss: sell: not docked".to_owned()))?;
    let server = server_of(c)?;
    BOARD.enroll(server, me(c), clock::instant());
    let prices: BTreeMap<i64, f64> = c.adjusted_prices()?.into_iter().collect();
    let mut lines = Vec::new();
    for row in c.hangar_items()?.iter().filter(|r| is_abyssal_good(r)) {
        let book = c.market_orders(row.type_id)?;
        let Some(limit) = crate::builder::sell_limit(prices.get(&row.type_id).copied(), &book)
        else {
            continue;
        };
        if book.best_bid().is_none_or(|b| b.price < limit) {
            continue;
        }
        lines.push(crate::verbs::SellLine {
            station_id: station,
            type_id: row.type_id,
            item_id: row.item_id,
            price: limit,
            quantity: row.quantity,
        });
    }
    if !lines.is_empty() {
        c.sell(&lines, 0, crate::builder::BROKER_FEE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Invented ids and rows only; the constants appear by name. No `[A]`
    //! number is asserted: tests read kinds, order and the lists.
    use super::*;
    use eve_server::service::ServiceError;

    const FRIGATE_GROUP: i64 = 990_931;
    const CRUISER_GROUP: i64 = 990_932;
    const DESTROYER_GROUP: i64 = 990_933;

    fn lists() -> Lists {
        let by = |g: i64| TypeList {
            inc_groups: vec![g],
            ..TypeList::default()
        };
        Lists::new([
            (Mode::Solo.ship_list(), by(CRUISER_GROUP)),
            (Mode::CoOp.ship_list(), by(FRIGATE_GROUP)),
            (Mode::TwoPlayer.ship_list(), by(DESTROYER_GROUP)),
        ])
    }

    fn hull(group: i64) -> Hull {
        Hull {
            type_id: 990_900 + group % 10,
            group_id: group,
            category_id: 6,
        }
    }

    fn key(tier: i64, weather: i64, qty: i64, type_id: i64) -> KeyRow {
        KeyRow {
            item_id: 100_000 + type_id,
            type_id,
            tier,
            weather,
            qty,
        }
    }

    const NO_WEATHER: i64 = 990_950;

    #[test]
    fn the_mode_follows_the_gangs_hulls_and_its_key_stack() {
        let l = lists();
        let f = hull(FRIGATE_GROUP);
        let c = hull(CRUISER_GROUP);
        let d = hull(DESTROYER_GROUP);
        assert_eq!(mode_for(&[c], 1, &l), Some(Mode::Solo));
        assert_eq!(mode_for(&[f, f, f], 3, &l), Some(Mode::CoOp));
        assert_eq!(mode_for(&[d, d], 2, &l), Some(Mode::TwoPlayer));
        // The stack is short, the hull is on another list, or the gang has
        // a size no mode has.
        assert_eq!(mode_for(&[f, f, f], 2, &l), None);
        assert_eq!(mode_for(&[f, f, c], 3, &l), None);
        assert_eq!(mode_for(&[c, c], 2, &l), None);
        assert_eq!(mode_for(&[f, f, f, f], 9, &l), None);
        assert_eq!(mode_for(&[], 9, &l), None);
    }

    fn offer(hull: Hull, holds: i64, at: Instant) -> Offer {
        Offer {
            station: 1,
            hull,
            holds,
            at,
        }
    }

    #[test]
    fn a_leader_plans_the_biggest_gang_its_keys_pay_for() {
        let l = lists();
        let now = clock::instant();
        let f = hull(FRIGATE_GROUP);
        let ctx = Ctx {
            max_tier: 200,
            pirate: false,
            tank: BOT_TANK,
            losses: 0,
        };
        let others = [(11, offer(f, 0, now)), (12, offer(f, 0, now))];
        let keys = [key(0, NO_WEATHER, 3, 7)];
        let p = plan(&f, &keys, &others, &l, ctx);
        assert_eq!(
            p.map(|p| (p.mode, p.mates)),
            Some((Mode::CoOp, vec![11, 12]))
        );
        // Two keys: a pair is not on the frigates' list, so no gang.
        let two = [key(0, NO_WEATHER, 2, 7)];
        assert_eq!(plan(&f, &two, &others, &l, ctx), None);
        // The lowest id of the key-holders leads.
        assert!(leads(10, 3, &[(11, offer(f, 3, now))]));
        assert!(!leads(12, 3, &[(11, offer(f, 3, now))]));
        assert!(!leads(10, 0, &[]));
    }

    #[test]
    fn the_key_never_exceeds_the_systems_tier_but_for_a_pirate() {
        let keys = [key(3, NO_WEATHER, 3, 7)];
        let tank = BOT_TANK;
        // Cap allows it; the system's limit does not.
        assert_eq!(pick_key(&keys, 3, 6, 2, false, tank), None);
        // A pirate takes it past the limit...
        assert_eq!(
            pick_key(&keys, 3, 6, 2, true, tank).map(|k| k.type_id),
            Some(7)
        );
        // ...but not into a system the server refuses every tier in.
        assert_eq!(pick_key(&keys, 3, 6, 0, true, tank), None);
        // The gang's own cap holds for a pirate too.
        assert_eq!(pick_key(&keys, 3, 1, 200, true, tank), None);
        // A demo key (the special tier 0 id) is no suspect anywhere.
        let demo = [key(abyss::TIER_0_DEMO, NO_WEATHER, 3, 8)];
        assert_eq!(
            pick_key(&demo, 3, 0, 0, false, tank).map(|k| k.type_id),
            Some(8)
        );
    }

    #[test]
    fn a_key_is_picked_low_tier_first_and_shy_of_a_weather_that_hits_the_tank() {
        let hit = WEATHERS
            .iter()
            .find(|w| w.penalty == 99)
            .map(|w| w.beacon)
            .expect("a kinetic-penalty weather");
        let keys = [
            key(0, hit, 3, 7),
            key(1, NO_WEATHER, 3, 8),
            key(1, NO_WEATHER, 3, 6),
        ];
        // A tank of the test's own (the bots' tank is an [A], not asserted).
        let tank = Tank {
            shield: true,
            turrets: false,
        };
        let k = pick_key(&keys, 3, 1, 200, false, tank).expect("a key");
        assert_eq!(
            k.type_id, 6,
            "the weather that hits is passed over, then the type order"
        );
        // With nothing else, the weather is still played.
        let k = pick_key(&keys[..1], 3, 1, 200, false, tank).expect("a key");
        assert_eq!(k.type_id, 7);
        // A short stack is no key.
        assert_eq!(pick_key(&keys, 4, 1, 200, false, tank), None);
    }

    #[test]
    fn a_proximity_refusal_moves_to_the_next_spot() {
        let spots = [41, 42, 43];
        let mut tried = BTreeSet::new();
        assert_eq!(next_spot(&tried, &spots), Some(41));
        tried.insert(41);
        assert_eq!(next_spot(&tried, &spots), Some(42));
        tried.extend([42, 43]);
        assert_eq!(next_spot(&tried, &spots), None);
        assert_eq!(next_spot(&BTreeSet::new(), &[]), None);
        // The code the walk reacts to is the server's own.
        let pairs = [(err::PROXIMITY_RESTRICTION, vec![990_001])];
        assert!(pairs.iter().any(|(c, _)| *c == err::PROXIMITY_RESTRICTION));
    }

    #[test]
    fn the_refusal_pairs_read_back() {
        let errors = vec![
            (err::PROXIMITY_RESTRICTION, vec![990_001]),
            (err::PVP_TIMER_ACTIVE, vec![]),
            (err::INVALID_KEY_AMOUNT, vec![3]),
        ];
        let ServiceError::Raise(v) = abyss::abyss_error(&errors) else {
            panic!("the server raises it");
        };
        assert_eq!(refusal_pairs(&v), errors);
        assert!(refusal_pairs(&PyValue::None).is_empty());
        let other = eve_proto::user_error("CustomNotify", Vec::new());
        assert!(refusal_pairs(&other).is_empty());
    }

    #[test]
    fn pace_skips_the_loot_when_the_clock_is_short() {
        let s = 10_000_000_i64;
        let now = 1_000 * s;
        // Plenty left for the rooms ahead (the reserve of each, and a
        // second): fight on. Nothing left: skip.
        let plenty = ROOM_RESERVE.as_secs() as i64 * 3 + 1;
        assert_eq!(pace(now, now + plenty * s, 2), Pace::Fight);
        assert_eq!(pace(now, now + 5, 2), Pace::SkipLoot);
        assert_eq!(pace(now, now - 5, 0), Pace::SkipLoot);
        // More rooms ahead never make the same clock look roomier.
        let end = now + 700 * s;
        let few = pace(now, end, 1) == Pace::SkipLoot;
        let many = pace(now, end, 3) == Pace::SkipLoot;
        assert!(many || !few, "{few} {many}");
    }

    #[test]
    fn a_loss_lowers_the_next_tier() {
        let f = hull(FRIGATE_GROUP);
        let _ = f;
        assert!(lowered(3, 1) < 3);
        assert!(lowered(3, 2) <= lowered(3, 1));
        assert_eq!(lowered(0, 5), 0, "never below tier 0");
        let now = clock::instant();
        let mut s = State {
            phase: Phase::Room,
            ..State::default()
        };
        s.lose(now);
        assert!(s.losses > 0, "a loss is counted");
        assert_eq!(s.phase, Phase::Docked);
        let after_loss = s.losses;
        s.win(now);
        assert!(s.losses < after_loss, "a win forgives a loss");
        assert_eq!(s.phase, Phase::Home);
        assert!(s.sell_due);
    }

    #[test]
    fn the_board_hands_out_fresh_offers_of_others_oldest_first() {
        let board = Board::new();
        let server: SocketAddr = "127.0.0.1:1".parse().expect("literal");
        let now = clock::instant();
        let f = hull(FRIGATE_GROUP);
        board.offer(server, 12, offer(f, 0, now));
        board.offer(server, 11, offer(f, 3, now));
        board.offer(
            server,
            13,
            Offer {
                station: 2,
                ..offer(f, 0, now)
            },
        );
        let seen: Vec<i64> = board
            .others_at(server, 1, 10, now)
            .into_iter()
            .map(|(w, _)| w)
            .collect();
        assert_eq!(seen, vec![11, 12]);
        board.withdraw(server, 11);
        assert_eq!(board.others_at(server, 1, 10, now).len(), 1);
        assert!(board.others_at(server, 1, 10, now + OFFER_TTL).is_empty());
        board.open(server, 10, vec![11, 12], now);
        board.entered(server, 10, 11);
        let g = board.gang(server, 10).expect("open");
        assert!(g.entered.contains(&11) && !g.entered.contains(&12));
        board.close(server, 10);
        assert!(board.gang(server, 10).is_none());
    }
    #[test]
    fn a_gang_takes_each_gate_together() {
        let server: SocketAddr = "127.0.0.1:2".parse().expect("literal");
        let now = clock::instant();
        let board = Board::new();
        let none = Duration::ZERO;
        // No record (solo, or closed): everyone goes.
        assert!(may_take_gate(None, 10, 10, 1, none, false));
        board.open(server, 10, vec![11, 12], now);
        board.at_room(server, 10, 10, 1);
        board.at_room(server, 10, 11, 1);
        board.at_room(server, 10, 12, 1);
        let g = board.gang(server, 10);
        // The leader still outside with the entrance open: the mates wait
        // for it, however long (until a short clock, the seal or the close).
        let mut outside = board.gang(server, 10).expect("open");
        outside.leader_room = 0;
        outside.opened = Some(now);
        assert!(!may_take_gate(
            Some(&outside),
            11,
            10,
            1,
            COMPANY_WAIT,
            false
        ));
        assert!(may_take_gate(Some(&outside), 11, 10, 1, none, true));
        // A leader that never came: after the entrance wait and the company
        // wait the mates go on.
        let later = now + ENTRANCE_WAIT + COMPANY_WAIT + Duration::from_secs(1);
        assert!(!gate_open_for(Some(&outside), 11, 10, 1, none, false, now));
        assert!(gate_open_for(Some(&outside), 11, 10, 1, none, false, later));
        outside.opened = None;
        assert!(may_take_gate(
            Some(&outside),
            11,
            10,
            1,
            COMPANY_WAIT,
            false
        ));
        // The mates wait for the leader; the leader goes, all mates being in.
        assert!(!may_take_gate(g.as_ref(), 11, 10, 1, none, false));
        assert!(may_take_gate(g.as_ref(), 10, 10, 1, none, false));
        board.at_room(server, 10, 10, 2);
        let g = board.gang(server, 10);
        assert!(may_take_gate(g.as_ref(), 11, 10, 1, none, false));
        // A mate behind holds the leader back until it is in, dies, or the
        // wait or the clock runs out.
        assert!(!may_take_gate(g.as_ref(), 10, 10, 2, none, false));
        assert!(may_take_gate(g.as_ref(), 10, 10, 2, COMPANY_WAIT, false));
        assert!(may_take_gate(g.as_ref(), 10, 10, 2, none, true));
        board.leave(server, 10, 11);
        board.at_room(server, 10, 12, 2);
        let g = board.gang(server, 10);
        assert!(may_take_gate(g.as_ref(), 10, 10, 2, none, false));
        // The entrance shuts once the leader is in; the record stays.
        board.seal(server, 10);
        let g = board.gang(server, 10).expect("kept");
        assert!(g.opened.is_none());
    }

    #[test]
    fn a_known_end_time_survives_a_missed_push() {
        let now = 5_000_000_000_i64;
        assert_eq!(run_end_after(Some(7), Some(9), now), Some(7));
        assert_eq!(run_end_after(None, Some(9), now), Some(9));
        let fresh = run_end_after(None, None, now).expect("a fallback");
        assert!(fresh > now);
    }

    #[test]
    fn the_roster_counts_runners_seen_lately() {
        let board = Board::new();
        let server: SocketAddr = "127.0.0.1:3".parse().expect("literal");
        let now = clock::instant();
        assert!(!board.has_runners(Some(server), now));
        board.enroll(server, 5, now);
        assert!(board.has_runners(Some(server), now));
        assert!(board.has_runners(None, now));
        let other: SocketAddr = "127.0.0.1:4".parse().expect("literal");
        assert!(!board.has_runners(Some(other), now));
        assert!(!board.has_runners(Some(server), now + ROSTER_TTL));
    }

    fn gang_of(mates: &[i64], entered: &[i64]) -> Gang {
        Gang {
            mates: mates.to_vec(),
            entered: entered.iter().copied().collect(),
            ..Gang::default()
        }
    }

    #[test]
    fn a_threatened_leader_at_the_entrance_goes_in_with_a_mate_inside_else_abandons() {
        let none = Duration::ZERO;
        // Nothing wrong, mates missing: wait.
        assert_eq!(
            entrance_verdict(&gang_of(&[11, 12], &[11]), none, false, true),
            Entrance::Wait
        );
        // Threatened with a mate inside: in at once, the other mate missing.
        assert_eq!(
            entrance_verdict(&gang_of(&[11, 12], &[11]), none, true, true),
            Entrance::Enter { all_in: false }
        );
        // Threatened with nobody inside: the gang is abandoned, whatever the
        // key's tier is known to be.
        assert_eq!(
            entrance_verdict(&gang_of(&[11, 12], &[]), none, true, false),
            Entrance::Abandon
        );
        // All in: in, threatened or not.
        assert_eq!(
            entrance_verdict(&gang_of(&[11], &[11]), none, false, true),
            Entrance::Enter { all_in: true }
        );
        // The wait run out: with nobody inside the gang is abandoned (a key
        // tier known), with a mate inside the leader goes in short.
        assert_eq!(
            entrance_verdict(&gang_of(&[11], &[]), ENTRANCE_WAIT, false, true),
            Entrance::Abandon
        );
        assert_eq!(
            entrance_verdict(&gang_of(&[11, 12], &[12]), ENTRANCE_WAIT, false, true),
            Entrance::Enter { all_in: false }
        );
    }

    #[test]
    fn a_leader_who_goes_in_short_books_one_loss_a_win_forgives() {
        let now = clock::instant();
        let mut s = State::default();
        assert!(s.book_short_gang());
        assert_eq!(s.losses, 1);
        // The act is retried turn after turn: booked once.
        assert!(!s.book_short_gang());
        assert_eq!(s.losses, 1);
        // The next run books again; the run's win forgives its loss.
        s.forget_run();
        assert!(s.book_short_gang());
        assert_eq!(s.losses, 2);
        s.win(now);
        assert_eq!(s.losses, 1);
        assert!(!s.short_booked);
    }

    #[test]
    fn a_short_run_that_then_dies_is_one_loss_not_two() {
        let now = clock::instant();
        let mut s = State::default();
        assert!(s.book_short_gang());
        s.lose(now);
        assert_eq!(s.losses, 1);
        // A run that was not short books its own loss.
        s.lose(now);
        assert_eq!(s.losses, 2);
    }

    #[test]
    fn only_the_no_gate_line_is_the_no_gate_outcome() {
        assert!(is_no_gate(NO_GATE));
        assert!(!is_no_gate("abyss: no key in the hold"));
    }

    #[test]
    fn looted_key_rows_of_one_type_are_stacked_at_the_dock() {
        let row = |item_id: i64, type_id: i64, group_id: i64, singleton: bool| crate::ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id,
            category_id: 4,
            singleton,
        };
        let key = abyss::KEY_GROUP;
        assert!(!needs_stack(&[]));
        assert!(!needs_stack(&[row(1, 50, key, false)]));
        assert!(needs_stack(&[
            row(1, 50, key, false),
            row(2, 50, key, false)
        ]));
        // Two kinds of key, one row each; two rows of something else; two
        // singletons: nothing a stack would merge into a key stack.
        assert!(!needs_stack(&[
            row(1, 50, key, false),
            row(2, 51, key, false)
        ]));
        assert!(!needs_stack(&[row(1, 60, 7, false), row(2, 60, 7, false)]));
        assert!(!needs_stack(&[
            row(1, 50, key, true),
            row(2, 50, key, true)
        ]));
    }

    #[test]
    fn the_board_purges_stale_offers_and_gangs() {
        let server: SocketAddr = "127.0.0.1:5".parse().expect("literal");
        let t0 = clock::instant();
        let board = Board::new();
        let f = Hull::of(990_001);
        board.offer(server, 11, offer(f, 1, t0));
        board.open(server, 10, vec![11], t0);
        board.open(server, 20, vec![21], t0 + GANG_TTL);
        // The second open purged the first gang (a leader long gone) and the
        // stale offer.
        assert_eq!(board.sizes(), (0, 1));
        assert!(board.gang(server, 10).is_none());
        assert!(board.gang(server, 20).is_some());
        // A fresh offer; the live gang stays.
        board.offer(server, 12, offer(f, 0, t0 + OFFER_TTL));
        assert_eq!(board.sizes(), (1, 1));
        assert_eq!(board.others_at(server, 1, 99, t0 + OFFER_TTL).len(), 1);
        // A gang inside its life survives a purge; past it, it goes.
        board.purge(t0 + GANG_TTL + GANG_TTL - Duration::from_secs(1));
        assert_eq!(board.sizes().1, 1);
        board.purge(t0 + GANG_TTL + GANG_TTL);
        assert_eq!(board.sizes(), (0, 0));
    }

    /// A leader at its open entrance, in space, for [`k_space`]: the board's
    /// record is made here (a unique server address per test, the board
    /// being process-wide).
    fn at_the_gate(
        port: u16,
        mates: &[i64],
        entered: &[i64],
        hostile: bool,
    ) -> (Memory, Observation, brain::ParkView, SocketAddr) {
        let server: SocketAddr = format!("127.0.0.1:{port}").parse().expect("literal");
        let now = clock::instant();
        let me = 70_001;
        BOARD.open(server, me, mates.to_vec(), now);
        for who in entered {
            BOARD.entered(server, me, *who);
        }
        let ball = |id: i64| brain::Ball {
            id,
            position: [0.0; 3],
            radius: 10.0,
        };
        let park = brain::ParkView {
            ego: ball(1),
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: if hostile {
                vec![brain::Hostile {
                    ball: ball(2),
                    type_id: 990_999,
                    name: "Invented Raider".to_owned(),
                    shooting: Some(1),
                }]
            } else {
                vec![]
            },
            wrecks: vec![],
            ego_tether: None,
        };
        let mut o = Observation::empty(now);
        o.me = me;
        o.place = Place::InSpace {
            system: 1,
            bound: true,
        };
        o.park = Some(park.clone());
        let mut m = Memory::new("Lead", brain::Policy::fast());
        m.abyss.server = Some(server);
        m.abyss.phase = Phase::Deploy;
        m.abyss.boss = Some(me);
        m.abyss.key_tier = 1;
        (m, o, park, server)
    }

    #[test]
    fn three_refused_gate_tries_in_a_room_do_not_silence_the_guns() {
        let (mut m, mut o, _park, _server) = at_the_gate(61_266, &[71], &[71], true);
        o.place = Place::InSpace {
            system: VOID_SYSTEMS.start,
            bound: true,
        };
        m.abyss.phase = Phase::Room;
        let engages = |a: &[Act]| a.iter().any(|x| matches!(x, Act::Orbit { id: 2, .. }));
        m.policy.role = brain::Role::Escort;
        o.guns = vec![brain::GunInfo {
            item_id: 501,
            type_id: 502,
            flag_id: 27,
            online: true,
            active: false,
            target: None,
            rounds: Some(100),
            capacity: 200,
            range: 3_000.0,
            reload: Duration::from_secs(4),
            kind: brain::GunKind::Weapon,
            reach: 0.0,
        }];
        o.lock_range = 20_000.0;
        // The first pass notices the hostile; the next one acts on it.
        let _ = in_space(&mut m, &o);
        let quiet = in_space(&mut m, &o);
        assert!(engages(&quiet), "{quiet:?}");
        o.failures.insert(ActKind::Abyss, GIVE_UP_AFTER);
        let acts = in_space(&mut m, &o);
        assert!(engages(&acts), "{acts:?}");
        assert_eq!(m.homing, None);
        // Another kind of failure still recovers.
        o.failures.clear();
        o.failures.insert(ActKind::Lock, GIVE_UP_AFTER);
        let acts = in_space(&mut m, &o);
        assert!(!engages(&acts), "{acts:?}");
    }

    #[test]
    fn a_leader_attacked_at_the_entrance_goes_in_when_a_mate_is_inside() {
        let (mut m, o, park, server) = at_the_gate(61_247, &[71, 72], &[71], true);
        let acts = k_space(&mut m, &o, &park).expect("the leader acts");
        assert_eq!(acts, vec![Act::Abyss(Op::Enter { leader: o.me })]);
        // Entering short is a loss, booked once.
        assert_eq!(m.abyss.losses, 1);
        let again = k_space(&mut m, &o, &park).expect("the leader acts");
        assert_eq!(again, acts);
        assert_eq!(m.abyss.losses, 1);
        assert!(BOARD.gang(server, o.me).is_some());
        BOARD.close(server, o.me);
    }

    #[test]
    fn a_leader_attacked_at_the_entrance_with_nobody_inside_closes_the_gang() {
        let (mut m, o, park, server) = at_the_gate(61_248, &[71, 72], &[], true);
        // Hostiles on the grid: the gang is closed, the loss booked, the turn
        // handed to the site runner (nothing returned).
        assert!(k_space(&mut m, &o, &park).is_none());
        assert_eq!(m.abyss.phase, Phase::Docked);
        assert_eq!(m.abyss.losses, 1);
        assert!(BOARD.gang(server, o.me).is_none());
        // A homing does the same, with no hostile in sight.
        let (mut m, o, park, server) = at_the_gate(61_249, &[71], &[], false);
        m.homing = Some(brain::Homing::Recovery);
        assert!(k_space(&mut m, &o, &park).is_none());
        assert_eq!(m.abyss.phase, Phase::Docked);
        assert!(BOARD.gang(server, o.me).is_none());
    }

    #[test]
    fn giving_up_at_the_gate_forgets_the_run_so_a_short_booking_does_not_leak() {
        let (mut m, mut o, park, server) = at_the_gate(61_252, &[71, 72], &[71], true);
        o.failures.insert(ActKind::Abyss, GIVE_UP_AFTER);
        assert!(k_space(&mut m, &o, &park).is_none());
        assert_eq!(m.abyss.phase, Phase::Docked);
        assert!(!m.abyss.short_booked);
        assert!(m.abyss.mates.is_empty());
        // Booked short once on the way out, and counted.
        assert_eq!(m.abyss.losses, 1);
        assert!(BOARD.gang(server, o.me).is_none());
    }

    #[test]
    fn a_passive_rat_on_the_grid_does_not_end_the_leaders_wait() {
        let (mut m, o, mut park, server) = at_the_gate(61_251, &[71, 72], &[71], true);
        // The same raider, its gun on nobody, and one on another ship.
        park.hostiles[0].shooting = None;
        let mut other = park.hostiles[0].clone();
        other.shooting = Some(park.ego.id + 100);
        park.hostiles.push(other);
        let acts = k_space(&mut m, &o, &park).expect("the leader acts");
        assert_eq!(acts, vec![Act::Wait(brain::SHORT_WAIT)]);
        assert_eq!(m.abyss.phase, Phase::Deploy);
        assert_eq!(m.abyss.losses, 0);
        assert!(BOARD.gang(server, o.me).is_some());
        BOARD.close(server, o.me);
    }

    #[test]
    fn a_quiet_leader_waits_for_its_mates_at_the_entrance() {
        let (mut m, o, park, server) = at_the_gate(61_250, &[71, 72], &[71], false);
        let acts = k_space(&mut m, &o, &park).expect("the leader acts");
        assert_eq!(acts, vec![Act::Wait(brain::SHORT_WAIT)]);
        assert_eq!(m.abyss.phase, Phase::Deploy);
        assert_eq!(m.abyss.losses, 0);
        BOARD.close(server, o.me);
    }
}
