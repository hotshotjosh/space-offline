//! **The abyssal instance** (issue #92, lane abyss-1; the probe is #91): a
//! filament in the cargo takes a pilot (or up to three in a fleet) through
//! rooms of a pocket to an exit gate, against a clock that kills a ship that
//! stays too long.
//!
//! ## The client's flow **[V]** (`abyss/client/abyssal_service.py`,
//! `abyss/common/validator.py`, `abyss/game_mode.py`, `abyss/error.py`)
//!
//! - **Deploy.** The key's menu opens the window; `DeployActivationController`
//!   calls `abyssalMgr.AbyssalEntranceDeployment(keyItemID, gameModeID)`. A
//!   solo key runs it inside `PerformSessionChange('JumpToAbyss', ...)` (the
//!   pilot jumps), a co-op or two-player key does not: the call puts an
//!   **entrance gate** (`typeAbyssEntranceGate`) next to the ship and each
//!   pilot, the deployer too, calls `AbyssalEntranceGateActivation(gateID)`
//!   in a `JumpToAbyss` session change. The gate's slim carries
//!   `abyssFilamentTypeID`, `gameModeID` and `isAbyssGateOpen`
//!   ([`gate_pairs`]).
//! - **Between rooms.** `AbyssalGateActivation(gateID)` (type
//!   `typeAbyssEncounterGate`, `abyssToAbyssJump`, `violateSafetyTimer`) and,
//!   in the last room, `AbyssalEndGateActivation(gateID)` (type
//!   `typeAbyssExitGate`, `abyssToKnownJump`) are session changes.
//! - **Refusals** are `UserError('AbyssError', {'errors': [(code, args)]})`
//!   ([`abyss_error`]); the codes are `abyss/error.py`'s ([`err`]) and the
//!   client shows the label of each.
//! - **Pushes**: `OnAbyssalLocationEntered`, `OnAddExpiryDurationForAbyssalContent`
//!   and `OnAbyssalContentFinished` ([`Entered`], [`Expiry`], [`Finished`]).
//!
//! ## The rules the server checks **[V]**
//!
//! [`deploy_errors`] and [`jump_errors`] are `BaseValidator.validate_deploy`
//! and `validate_jump` in the client's order, over [`Facts`] read from the
//! world ([`facts`]): the ship by typelist (57 solo, 92 co-op, 233 two-player
//! in `typelists.tsv`), the key's group, owner, reach and stack (1, 3 or 2),
//! the fleet, the system's security against the key's tier
//! ([`max_tier`]), docked, cloaked, invulnerable, PvP timer, self-destruct,
//! tether, warp, scramble, already in the abyss, a jump in progress, and a
//! restricted celestial within 1000 km (typelist 50, [`closest_restricted`]).
//!
//! ## The run **[A]**
//!
//! A [`Run`] is the pocket of one filament: its pilots, its [`Pocket`]s (one
//! per room, each its own abyssal system and site) and its clock.
//!
//! - **Three rooms** ([`ROOMS`] **[V-data]**: the objective chain's default
//!   `num_rooms`, no setter). The first two end in an
//!   encounter gate, the last in the exit gate, both on the room's exit
//!   locator.
//! - **The gate is placed when the room is cleared** **[V-data]**: no NPC of
//!   typelist [`TARGETS_LIST`] (the client's `AbyssTargets`: swarmers and
//!   caches are outside it) tagged to the room's site is alive
//!   ([`room_clear`]). ABYSS-2 spawns the waves
//!   ([`seed_room`] is its seam); until then a room has none and its gate
//!   stands from the start.
//! - **A pilot moves on alone**: `AbyssalGateActivation` takes the caller to
//!   the next pocket, which is made for the first of them and shared by the
//!   rest; the one before closes when nobody is in it. (The client moves only
//!   the caller's session.)
//! - **The clock** is [`DURATION_S`] from the first pilot in. At the end the
//!   ship **and the pod die** ([`kill_pilot`]), as on TQ: the death is
//!   `combat::self_destruct_kill` (the ship, then the pod [`POD_DELAY_S`] later), with the pilot as the cause, so
//!   insurance does what it does for any kill by the owner (no change here).
//! - **A pilot who is gone** (no session, no ship in space) leaves the run
//!   alone, and **the origin is a logoff spot**: when a pilot joins, the
//!   spot they left from is written as their `logoffspot` row, and
//!   `logoff_spot::record` leaves it alone while they are in a run. After a
//!   restart, or a logoff in the pocket, the next login puts the ship at its
//!   origin, unharmed. Nothing else is persisted.
//!   (A pod waiting out [`POD_DELAY_S`] is written as the pilot's `logoffspot`
//!   row, in the void system, for as long as it waits. A restart in those
//!   seconds finds the row and [`restore_sweep`] puts the pod back in the
//!   roster, due now, so the next tick kills it as the live rule would
//!   (#243). **[A]** kill, not revive: the collapse had already killed the
//!   ship, and the pod's death follows it on TQ. Falsified by a retail
//!   capture of a pilot who logs in after a server restart inside a collapse
//!   and is let live.
//!   **[I]** A pilot mid-jump out of the pocket at the collapse is let off
//!   live while the server runs ([`kill_due_pods`] sees an ordinary system),
//!   but killed if the server restarts in that window: the row still names
//!   the void system. A rare overlap of two windows of seconds; closing it
//!   wants the pod's store location, which a mid-jump pod does not yet have
//!   changed.)
//! - **The co-op gate** stays [`UNENTERED_GATE_LIFETIME_S`] **[V]**; a key
//!   whose gate was never used is lost with it, as on TQ.
//!
//! ## The waves **[A]** (ABYSS-2)
//!
//! The client ships no spawn data for the abyssal rooms (`spawns.tsv` has no
//! row of archetype 51 **[V-data]**), so what a room holds is
//! `content/npc/abyss-waves.tsv`: per played tier ([`play_tier`]) and room,
//! one or more waves of `type_id` x `count`. The first wave is spawned when
//! the room is made ([`seed_room`]); when no target is alive and a wave is
//! left, the stage spawns the next one ([`spawn_wave`]); the gate comes when
//! the last is dead. NPCs are tagged to the room's site (`site::tag`) and
//! stand on a ring round the entry ([`wave_spot`]). They are derived from the
//! type data alone ([`crate::npc::resolve_law`]); a type the bundle lacks is
//! skipped, a wave with none in space is spent, so a bundle without them has
//! rooms that are clear at once.
//!
//! ## The loot **[A]** (ABYSS-3, issue #5; `abyss/loot.rs`)
//!
//! Each room holds one Triglavian Bioadaptive Cache and the nodes its dungeon
//! lists, NPCs of group 2009 that fight nothing and are outside the gate's
//! target list ([`loot::place`] from [`seed_room`]). A cache that dies leaves
//! its own wreck, filled from the authored table
//! `content/pve/abyss-loot.tsv` by the economy module's kill-hook row
//! (`loot::on_npc_kill`). Wave NPCs get their wrecks too ([`loot::kind_for`]).
//! `/abyss clear` leaves caches and nodes standing. A closed pocket takes its
//! wrecks with it, since a closed pocket is never entered again and a system
//! with a wreck is not reused. The `[A]`s and their falsifiers are in
//! `abyss/loot.rs`'s header.
//!
//! ## The weather **[V]/[A]** (ABYSS-4, issue #6; `abyss/weather.rs`)
//!
//! The key's `weatherID` is a dbuff pair, held as continuous buffs on every
//! ship in an open room (`dbuff::hold`, source the room's site id, every tick)
//! and released when the ship is in another room or the room closes. The
//! stage of a room (1 to 3) is drawn per (run, room) from
//! `content/pve/abyss-weather.tsv` **[A]**; a beacon and the stage's visual
//! ball stand in the room. NPCs are not weathered **[A]**. The `[V]`s, `[I]`s
//! and `[A]`s with their falsifiers are in `abyss/weather.rs`'s header.
//!
//! ## The suspect flag and the trace **[V]/[I]/[A]** (ABYSS-5, issue #62; `abyss/trace.rs`)
//!
//! A deploy with a key above the system's limit ([`Facts::suspicious`]) that
//! the pilot's safety allowed makes the **deployer** a suspect
//! (`crimewatch::suspect`) **[I]**: the client's jump checks never test
//! safety (`validator.py:219-230`), so an entrant through a fleet's gate is
//! never flagged. The run leaves a trace (`typeKnownToAbyssalBeacon`, or the
//! `Suspect` type for such a key) where the pilot left: a solo run at the
//! origin when the pilot goes in, a fleet run where the entrance gate stood
//! once it goes. The departure effect is the client's own, naming the trace
//! (solo), the entrance gate (fleet) or the room's gate (between rooms)
//! ([`crate::jump_drive::depart_abyssal`]). Gaps in the validators: the
//! client's void-space band ([`err::VOID_SPACE_DISALLOWED`]), Zarzakh
//! ([`err::IN_BANNED_SYSTEM`]) and a gate used after the clock ran out
//! ([`err::POCKET_COLLAPSING`]). `SHIP_IN_FORCE_FIELD` (13) and
//! `CONGESTION_BAN` (33) stay unraised: this server has no force fields and
//! no congestion rule. `/abyss expire <seconds>` moves the clock (a GM tool
//! for the collapse's scenario).
//!
//! ## Not here (seams)
//!
//! The objective chain's context, and bots (filaments come from drops, bots
//! never enter).
//!
//! ## The room and the system **[V-data]**
//!
//! As in the probe: archetype [`ARCHETYPE`] dungeons of one room with an
//! entry object and an [`EXIT_LOCATOR`]; `32xxxxxx` systems with no
//! celestials ([`free_system`]).
//!
//! ## The [A]s (each with its falsifier)
//!
//! - **The waves**: the table, the ring, the owner [`WAVE_OWNER`] and the
//!   next wave coming in the same call as the last one's death (no pause
//!   between waves) are all invented. Falsified by the
//!   client's own abyssal room data (a dungeon with spawn rows of archetype
//!   51), or by a pilot who finds the rooms too easy or too hard for their
//!   tier. The filament's weather does not change the waves.
//!
//! - **The arrival copes with a system with no celestials**: green in
//!   `fci_abyss_enter`.
//! - **The nebula** by tier ([`nebula`]); a tier above 6 is 1001 (tier 5
//!   shipyard) or 1002 (tier 0 demo) and is played as 5 or 0 ([`play_tier`]).
//!   Falsified by a scene that does not suit its tier.
//! - **`content_id` is the first room's site id** for the whole run: the
//!   client only compares it. Falsified by a client path that looks it up.
//! - **`OnAbyssalContentFinished`'s `timer_type`** is
//!   [`TIMER_ABYSSAL_CONTENT_EXPIRATION`] **[V]**: the client deletes that
//!   timer, so a 0 (the Weapons timer) cleared the wrong one.
//! - **Departure FX**: the way in and between rooms is the client's own
//!   abyssal effect **[V]**; **the way home keeps the jump drive's** **[A]**
//!   (no abyssal effect for the way out is known). Falsified by a capture or
//!   a client path naming one.
//! - **The trace's lifetime** is the run's **[A]**: until the run closes.
//!   Falsified by a TQ trace that stays after the last pilot is home, or goes
//!   sooner.
//! - **`POCKET_COLLAPSING` from a gate at the end of the clock** **[A]**: the
//!   client has the code and no caller in its own code. Falsified by a TQ gate
//!   that still jumps at 0:00.
//! - **A gate's range**: the entrance gate is checked at
//!   [`MAX_GATE_DISTANCE`] plus the gate's radius plus [`SHIP_SLACK`]; the
//!   encounter and exit gates are not (the client's menu is). Falsified by a
//!   legitimate jump refused, or a modified client that jumps from across a
//!   room.
//! - **Three rooms, 20 minutes, gate lifetime 1800 s**: the last two are
//!   `DEFAULT_ABYSS_CONTENT_DURATION` and `UNENTERED_GATE_LIFETIME` **[V]**.
//! - **The proximity check sees celestials and Upwell structures** only.
//!   Falsified by a restricted thing that is neither.
//! - **A typelist the bundle lacks admits any ship.** Falsified by a bundle
//!   without lists 57, 92 or 233 on a live server.
//! - **The origin is where the pilot stood** when the key was used (solo) or
//!   the gate was entered (co-op).

use crate::celestial::{Catalogue, Universe, VOID_SYSTEMS};
use crate::damage::BlueTime;
use crate::dungeon_data::{Dungeon, Entry, Table};
use crate::item::Edit;
use crate::modules::user_error;
use crate::population::{self, Site};
use crate::service::args::Args;
use crate::service::{Registry, Request, ServiceError};
use crate::site::{self, Sites};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub mod loot;
pub mod trace;
pub mod weather;

/// The services this module answers (`service::DOMAINS`, coordinator-only).
pub const SERVICES: &[&str] = &["abyssalMgr"];

/// The registration row in `world_ext::SLOTS` (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "abyss",
    make,
};

/// The dungeon archetype of the abyssal rooms **[V-data]** (`archetypes.tsv`).
pub const ARCHETYPE: i64 = 51;
/// `exit_locator` **[V-data]** (group 1973, `Locators`): where the room's gate
/// goes. The entry locator needs no id: it is the dungeon's `entryObjectID`.
pub const EXIT_LOCATOR: i64 = 47464;
/// The area-of-effect locators (group 1973) **[V-data]**: the PvE rooms have
/// at least one, the arenas and construction sites none.
pub const AOE_LOCATORS: std::ops::RangeInclusive<i64> = 47460..=47462;
/// `typeAbyssPvPGate` **[V-data]**: its menu calls the unserved
/// `pvpFilamentMgr`, so a PvE room never places it.
pub const PVP_GATE: i64 = 49563;
/// `groupAbyssNode` (the sub-nodes and nodes a room lists) **[V-data]**: the
/// static layout drops them ([`is_stray`]); [`loot::place`] spawns them as
/// NPCs instead.
pub const NODE_GROUP: i64 = loot::GROUP_CACHE;
/// `AbyssTargets` **[V-data]** (`typelists.tsv`): what must die before the
/// gate opens. Swarmers and caches are outside it.
pub const TARGETS_LIST: i64 = 826;
/// `TimerType.AbyssalContentExpiration` **[V]** (`crimewatchTimers`); 0 is
/// the Weapons timer.
pub const TIMER_ABYSSAL_CONTENT_EXPIRATION: i64 = 11;
/// The entity category: an NPC's, for a typelist's category test.
const ENTITY_CATEGORY: i64 = 11;
/// `typeAbyssEncounterGate` **[V]** (`inventorycommon/const`): the gate to the
/// next room (`ActivateAbyssalAccelerationGate`).
pub const ENCOUNTER_GATE: i64 = 47685;
/// `typeAbyssExitGate` **[V]**: the last room's gate, out of the pocket.
pub const EXIT_GATE: i64 = 47686;
/// `typeAbyssEntranceGate` **[V]**: the co-op gate in the pilots' own space.
pub const ENTRANCE_GATE: i64 = 48964;
/// `groupAbyssalKeys` **[V]**: the filaments.
pub const KEY_GROUP: i64 = 1979;
/// `attributeWeatherID` and `attributeDifficultyTier` **[V]** (`dogma/const`).
pub const ATTR_WEATHER: i64 = 2760;
pub const ATTR_TIER: i64 = 2761;
/// `FILAMENT_PROXIMITY_RESTRICTION_LIST_ID` and `_RANGE` **[V]**.
pub const PROXIMITY_LIST: i64 = 50;
pub const PROXIMITY_RANGE: f64 = 1_000_000.0;
/// `maxStargateJumpingDistance` **[V]** (`appConst`).
pub const MAX_GATE_DISTANCE: f64 = 2_500.0;
/// **[A]** Allowance for the ship's own size at a gate (see the header).
pub const SHIP_SLACK: f64 = 1_000.0;
/// **[A]** The rooms of a filament's pocket.
pub const ROOMS: usize = 3;
/// `TIER_5_SHIPYARD` and `TIER_0_DEMO` **[V]** (`abyss/common/constants`).
pub const TIER_5_SHIPYARD: i64 = 1001;
pub const TIER_0_DEMO: i64 = 1002;

/// `DEFAULT_ABYSS_CONTENT_DURATION` **[V]**: 20 minutes, in seconds.
pub const DURATION_S: i64 = 20 * 60;
/// `UNENTERED_GATE_LIFETIME` **[V]**, in seconds.
pub const UNENTERED_GATE_LIFETIME_S: i64 = 1800;
const FILETIME_PER_S: i64 = 10_000_000;
/// How long past the end a pilot still on a jump is waited for.
const TRANSIT_GRACE_S: i64 = 60;
/// How long after the ship the pod dies at a collapse **[A]**: long enough
/// for the client to finish the capsule change. Falsified if the client still
/// logs tracebacks at the pod's death, or TQ's pod dies sooner.
const POD_DELAY_S: i64 = 3;
/// Where the co-op gate stands, from the deploying ship.
const GATE_OFFSET: [f64; 3] = [1_500.0, 0.0, 0.0];

/// The tiers the client knows (`abyss/common/constants.py`: 0 to 6).
pub const TIERS: std::ops::RangeInclusive<i64> = 0..=6;

/// The site ids of runs: 8.05e9 up to the mission site band (8.075e9, which
/// runs on to the population site band at 8.1e9), past
/// the customs offices (8e9 plus a planet id under 5e7) and below the instance
/// object band (8.7e9 up), so a run room's site id is never the id of a ball
/// in any instance (the clouds' dbuff sources are such balls).
pub const SITE_BASE: i64 = 8_050_000_000;
/// One past the band.
pub const SITE_END: i64 = 8_075_000_000;

const _: () = assert!(SITE_END <= crate::mission::SITE_BASE);
const _: () = assert!(SITE_BASE >= crate::planet::FIRST_CUSTOMS_OFFICE_ID + 50_000_000);
const _: () = assert!(SITE_END <= crate::site::INSTANCE_BASE);

/// Whether a site id is a run's. `site.rs` leaves these instances alone, as
/// it does a wormhole's and an escalation's.
pub fn is_abyss_site(site_id: i64) -> bool {
    (SITE_BASE..SITE_END).contains(&site_id)
}

/// **[A]** The `ad0N_A_cube` graphic ids by tier (tier 0 and 1 share the
/// first). Falsified by a scene whose nebula does not suit its tier.
const NEBULAE: [i64; 7] = [22041, 22041, 22042, 22045, 22048, 22209, 22212];

/// Salt of the stream the weather stage is drawn from **[A]** (an invented
/// number; the draw only has to be stable).
const WEATHER_STREAM: u64 = 0x7765_6174;

/// The nebula graphic id of a tier ([`NEBULAE`]).
pub fn nebula(tier: i64) -> i64 {
    NEBULAE[usize::try_from(tier).unwrap_or(0).min(NEBULAE.len() - 1)]
}

/// The tier a key plays at: the two special values are tier 5 and tier 0.
pub fn play_tier(key_tier: i64) -> i64 {
    match key_tier {
        TIER_5_SHIPYARD => 5,
        TIER_0_DEMO => 0,
        t => t.clamp(0, 6),
    }
}

// ---------------------------------------------------------------------------
// The modes and the client's error codes
// ---------------------------------------------------------------------------

/// `abyss/game_mode.py` **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Solo,
    CoOp,
    TwoPlayer,
}

impl Mode {
    /// `GameModeID`: 1 solo, 2 co-op, 3 two-player.
    pub fn from_wire(id: i64) -> Option<Mode> {
        match id {
            1 => Some(Mode::Solo),
            2 => Some(Mode::CoOp),
            3 => Some(Mode::TwoPlayer),
            _ => None,
        }
    }

    pub fn wire(self) -> i64 {
        match self {
            Mode::Solo => 1,
            Mode::CoOp => 2,
            Mode::TwoPlayer => 3,
        }
    }

    /// `allowed_ships_list_id`.
    pub fn ship_list(self) -> i64 {
        match self {
            Mode::Solo => 57,
            Mode::CoOp => 92,
            Mode::TwoPlayer => 233,
        }
    }

    /// `key_stack_size`: the filaments one run takes.
    pub fn key_stack(self) -> i64 {
        match self {
            Mode::Solo => 1,
            Mode::CoOp => 3,
            Mode::TwoPlayer => 2,
        }
    }

    pub fn max_players(self) -> usize {
        match self {
            Mode::Solo => 1,
            Mode::CoOp => 3,
            Mode::TwoPlayer => 2,
        }
    }

    /// `ship_restriction_error_id`.
    pub fn ship_error(self) -> i64 {
        match self {
            Mode::Solo => err::INVALID_SHIP_TYPE,
            Mode::CoOp => err::INVALID_SHIP_TYPE_THREE_PLAYER,
            Mode::TwoPlayer => err::INVALID_SHIP_TYPE_TWO_PLAYER,
        }
    }

    /// `should_jump_on_deploy`.
    pub fn jumps_on_deploy(self) -> bool {
        self == Mode::Solo
    }

    /// `is_fleet_required` (`is_multiplayer`).
    pub fn fleet_required(self) -> bool {
        self.max_players() > 1
    }
}

/// `abyss/error.py`'s `Error` codes **[V]**, the ones the server raises.
pub mod err {
    pub const ABYSS_CONTENT_ACTIVE: i64 = 1;
    pub const IN_ABYSS_SYSTEM: i64 = 2;
    pub const INVALID_KEY_TYPE: i64 = 3;
    pub const INVALID_OWNER: i64 = 4;
    pub const INVALID_SHIP_TYPE: i64 = 5;
    pub const ITEM_NOT_REACHABLE: i64 = 6;
    pub const KEY_NOT_FOUND: i64 = 7;
    pub const PROXIMITY_RESTRICTION: i64 = 8;
    pub const PVP_TIMER_ACTIVE: i64 = 9;
    pub const SAFETY_ENGAGED: i64 = 10;
    pub const SESSION_CHANGE_IN_PROGRESS: i64 = 11;
    pub const SHIP_IN_DUNGEON: i64 = 12;
    pub const SHIP_IN_WARP: i64 = 14;
    pub const SHIP_IS_CLOAKED: i64 = 15;
    pub const SHIP_IS_DOCKED: i64 = 16;
    pub const SHIP_IS_INVULNERABLE: i64 = 17;
    pub const SHIP_IS_SELF_DESTRUCTING: i64 = 18;
    pub const SHIP_IS_TETHERED: i64 = 19;
    pub const SHIP_IS_WARP_SCRAMBLED: i64 = 20;
    pub const UNKNOWN_ERROR: i64 = 22;
    pub const SHIP_NOT_IN_FLEET: i64 = 23;
    pub const INVALID_SHIP_TYPE_THREE_PLAYER: i64 = 24;
    pub const SHIP_NOT_IN_SAME_FLEET: i64 = 25;
    pub const SHIP_HAS_ENTERED: i64 = 26;
    pub const MAX_ENTERED: i64 = 27;
    pub const ENTRANCE_CLOSED: i64 = 28;
    pub const PROXIMITY_ENTRANCE: i64 = 29;
    pub const ENTRANCE_MISSING: i64 = 30;
    pub const INVALID_KEY_AMOUNT: i64 = 31;
    pub const POCKET_COLLAPSING: i64 = 32;
    pub const INVALID_SHIP_TYPE_TWO_PLAYER: i64 = 34;
    pub const VOID_SPACE_DISALLOWED: i64 = 35;
    pub const VERY_SECURE_SYSTEM_DISALLOWED: i64 = 36;
    pub const IN_BANNED_SYSTEM: i64 = 37;
}

/// `solarSystemZarzakh` **[V]** (`appConst`): the one system the client's
/// `is_hazard_system` bans filaments in (`IN_BANNED_SYSTEM`).
pub const ZARZAKH: i64 = 30_100_000;

/// One refusal: the code and its label arguments.
pub type Refusal = (i64, Vec<i64>);

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// One pilot in a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub character: i64,
    pub origin_system: i64,
    pub origin_spot: [f64; 3],
    /// The room the pilot is in or on the way to, 1-based.
    pub room: usize,
    /// The room the pilot was last told of (0: none yet).
    pub told: usize,
}

/// One room of the pocket: its own system and site.
#[derive(Debug, Clone, PartialEq)]
pub struct Pocket {
    /// 1-based.
    pub room: usize,
    pub system: i64,
    pub site_id: i64,
    pub dungeon: i64,
    pub entry: [f64; 3],
    pub exit_at: [f64; 3],
    /// The gate's item id (changes if the instance is re-made); 0: none yet.
    pub gate: i64,
    pub cleared: bool,
    /// How many waves have been spawned in this room (0: none yet); the
    /// current one is `wave - 1`. (`site::vacate` drops the site's tags, so a
    /// re-made room that is not `cleared` has its current wave made again and
    /// the NPCs left in its system sent away: [`seed_room`].)
    pub wave: usize,
    /// Taken out of space: nobody is in it and nobody will be.
    pub closed: bool,
    /// The room's cache and nodes (empty until the room is first made); a room
    /// made again respawns the ones not destroyed.
    pub caches: Vec<loot::CacheSlot>,
    /// The room's weather stage 1-3 **[A]** (0: none), drawn when the pocket
    /// is allocated ([`weather::draw`]).
    pub stage: u8,
}

/// The co-op gate in the deployer's system.
#[derive(Debug, Clone, PartialEq)]
pub struct Entrance {
    pub site_id: i64,
    pub gate: i64,
    pub system: i64,
    pub at: [f64; 3],
    pub expires: BlueTime,
}

/// One filament's pocket.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: u64,
    pub creator: i64,
    pub mode: Mode,
    pub tier: i64,
    /// The key's `weatherID` **[V]**: the weather held on every ship in the
    /// pocket ([`weather`]); 0 has none.
    pub weather: i64,
    /// The key's type: the entrance gate's `abyssFilamentTypeID`.
    pub filament: i64,
    /// The client's `content_id` **[A]**: the first room's site id.
    pub content_id: i64,
    pub rooms: usize,
    pub pilots: Vec<Pilot>,
    /// Everyone who has ever been in (a pilot who left cannot come back).
    pub ever: Vec<i64>,
    pub pockets: Vec<Pocket>,
    pub entrance: Option<Entrance>,
    pub started: bool,
    /// Filetime the pocket collapses (0 until the first pilot is in).
    pub ends_at: BlueTime,
    /// The key was above the system's limit: the trace is the suspect one.
    pub suspect: bool,
    /// The run's trace in the pilots' own space (`None` until it is placed).
    pub trace: Option<trace::Trace>,
}

impl Run {
    pub fn pilot(&self, character: i64) -> Option<&Pilot> {
        self.pilots.iter().find(|p| p.character == character)
    }

    pub fn pocket_of(&self, p: &Pilot) -> Option<&Pocket> {
        self.pockets.get(p.room.checked_sub(1)?)
    }

    fn open_pockets(&self) -> impl Iterator<Item = &Pocket> {
        self.pockets.iter().filter(|p| !p.closed)
    }

    /// Whether the entrance can still admit a pilot at `ft`.
    fn admits(&self, ft: BlueTime) -> bool {
        self.ever.len() < self.mode.max_players()
            && self.entrance.as_ref().is_some_and(|e| ft < e.expires)
    }
}

/// The slot: the runs, and the next run and site numbers.
#[derive(Debug, Default)]
pub struct AbyssState {
    runs: BTreeMap<u64, Run>,
    next_run: u64,
    next_site: u64,
    /// Pilots who left a run on a jump that had not landed: sent home when
    /// they do (or forgotten when they are gone; their logoff spot stays).
    rescue: BTreeSet<i64>,
    /// The stage's clock at its last tick, and the instant it was read: a gate
    /// is refused with `POCKET_COLLAPSING` once a run's end has passed on
    /// the clock carried forward from there ([`AbyssState::now_on_clock`]).
    clock: BlueTime,
    clock_at: Option<Instant>,
    /// The wave table in force; `None` is the shipped one. For tests, which
    /// bring rows of invented types.
    waves: Option<Vec<WaveRow>>,
    /// The wave and cache kinds with their wrecks, one per type, derived once
    /// ([`loot::kind_for`]).
    loot_kinds: BTreeMap<i64, &'static crate::npc::NpcType>,
    /// The caches and nodes in space, by NPC id: how the kill hook knows one
    /// from any NPC.
    cache_of: BTreeMap<i64, loot::CacheTag>,
    /// Runs already told that a cache's wreck is a record only.
    warned: BTreeSet<u64>,
    /// The weathers in force; `None` is [`weather::WEATHERS`]. For tests,
    /// which bring invented types.
    weathers: Option<Vec<weather::Weather>>,
    /// The stage table in force; `None` is the shipped one. For tests.
    stages: Option<weather::Stages>,
    /// Pilots whose ship died at a collapse and whose pod is still to die,
    /// with when. Outside the run: the run closes in the tick of the ship's
    /// death.
    /// Character -> (when, the pod's ship id).
    pods_due: BTreeMap<i64, (BlueTime, i64)>,
}

impl AbyssState {
    /// The time on the stage's clock now: its last tick's time plus what has
    /// passed since, so a gate is not usable for a tick after the end, and a
    /// test that ticks on a fake clock keeps its fake time. 0 before any tick.
    fn now_on_clock(&self) -> BlueTime {
        match self.clock_at {
            Some(at) => {
                self.clock + (crate::game_clock::clock::since(at).as_nanos() / 100) as BlueTime
            }
            None => 0,
        }
    }
}

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(AbyssState::default())
}

/// The run a pilot is in.
pub fn run_of(w: &Registry, character: i64) -> Option<&Run> {
    w.ext::<AbyssState>()
        .runs
        .values()
        .find(|r| r.pilot(character).is_some())
}

/// Every run.
pub fn runs(w: &Registry) -> impl Iterator<Item = &Run> {
    w.ext::<AbyssState>().runs.values()
}

/// Whether a pilot is in a run: `logoff_spot::record` keeps the origin they
/// left from while this holds.
/// A pilot whose pod is still to die at a collapse counts as in the run, so
/// a log-off hold on it is not released and no origin is recorded (#241).
pub fn in_run(w: &Registry, character: i64) -> bool {
    run_of(w, character).is_some() || w.ext::<AbyssState>().pods_due.contains_key(&character)
}

// ---------------------------------------------------------------------------
// Pure rules
// ---------------------------------------------------------------------------

/// Where the pilot lands and where the exit gate stands, in the room's
/// coordinates (the instance is anchored at the origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spots {
    pub entry: [f64; 3],
    pub exit: [f64; 3],
}

/// The spots of a dungeon, or `None` when it is not an abyssal room: one
/// room, an entry object that exists, and an exit locator (the lowest id).
pub fn spots(d: &Dungeon) -> Option<Spots> {
    if d.archetype != ARCHETYPE {
        return None;
    }
    let rooms: BTreeSet<i64> = d.objects.iter().map(|o| o.room).collect();
    if rooms.len() != 1 {
        return None;
    }
    let entry = d
        .entry_object
        .and_then(|e| d.objects.iter().find(|o| o.id == e))?;
    let exit = d
        .objects
        .iter()
        .filter(|o| o.type_id == EXIT_LOCATOR)
        .min_by_key(|o| o.id)?;
    Some(Spots {
        entry: entry.pos,
        exit: exit.pos,
    })
}

/// Whether a room is a PvE one **[V-data]**: no faction and at least one
/// area-of-effect locator (15 rooms; the arenas, 6435-6437 and the
/// construction sites have none). Falsified by a count other than 15 on a new
/// bundle.
pub fn is_pve_room(d: &Dungeon) -> bool {
    d.faction.is_none() && d.objects.iter().any(|o| AOE_LOCATORS.contains(&o.type_id))
}

/// The abyssal PvE rooms of the catalogue, by dungeon id.
pub fn rooms(t: &Table) -> Vec<(i64, Spots)> {
    t.dungeons
        .values()
        .filter(|d| is_pve_room(d))
        .filter_map(|d| spots(d).map(|s| (d.id, s)))
        .collect()
}

/// Whether a room object is one no PvE room places: the PvP gate and the
/// nodes (`site::instantiate` drops them in an abyssal dungeon).
pub fn is_stray(content: &crate::content::Content, type_id: i64) -> bool {
    type_id == PVP_GATE || crate::scan::group_of(content, type_id) == Some(NODE_GROUP)
}

/// A room for `seed` (the list is in dungeon-id order, so the draw is
/// stable): `None` when there is none.
pub fn pick_room(list: &[(i64, Spots)], seed: u64) -> Option<(i64, Spots)> {
    if list.is_empty() {
        return None;
    }
    let mut s = crate::seed::Stream::new(seed);
    Some(list[s.range(0, list.len() as u64) as usize])
}

/// The lowest abyssal system no pocket holds.
pub fn free_system(u: &Universe, taken: &BTreeSet<i64>) -> Option<i64> {
    u.systems()
        .map(|s| s.id)
        .filter(|id| VOID_SYSTEMS.contains(id) && !taken.contains(id))
        .min()
}

/// The tier a slash argument names.
pub fn tier_arg(arg: &str) -> Option<i64> {
    arg.parse().ok().filter(|t| TIERS.contains(t))
}

/// `suspicion.get_max_allowed_tier_by_security_level` **[V]**: the highest
/// tier a system's security allows without being suspect. `200` is no limit.
pub fn max_tier(security: f64) -> i64 {
    let s = if 0.0 < security && security < 0.05 {
        0.05
    } else {
        security
    };
    match (s * 10.0).round() as i64 {
        9 | 10 => 0,
        8 => 3,
        7 => 4,
        6 => 5,
        5 => 6,
        _ => 200,
    }
}

/// What the validators read about a pilot and their ship: the common part.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub security: f64,
    /// The key's `difficultyTier` (1001 and 1002 are the special ones).
    pub key_tier: i64,
    pub docked: bool,
    pub cloaked: bool,
    pub invulnerable: bool,
    /// The pilot already has a run or an open entrance.
    pub in_encounter: bool,
    pub pvp_timer: bool,
    pub self_destructing: bool,
    pub tethered: bool,
    pub warping: bool,
    pub scrambled: bool,
    pub in_abyss_system: bool,
    pub session_change: bool,
    pub ship_allowed: bool,
    pub safety_green: bool,
    /// The system is in the client's void-space band (`34xxxxxx`).
    pub void_space: bool,
    /// The system is Zarzakh ([`ZARZAKH`]).
    pub banned_system: bool,
}

/// The key, as the deploy checks read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyFacts {
    pub type_id: i64,
    pub is_key: bool,
    pub owned: bool,
    pub reachable: bool,
    pub stack: i64,
}

/// What only a deploy reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeployFacts {
    pub in_fleet: bool,
    /// `None`: no such item.
    pub key: Option<KeyFacts>,
    /// The type of a restricted thing within range.
    pub closest_restricted: Option<i64>,
    pub in_dungeon: bool,
}

/// What only a gate jump reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JumpFacts {
    /// `None`: the gate is not there.
    pub distance: Option<f64>,
    pub reach: f64,
    pub same_fleet: bool,
    pub open: bool,
    pub has_entered: bool,
    pub full: bool,
}

impl Facts {
    fn demo(&self) -> bool {
        self.key_tier == TIER_0_DEMO
    }

    /// `is_suspicious`: the key is above what the system allows.
    pub fn suspicious(&self) -> bool {
        self.key_tier > max_tier(self.security)
    }
}

/// `perform_base_validation_checks`, in the client's order.
pub fn base_errors(mode: Mode, f: &Facts) -> Vec<Refusal> {
    let mut e = Vec::new();
    if max_tier(f.security) == 0 && f.key_tier > 0 {
        e.push((err::VERY_SECURE_SYSTEM_DISALLOWED, vec![]));
    }
    // `CONGESTION_BAN` (33) sits here in the client and is never raised.
    if f.void_space {
        e.push((err::VOID_SPACE_DISALLOWED, vec![]));
    }
    if !f.ship_allowed {
        e.push((mode.ship_error(), vec![]));
    }
    if f.docked {
        e.push((err::SHIP_IS_DOCKED, vec![]));
    }
    if f.cloaked {
        e.push((err::SHIP_IS_CLOAKED, vec![]));
    }
    if f.invulnerable && !f.demo() {
        e.push((err::SHIP_IS_INVULNERABLE, vec![]));
    }
    if f.in_encounter {
        e.push((err::ABYSS_CONTENT_ACTIVE, vec![]));
    }
    if f.pvp_timer {
        e.push((err::PVP_TIMER_ACTIVE, vec![]));
    }
    if f.self_destructing {
        e.push((err::SHIP_IS_SELF_DESTRUCTING, vec![]));
    }
    if f.tethered {
        e.push((err::SHIP_IS_TETHERED, vec![]));
    }
    if f.warping {
        e.push((err::SHIP_IN_WARP, vec![]));
    }
    if f.scrambled {
        e.push((err::SHIP_IS_WARP_SCRAMBLED, vec![]));
    }
    if f.in_abyss_system {
        e.push((err::IN_ABYSS_SYSTEM, vec![]));
    }
    if f.session_change {
        e.push((err::SESSION_CHANGE_IN_PROGRESS, vec![]));
    }
    if f.banned_system {
        e.push((err::IN_BANNED_SYSTEM, vec![]));
    }
    e
}

/// `validate_deploy`: the base checks, then the deploy ones.
pub fn deploy_errors(mode: Mode, f: &Facts, d: &DeployFacts) -> Vec<Refusal> {
    let mut e = base_errors(mode, f);
    if mode.fleet_required() && !d.in_fleet {
        e.push((err::SHIP_NOT_IN_FLEET, vec![]));
    }
    match &d.key {
        None => e.push((err::KEY_NOT_FOUND, vec![])),
        Some(k) => {
            if !k.is_key {
                e.push((err::INVALID_KEY_TYPE, vec![]));
            }
            if !k.owned {
                e.push((err::INVALID_OWNER, vec![]));
            }
            if !k.reachable {
                e.push((err::ITEM_NOT_REACHABLE, vec![k.type_id]));
            }
            if k.stack < mode.key_stack() {
                e.push((err::INVALID_KEY_AMOUNT, vec![mode.key_stack()]));
            }
        }
    }
    if !f.demo()
        && let Some(t) = d.closest_restricted
    {
        e.push((err::PROXIMITY_RESTRICTION, vec![t]));
    }
    if d.in_dungeon {
        e.push((err::SHIP_IN_DUNGEON, vec![]));
    }
    if f.suspicious() && f.safety_green && !f.demo() {
        e.push((err::SAFETY_ENGAGED, vec![]));
    }
    e
}

/// `validate_jump`: the base checks, then the gate's.
pub fn jump_errors(mode: Mode, f: &Facts, j: &JumpFacts) -> Vec<Refusal> {
    let mut e = base_errors(mode, f);
    match j.distance {
        None => e.push((err::ENTRANCE_MISSING, vec![])),
        Some(d) if d > j.reach => {
            e.push((err::PROXIMITY_ENTRANCE, vec![MAX_GATE_DISTANCE as i64]));
        }
        Some(_) => {}
    }
    if !j.same_fleet {
        e.push((err::SHIP_NOT_IN_SAME_FLEET, vec![]));
    }
    if !j.open {
        e.push((err::ENTRANCE_CLOSED, vec![]));
    }
    if j.has_entered {
        e.push((err::SHIP_HAS_ENTERED, vec![]));
    }
    if j.full {
        e.push((err::MAX_ENTERED, vec![]));
    }
    e
}

/// `UserError('AbyssError', errors=[(code, args)])` **[V]**: what the
/// activation controllers read from `error.dict['errors']`.
pub fn abyss_error(errors: &[Refusal]) -> ServiceError {
    let list = errors
        .iter()
        .map(|(code, args)| {
            PyValue::Tuple(vec![
                PyValue::Int(*code),
                PyValue::Tuple(args.iter().map(|a| PyValue::Int(*a)).collect()),
            ])
        })
        .collect();
    user_error("AbyssError", vec![("errors", PyValue::List(list))])
}

/// Whether a pocket's clock has run out.
pub fn expired(ends_at: BlueTime, ft: BlueTime) -> bool {
    ends_at > 0 && ft >= ends_at
}

/// The room after `room`, or `None` after the last.
pub fn room_after(room: usize, rooms: usize) -> Option<usize> {
    (room < rooms).then_some(room + 1)
}

/// The gate a room ends in: the exit gate in the last room, else the
/// encounter gate.
pub fn gate_type(room: usize, rooms: usize) -> i64 {
    if room >= rooms {
        EXIT_GATE
    } else {
        ENCOUNTER_GATE
    }
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnAbyssalLocationEntered(client_data)` **[V]**: the pilot is in room
/// `location_id` of content `content_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entered {
    pub to: i64,
    pub content_id: i64,
    pub nebula_graphic_id: i64,
    pub tier: i64,
    pub room: i64,
}

impl crate::notify::Push for Entered {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAbyssalLocationEntered",
            vec![crate::pyutil::pydict(vec![
                ("content_id", PyValue::Int(self.content_id)),
                ("nebula_graphic_id", PyValue::Int(self.nebula_graphic_id)),
                ("difficulty_tier", PyValue::Int(self.tier)),
                ("location_id", PyValue::Int(self.room)),
            ])],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnAddExpiryDurationForAbyssalContent(end_time)` **[V]**: the panel timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expiry {
    pub to: i64,
    pub end_time: BlueTime,
}

impl crate::notify::Push for Expiry {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAddExpiryDurationForAbyssalContent",
            vec![PyValue::Int(self.end_time)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnAbyssalContentFinished(content_id, timer_type)` **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub to: i64,
    pub content_id: i64,
}

impl crate::notify::Push for Finished {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnAbyssalContentFinished",
            vec![
                PyValue::Int(self.content_id),
                PyValue::Int(TIMER_ABYSSAL_CONTENT_EXPIRATION),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// Reading the world
// ---------------------------------------------------------------------------

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// `slim_extra` pairs for an entrance gate: `abyssFilamentTypeID`,
/// `gameModeID` and `isAbyssGateOpen` **[V]** (`AbyssEntranceWindowController`,
/// `ClientValidator.is_entrance_gate_open`). Empty for any other item. Rides
/// on `site::slim_extras`' entry for the gate (a slim takes only the first).
pub fn gate_pairs(w: &Registry, item_id: i64) -> Vec<(&'static str, PyValue)> {
    // Found by the gate's site, not by `Entrance::gate`: `deploy` announces
    // the gate (and so encodes its slim) before the gate's id is written
    // into the run (#241).
    let Some(inst) = w.ext::<Sites>().of_item(item_id) else {
        return Vec::new();
    };
    if !inst
        .objects
        .iter()
        .any(|o| o.item_id == item_id && o.type_id == ENTRANCE_GATE)
    {
        return Vec::new();
    }
    let site_id = inst.site_id;
    w.ext::<AbyssState>()
        .runs
        .values()
        .find(|r| r.entrance.as_ref().is_some_and(|e| e.site_id == site_id))
        .map(|r| {
            vec![
                ("abyssFilamentTypeID", PyValue::Int(r.filament)),
                ("gameModeID", PyValue::Int(r.mode.wire())),
                ("isAbyssGateOpen", PyValue::Bool(true)),
            ]
        })
        .unwrap_or_default()
}

fn ship_allowed(reg: &Registry, mode: Mode, ship_id: i64) -> bool {
    let Some(item) = reg.store().get(ship_id) else {
        return false;
    };
    let Some(list) = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.typelists.get(&mode.ship_list()))
    else {
        return true;
    };
    list.contains(item.type_id, item.group_id, item.category_id)
}

/// A restricted type (typelist [`PROXIMITY_LIST`]) within [`PROXIMITY_RANGE`]
/// of `at` in `system`: the celestials and the Upwell structures. The
/// closest, by distance.
pub fn closest_restricted(reg: &Registry, system: i64, at: [f64; 3]) -> Option<i64> {
    let list = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.typelists.get(&PROXIMITY_LIST))?;
    let table = crate::typedata::installed_in(reg.content()).table();
    let restricted = |type_id: i64| {
        let (group, category) = table
            .get(type_id)
            .map_or((0, 0), |r| (r.group_id, r.category_id));
        list.contains(type_id, group, category)
    };
    let dist = |p: [f64; 3]| {
        ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt()
    };
    let mut found: Vec<(f64, i64)> = Vec::new();
    if let Some(c) = reg.content().try_get::<Catalogue>() {
        found.extend(
            c.served_in(system)
                .map(|c| (dist(c.position) - c.radius, c.type_id)),
        );
    }
    found.extend(
        crate::structure::in_system(reg.deltas(), system)
            .iter()
            .map(|s| (dist(s.pos), s.type_id)),
    );
    found
        .into_iter()
        .filter(|(d, t)| *d < PROXIMITY_RANGE && restricted(*t))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t)
}

/// Whether the pilot stands inside a dungeon instance that is not a pocket.
fn in_dungeon(reg: &Registry, system: i64, motion: &crate::space_state::Motion) -> bool {
    reg.ext::<Sites>().iter().any(|i| {
        i.system == system
            && i.room_id.is_some()
            && !is_abyss_site(i.site_id)
            && crate::dungeon_tracking::is_inside(motion, i.anchor)
    })
}

/// The common facts of a pilot at `ft`, with `key_type` as the key's type.
pub fn facts(
    reg: &Registry,
    mode: Mode,
    actor: i64,
    key_type: i64,
    deploying: bool,
    ft: BlueTime,
) -> Facts {
    let here = reg.roster().get(actor).copied();
    let key_tier = crate::typedata::own_attribute_in(reg.content(), key_type, ATTR_TIER)
        .map_or(0, |v| v as i64);
    let ship = reg.character(actor).map_or(0, |c| c.ship_id);
    let system = here.map_or(0, |p| p.solar_system_id);
    let security = reg
        .content()
        .try_get::<Universe>()
        .and_then(|u| u.system(system))
        .map_or(0.0, |s| s.security);
    let motion = here.map(|p| p.motion);
    Facts {
        security,
        key_tier,
        docked: here.is_none(),
        cloaked: crate::cloak::is_cloaked(reg, ship),
        invulnerable: crate::invuln::character_protected(reg, actor, ft),
        // A deploy is refused for a pilot with a run or an open gate of their
        // own; a jump through someone's gate, for a pilot already in a run.
        in_encounter: runs(reg)
            .any(|r| r.pilot(actor).is_some() || (deploying && r.creator == actor)),
        pvp_timer: reg
            .combat()
            .crimewatch()
            .expiry(actor, crate::crimewatch::Timer::Pvp, ft)
            .is_some(),
        self_destructing: crate::self_destruct::is_running(reg, ship),
        tethered: motion.is_some_and(|m| m.tether().is_some()),
        warping: motion.is_some_and(|m| crate::dungeon_tracking::in_warp(&m)),
        scrambled: crate::tackle::gate_scrambled(reg, actor).is_some(),
        in_abyss_system: VOID_SYSTEMS.contains(&system),
        session_change: reg.jumps().contains(actor),
        ship_allowed: ship_allowed(reg, mode, ship),
        safety_green: crate::crimewatch::safety(reg, actor) == crate::crimewatch::Safety::Full,
        void_space: crate::celestial::VOID_SPACE_SYSTEMS.contains(&system),
        banned_system: system == ZARZAKH,
    }
}

fn key_facts(reg: &Registry, actor: i64, key_item: i64) -> Option<KeyFacts> {
    let item = reg.store().get(key_item)?;
    let ship = reg.character(actor).map(|c| c.ship_id);
    Some(KeyFacts {
        type_id: item.type_id,
        is_key: item.group_id == KEY_GROUP,
        owned: item.owner_id == actor,
        reachable: ship == Some(item.location_id),
        stack: item.quantity,
    })
}

fn fleet_of(reg: &Registry, character: i64) -> Option<i64> {
    reg.fleets().fleet_of(character).map(|f| f.fleet_id)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ---------------------------------------------------------------------------
// The world side: rooms
// ---------------------------------------------------------------------------

/// The population record the site path takes an instance from.
fn site_of(system: i64, site_id: i64, dungeon: i64, ft: BlueTime) -> Site {
    Site {
        system,
        row: 0,
        k: 0,
        slot: 0,
        generation: 0,
        site_id,
        dungeon_id: dungeon,
        archetype: ARCHETYPE,
        entry: Entry::Anomaly,
        anchor: [0.0; 3],
        target_id: "ABY-000".to_owned(),
        state: population::State::Live,
        at: ft,
    }
}

// ---- the waves --------------------------------------------------------------

/// The authored table (`content/npc/abyss-waves.tsv`) **[A]**.
const SHIPPED_WAVES: &str = include_str!("../../../../content/npc/abyss-waves.tsv");

/// The table's header.
pub const WAVES_HEADER: &str = "#tier\troom\twave\ttype_id\tcount";

/// `ownerID` and `corpID` on an abyssal NPC's slim **[A]**: the Triglavian
/// corporation of the bundle's faction rows. Falsified by a client that shows
/// another owner for the abyss's drones and Triglavians.
pub const WAVE_OWNER: i64 = 1_000_298;
/// The ring the first wave stands on round the entry, and what each later
/// wave adds, in metres **[A]**.
const WAVE_RING: f64 = 15_000.0;
const WAVE_RING_STEP: f64 = 2_000.0;

/// One row of the wave table: `count` NPCs of `type_id` in wave `wave` of
/// room `room` (both 1-based) of a run played at `tier`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveRow {
    pub tier: i64,
    pub room: usize,
    pub wave: usize,
    pub type_id: i64,
    pub count: usize,
}

/// Parse a wave table: `#` lines and blank lines are skipped, the rest are
/// five tab-separated numbers; a bad row is an error naming its line.
pub fn parse_waves(text: &str) -> Result<Vec<WaveRow>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .ok_or_else(|| {
                    format!(
                        "abyss-waves line {}: field {} is not a number",
                        n + 1,
                        i + 1
                    )
                })
        };
        if f.len() != 5 {
            return Err(format!(
                "abyss-waves line {}: {} fields, want 5",
                n + 1,
                f.len()
            ));
        }
        let (room, wave, count) = (num(1)?, num(2)?, num(4)?);
        if room < 1 || wave < 1 || count < 1 {
            return Err(format!(
                "abyss-waves line {}: room, wave and count are 1 or more",
                n + 1
            ));
        }
        out.push(WaveRow {
            tier: num(0)?,
            room: room as usize,
            wave: wave as usize,
            type_id: num(3)?,
            count: count as usize,
        });
    }
    Ok(out)
}

fn shipped_waves() -> &'static [WaveRow] {
    static ROWS: std::sync::OnceLock<Vec<WaveRow>> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| match parse_waves(SHIPPED_WAVES) {
        Ok(r) => r,
        Err(e) => {
            crate::errln!("[abyss] {e}");
            Vec::new()
        }
    })
}

/// The waves of one room: wave by wave, each its `(type_id, count)` rows in
/// file order. Empty for a tier or room the table lacks.
pub fn waves_for(rows: &[WaveRow], tier: i64, room: usize) -> Vec<Vec<(i64, usize)>> {
    let mut by: BTreeMap<usize, Vec<(i64, usize)>> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.tier == tier && r.room == room) {
        by.entry(r.wave).or_default().push((r.type_id, r.count));
    }
    by.into_values().collect()
}

/// A wave table carried in the content bundle (`Content::insert`): a test
/// harness in another crate has no registry to set `AbyssState::waves` on, and
/// the table rides with its own server's content, so no other server in the
/// process sees it. Read after the registry's own and before the shipped one.
#[derive(Debug, Clone, Default)]
pub struct WaveTable(pub Vec<WaveRow>);

fn room_waves(reg: &Registry, run: &Run, p: &Pocket) -> Vec<Vec<(i64, usize)>> {
    let tier = play_tier(run.tier);
    let own = reg.ext::<AbyssState>().waves.as_ref();
    match own.or_else(|| reg.content().try_get::<WaveTable>().map(|t| &t.0)) {
        Some(rows) => waves_for(rows, tier, p.room),
        None => waves_for(shipped_waves(), tier, p.room),
    }
}

/// Where member `i` of `n` in wave `wave` (0-based) stands: on a ring round
/// the room's entry, evenly spaced, each wave a little further out **[A]**.
pub fn wave_spot(entry: [f64; 3], wave: usize, i: usize, n: usize) -> [f64; 3] {
    let angle = std::f64::consts::TAU * (i as f64 + 0.5 * (wave % 2) as f64) / n.max(1) as f64;
    let r = WAVE_RING + WAVE_RING_STEP * wave as f64;
    [
        entry[0] + r * angle.cos(),
        entry[1],
        entry[2] + r * angle.sin(),
    ]
}

/// Put wave `index` (0-based) of `p`'s room in space, tagged to its site, and
/// tell the room. The ids of the NPCs made; none when the wave names no type
/// the bundle knows.
fn put_wave(reg: &mut Registry, run: &Run, p: &Pocket, index: usize) -> Vec<i64> {
    let Some(wave) = room_waves(reg, run, p).into_iter().nth(index) else {
        return Vec::new();
    };
    let members: Vec<i64> = wave
        .iter()
        .flat_map(|(t, c)| std::iter::repeat_n(*t, *c))
        .collect();
    let n = members.len();
    let mut ids = Vec::new();
    for (i, type_id) in members.into_iter().enumerate() {
        let Some(kind) = loot::kind_for(reg, type_id) else {
            continue;
        };
        let at = wave_spot(p.entry, index, i, n);
        let Some(id) = crate::npc::spawn(reg, kind, p.system, at) else {
            break;
        };
        site::tag(
            reg,
            id,
            site::Tag {
                site_id: p.site_id,
                dun_object_id: 0,
                entity_group: None,
            },
        );
        ids.push(id);
    }
    crate::npc::announce(reg, p.system, &ids, true);
    ids
}

/// Spawn the room's next wave, passing over a wave that puts nobody in
/// space. `false` when no wave is left.
fn spawn_wave(reg: &mut Registry, run: &Run, p: &mut Pocket) -> bool {
    let total = room_waves(reg, run, p).len();
    while p.wave < total {
        let index = p.wave;
        p.wave += 1;
        if !put_wave(reg, run, p, index).is_empty() {
            return true;
        }
    }
    false
}

/// Every NPC in `system` leaves space (no wreck, no bounty). A pocket's
/// system belongs to that pocket alone ([`systems_taken`]), so all of them
/// are the room's, tagged or not.
fn sweep_system(reg: &mut Registry, system: i64) {
    let left: Vec<i64> = reg
        .npcs()
        .iter()
        .filter(|n| n.solar_system_id == system)
        .map(|n| n.id)
        .collect();
    crate::npc::despawn_ids(reg, system, &left);
}

/// **The first wave**, called once when a room's instance is made. A room
/// made again (the site stage dropped it) that is not clear sends away what
/// is left in its system and makes its current wave again.
fn seed_room(reg: &mut Registry, run: &Run, p: &mut Pocket) {
    if p.wave == 0 {
        // A first wave starts on an empty system: whatever an earlier run
        // left here goes. (A room with no waves made again is here too:
        // its caches are told which of them died before they are swept.)
        // A room with no waves made again keeps its caches as they are.
        loot::sync(reg, p);
        if p.caches.is_empty() {
            sweep_system(reg, p.system);
        }
        spawn_wave(reg, run, p);
        loot::place(reg, run, p);
    } else if !p.cleared {
        loot::sync(reg, p);
        // The caches stay (at the hit points they have): only the waves go.
        let left: Vec<i64> = reg
            .npcs()
            .iter()
            .filter(|n| {
                n.solar_system_id == p.system
                    && !reg.ext::<AbyssState>().cache_of.contains_key(&n.id)
            })
            .map(|n| n.id)
            .collect();
        crate::npc::despawn_ids(reg, p.system, &left);
        if put_wave(reg, run, p, p.wave - 1).is_empty() {
            p.wave -= 1;
            spawn_wave(reg, run, p);
        }
        loot::place(reg, run, p);
    } else {
        // A cleared room made again: the caches still standing are tagged
        // again, one that is gone and was not destroyed is made again.
        loot::place(reg, run, p);
    }
}

/// Whether no target is alive in the room **[V-data]**: no NPC tagged to its
/// site that the client's `AbyssTargets` list names. A bundle without the
/// list counts every tagged NPC but the room's caches and nodes, which are
/// never a target.
pub fn room_clear(reg: &Registry, site_id: i64) -> bool {
    site::tagged(reg, site_id)
        .into_iter()
        .all(|id| reg.npcs().get(id).is_none_or(|n| !is_target(reg, n)))
}

/// Whether a living NPC is one the gate waits for: on the client's
/// `AbyssTargets` list (every NPC, with a bundle that lacks the list), and
/// not a cache or a node.
fn is_target(reg: &Registry, n: &crate::npc::Npc) -> bool {
    if reg.ext::<AbyssState>().cache_of.contains_key(&n.id) {
        return false;
    }
    match reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.typelists.get(&TARGETS_LIST))
    {
        Some(l) => l.contains(n.kind.type_id, n.kind.group_id, ENTITY_CATEGORY),
        None => true,
    }
}

/// Make sure the room is in space, and its gate if it is clear. `site::tick`
/// vacates an instance whose system has gone cold, so this is also how a room
/// comes back. `false` when it could not be made.
fn ensure_pocket(reg: &mut Registry, run: &Run, p: &mut Pocket, ft: BlueTime) -> bool {
    if reg.ext::<Sites>().get(p.site_id).is_none() {
        if !site::instantiate(reg, &site_of(p.system, p.site_id, p.dungeon, ft)) {
            return false;
        }
        p.gate = 0;
        seed_room(reg, run, p);
        weather::place(reg, run, p);
    }
    if !p.cleared && room_clear(reg, p.site_id) && !spawn_wave(reg, run, p) {
        p.cleared = true;
    }
    if p.cleared && (p.gate == 0 || site::object(reg, p.gate).is_none()) {
        match site::add_object(reg, p.site_id, gate_type(p.room, run.rooms), p.exit_at) {
            Some(g) => p.gate = g.item_id,
            None => {
                site::vacate(reg, p.site_id);
                return false;
            }
        }
    }
    true
}

fn next_site(reg: &mut Registry) -> i64 {
    let s = reg.ext_mut::<AbyssState>();
    let n = s.next_site;
    s.next_site += 1;
    SITE_BASE + (n % (SITE_END - SITE_BASE) as u64) as i64
}

pub(crate) fn systems_taken(reg: &Registry) -> BTreeSet<i64> {
    runs(reg)
        .flat_map(|r| r.open_pockets().map(|p| p.system))
        .collect()
}

/// A new pocket for room `room` of run `run_id`: a room from the catalogue
/// and a free system. `Err` is for the pilot.
fn alloc_pocket(
    reg: &mut Registry,
    run_id: u64,
    room: usize,
    tier: i64,
    weather: i64,
) -> Result<Pocket, String> {
    let seed = crate::seed::mix(reg.world().seed, &[run_id, room as u64]);
    // Its own stream, so the room drawn and the stage drawn do not move together.
    let stage = weather::draw(
        reg,
        tier,
        weather,
        crate::seed::mix(seed, &[WEATHER_STREAM]),
    );
    let Some((dungeon, sp)) = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| pick_room(&rooms(t), seed))
    else {
        return Err("the bundle has no abyssal room (dungeons, archetype 51)".to_owned());
    };
    let mut taken = systems_taken(reg);
    // A system with a wreck or an abandoned ship in it is not reused: the
    // pocket's leavings stay where they fell (abandoned ships stay for good).
    taken.extend(reg.combat().wrecks().map(|w| w.solar_system_id));
    taken.extend(
        reg.ext::<crate::pilotless::State>()
            .iter()
            .map(|a| a.system),
    );
    let Some(system) = reg
        .content()
        .try_get::<Universe>()
        .and_then(|u| free_system(u, &taken))
    else {
        return Err("no free abyssal system".to_owned());
    };
    Ok(Pocket {
        room,
        system,
        site_id: next_site(reg),
        dungeon,
        entry: sp.entry,
        exit_at: sp.exit,
        gate: 0,
        cleared: false,
        wave: 0,
        closed: false,
        caches: Vec::new(),
        stage,
    })
}

fn put(reg: &mut Registry, run: Run) {
    reg.ext_mut::<AbyssState>().runs.insert(run.id, run);
}

fn close_pocket(reg: &mut Registry, p: &mut Pocket) {
    if !p.closed {
        p.closed = true;
        // The wave NPCs would outlive the room and haunt the next run to
        // be given this system; they are untagged once the site is gone.
        sweep_system(reg, p.system);
        loot::forget(reg, p);
        weather::release_pocket(reg, p);
        // A closed pocket is never entered again, and a system with a wreck
        // is not reused ([`alloc_pocket`]): its wrecks go with it **[A]**.
        // A pilot's own wreck goes too; abandoned ships stay.
        let wrecks: Vec<i64> = reg
            .combat()
            .wrecks()
            .filter(|w| w.solar_system_id == p.system)
            .map(|w| w.wreck_id)
            .collect();
        if !wrecks.is_empty() {
            crate::outln!(
                "[abyss] system {} closed: {} wreck(s) removed with it (a pilot's own included)",
                p.system,
                wrecks.len()
            );
        }
        for id in wrecks {
            crate::combat::remove_wreck(reg, id);
        }
        site::vacate(reg, p.site_id);
    }
}

/// **A run's end**: every room and the entrance leave space, the run goes.
fn close_run(reg: &mut Registry, mut run: Run) {
    for p in &mut run.pockets {
        close_pocket(reg, p);
    }
    if let Some(e) = run.entrance.take() {
        site::vacate(reg, e.site_id);
    }
    trace::vacate(reg, &mut run);
    reg.ext_mut::<AbyssState>().runs.remove(&run.id);
    crate::outln!("[abyss] run {} closed", run.id);
}

/// A pocket every pilot is past, with the entrance admitting no one more,
/// goes out of space. A pilot still ahead of a room (or still to come in)
/// has to pass through it, so it stays: with Alice in room 3 and Bob in
/// room 1, room 2 is Bob's.
fn sweep(reg: &mut Registry, run: &mut Run, ft: BlueTime) {
    let admits = run.admits(ft);
    let hindmost = run.pilots.iter().map(|p| p.room).min();
    let started = run.started;
    for p in &mut run.pockets {
        if started && !p.closed && !admits && hindmost.is_none_or(|h| h > p.room) {
            close_pocket(reg, p);
        }
    }
}

// ---------------------------------------------------------------------------
// The world side: entering, moving on, leaving
// ---------------------------------------------------------------------------

/// Put `actor` into room 1 of run `run_id`: the departure, the origin, the
/// clock if they are the first.
fn join(reg: &mut Registry, run_id: u64, actor: i64, ft: BlueTime) -> Result<(), String> {
    let Some(mut run) = reg.ext::<AbyssState>().runs.get(&run_id).cloned() else {
        return Err("the pocket is gone".to_owned());
    };
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err("you are not in space: undock first".to_owned());
    };
    let mut pocket = run.pockets[0].clone();
    if !ensure_pocket(reg, &run, &mut pocket, ft) {
        return Err("the room could not be placed".to_owned());
    }
    // A solo run's trace stands where the pilot leaves, placed before the
    // effect that names it ([`trace`]); a fleet's, where the gate stood.
    if !run.started && run.mode.jumps_on_deploy() {
        trace::place_at(
            reg,
            &mut run,
            here.solar_system_id,
            here.motion.position,
            ft,
        );
    }
    // What the effect names: the trace (solo) or the entrance gate.
    let target = if run.mode.jumps_on_deploy() {
        run.trace.as_ref().map(|t| t.ball)
    } else {
        run.entrance.as_ref().map(|e| e.gate)
    }
    .filter(|b| *b != 0);
    if !run.started {
        run.started = true;
        run.ends_at = ft + DURATION_S * FILETIME_PER_S;
    }
    run.pilots.push(Pilot {
        character: actor,
        origin_system: here.solar_system_id,
        origin_spot: here.motion.position,
        room: 1,
        told: 0,
    });
    run.ever.push(actor);
    let (system, entry) = (pocket.system, pocket.entry);
    run.pockets[0] = pocket;
    crate::outln!(
        "[abyss] {actor}: run {}, tier {}, system {system}, from {}",
        run.id,
        run.tier,
        here.solar_system_id
    );
    // The origin is the pilot's logoff spot while they are inside.
    crate::logoff_spot::set(
        reg,
        actor,
        crate::logoff_spot::Spot {
            ship_id: here.ship_id,
            system: here.solar_system_id,
            position: here.motion.position,
        },
    );
    match target {
        Some(target) => crate::jump_drive::depart_abyssal(
            reg,
            actor,
            here.ship_id,
            here.solar_system_id,
            system,
            entry,
            target,
            false,
            nebula(run.tier),
        ),
        None => crate::jump_drive::depart(
            reg,
            actor,
            here.ship_id,
            here.solar_system_id,
            system,
            entry,
        ),
    }
    // The entrance goes when the last seat is taken; the trace stands where
    // it stood.
    if run.ever.len() >= run.mode.max_players()
        && let Some(e) = run.entrance.take()
    {
        site::vacate(reg, e.site_id);
        trace::place_at(reg, &mut run, e.system, e.at, ft);
    }
    put(reg, run);
    Ok(())
}

/// **Begin a run**: the first room, the run, and for a solo run the pilot's
/// departure; for a fleet run the entrance gate beside the ship. `Err` is
/// the text for the pilot; nothing has changed then.
#[allow(clippy::too_many_arguments)]
pub fn begin(
    reg: &mut Registry,
    actor: i64,
    mode: Mode,
    tier: i64,
    weather: i64,
    filament: i64,
    rooms: usize,
    ft: BlueTime,
) -> Result<u64, String> {
    begin_as(reg, actor, mode, tier, weather, filament, rooms, false, ft)
}

/// [`begin`] for a key above the system's limit when `suspect`: the run's
/// trace is the suspect one.
#[allow(clippy::too_many_arguments)]
pub fn begin_as(
    reg: &mut Registry,
    actor: i64,
    mode: Mode,
    tier: i64,
    weather: i64,
    filament: i64,
    rooms: usize,
    suspect: bool,
    ft: BlueTime,
) -> Result<u64, String> {
    if run_of(reg, actor).is_some() {
        return Err("you are in a pocket already: /abyss leave".to_owned());
    }
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err("you are not in space: undock first".to_owned());
    };
    if reg.jumps().contains(actor) || crate::dungeon_tracking::in_warp(&here.motion) {
        return Err("you are jumping or warping: wait".to_owned());
    }
    let id = {
        let s = reg.ext_mut::<AbyssState>();
        s.next_run += 1;
        s.next_run
    };
    let pocket = alloc_pocket(reg, id, 1, tier, weather)?;
    let mut run = Run {
        id,
        creator: actor,
        mode,
        tier,
        weather,
        filament,
        content_id: pocket.site_id,
        rooms: rooms.max(1),
        pilots: Vec::new(),
        ever: Vec::new(),
        pockets: vec![pocket],
        entrance: None,
        started: false,
        ends_at: 0,
        suspect,
        trace: None,
    };
    if mode.jumps_on_deploy() {
        put(reg, run);
        if let Err(e) = join(reg, id, actor, ft) {
            if let Some(r) = reg.ext::<AbyssState>().runs.get(&id).cloned() {
                close_run(reg, r);
            }
            return Err(e);
        }
        return Ok(id);
    }
    let p = here.motion.position;
    let mut entrance = Entrance {
        site_id: next_site(reg),
        gate: 0,
        system: here.solar_system_id,
        at: [
            p[0] + GATE_OFFSET[0],
            p[1] + GATE_OFFSET[1],
            p[2] + GATE_OFFSET[2],
        ],
        expires: ft + UNENTERED_GATE_LIFETIME_S * FILETIME_PER_S,
    };
    // The run is stored before the gate is announced, so the gate's slim
    // finds it ([`gate_pairs`]); the gate id follows once it is known.
    run.entrance = Some(entrance.clone());
    put(reg, run);
    if !place_entrance(reg, &mut entrance, ft) {
        reg.ext_mut::<AbyssState>().runs.remove(&id);
        return Err("the entrance gate could not be placed".to_owned());
    }
    if let Some(r) = reg.ext_mut::<AbyssState>().runs.get_mut(&id) {
        r.entrance = Some(entrance);
    }
    Ok(id)
}

/// Put the entrance gate into space (an instance with no dungeon, and the
/// gate in it). `false` when it could not be made.
fn place_entrance(reg: &mut Registry, e: &mut Entrance, ft: BlueTime) -> bool {
    if reg.ext::<Sites>().get(e.site_id).is_some() {
        return true;
    }
    if !site::instantiate(reg, &site_of(e.system, e.site_id, 0, ft)) {
        return false;
    }
    #[cfg(test)]
    tests::STORED_AT_ANNOUNCE.with(|c| {
        c.set(Some(reg.ext::<AbyssState>().runs.values().any(|r| {
            r.entrance.as_ref().is_some_and(|x| x.site_id == e.site_id)
        })))
    });
    match site::add_object(reg, e.site_id, ENTRANCE_GATE, e.at) {
        Some(g) => {
            e.gate = g.item_id;
            true
        }
        None => {
            site::vacate(reg, e.site_id);
            false
        }
    }
}

/// **Take a pilot into a pocket** (`/abyss enter <tier>`): the probe's door,
/// a solo run of one room, no key.
pub fn enter(reg: &mut Registry, actor: i64, tier: i64, ft: BlueTime) -> Result<String, String> {
    let id = begin(reg, actor, Mode::Solo, tier, 0, 0, 1, ft)?;
    let system = reg
        .ext::<AbyssState>()
        .runs
        .get(&id)
        .and_then(|r| r.pockets.first())
        .map_or(0, |p| p.system);
    Ok(format!("into abyssal system {system}, tier {tier}"))
}

/// **Send a pilot home**: the way out of the last room, `/abyss leave`, or a
/// run that is over. The pilot leaves for where they came from; the rooms go
/// when nobody is left in them.
pub fn leave(reg: &mut Registry, actor: i64) -> bool {
    let Some(mut run) = run_of(reg, actor).cloned() else {
        return false;
    };
    let Some(i) = run.pilots.iter().position(|p| p.character == actor) else {
        return false;
    };
    let pilot = run.pilots.remove(i);
    reg.push(Finished {
        to: actor,
        content_id: run.content_id,
    });
    let in_pocket = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| run.open_pockets().any(|k| k.system == p.solar_system_id));
    if let Some(p) = in_pocket
        && !reg.jumps().contains(actor)
    {
        crate::jump_drive::depart(
            reg,
            actor,
            p.ship_id,
            p.solar_system_id,
            pilot.origin_system,
            pilot.origin_spot,
        );
    }
    if reg.jumps().contains(actor) {
        // On the way home, or still on the way in (sent home when they
        // land): the origin row goes when they have landed
        // ([`rescue_stranded`]), not before; a disconnect on the way keeps it.
        reg.ext_mut::<AbyssState>().rescue.insert(actor);
    } else {
        crate::logoff_spot::clear(reg, actor);
    }
    crate::outln!("[abyss] {actor}: home to {}", pilot.origin_system);
    let ft = crate::pyutil::filetime_now();
    if run.pilots.is_empty() && !run.admits(ft) {
        close_run(reg, run);
    } else {
        sweep(reg, &mut run, ft);
        put(reg, run);
    }
    true
}

/// **A pocket out of time**: the ship and the pod die, as on TQ. The pilot is
/// the cause **[A]** (the same kill as a self-destruct), so insurance does
/// what it does for an owner's own kill.
///
/// The ship dies now; the pod [`POD_DELAY_S`] later ([`kill_due_pods`]), when
/// the pilot is in it; at once for a pilot with no session. Both in one tick made the client throw (#241).
fn kill_pilot(reg: &mut Registry, character: i64, ft: BlueTime) {
    crate::outln!("[abyss] {character}: the pocket collapses");
    crate::combat::self_destruct_kill(reg, character, ft);
    if reg.roster().get(character).is_some() {
        if sessions_gone(reg, character) {
            // Nobody is there to see it, and a delay would let the log-off
            // hold take the pod out of space alive: it dies now.
            crate::combat::self_destruct_kill(reg, character, ft);
        } else if let Some(pod) = reg.character(character).map(|c| c.ship_id) {
            reg.ext_mut::<AbyssState>()
                .pods_due
                .insert(character, (ft + POD_DELAY_S * FILETIME_PER_S, pod));
            // The wait survives a restart as a row in the void system
            // ([`restore_sweep`]); it goes when the pod's time comes.
            if let Some(p) = reg.roster().get(character).copied() {
                crate::logoff_spot::set(
                    reg,
                    character,
                    crate::logoff_spot::Spot {
                        ship_id: pod,
                        system: p.solar_system_id,
                        position: p.motion.position,
                    },
                );
                return;
            }
        }
    }
    crate::logoff_spot::clear(reg, character);
}

/// A pod left pending in a void system, as its `logoffspot` row says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrandedPod {
    pub character: i64,
    pub ship_id: i64,
    pub system: i64,
    pub position: [f64; 3],
}

/// The pure half of [`restore_sweep`]: a `logoffspot` row (`char -> ship
/// system x y z`) that names a void system. A run's origin is never one
/// (`in_abyss_system` refuses a start from a pocket), so such a row is a
/// pod that was waiting out [`POD_DELAY_S`]. `None` for any other row, a row
/// that does not parse, and a row whose ship is in the store and is no capsule.
pub fn stranded_pod(
    store: &crate::item::Store,
    key: &[String],
    row: &[String],
) -> Option<StrandedPod> {
    let n = |i: usize| row.get(i)?.parse::<i64>().ok();
    let f = |i: usize| row.get(i)?.parse::<f64>().ok();
    let system = n(1)?;
    VOID_SYSTEMS.contains(&system).then_some(())?;
    // A rescue pilot's release hold can write a void row that holds the real
    // hull (crimewatch `release_holds`); that is no pod, and is left alone.
    // A ship the store no longer has is let through: the sweep clears it.
    let ship_id = n(0)?;
    store
        .get(ship_id)
        .is_none_or(|i| i.group_id == crate::fitting::GROUP_CAPSULE)
        .then_some(())?;
    Some(StrandedPod {
        character: key.first()?.parse().ok()?,
        ship_id,
        system,
        position: [f(2)?, f(3)?, f(4)?],
    })
}

/// **After a world load** (#243): a pod that was pending when the server
/// stopped is put back in the roster where it was and made due now, so the
/// next [`tick`] kills it (and revives the pilot in the clone) as the live
/// rule does. A row whose pod is no longer the character's ship is cleared.
/// The caller is the restore path (`Registry::apply_snapshot`, after
/// `logoff_spot::restore`).
pub fn restore_sweep(reg: &mut Registry) {
    let found: Vec<StrandedPod> = reg
        .deltas()
        .of(crate::logoff_spot::KIND)
        .iter()
        .filter_map(|(k, v)| stranded_pod(reg.store(), k, v))
        .collect();
    for s in found {
        // The pod is the character's capsule: their ship, or parked in the
        // hull they boarded during the wait (`restore_active_ships` names
        // that hull as their ship after a load).
        let live = reg.character(s.character).is_some_and(|c| {
            crate::fitting::capsule_of(reg.store(), c).is_some_and(|p| p.item_id == s.ship_id)
        }) && reg.roster().get(s.character).is_none();
        if reg.ext::<AbyssState>().pods_due.contains_key(&s.character) {
            continue; // already swept (the load did it)
        }
        if !live {
            crate::logoff_spot::clear(reg, s.character);
            continue;
        }
        crate::outln!(
            "[abyss] {}: the pocket collapsed before the restart; the pod dies",
            s.character
        );
        // The roster holds what they fly: the pod, or the hull it sits in;
        // `kill_due_pods` then kills both in that case.
        let flown = reg.character(s.character).map_or(s.ship_id, |c| c.ship_id);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                s.character,
                flown,
                s.system,
                crate::space_state::Motion::arrived(s.position, [0.0; 3]),
            ));
        reg.ext_mut::<AbyssState>()
            .pods_due
            .insert(s.character, (0, s.ship_id));
    }
}

/// The pods whose time has come die. A pilot who flies something else by
/// then loses that hull first, then the pod. One who is in an ordinary system
/// is let off (logged); one not in space is left alone (logged).
fn kill_due_pods(reg: &mut Registry, ft: BlueTime) {
    let due: Vec<(i64, i64)> = reg
        .ext::<AbyssState>()
        .pods_due
        .iter()
        .filter(|(_, (at, _))| ft >= *at)
        .map(|(id, (_, pod))| (*id, *pod))
        .collect();
    for (id, pod) in due {
        reg.ext_mut::<AbyssState>().pods_due.remove(&id);
        let Some(here) = reg.roster().get(id).copied() else {
            crate::outln!("[abyss] {id}: the pod is no longer in space; left alone");
            crate::logoff_spot::clear(reg, id);
            continue;
        };
        if !VOID_SYSTEMS.contains(&here.solar_system_id) {
            crate::outln!(
                "[abyss] {id}: out of the pocket before the pod's time (system {}); let off",
                here.solar_system_id
            );
            crate::logoff_spot::clear(reg, id);
            continue;
        }
        let other_hull = reg.character(id).is_some_and(|c| c.ship_id != pod);
        crate::combat::self_destruct_kill(reg, id, ft);
        if other_hull && reg.roster().get(id).is_some() {
            crate::combat::self_destruct_kill(reg, id, ft);
        }
        crate::logoff_spot::clear(reg, id);
    }
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// **The stage** (`tick::STAGES`, coordinator-only): end the runs that are
/// out of time, drop the pilots who are gone, bring back a room the site
/// stage dropped and place the gate of a room that is clear, tell a pilot who
/// has arrived, and close an entrance that has lived its life.
pub fn tick(reg: &mut Registry, now: Instant, ft: BlueTime) {
    let s = reg.ext_mut::<AbyssState>();
    s.clock = ft;
    s.clock_at = Some(now);
    if !reg.ext::<AbyssState>().rescue.is_empty() {
        rescue_stranded(reg);
    }
    if !reg.ext::<AbyssState>().pods_due.is_empty() {
        kill_due_pods(reg, ft);
    }
    if reg.ext::<AbyssState>().runs.is_empty() {
        return;
    }
    let ids: Vec<u64> = reg.ext::<AbyssState>().runs.keys().copied().collect();
    for id in ids {
        if let Some(run) = reg.ext::<AbyssState>().runs.get(&id).cloned() {
            tick_run(reg, run, ft);
        }
    }
}

fn sessions_gone(reg: &Registry, character: i64) -> bool {
    reg.sessions().of_character(character).is_empty()
}

/// Send home the pilots who left a run on a jump that had not landed, once
/// it has: to the origin their logoff row names.
fn rescue_stranded(reg: &mut Registry) {
    let ids: Vec<i64> = reg.ext::<AbyssState>().rescue.iter().copied().collect();
    for id in ids {
        if reg.jumps().contains(id) {
            continue;
        }
        let here = reg.roster().get(id).copied();
        match here {
            Some(p) if VOID_SYSTEMS.contains(&p.solar_system_id) => {
                if let Some(spot) = crate::logoff_spot::get(reg, id) {
                    // Stays in the set: the row goes when the ship has landed.
                    crate::jump_drive::depart(
                        reg,
                        id,
                        p.ship_id,
                        p.solar_system_id,
                        spot.system,
                        spot.position,
                    );
                    continue;
                }
            }
            // Landed: the row has done its work.
            Some(_) => crate::logoff_spot::clear(reg, id),
            None if !sessions_gone(reg, id) => continue,
            None => {}
        }
        reg.ext_mut::<AbyssState>().rescue.remove(&id);
    }
}

/// Whether a pilot is in an ordinary system: no reason to return to a pocket.
fn not_a_pocket(at: Option<crate::space_state::Presence>) -> bool {
    at.is_some_and(|p| !VOID_SYSTEMS.contains(&p.solar_system_id))
}

fn tick_run(reg: &mut Registry, mut run: Run, ft: BlueTime) {
    // The entrance: closed when its life is over; a key never used is lost.
    if let Some(mut e) = run.entrance.take() {
        if ft >= e.expires {
            site::vacate(reg, e.site_id);
            if !run.started {
                close_run(reg, run);
                return;
            }
            // Someone is inside: the trace stands where the gate stood.
            trace::place_at(reg, &mut run, e.system, e.at, ft);
        } else {
            let watched = !reg.roster().in_system(e.system).is_empty();
            if watched && !place_entrance(reg, &mut e, ft) {
                crate::errln!("[abyss] run {}: the entrance could not be re-made", run.id);
            }
            run.entrance = Some(e);
        }
    }
    trace::rewarm(reg, &mut run, ft);
    let over = expired(run.ends_at, ft);
    let systems: BTreeSet<i64> = run.open_pockets().map(|p| p.system).collect();
    let grace_over = over && ft >= run.ends_at + TRANSIT_GRACE_S * FILETIME_PER_S;
    for pilot in run.pilots.clone() {
        let id = pilot.character;
        let at = reg.roster().get(id).copied();
        let present = at.is_some_and(|p| systems.contains(&p.solar_system_id));
        let transit = reg.jumps().contains(id) && !grace_over;
        if present && over {
            reg.push(Finished {
                to: id,
                content_id: run.content_id,
            });
            kill_pilot(reg, id, ft);
            run.pilots.retain(|p| p.character != id);
        } else if !present && !transit && (over || at.is_none() || not_a_pocket(at)) {
            // Out of time and not in a pocket, or gone: the origin row stays
            // for their next login; one that is somewhere else is sent home.
            // A pilot with a session who is docked (the pod was killed and
            // they woke at home) or in an ordinary system has nothing to
            // come back to, and the row would restore a ship that is gone.
            let gone = at.is_none() && sessions_gone(reg, id);
            run.pilots.retain(|p| p.character != id);
            if over {
                reg.ext_mut::<AbyssState>().rescue.insert(id);
            } else if !gone {
                crate::logoff_spot::clear(reg, id);
            }
            crate::outln!("[abyss] {id}: out of run {}", run.id);
        }
    }
    if run.started && run.pilots.is_empty() && (over || !run.admits(ft)) {
        close_run(reg, run);
        return;
    }
    // The rooms with a pilot present: made again if dropped, gate if clear.
    for k in 0..run.pockets.len() {
        let mut p = run.pockets[k].clone();
        if p.closed {
            continue;
        }
        let here = run.pilots.iter().any(|q| {
            q.room == p.room
                && reg
                    .roster()
                    .get(q.character)
                    .is_some_and(|r| r.solar_system_id == p.system)
        });
        let placed = !here || ensure_pocket(reg, &run, &mut p, ft);
        // Written back even when the gate could not be placed: the cleared
        // flag and the wave count must not be lost, or the last wave is
        // made again on every tick.
        run.pockets[k] = p;
        if !placed {
            continue;
        }
    }
    // The news: a pilot who has arrived in a room is told of it once.
    for q in 0..run.pilots.len() {
        let pilot = run.pilots[q].clone();
        let Some(p) = run.pocket_of(&pilot) else {
            continue;
        };
        let arrived = reg
            .roster()
            .get(pilot.character)
            .is_some_and(|r| r.solar_system_id == p.system);
        if arrived && pilot.told != pilot.room {
            run.pilots[q].told = pilot.room;
            reg.push(Entered {
                to: pilot.character,
                content_id: run.content_id,
                nebula_graphic_id: nebula(run.tier),
                tier: run.tier,
                room: pilot.room as i64,
            });
            reg.push(Expiry {
                to: pilot.character,
                end_time: run.ends_at,
            });
        }
    }
    weather::reconcile(reg, &run, ft);
    sweep(reg, &mut run, ft);
    put(reg, run);
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// The service's methods.
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "AbyssalEndGateActivation" => end_gate(reg, req),
        "AbyssalGateActivation" => next_gate(reg, req),
        "AbyssalEntranceDeployment" => deploy(reg, req),
        "AbyssalEntranceGateActivation" => entrance_gate(reg, req),
        "ClientIsReady" => Ok(PyValue::None),
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

/// `abyssalMgr.AbyssalEntranceDeployment(keyItemID, gameModeID)`.
fn deploy(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let args = Args::new(req);
    let (Some(item), Some(mode_id)) = (args.int(0), args.int(1)) else {
        return Err(notify(
            "AbyssalEntranceDeployment needs (itemID, gameMode).",
        ));
    };
    let Some(mode) = Mode::from_wire(mode_id) else {
        return Err(abyss_error(&[(err::UNKNOWN_ERROR, vec![])]));
    };
    let ft = crate::pyutil::filetime_now();
    reg.advance_character(actor);
    let key = key_facts(reg, actor, item);
    let key_type = key.as_ref().map_or(0, |k| k.type_id);
    let f = facts(reg, mode, actor, key_type, true, ft);
    let here = reg.roster().get(actor).copied();
    let d = DeployFacts {
        in_fleet: fleet_of(reg, actor).is_some(),
        key,
        closest_restricted: here
            .and_then(|p| closest_restricted(reg, p.solar_system_id, p.motion.position)),
        in_dungeon: here.is_some_and(|p| in_dungeon(reg, p.solar_system_id, &p.motion)),
    };
    let errors = deploy_errors(mode, &f, &d);
    if !errors.is_empty() {
        return Err(abyss_error(&errors));
    }
    let tier = play_tier(f.key_tier);
    let weather = crate::typedata::own_attribute_in(reg.content(), key_type, ATTR_WEATHER)
        .map_or(0, |v| v as i64);
    // A demo key flags nobody; any other above the system's limit does, and
    // leaves the suspect trace.
    let suspect = f.suspicious() && !f.demo();
    begin_as(
        reg, actor, mode, tier, weather, key_type, ROOMS, suspect, ft,
    )
    .map_err(|e| notify(&e))?;
    // The key is paid once the run exists. Nothing refuses after this: a
    // stack the store will not take is logged and the run goes on.
    let stack = reg.store().get(item).map_or(0, |i| i.quantity);
    let need = mode.key_stack();
    let edit = if stack <= need {
        Edit::Destroy { item_id: item }
    } else {
        Edit::Consume {
            item_id: item,
            quantity: need,
        }
    };
    if let Err(e) = reg.apply_items(&[edit]) {
        crate::errln!("[abyss] key {item} of {actor} not taken: {e:?}");
    }
    // Only the deployer **[I]**: an entrant through the gate is never checked
    // for safety by the client, so flagging him would break his safety.
    if suspect {
        crate::crimewatch::suspect(reg, actor, ft);
    }
    Ok(PyValue::None)
}

/// `abyssalMgr.AbyssalEntranceGateActivation(gateID)`: a pilot of the
/// deployer's fleet goes in by the co-op gate.
fn entrance_gate(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(gate) = Args::new(req).int(0) else {
        return Err(notify("AbyssalEntranceGateActivation needs (itemID,)."));
    };
    let ft = crate::pyutil::filetime_now();
    reg.advance_character(actor);
    let Some(run) = reg
        .ext::<AbyssState>()
        .runs
        .values()
        .find(|r| r.entrance.as_ref().is_some_and(|e| e.gate == gate))
        .cloned()
    else {
        return Err(abyss_error(&[(err::ENTRANCE_MISSING, vec![])]));
    };
    let Some(e) = run.entrance.clone() else {
        return Err(abyss_error(&[(err::ENTRANCE_MISSING, vec![])]));
    };
    let f = facts(reg, run.mode, actor, run.filament, false, ft);
    let reach =
        site::object(reg, gate).map_or(0.0, |(p, _)| p.radius) + MAX_GATE_DISTANCE + SHIP_SLACK;
    let near = reg
        .roster()
        .get(actor)
        .filter(|p| p.solar_system_id == e.system)
        .map(|p| distance(p.motion.position, e.at));
    let creator_fleet = fleet_of(reg, run.creator);
    let j = JumpFacts {
        distance: near,
        reach,
        same_fleet: actor == run.creator
            || (creator_fleet.is_some() && fleet_of(reg, actor) == creator_fleet),
        open: ft < e.expires,
        has_entered: run.ever.contains(&actor),
        full: run.ever.len() >= run.mode.max_players(),
    };
    let errors = jump_errors(run.mode, &f, &j);
    if !errors.is_empty() {
        return Err(abyss_error(&errors));
    }
    join(reg, run.id, actor, ft).map_err(|e| notify(&e))?;
    Ok(PyValue::None)
}

/// `abyssalMgr.AbyssalGateActivation(gateID)`: the room's gate takes the
/// caller on to the next room.
fn next_gate(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(gate) = Args::new(req).int(0) else {
        return Err(notify("AbyssalGateActivation needs (itemID,)."));
    };
    let ft = crate::pyutil::filetime_now();
    let Some(mut run) = run_of(reg, actor).cloned() else {
        return Err(notify("You are not in an abyssal pocket."));
    };
    let Some(pilot) = run.pilot(actor).cloned() else {
        return Err(notify("You are not in an abyssal pocket."));
    };
    if expired(run.ends_at, reg.ext::<AbyssState>().now_on_clock()) {
        return Err(abyss_error(&[(err::POCKET_COLLAPSING, vec![])]));
    }
    let Some(here) = run.pocket_of(&pilot).cloned() else {
        return Err(abyss_error(&[(err::ENTRANCE_MISSING, vec![])]));
    };
    if here.gate == 0 || here.gate != gate {
        return Err(abyss_error(&[(err::ENTRANCE_MISSING, vec![])]));
    }
    let Some(next) = room_after(here.room, run.rooms) else {
        return Err(notify("That gate leads out: use it as the exit."));
    };
    let Some(me) = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.solar_system_id == here.system)
    else {
        return Err(abyss_error(&[(err::SESSION_CHANGE_IN_PROGRESS, vec![])]));
    };
    if reg.jumps().contains(actor) {
        return Err(abyss_error(&[(err::SESSION_CHANGE_IN_PROGRESS, vec![])]));
    }
    let mut pocket = match run.pockets.get(next - 1).cloned() {
        Some(p) => p,
        None => alloc_pocket(reg, run.id, next, run.tier, run.weather).map_err(|e| notify(&e))?,
    };
    if !ensure_pocket(reg, &run, &mut pocket, ft) {
        return Err(notify("The next room could not be placed."));
    }
    let (system, entry) = (pocket.system, pocket.entry);
    if run.pockets.len() < next {
        run.pockets.push(pocket);
    } else {
        run.pockets[next - 1] = pocket;
    }
    if let Some(q) = run.pilots.iter_mut().find(|q| q.character == actor) {
        q.room = next;
    }
    crate::jump_drive::depart_abyssal(
        reg,
        actor,
        me.ship_id,
        me.solar_system_id,
        system,
        entry,
        here.gate,
        true,
        nebula(run.tier),
    );
    crate::outln!("[abyss] {actor}: room {next} of run {}", run.id);
    sweep(reg, &mut run, ft);
    put(reg, run);
    Ok(PyValue::None)
}

/// `abyssalMgr.AbyssalEndGateActivation(itemID)`: the last room's exit gate
/// sends its pilot home.
fn end_gate(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let Some(gate) = Args::new(req).int(0) else {
        return Err(notify("AbyssalEndGateActivation needs (itemID,)."));
    };
    let Some(run) = run_of(reg, actor) else {
        return Err(notify("You are not in an abyssal pocket."));
    };
    if expired(run.ends_at, reg.ext::<AbyssState>().now_on_clock()) {
        return Err(abyss_error(&[(err::POCKET_COLLAPSING, vec![])]));
    }
    let ok = run
        .pilot(actor)
        .and_then(|p| run.pocket_of(p))
        .is_some_and(|p| p.gate != 0 && p.gate == gate && p.room >= run.rooms);
    if !ok {
        return Err(notify("That is not the exit gate of your pocket."));
    }
    leave(reg, actor);
    Ok(PyValue::None)
}

/// **Clear the pilot's room** (`/abyss clear`): the target NPCs tagged to it
/// leave space (no wreck, no bounty; its caches and nodes stay, they are not
/// targets), the stage places the gate on its next tick or the next wave. A GM tool for the scenarios and for testers; answers how
/// many left, or `None` for a pilot in no room.
fn clear_room(reg: &mut Registry, actor: i64) -> Option<usize> {
    let run = run_of(reg, actor)?;
    let pilot = run.pilot(actor)?;
    let p = run.pocket_of(pilot)?;
    let (site_id, system) = (p.site_id, p.system);
    let live: Vec<i64> = site::tagged(reg, site_id)
        .into_iter()
        .filter(|id| reg.npcs().get(*id).is_some_and(|n| is_target(reg, n)))
        .collect();
    crate::npc::despawn_ids(reg, system, &live);
    Some(live.len())
}

/// **`/abyss expire <seconds>`** (a GM tool for the collapse's scenario): the
/// pilot's pocket ends `seconds` from `ft`, and each of its pilots is told the
/// new end (the client clears and reschedules its warnings on
/// `OnAddExpiryDurationForAbyssalContent` **[V]**).
pub fn expire(reg: &mut Registry, actor: i64, seconds: i64, ft: BlueTime) -> String {
    let Some(id) = run_of(reg, actor).map(|r| r.id) else {
        return "you are not in a pocket".to_owned();
    };
    let end_time = ft + seconds * FILETIME_PER_S;
    let Some(run) = reg.ext_mut::<AbyssState>().runs.get_mut(&id) else {
        return "you are not in a pocket".to_owned();
    };
    run.ends_at = end_time;
    let pilots: Vec<i64> = run.pilots.iter().map(|p| p.character).collect();
    for to in pilots {
        reg.push(Expiry { to, end_time });
    }
    format!("the pocket collapses in {seconds} s")
}

/// **`/abyss enter <tier> | leave | status | clear`**: a GM door to the probe (one
/// room, no key; the filament path is the real one).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |t: &str| Ok(crate::pyutil::bstr(t));
    let usage = "usage: /abyss enter <tier 0-6> | leave | status | clear | cloud [clear] | expire <seconds>";
    let Some(actor) = actor else {
        return say("no character");
    };
    match args {
        ["enter", tier] => match tier_arg(tier) {
            None => say(usage),
            Some(tier) => match enter(reg, actor, tier, crate::pyutil::filetime_now()) {
                Ok(t) | Err(t) => say(&t),
            },
        },
        ["leave"] => say(if leave(reg, actor) {
            "leaving"
        } else {
            "you are not in a pocket"
        }),
        ["clear"] => match clear_room(reg, actor) {
            Some(n) => say(&format!("{n} NPC(s) sent away")),
            None => say("you are not in a pocket"),
        },
        ["expire", secs] => match secs
            .parse::<i64>()
            .ok()
            .filter(|s| (0..=86_400).contains(s))
        {
            None => say(usage),
            Some(s) => say(&expire(reg, actor, s, crate::pyutil::filetime_now())),
        },
        ["cloud"] => say(&weather::gm_cloud(reg, actor)),
        ["cloud", "clear"] => say(&weather::gm_clear(reg, actor)),
        ["status"] => match run_of(reg, actor) {
            Some(r) => {
                let p = r.pilot(actor);
                let room = p.and_then(|p| r.pocket_of(p));
                say(&format!(
                    "run {} room {}/{} in system {} (tier {}, {:?}), wave {}, gate {}, from {}",
                    r.id,
                    p.map_or(0, |p| p.room),
                    r.rooms,
                    room.map_or(0, |k| k.system),
                    r.tier,
                    r.mode,
                    room.map_or(0, |k| k.wave),
                    room.map_or(0, |k| k.gate),
                    p.map_or(0, |p| p.origin_system)
                ))
            }
            None => say(&format!("no pocket; {} running", runs(reg).count())),
        },
        _ => say(usage),
    }
}

#[cfg(test)]
mod tests;
