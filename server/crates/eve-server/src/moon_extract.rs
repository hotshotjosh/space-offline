//! Moon mining extractions (#53): the client's model, and the world part that
//! schedules one.
//!
//! Spec: `parity-moon-drills.md` §2 and §L2.1 (the lane specs). The first half of
//! this file decides what needs no `Registry`: the client's constants, the
//! extraction record, the duration and yield rules, the stage, the row text and
//! the `KeyVal` the client reads. The second half ("The world part") is the
//! rows ([`get`], [`put`]), [`start`], [`cancel`], [`end`], the clock's [`tick`]
//! (arrival and the ends), the pushes and notices 202-204, the calendar event,
//! the crater on the moon's slim and `/extraction`; its wire is
//! `service/structures/moon.rs` (`moonExtractions`). The chunk, the fracture and
//! the field are #54's.
//!
//! ## Provenance tags
//!
//! `[V]` verified against the decompiled client (`C:` is the client source
//! root, `out/research/_w/src`; the pinned 3670638 changed none of these
//! modules), `[I]` inferred, `[A]` assumed with its falsifier.
//!
//! ## Time
//!
//! Stored stamps are wall FILETIMEs (100 ns ticks, [`crate::structure::SECOND`]). A span
//! of game time is turned into ticks by the caller's `span` (`GameClock::span`)
//! so that a compressed clock shortens the deadlines and not the stamps'
//! unit, as `game_clock.rs` explains.

use std::time::{Duration, Instant};

use eve_marshal::PyValue;

use crate::calendar;
use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::moon::Share;
use crate::notifications::{Data, Value, data};
use crate::notify::Push;
use crate::pyutil::{bstr, keyval};
use crate::service::{Registry, ServiceError};
use crate::slim_extra::SlimExtras;
use crate::structure::{self, SECOND};
use crate::structure_dock::control;
use crate::structure_profile as sp;
use crate::structure_services as svc;
use crate::world_ext::{Boxed, SlotSpec};

// ---------------------------------------------------------------------------
// moonmining.const [V] (C:moonmining/const.py:6-25)
// ---------------------------------------------------------------------------

/// `BOMBARDMENT_DURATION` [V] (`C:moonmining/const.py:12`): seconds.
pub const BOMBARDMENT_DURATION: i64 = 60;
/// `MINIMUM_EXTRACTION_DURATION = BOMBARDMENT_DURATION + 518400` [V]
/// (`C:moonmining/const.py:16`): seconds.
pub const MINIMUM_EXTRACTION_DURATION: i64 = BOMBARDMENT_DURATION + 518_400;
/// `MAXIMUM_EXTRACTION_DURATION` [V] (`C:moonmining/const.py:17`): seconds.
pub const MAXIMUM_EXTRACTION_DURATION: i64 = 4_838_400;
/// `YIELD_PER_DAY` [V] (`C:moonmining/const.py:18`): m3 per day at multiplier 1.
pub const YIELD_PER_DAY: f64 = 720_000.0;
/// `START_FRACTURE_TO_CHUNK_EXPLOSION_DELAY` [V] (`C:moonmining/const.py:13`,
/// the client holds `25.0`, a float; the value is whole seconds).
pub const START_FRACTURE_TO_CHUNK_EXPLOSION_DELAY: i64 = 25;
/// `CHUNK_EXPLOSION_TO_ASTEROID_SPEW_DELAY` [V] (`C:moonmining/const.py:14`,
/// `5.0`).
pub const CHUNK_EXPLOSION_TO_ASTEROID_SPEW_DELAY: i64 = 5;
/// `FRACTURE_TOTAL_DURATION`, the sum of the two above [V]
/// (`C:moonmining/const.py:15`).
pub const FRACTURE_TOTAL_DURATION: i64 =
    START_FRACTURE_TO_CHUNK_EXPLOSION_DELAY + CHUNK_EXPLOSION_TO_ASTEROID_SPEW_DELAY;
/// `MAXIMUM_MINING_BEACON_DISTANCE` [V] (`C:moonmining/const.py:19`): metres,
/// structure to beacon.
pub const MAXIMUM_MINING_BEACON_DISTANCE: i64 = 250_000;
/// `ASTEROID_DECAY_DURATION` [V] (`C:moonmining/const.py:20`): seconds, the
/// field's life.
pub const ASTEROID_DECAY_DURATION: i64 = 172_800;
/// `MOON_CRATER_SLIMITEM_NAME` [V] (`C:moonmining/const.py:24`).
pub const MOON_CRATER_SLIMITEM_NAME: &str = "ExtractionCrater";

// dogma.const [V] (C:dogma/const.py:1332-1335)

/// `attributeExtractionAutoFractureDelay` [V] (`C:dogma/const.py:1332`).
pub const ATTR_AUTO_FRACTURE_DELAY: i64 = 2_698;
/// `attributeExtractionYieldMultiplier` [V] (`C:dogma/const.py:1333`).
pub const ATTR_YIELD_MULTIPLIER: i64 = 2_704;
/// `attributeExtractionFieldSizeMultiplier` [V] (`C:dogma/const.py:1334`).
pub const ATTR_FIELD_SIZE_MULTIPLIER: i64 = 2_705;
/// `attributeExtractionAsteroidDecayTime` [V] (`C:dogma/const.py:1335`). The
/// content data names the attribute `moonAsteroidDecayTimeMultiplier`,
/// default 1.0 [V] (`out/content/types/typedata.tsv`, attribute 2706): a
/// multiplier of [`ASTEROID_DECAY_DURATION`], not a time.
pub const ATTR_ASTEROID_DECAY_MULTIPLIER: i64 = 2_706;
/// `typeStandupMoonDrill` [V] (`C:inventorycommon/const.py:1771`).
pub const TYPE_STANDUP_MOON_DRILL: i64 = 45_009;
/// `SERVICE_MOONMINING` [V] (`C:structures/services.py:20`).
pub const SERVICE_MOONMINING: i64 = 14;
/// `calendarAutoEventMoonExtaction` (sic) [V] (`C:eve/common/lib/appConst.py:1828`).
pub const CALENDAR_AUTO_EVENT_MOON_EXTRACTION: i64 = 2;
/// Notification ids [V] (`C:eve/common/script/util/notificationconst.py:188-192`).
pub const NOTE_STARTED: i64 = 202;
pub const NOTE_CANCELLED: i64 = 203;
pub const NOTE_FINISHED: i64 = 204;
pub const NOTE_LASER_FIRED: i64 = 205;
pub const NOTE_AUTO_FRACTURE: i64 = 206;
/// The delta kind of the extraction rows (the world part, later).
pub const KIND: &str = "moonextract";

// The constants keep their relations (checked at compile time).
const _: () = {
    assert!(MINIMUM_EXTRACTION_DURATION > BOMBARDMENT_DURATION);
    assert!(MINIMUM_EXTRACTION_DURATION < MAXIMUM_EXTRACTION_DURATION);
    assert!(ASTEROID_DECAY_DURATION > FRACTURE_TOTAL_DURATION);
};

/// Why an extraction ended: `COMPLETED_REASON_*` [V]
/// (`C:moonmining/const.py:6-11`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Fracture = 1,
    Decay = 2,
    Offlined = 3,
    Invalid = 4,
    Slash = 5,
    Manual = 6,
}

/// Where an extraction is. The client derives its own stage
/// (`C:eve/client/script/ui/moonmining/__init__.py:84-93`): no extraction is
/// stopped, `chunkAvailableTime > now` is in progress, else fracturing while
/// its own laser stamp is set, else ready. The server's [`Phase`] adds what
/// the client cannot know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The chunk is on its way (client: in progress).
    Travel,
    /// The chunk is there and waits for the laser (client: ready).
    Ready,
    /// The laser was fired at `fired_at` (FILETIME) by `by`, `None` for the
    /// automatic fracture.
    Fracturing { fired_at: i64, by: Option<i64> },
    /// The asteroid field lives until `until` (FILETIME).
    Field { until: i64 },
}

/// Why a duration is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// UserError `MoonminingEventTooShort` [V name] (`C:eve/client/script/ui/
    /// moonmining/schedulingSvc.py:48`).
    TooShort,
    /// CustomNotify, text [A]; the client sends no such error.
    TooLong,
}

/// One extraction. Stamps are FILETIMEs.
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    pub id: i64,
    pub structure_id: i64,
    pub moon_id: i64,
    pub system_id: i64,
    pub owner_corp: i64,
    pub started_by: i64,
    pub started: i64,
    /// `started + span(BOMBARDMENT_DURATION)` [A]: the 60 s bombardment is in
    /// the minimum duration and the client's progress runs from
    /// `startMoveTime` (`inProgressCont.py:67-68,100-119`). Falsifier: the
    /// in-progress bar not at 0 at the start.
    pub start_move: i64,
    /// `started + span(duration_s)`.
    pub chunk_available: i64,
    /// `chunk_available + span(auto fracture delay)`.
    pub natural_decay: i64,
    /// The game seconds asked for.
    pub duration_s: i64,
    /// The structure's 2704 times the world's moon yield.
    pub yield_mult: f64,
    pub calendar_event: Option<i64>,
    pub phase: Phase,
    pub chunk_pos: Option<[f64; 3]>,
    pub field_seq: Option<u64>,
}

/// The three deadlines of a new extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub start_move: i64,
    pub chunk_available: i64,
    pub natural_decay: i64,
}

/// A duration inside `[MINIMUM_EXTRACTION_DURATION, MAXIMUM_EXTRACTION_DURATION]`
/// (seconds, both ends allowed). Below the minimum the client itself refuses
/// with `MoonminingEventTooShort` [V]; the maximum is the client's own slider
/// range [V] (`C:eve/client/script/ui/moonmining/__init__.py:60-64`,
/// `GetRangeTimeForSlider`), the server enforces the same bound.
pub fn check_duration(duration_s: i64) -> Result<(), Refusal> {
    if duration_s < MINIMUM_EXTRACTION_DURATION {
        Err(Refusal::TooShort)
    } else if duration_s > MAXIMUM_EXTRACTION_DURATION {
        Err(Refusal::TooLong)
    } else {
        Ok(())
    }
}

/// The deadlines of an extraction started at `now` (FILETIME).
///
/// `span` converts game seconds to ticks of the stored stamps (pass
/// `|s| clock.span(s * SECOND)`). `start_move = now + span(60 s)` [A,
/// falsifier on [`Extraction::start_move`]]; `chunk_available = now +
/// span(duration)`; `natural_decay = chunk_available + span(auto fracture)`.
pub fn plan(now: i64, duration_s: i64, auto_fracture_s: i64, span: impl Fn(i64) -> i64) -> Plan {
    let chunk_available = now + span(duration_s);
    Plan {
        start_move: now + span(BOMBARDMENT_DURATION),
        chunk_available,
        natural_decay: chunk_available + span(auto_fracture_s),
    }
}

/// `CalculateExtractionTotalYield(d, m) = YIELD_PER_DAY * d / 86400 * m` [V]
/// (`C:moonmining/util.py:8-10`). Which `d` the client feeds it depends on
/// the window: the in-progress window uses `chunkAvailableTime -
/// startMoveTime` (`C:eve/client/script/ui/moonmining/inProgressCont.py:67-68`),
/// the planning window uses `timestamp - now` (`planningCont.py:55`, with no
/// bombardment taken off). This function follows the in-progress span, so
/// `d = duration_s - BOMBARDMENT_DURATION` at clock scale 1 [A, because
/// `start_move` is itself [A]]. Never negative.
///
/// [A at clock scale above 1: this is in game seconds while the window
/// computes from the wall stamps, so the window shows less; falsifier: the
/// owner wants the window and the field to agree at any scale.]
pub fn total_yield_m3(duration_s: i64, yield_mult: f64) -> f64 {
    let d = (duration_s - BOMBARDMENT_DURATION).max(0) as f64;
    YIELD_PER_DAY * d / 86_400.0 * yield_mult
}

/// Each ore's volume: `fraction * total`, in the order of `shares`.
pub fn ore_volumes(shares: &[Share], total_m3: f64) -> Vec<(i64, f64)> {
    shares
        .iter()
        .map(|s| (s.basic, s.fraction() * total_m3))
        .collect()
}

/// Whole units per ore: `floor(total * percent / 100 / type volume)`, in the
/// order of `shares`, in integer arithmetic so that no unit is lost to float
/// rounding. The total and each type volume are taken at micro-m3 (rounded);
/// a type whose volume is not a positive number is skipped, and a total that
/// is not a finite number gives nothing.
pub fn ore_units(
    shares: &[Share],
    total_m3: f64,
    volume_of: impl Fn(i64) -> f64,
) -> Vec<(i64, i64)> {
    const MICRO: f64 = 1_000_000.0;
    if !total_m3.is_finite() || total_m3 < 0.0 {
        return Vec::new();
    }
    // Capped so that `total * percent` cannot overflow: the cap is far above
    // any real yield, and a float cast saturates anyway.
    let total = ((total_m3 * MICRO).round() as i128).min(i128::MAX / 100);
    shares
        .iter()
        .filter_map(|s| {
            let v = volume_of(s.basic);
            if !v.is_finite() || v <= 0.0 {
                return None;
            }
            let v = (v * MICRO).round() as i128;
            (v > 0).then(|| {
                let units = total * i128::from(s.percent) / 100 / v;
                (s.basic, i64::try_from(units).unwrap_or(i64::MAX))
            })
        })
        .collect()
}

/// The phase at `now`. Before `chunk_available` it is [`Phase::Travel`]
/// whatever is stored (the client's rule: `chunkAvailableTime > now` is in
/// progress); from then on the stored phase, with a stored `Travel` read as
/// [`Phase::Ready`] (the tick has not moved it yet).
pub fn stage(e: &Extraction, now: i64) -> Phase {
    if now < e.chunk_available {
        return Phase::Travel;
    }
    match e.phase {
        Phase::Travel => Phase::Ready,
        p => p,
    }
}

/// The tick's arrival step: a `Travel` row whose `chunk_available` has come
/// becomes `Ready`. True when it moved (the world part then sends 204).
pub fn arrive(e: &mut Extraction, now: i64) -> bool {
    if e.phase == Phase::Travel && now >= e.chunk_available {
        e.phase = Phase::Ready;
        true
    } else {
        false
    }
}

/// The window's GM buttons, `/extraction advance <mode>` [V names]
/// (`scheduling.py:254-259`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    Mid,
    Stop,
    Ready,
    Decay,
}

impl Advance {
    pub fn parse(word: &str) -> Option<Advance> {
        match word {
            "mid" => Some(Advance::Mid),
            "stop" => Some(Advance::Stop),
            "ready" => Some(Advance::Ready),
            "decay" => Some(Advance::Decay),
            _ => None,
        }
    }
}

/// Apply a GM advance at `now`, keeping the auto-fracture delay
/// (`natural_decay - chunk_available`) except for `Decay`. True when the
/// extraction was changed; false when nothing was done (a later slash handler
/// answers a refusal then).
///
/// Only an extraction still in [`Phase::Travel`] moves (`Decay` also from
/// [`Phase::Ready`]); a fracturing one or a field is left alone.
///
/// - `Mid`: `now` becomes half way from `start_move` to `chunk_available`
///   (unchanged, false, when `now` is not after `start_move`);
/// - `Ready`: `chunk_available = now`;
/// - `Stop`: not modelled, false and no change. The client says the chunk
///   "WILL NOT be available for fracture until it reaches the 'ready' phase"
///   (`C:eve/client/script/ui/moonmining/scheduling.py:258-259`), so it is not
///   `Ready`; the server has no stop position until the chunk-motion lane;
/// - `Decay`: `natural_decay = now + span(1 s)`, and a chunk still on its way
///   arrives now so that the decay does not precede it [I].
///
/// `span` is as in [`plan`].
pub fn advance(e: &mut Extraction, mode: Advance, now: i64, span: impl Fn(i64) -> i64) -> bool {
    let delay = e.natural_decay - e.chunk_available;
    match (mode, e.phase) {
        (Advance::Mid, Phase::Travel) if now > e.start_move => {
            e.chunk_available = e.start_move + 2 * (now - e.start_move);
            e.natural_decay = e.chunk_available + delay;
            true
        }
        (Advance::Ready, Phase::Travel) => {
            e.chunk_available = now;
            e.natural_decay = now + delay;
            true
        }
        (Advance::Decay, Phase::Travel | Phase::Ready) => {
            e.chunk_available = e.chunk_available.min(now);
            e.natural_decay = now + span(1);
            true
        }
        _ => false,
    }
}

/// The `OnMoonExtractionUpdate` / `GetExtractionAndEventIDFromStructureID`
/// value: a `KeyVal` with the seven fields the client reads [V]
/// (`chunkAvailableTime`, `startMoveTime`, `naturalDecayTime`,
/// `yieldMultiplier`: `inProgressCont.py:67-68,100-119`; `structureID`,
/// `solarSystemID`, `moonID`: `moonminingEventsCont.py:88-102`) and three
/// extras (`extractionID`, `ownerID`, `durationSeconds`) the client ignores [I].
pub fn to_keyval(e: &Extraction) -> PyValue {
    keyval(vec![
        ("extractionID", PyValue::Int(e.id)),
        ("structureID", PyValue::Int(e.structure_id)),
        ("moonID", PyValue::Int(e.moon_id)),
        ("solarSystemID", PyValue::Int(e.system_id)),
        ("ownerID", PyValue::Int(e.owner_corp)),
        ("startMoveTime", PyValue::Long(e.start_move)),
        ("chunkAvailableTime", PyValue::Long(e.chunk_available)),
        ("naturalDecayTime", PyValue::Long(e.natural_decay)),
        ("durationSeconds", PyValue::Int(e.duration_s)),
        ("yieldMultiplier", PyValue::Real(e.yield_mult)),
    ])
}

// ---------------------------------------------------------------------------
// The row: delta key [structure_id], value in this field order.
// ---------------------------------------------------------------------------

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "-".to_string(), |x| x.to_string())
}

fn phase_text(p: Phase) -> String {
    match p {
        Phase::Travel => "t".into(),
        Phase::Ready => "r".into(),
        Phase::Fracturing { fired_at, by } => format!("f:{fired_at}:{}", opt(by)),
        Phase::Field { until } => format!("d:{until}"),
    }
}

fn parse_phase(s: &str) -> Option<Phase> {
    let mut it = s.split(':');
    let p = match it.next()? {
        "t" => Phase::Travel,
        "r" => Phase::Ready,
        "f" => {
            let fired_at = it.next()?.parse().ok()?;
            let by = match it.next()? {
                "-" => None,
                n => Some(n.parse().ok()?),
            };
            Phase::Fracturing { fired_at, by }
        }
        "d" => Phase::Field {
            until: it.next()?.parse().ok()?,
        },
        _ => return None,
    };
    it.next().is_none().then_some(p)
}

/// The row value (the structure id is the key, not a field): `id, moon,
/// system, owner_corp, started_by, started, start_move, chunk_available,
/// natural_decay, duration_s, yield_mult, calendar_event, phase, chunk_pos,
/// field_seq`; `-` for `None`; `chunk_pos` as `x,y,z`.
pub fn row(e: &Extraction) -> Vec<String> {
    vec![
        e.id.to_string(),
        e.moon_id.to_string(),
        e.system_id.to_string(),
        e.owner_corp.to_string(),
        e.started_by.to_string(),
        e.started.to_string(),
        e.start_move.to_string(),
        e.chunk_available.to_string(),
        e.natural_decay.to_string(),
        e.duration_s.to_string(),
        e.yield_mult.to_string(),
        opt(e.calendar_event),
        phase_text(e.phase),
        e.chunk_pos.map_or_else(
            || "-".to_string(),
            |p| format!("{},{},{}", p[0], p[1], p[2]),
        ),
        opt(e.field_seq),
    ]
}

fn opt_parse<T: std::str::FromStr>(s: &str) -> Option<Option<T>> {
    if s == "-" {
        Some(None)
    } else {
        s.parse().ok().map(Some)
    }
}

/// The inverse of [`row`]; `None` for a row of the wrong length or with a
/// field that does not parse (the caller logs and drops it).
pub fn parse(structure_id: i64, f: &[String]) -> Option<Extraction> {
    if f.len() != 15 {
        return None;
    }
    let chunk_pos = if f[13] == "-" {
        None
    } else {
        let p: Vec<f64> = f[13]
            .split(',')
            .map(|x| x.parse().ok().filter(|v: &f64| v.is_finite()))
            .collect::<Option<_>>()?;
        Some(<[f64; 3]>::try_from(p).ok()?)
    };
    let yield_mult: f64 = f[10].parse().ok().filter(|v: &f64| v.is_finite())?;
    Some(Extraction {
        id: f[0].parse().ok()?,
        structure_id,
        moon_id: f[1].parse().ok()?,
        system_id: f[2].parse().ok()?,
        owner_corp: f[3].parse().ok()?,
        started_by: f[4].parse().ok()?,
        started: f[5].parse().ok()?,
        start_move: f[6].parse().ok()?,
        chunk_available: f[7].parse().ok()?,
        natural_decay: f[8].parse().ok()?,
        duration_s: f[9].parse().ok()?,
        yield_mult,
        calendar_event: opt_parse(&f[11])?,
        phase: parse_phase(&f[12])?,
        chunk_pos,
        field_seq: opt_parse(&f[14])?,
    })
}

// ---------------------------------------------------------------------------
// The world part (#53 MOON-SCHEDULE)
// ---------------------------------------------------------------------------

/// The first id of a chunk ball: the id of the extraction added to it. The
/// ball itself is #54's (the fracture lane); until then the client finds no ball
/// and returns quietly [V] (`planet.py:102-104`). [I] the band is the one
/// `parity-moon-drills.md` §L3.2 names, past `moon_belt`'s and below the NPC
/// orders; falsifier: #54 picks another band and the crater names a ball that
/// is not the chunk.
pub const CHUNK_BASE: i64 = 9_700_100_000_000;

/// The automatic fracture delay when the structure type is missing from the
/// type table altogether, in seconds. A type that is in the table always has a
/// value (the data's own default for 2698 is three hours), so the fallback never
/// applies to a real drilling platform. [A] two days; falsifier: a drilling
/// platform whose type row is absent and whose retail delay differs.
pub const FALLBACK_AUTO_FRACTURE_S: i64 = 172_800;

/// The type the client builds a character link with (`showinfo:1373//<id>`).
/// [A] any character type id opens the character's info; falsifier: the link
/// opens nothing.
const CHARACTER_LINK_TYPE: i64 = 1_373;

/// What the world slot keeps: the stamp of the last tick. The rows are the
/// truth, a restart rebuilds from [`KIND`].
#[derive(Debug, Default)]
pub struct Cache {
    pub last_tick: i64,
    /// The stamp of the last full drill check ([`DRILL_CHECK_EVERY`]).
    pub last_check: i64,
    /// `structure -> (name, type)` as last seen, so a notice for a structure
    /// that is gone still names it.
    pub names: std::collections::BTreeMap<i64, (String, i64)>,
}

/// How often the tick runs the full drill check (`online_with`), in seconds. The
/// hook ([`on_services_changed`]) ends an extraction the moment its drill goes
/// offline; this is the safety net for a change that bypassed it (a restored
/// row). [A] a minute; falsifier: a drill offline for longer than that with its
/// extraction still running.
pub const DRILL_CHECK_EVERY: i64 = 60;

/// The [`Cache`] world slot (`world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "moon_extract",
    make: |_| Box::new(Cache::default()) as Boxed,
};

fn row_key(structure_id: i64) -> Vec<String> {
    vec![structure_id.to_string()]
}

/// The extraction of `structure_id`, if it has one.
pub fn get(d: &Deltas, structure_id: i64) -> Option<Extraction> {
    parse(structure_id, d.of(KIND).get(&row_key(structure_id))?)
}

/// Every extraction, in structure-id string order. A row that does not parse
/// is skipped here; [`tick`] logs and drops it.
pub fn all(d: &Deltas) -> Vec<Extraction> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 1 && k[0] != "next")
        .filter_map(|(k, v)| parse(k[0].parse().ok()?, v))
        .collect()
}

/// The extractions of `corp`.
pub fn of_corp(d: &Deltas, corp: i64) -> Vec<Extraction> {
    all(d)
        .into_iter()
        .filter(|e| e.owner_corp == corp)
        .collect()
}

/// Write one extraction's row.
pub fn put(d: &mut Deltas, e: &Extraction) {
    d.put(KIND, row_key(e.structure_id), row(e));
}

/// Delete the row of `structure_id`.
pub fn remove(d: &mut Deltas, structure_id: i64) {
    d.del(KIND, &row_key(structure_id));
}

/// Take the next extraction id (1, 2, ...), as [`calendar::alloc_id`] does.
fn alloc_id(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let n = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    d.put(KIND, key, vec![(n + 1).to_string()]);
    n
}

/// Why a call is refused. [`Denied::key`] is the client's own `UserError` key
/// where the client has one [V] (`findings/user-errors.tsv`); where it has none
/// the refusal is a `CustomNotify` with our own words [A text].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// The pilot is not at the controls of the structure asked about.
    NotControlling,
    /// The structure is not a drilling platform (or does not exist).
    NotDrillingPlatform,
    /// No online Standup Moon Drill.
    DrillOffline,
    /// No mining beacon within 250 km.
    NoBeacon,
    /// The structure has an extraction already.
    Busy,
    /// The duration is out of range.
    Duration(Refusal),
    /// The moon has no makeup to extract.
    NothingToExtract,
    /// The pilot may not stop the extraction (no station manager role of the owner corporation).
    NotAllowed,
    /// `CancelExtraction` with no extraction.
    NoExtraction,
    /// `CancelExtraction` after the chunk arrived (the client shows Stop only in transit).
    NotInTravel,
    /// `GetMoonResourcesForStructure` with no online drill.
    ResourcesNeedDrill,
    /// `GetMoonResourcesForStructure` with no beacon.
    ResourcesNeedBeacon,
    /// `FractureChunkWithStructure` before #54 (the fracture lane) exists.
    NotYetFracturable,
}

impl Denied {
    /// The `UserError` key of the refusal.
    pub fn key(self) -> &'static str {
        match self {
            Denied::NotControlling
            | Denied::NotAllowed
            | Denied::NoExtraction
            | Denied::NotInTravel => "StructureMoonminingDenied",
            Denied::NotDrillingPlatform => "StructureServiceUnavailable",
            Denied::DrillOffline => "CannotStartExtractionMustHaveOnlineMoonDrill",
            Denied::NoBeacon => "CannotStartExtractionMustBeNearMoonMiningBeacon",
            Denied::Busy => "CannotStartExtractionAnotherExtractionInProgress",
            Denied::Duration(Refusal::TooShort) => "MoonminingEventTooShort",
            Denied::Duration(Refusal::TooLong) | Denied::NothingToExtract => "CustomNotify",
            Denied::ResourcesNeedDrill => "CannotGetResourcesMustHaveOnlineMoonDrill",
            Denied::ResourcesNeedBeacon => "CannotGetResourcesMustBeNearMoonMiningBeacon",
            Denied::NotYetFracturable => "ExtractionNotAvailableForFracture",
        }
    }

    /// The error the wire raises.
    pub fn into_error(self) -> ServiceError {
        let text = match self {
            Denied::Duration(Refusal::TooLong) => Some(format!(
                "An extraction cannot last longer than {} days.",
                MAXIMUM_EXTRACTION_DURATION / 86_400
            )),
            Denied::NothingToExtract => Some("This moon has nothing to extract.".to_owned()),
            _ => None,
        };
        match text {
            Some(t) => crate::modules::user_error("CustomNotify", vec![("notify", bstr(&t))]),
            None => crate::modules::user_error(self.key(), vec![]),
        }
    }
}

/// The hull group of a structure type: its type row's, else the profile's table
/// (as `structure_services` reads it).
fn hull_group(reg: &Registry, type_id: i64) -> i64 {
    crate::typedata::installed_in(reg.content())
        .table()
        .get(type_id)
        .map_or_else(|| sp::group_of(type_id), |t| t.group_id)
}

/// A structure that exists and is a drilling platform, as the client's
/// `IsDrillingPlatform` reads the group [V].
fn drilling_platform(reg: &Registry, sid: i64) -> Option<structure::Structure> {
    structure::get(reg.deltas(), sid)
        .filter(|s| structure::is_drilling_platform(hull_group(reg, s.type_id)))
}

/// Start an extraction at `structure_id` for `actor`, at the controls, in the
/// order the wire table lists: control, a drilling platform, an online drill, a
/// beacon, no extraction yet, the duration, a makeup. Anyone at the controls may
/// start (the controls already needed setting 17 or the owner's bypass): the
/// client lets a pilot without the station manager role start after its own
/// warning that he cannot stop it [V] (`schedulingSvc.py:61-63`); only stopping
/// needs the role ([`cancel`]). Then
/// the row, the calendar event when asked, notification 202, the push and the
/// crater. With the fuel burn on, a start past the fuel expiry is allowed [V]
/// (the client asked the pilot already, `schedulingSvc.py:59`).
pub fn start(
    reg: &mut Registry,
    actor: i64,
    structure_id: i64,
    duration_s: i64,
    add_to_calendar: bool,
    now: BlueTime,
) -> Result<Extraction, Denied> {
    if control::controlling(reg, actor) != Some(structure_id) {
        return Err(Denied::NotControlling);
    }
    let s = drilling_platform(reg, structure_id).ok_or(Denied::NotDrillingPlatform)?;
    if !svc::online_with(reg, structure_id, SERVICE_MOONMINING) {
        return Err(Denied::DrillOffline);
    }
    let moon = crate::moon_beacon::moon_in_range(reg, s.system, s.pos).ok_or(Denied::NoBeacon)?;
    if get(reg.deltas(), structure_id).is_some() {
        return Err(Denied::Busy);
    }
    check_duration(duration_s).map_err(Denied::Duration)?;
    if crate::moon_scan::makeup(reg, moon).is_none_or(|m| m.is_empty()) {
        return Err(Denied::NothingToExtract);
    }
    let content = reg.content();
    let auto_s = crate::typedata::attribute_in(content, s.type_id, ATTR_AUTO_FRACTURE_DELAY)
        .map_or(FALLBACK_AUTO_FRACTURE_S, |v| v as i64);
    // [A] no rig or skill modifies it until structure rigs exist; falsifier: the
    // planning window's `GetYieldMultiplier(structureID)`, read from the client's
    // dogma, differs from `yieldMultiplier` after the start.
    let yield_mult = crate::typedata::attribute_in(content, s.type_id, ATTR_YIELD_MULTIPLIER)
        .unwrap_or(1.0)
        * reg.config().rules.moon_yield_mult;
    let clock = crate::game_clock::of(reg);
    let p = plan(now, duration_s, auto_s, |sec| clock.span(sec * SECOND));
    let calendar_event = add_to_calendar.then(|| {
        let id = calendar::alloc_id(reg.deltas_mut());
        let title: String = format!("Moon extraction: {}", s.name)
            .chars()
            .take(calendar::MAX_TITLE)
            .collect();
        let ev = calendar::Event {
            id,
            owner_id: s.owner_corp,
            creator_id: actor,
            scope: calendar::Scope::Corp,
            date_time: p.chunk_available,
            duration: None,
            title,
            description: String::new(),
            importance: 0,
            auto_event_type: Some(CALENDAR_AUTO_EVENT_MOON_EXTRACTION),
            deleted: false,
            modified: now,
            invitees: Vec::new(),
        };
        calendar::put(reg.deltas_mut(), &ev);
        // The client's calendar learns of it as of any shared event.
        crate::service::calendar::announce_new(reg, &ev);
        id
    });
    let e = Extraction {
        id: alloc_id(reg.deltas_mut()),
        structure_id,
        moon_id: moon,
        system_id: s.system,
        owner_corp: s.owner_corp,
        started_by: actor,
        started: now,
        start_move: p.start_move,
        chunk_available: p.chunk_available,
        natural_decay: p.natural_decay,
        duration_s,
        yield_mult,
        calendar_event,
        phase: Phase::Travel,
        chunk_pos: None,
        field_seq: None,
    };
    put(reg.deltas_mut(), &e);
    reg.mark_dirty();
    reg.ext_mut::<Cache>()
        .names
        .insert(structure_id, (s.name.clone(), s.type_id));
    let by = character_link(reg, Some(actor));
    tell(
        reg,
        &e,
        NOTE_STARTED,
        data([
            ("startedBy", Value::Int(actor)),
            ("startedByLink", by),
            ("readyTime", Value::Int(e.chunk_available)),
            ("autoTime", Value::Int(e.natural_decay)),
            ("oreVolumeByType", ore_volume_value(reg, &e)),
        ]),
    );
    push_update(reg, &e, to_keyval(&e));
    announce_moon(reg, e.moon_id, e.system_id);
    Ok(e)
}

/// Stop the extraction of the structure `actor` controls: only while the chunk is
/// on its way (the client shows Stop only then, `scheduling.py:217-226` [V]), only
/// the owner corporation's station manager, director or CEO (`may_launch`; the
/// client disables the button otherwise [V]). Ends with [`Reason::Manual`].
pub fn cancel(reg: &mut Registry, actor: i64, now: BlueTime) -> Result<(), Denied> {
    let sid = control::controlling(reg, actor).ok_or(Denied::NotControlling)?;
    let e = get(reg.deltas(), sid).ok_or(Denied::NoExtraction)?;
    let in_corp = reg.character(actor).map(|c| c.corporation_id) == Some(e.owner_corp);
    if !in_corp || !crate::service::sov_hub::may_launch(reg, actor) {
        return Err(Denied::NotAllowed);
    }
    if stage(&e, now) != Phase::Travel {
        return Err(Denied::NotInTravel);
    }
    end(reg, sid, Reason::Manual, Some(actor), now);
    Ok(())
}

/// End the extraction of `structure_id`: delete the row and its calendar event
/// (marked deleted, the row kept as the calendar does), clear the crater, push
/// `OnMoonExtractionUpdate(None)`, and send 203 for [`Reason::Manual`],
/// [`Reason::Offlined`] and [`Reason::Invalid`] (`cancelledBy` is `by`, `None`
/// for the last two). [A] 203 also for the two automatic ends; falsifier: a TQ
/// corp receiving no mail when a drill runs dry. A no-op without a row.
pub fn end(reg: &mut Registry, structure_id: i64, reason: Reason, by: Option<i64>, now: BlueTime) {
    let Some(e) = get(reg.deltas(), structure_id) else {
        return;
    };
    remove(reg.deltas_mut(), structure_id);
    if let Some(ev) = e
        .calendar_event
        .and_then(|id| calendar::get(reg.deltas(), id))
    {
        let ev = calendar::Event {
            deleted: true,
            modified: now,
            ..ev
        };
        calendar::put(reg.deltas_mut(), &ev);
        crate::service::calendar::announce_removed(reg, &ev);
    }
    reg.mark_dirty();
    announce_moon(reg, e.moon_id, e.system_id);
    push_update(reg, &e, PyValue::None);
    if matches!(reason, Reason::Manual | Reason::Offlined | Reason::Invalid) {
        let link = character_link(reg, by);
        tell(
            reg,
            &e,
            NOTE_CANCELLED,
            data([
                ("cancelledBy", by.map_or(Value::None, Value::Int)),
                ("cancelledByLink", link),
            ]),
        );
    }
    reg.ext_mut::<Cache>().names.remove(&structure_id);
}

/// A service of `sid` changed (`structure_services::on_services_changed`, the #53
/// hook line): the drill going offline ends the running extraction with
/// [`Reason::Offlined`].
pub fn on_services_changed(reg: &mut Registry, sid: i64) {
    if get(reg.deltas(), sid).is_some() && !svc::online_with(reg, sid, SERVICE_MOONMINING) {
        end(
            reg,
            sid,
            Reason::Offlined,
            None,
            crate::pyutil::filetime_now(),
        );
    }
}

/// Every second, for every row: a row that does not parse is logged and dropped;
/// the structure gone or no longer a drilling platform ends it with
/// [`Reason::Invalid`]; a `Travel` row reaching `chunk_available` becomes
/// `Ready`, notice 204 and a push. Every [`DRILL_CHECK_EVERY`] seconds the drill
/// not online ends it with [`Reason::Offlined`] too (the hook does it at once). `Ready`
/// reaching `natural_decay` is #54's (the automatic fracture), and does nothing
/// here (and logs nothing: it would repeat every second).
pub fn tick(reg: &mut Registry, _: Instant, now: BlueTime) {
    let check = {
        let c = reg.ext_mut::<Cache>();
        c.last_tick = now;
        let due = now - c.last_check >= DRILL_CHECK_EVERY * SECOND;
        if due {
            c.last_check = now;
        }
        due
    };
    let bad: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter(|(k, v)| {
            k.len() == 1
                && k[0] != "next"
                && k[0]
                    .parse()
                    .ok()
                    .is_none_or(|id: i64| parse(id, v).is_none())
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in bad {
        crate::errln!("[moonextract] dropping a row that does not parse: {k:?}");
        reg.deltas_mut().del(KIND, &k);
        reg.mark_dirty();
    }
    for mut e in all(reg.deltas()) {
        let Some(s) = drilling_platform(reg, e.structure_id) else {
            end(reg, e.structure_id, Reason::Invalid, None, now);
            continue;
        };
        reg.ext_mut::<Cache>()
            .names
            .insert(e.structure_id, (s.name, s.type_id));
        if check && !svc::online_with(reg, e.structure_id, SERVICE_MOONMINING) {
            end(reg, e.structure_id, Reason::Offlined, None, now);
        } else if arrive(&mut e, now) {
            put(reg.deltas_mut(), &e);
            reg.mark_dirty();
            tell(
                reg,
                &e,
                NOTE_FINISHED,
                data([
                    ("autoTime", Value::Int(e.natural_decay)),
                    ("oreVolumeByType", ore_volume_value(reg, &e)),
                ]),
            );
            push_update(reg, &e, to_keyval(&e));
        }
    }
}

// -- what the client reads ------------------------------------------------------

/// `<a href="showinfo:T//I">name</a>`, the client's link [V]
/// (`linkUtil.py`).
fn link(type_id: i64, id: i64, name: &str) -> String {
    format!("<a href=\"showinfo:{type_id}//{id}\">{name}</a>")
}

/// A character's link as a notification value; empty text for no character.
fn character_link(reg: &Registry, id: Option<i64>) -> Value {
    // The client's formatter makes the link of `None` as an empty text.
    let Some(id) = id else {
        return Value::Text(String::new());
    };
    let name = reg
        .character(id)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    Value::Text(link(CHARACTER_LINK_TYPE, id, &name))
}

/// The base data of 202-206 [V] (`moonmining.py` `GetBaseData`): `structureID`,
/// `structureName`, `structureTypeID`, `solarSystemID`, `moonID`, and
/// `structureLink` (which the formatter builds itself; ours is harmless).
fn data_base(reg: &Registry, e: &Extraction) -> Data {
    let (name, type_id) = structure::get(reg.deltas(), e.structure_id)
        .map(|s| (s.name, s.type_id))
        .or_else(|| reg.ext::<Cache>().names.get(&e.structure_id).cloned())
        .unwrap_or_default();
    data([
        ("structureID", Value::Int(e.structure_id)),
        (
            "structureLink",
            Value::Text(link(type_id, e.structure_id, &name)),
        ),
        ("structureName", Value::Text(name)),
        ("structureTypeID", Value::Int(type_id)),
        ("solarSystemID", Value::Int(e.system_id)),
        ("moonID", Value::Int(e.moon_id)),
    ])
}

/// `oreVolumeByType`: `{oreTypeID: m3}` with integer keys [V]
/// (`evetypes.GetName(oreTypeID)` in `GetOreText`), from the moon's makeup and
/// the extraction's yield.
fn ore_volume_value(reg: &Registry, e: &Extraction) -> Value {
    let shares = crate::moon_scan::makeup(reg, e.moon_id).unwrap_or_default();
    let total = total_yield_m3(e.duration_s, e.yield_mult);
    Value::IntDict(
        ore_volumes(&shares, total)
            .into_iter()
            .map(|(ore, m3)| (ore, Value::Real(m3)))
            .collect(),
    )
}

/// One notification of `type_id` with the base data and `extra`, to the owner
/// corporation's managers ([`crate::structure_notify::tell_corp`]; [A] every
/// member with `may_launch`, online or not, the corporation the sender;
/// falsifier: a TQ corp member without the role receiving them).
fn tell(reg: &mut Registry, e: &Extraction, type_id: i64, extra: Data) {
    let mut d = data_base(reg, e);
    d.extend(extra);
    crate::structure_notify::tell_corp(reg, e.owner_corp, type_id, d);
}

/// `OnMoonExtractionUpdate(extraction | None)` [V] (`schedulingSvc.py:130`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionUpdate {
    pub to: Vec<i64>,
    pub value: PyValue,
}

impl Push for ExtractionUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnMoonExtractionUpdate", vec![self.value.clone()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Who hears the update **[A]**: the pilot at the structure's controls and every
/// member of the owner corporation docked in it. Falsifier: a TQ corp member in
/// the structure browser not updated.
fn update_audience(reg: &Registry, e: &Extraction) -> Vec<i64> {
    let mut to: Vec<i64> = control::pilot_of(reg, e.structure_id).into_iter().collect();
    for c in reg.characters_at_station(e.structure_id) {
        if c.corporation_id == e.owner_corp && !to.contains(&c.character_id) {
            to.push(c.character_id);
        }
    }
    to
}

/// Push the update on the clock, as the other structure pushes are: the tick
/// runs outside any call, where a plain push would reach whoever spoke next.
fn push_update(reg: &mut Registry, e: &Extraction, value: PyValue) {
    let to = update_audience(reg, e);
    if !to.is_empty() {
        reg.schedule(Duration::ZERO, ExtractionUpdate { to, value });
    }
}

/// The crater on the moon's slim: `("ExtractionCrater", (direction, started,
/// chunk_id))` for each moon of `ids` with an extraction. `direction` is the unit
/// vector from the moon's centre to the structure [I] (the client's crater faces
/// `impactDirection`, and its drill component aims from the structure to the
/// moon, `moonDrillComponent.py:98-104`); `started` the row's stamp; `chunk_id`
/// [`CHUNK_BASE`] plus the extraction id. Falsifier: the crater on the wrong side
/// of the moon.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> SlimExtras {
    let rows = all(w.deltas());
    if rows.is_empty() {
        return Vec::new();
    }
    ids.iter()
        .filter_map(|moon| {
            let e = rows.iter().find(|e| e.moon_id == *moon)?;
            let at = w.celestials().get(*moon)?.position;
            let s = structure::get(w.deltas(), e.structure_id)?;
            let d = [s.pos[0] - at[0], s.pos[1] - at[1], s.pos[2] - at[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let unit = if len > 0.0 && len.is_finite() {
                [d[0] / len, d[1] / len, d[2] / len]
            } else {
                [0.0, 0.0, 1.0]
            };
            let crater = PyValue::Tuple(vec![
                PyValue::Tuple(unit.iter().map(|v| PyValue::Real(*v)).collect()),
                PyValue::Long(e.started),
                PyValue::Int(CHUNK_BASE + e.id),
            ]);
            Some((*moon, vec![(MOON_CRATER_SLIMITEM_NAME, crater)]))
        })
        .collect()
}

/// `OnSlimItemChange` of the moon, to the pilots in its system, so a crater
/// appears without a re-jump **[I]** (a celestial's slim can change like a hub's;
/// falsifier: no crater after the push while one appears after a re-jump). A
/// crater that goes is only dropped from the slim: the client never removes one
/// it has drawn until the chunk ball of #54 exists (falsifier: a crater that
/// vanishes on the push). The slim is the celestial's (owner 1, no character or corporation).
fn announce_moon(reg: &mut Registry, moon_id: i64, system: i64) {
    let to = crate::structure_space::in_space(reg, system);
    if to.is_empty() {
        return;
    }
    let Some(type_id) = reg.celestials().get(moon_id).map(|c| c.type_id) else {
        return;
    };
    let extras = crate::slim_extra::for_slims(reg, &[moon_id])
        .into_iter()
        .filter(|(id, _)| *id == moon_id)
        .flat_map(|(_, x)| x)
        .collect();
    let stamp = reg.next_destiny_stamp();
    reg.schedule(
        Duration::ZERO,
        crate::sov_hub::HubSlimChange {
            to,
            stamp,
            slim: crate::destiny::Slim {
                item_id: moon_id,
                type_id,
                owner_id: 1,
                char_id: None,
                corp_id: None,
                response_thresholds: None,
            },
            extras,
        },
    );
}

/// `/upwell moon list`: every extraction, for a GM.
pub fn list_text(reg: &Registry) -> String {
    let rows = all(reg.deltas());
    if rows.is_empty() {
        return "no extractions".to_owned();
    }
    rows.iter()
        .map(|e| {
            format!(
                "{} structure {} moon {} corp {} {:?} ready {} decay {}",
                e.id,
                e.structure_id,
                e.moon_id,
                e.owner_corp,
                e.phase,
                e.chunk_available,
                e.natural_decay
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `/extraction advance mid|stop|ready|decay`: the window's GM buttons [V names]
/// on the controlled structure's extraction (see [`advance`]); the row is pushed.
/// `stop` is not modelled (the chunk keeps travelling) and says so.
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    const USAGE: &str = "/extraction advance mid|stop|ready|decay";
    let Some(actor) = actor else {
        return Ok(bstr("no character"));
    };
    let (Some("advance"), Some(mode), 2) = (
        args.first().copied(),
        args.get(1).and_then(|w| Advance::parse(w)),
        args.len(),
    ) else {
        return Ok(bstr(USAGE));
    };
    let Some(sid) = control::controlling(reg, actor) else {
        return Ok(bstr("you are not at the controls of a structure"));
    };
    let Some(mut e) = get(reg.deltas(), sid) else {
        return Ok(bstr("that structure has no extraction"));
    };
    let now = crate::pyutil::filetime_now();
    let clock = crate::game_clock::of(reg);
    if !advance(&mut e, mode, now, |sec| clock.span(sec * SECOND)) {
        return Ok(bstr(match mode {
            Advance::Stop => "stop is not modelled: the chunk keeps travelling",
            _ => "nothing changed: the extraction is not in a phase that moves",
        }));
    }
    put(reg.deltas_mut(), &e);
    reg.mark_dirty();
    push_update(reg, &e, to_keyval(&e));
    Ok(bstr(&format!(
        "extraction {}: chunk ready {}, decay {}",
        e.id, e.chunk_available, e.natural_decay
    )))
}

#[cfg(test)]
mod world_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moon::Rarity;
    use crate::pyutil::dict_get;
    use crate::structure::SECOND;

    // Invented ids and values throughout.
    const NOW: i64 = 1_000_000 * SECOND;
    const DAY: i64 = 86_400;

    fn sec(s: i64) -> i64 {
        s * SECOND
    }

    fn scale1(s: i64) -> i64 {
        sec(s)
    }

    fn sample() -> Extraction {
        let p = plan(NOW, BOMBARDMENT_DURATION + 7 * DAY, 3_600, scale1);
        Extraction {
            id: 9_001,
            structure_id: 7_000_001,
            moon_id: 7_000_002,
            system_id: 7_000_003,
            owner_corp: 7_000_004,
            started_by: 7_000_005,
            started: NOW,
            start_move: p.start_move,
            chunk_available: p.chunk_available,
            natural_decay: p.natural_decay,
            duration_s: BOMBARDMENT_DURATION + 7 * DAY,
            yield_mult: 1.25,
            calendar_event: Some(42),
            phase: Phase::Travel,
            chunk_pos: None,
            field_seq: None,
        }
    }

    #[test]
    fn a_duration_under_the_minimum_is_refused() {
        assert_eq!(
            check_duration(MINIMUM_EXTRACTION_DURATION - 1),
            Err(Refusal::TooShort)
        );
        assert_eq!(check_duration(0), Err(Refusal::TooShort));
        assert_eq!(check_duration(MINIMUM_EXTRACTION_DURATION), Ok(()));
    }

    #[test]
    fn a_duration_over_the_maximum_is_refused() {
        assert_eq!(
            check_duration(MAXIMUM_EXTRACTION_DURATION + 1),
            Err(Refusal::TooLong)
        );
        assert_eq!(check_duration(MAXIMUM_EXTRACTION_DURATION), Ok(()));
    }

    #[test]
    fn the_plan_orders_the_deadlines_and_a_compressed_clock_shortens_the_spans() {
        let d = BOMBARDMENT_DURATION + 7 * DAY;
        let p = plan(NOW, d, 3_600, scale1);
        assert!(NOW <= p.start_move && p.start_move < p.chunk_available);
        assert_eq!(p.chunk_available - NOW, sec(d));
        assert_eq!(p.natural_decay - p.chunk_available, sec(3_600));
        // A compressed clock shortens every span, not the origin.
        let q = plan(NOW, d, 3_600, |s| sec(s) / 10);
        assert_eq!((p.start_move - NOW) / 10, q.start_move - NOW);
        assert_eq!(q.chunk_available - NOW, sec(d) / 10);
        assert_eq!(q.natural_decay - q.chunk_available, sec(3_600) / 10);
    }

    #[test]
    fn the_yield_follows_the_clients_formula() {
        // The span the in-progress window shows, from the plan as computed.
        let d = BOMBARDMENT_DURATION + 7 * DAY;
        let p = plan(NOW, d, 3_600, scale1);
        let days = (p.chunk_available - p.start_move) as f64 / sec(DAY) as f64;
        assert_eq!(total_yield_m3(d, 1.25), YIELD_PER_DAY * days * 1.25);
        // A second duration, again from the plan as computed.
        let d2 = d + 7 * DAY;
        let p2 = plan(NOW, d2, 3_600, scale1);
        let days2 = (p2.chunk_available - p2.start_move) as f64 / sec(DAY) as f64;
        assert_eq!(total_yield_m3(d2, 1.25), YIELD_PER_DAY * days2 * 1.25);
        // The multiplier scales it, and a short span never goes negative.
        assert_eq!(total_yield_m3(d, 2.0), 2.0 * total_yield_m3(d, 1.0));
        assert_eq!(total_yield_m3(0, 1.0), 0.0);
    }

    fn share(basic: i64, percent: u8) -> Share {
        Share {
            basic,
            rarity: Rarity::Common,
            percent,
        }
    }

    #[test]
    fn ore_volumes_split_the_yield_by_the_composition() {
        let shares = [share(990_010, 60), share(990_020, 30), share(990_030, 10)];
        let v = ore_volumes(&shares, 1_000.0);
        assert_eq!(
            v.iter().map(|x| x.0).collect::<Vec<_>>(),
            [990_010, 990_020, 990_030]
        );
        assert!((v[0].1 - 600.0).abs() < 1e-9);
        assert!((v[1].1 - 300.0).abs() < 1e-9);
        assert!((v[2].1 - 100.0).abs() < 1e-9);
        assert!((v.iter().map(|x| x.1).sum::<f64>() - 1_000.0).abs() < 1e-9);
    }

    #[test]
    fn ore_units_floor_by_type_volume() {
        let shares = [share(1, 100), share(2, 100), share(3, 100)];
        let units = ore_units(&shares, 1_000.0, |ty| match ty {
            1 => 16.0,
            2 => 10.0,
            _ => 0.0, // no volume: skipped
        });
        assert_eq!(units, vec![(1, 62), (2, 100)]);
        assert!(ore_units(&shares, f64::NAN, |_| 1.0).is_empty());
        assert!(ore_units(&shares, f64::INFINITY, |_| 1.0).is_empty());
        assert!(ore_units(&shares, 10.0, |_| f64::NAN).is_empty());
    }

    #[test]
    fn ore_units_lose_no_unit_to_float_rounding() {
        // 94 % of a long span at an invented multiplier 1 and an invented
        // volume 10: the total is a whole number of m3 and the exact product
        // is a whole number of units, where the float product can fall just
        // short.
        let total = total_yield_m3(BOMBARDMENT_DURATION + 49 * DAY, 1.0);
        let units = ore_units(&[share(5, 94)], total, |_| 10.0);
        assert_eq!(units, vec![(5, (total as i64) * 94 / 1_000)]);
        assert_eq!(units[0].1 * 10 * 100, (total as i64) * 94);
    }

    #[test]
    fn ore_units_survive_an_absurd_total() {
        let shares = [share(1, 100), share(2, 1)];
        let units = ore_units(&shares, 1e300, |_| 1e-6);
        assert_eq!(units.len(), 2);
        assert!(units.iter().all(|u| u.1 > 0));
        assert_eq!(units[0].1, i64::MAX);
    }

    #[test]
    fn the_stage_is_travel_until_the_chunk_is_available() {
        let e = sample();
        assert_eq!(stage(&e, e.chunk_available - 1), Phase::Travel);
        assert_eq!(stage(&e, e.chunk_available), Phase::Ready);
        let mut f = e.clone();
        f.phase = Phase::Fracturing {
            fired_at: 5,
            by: None,
        };
        assert_eq!(stage(&f, f.chunk_available + 1), f.phase);
        assert_eq!(stage(&f, f.chunk_available - 1), Phase::Travel);
    }

    #[test]
    fn arrival_moves_travel_to_ready_once() {
        let mut e = sample();
        let at = e.chunk_available;
        assert!(!arrive(&mut e, at - 1));
        assert_eq!(e.phase, Phase::Travel);
        assert!(arrive(&mut e, at));
        assert_eq!(e.phase, Phase::Ready);
        assert!(!arrive(&mut e, at + 1), "only once");
    }

    #[test]
    fn the_row_round_trips_every_phase() {
        let phases = [
            Phase::Travel,
            Phase::Ready,
            Phase::Fracturing {
                fired_at: 123,
                by: Some(77),
            },
            Phase::Fracturing {
                fired_at: 123,
                by: None,
            },
            Phase::Field { until: 456 },
        ];
        for p in phases {
            let mut e = sample();
            e.phase = p;
            e.chunk_pos = Some([1.5, -2.25, 3e9]);
            e.field_seq = Some(8);
            assert_eq!(parse(e.structure_id, &row(&e)), Some(e.clone()), "{p:?}");
        }
        let mut e = sample();
        e.calendar_event = None;
        assert_eq!(parse(e.structure_id, &row(&e)), Some(e));
    }

    #[test]
    fn a_row_that_does_not_parse_is_dropped() {
        let e = sample();
        let good = row(&e);
        assert!(parse(1, &good[..14]).is_none(), "short");
        let mut long = good.clone();
        long.push("0".into());
        assert!(parse(1, &long).is_none(), "long");
        for i in [0, 5, 10, 12, 13] {
            let mut bad = good.clone();
            bad[i] = "x:y".into();
            assert!(parse(1, &bad).is_none(), "field {i}");
        }
        let mut bad = good.clone();
        bad[12] = "f:1".into();
        assert!(parse(1, &bad).is_none(), "fracturing without its by");
        bad[12] = "t:extra".into();
        assert!(parse(1, &bad).is_none(), "trailing part");
        bad = good.clone();
        bad[13] = "1,2".into();
        assert!(parse(1, &bad).is_none(), "two coordinates");
        for w in ["NaN", "inf", "-inf"] {
            let mut bad = good.clone();
            bad[10] = w.into();
            assert!(parse(1, &bad).is_none(), "yield {w}");
            let mut bad = good.clone();
            bad[13] = format!("1,{w},3");
            assert!(parse(1, &bad).is_none(), "position {w}");
        }
    }

    #[test]
    fn the_keyval_carries_the_seven_fields_the_client_reads() {
        let e = sample();
        let v = to_keyval(&e);
        let PyValue::Instance { state, .. } = &v else {
            panic!("a KeyVal is an instance: {v:?}");
        };
        let get = |k: &str| dict_get(state, k.as_bytes()).unwrap_or_else(|| panic!("no {k}"));
        assert_eq!(get("structureID"), &PyValue::Int(e.structure_id));
        assert_eq!(get("solarSystemID"), &PyValue::Int(e.system_id));
        assert_eq!(get("moonID"), &PyValue::Int(e.moon_id));
        assert_eq!(get("startMoveTime"), &PyValue::Long(e.start_move));
        assert_eq!(get("chunkAvailableTime"), &PyValue::Long(e.chunk_available));
        assert_eq!(get("naturalDecayTime"), &PyValue::Long(e.natural_decay));
        assert_eq!(get("yieldMultiplier"), &PyValue::Real(1.25));
    }

    #[test]
    fn advance_ready_brings_the_arrival_to_now_and_keeps_the_delay() {
        let mut e = sample();
        let delay = e.natural_decay - e.chunk_available;
        let now = NOW + sec(100);
        assert!(advance(&mut e, Advance::Ready, now, scale1));
        assert_eq!(e.chunk_available, now);
        assert_eq!(e.natural_decay - e.chunk_available, delay);
        assert!(arrive(&mut e, now));
    }

    #[test]
    fn advance_stop_is_not_modelled_and_changes_nothing() {
        let mut a = sample();
        assert!(!advance(&mut a, Advance::Stop, NOW + sec(100), scale1));
        assert_eq!(a, sample());
    }

    #[test]
    fn advance_mid_puts_now_half_way_from_the_move_start() {
        let mut e = sample();
        let now = e.start_move + sec(1_000);
        assert!(advance(&mut e, Advance::Mid, now, scale1));
        assert_eq!(e.chunk_available - e.start_move, 2 * (now - e.start_move));
        // Before the move starts nothing changes.
        let mut f = sample();
        assert!(!advance(&mut f, Advance::Mid, NOW, scale1));
        assert_eq!(f, sample());
    }

    #[test]
    fn advance_decay_sets_the_automatic_fracture_one_second_out() {
        let mut e = sample();
        let now = NOW + sec(100);
        assert!(advance(&mut e, Advance::Decay, now, scale1));
        assert_eq!(e.natural_decay, now + sec(1));
        assert!(
            e.chunk_available <= now,
            "the chunk is there before it decays"
        );
        // A chunk that already arrived keeps its stamp.
        let mut r = sample();
        r.chunk_available = NOW + sec(10);
        assert!(advance(&mut r, Advance::Decay, now, scale1));
        assert_eq!(r.chunk_available, NOW + sec(10));
    }

    #[test]
    fn advance_acts_only_in_the_phases_it_names() {
        let now = NOW + sec(100);
        // Ready: Decay still moves, the others do not.
        let mut r = sample();
        r.phase = Phase::Ready;
        for m in [Advance::Mid, Advance::Ready, Advance::Stop] {
            let mut x = r.clone();
            assert!(!advance(&mut x, m, now, scale1), "{m:?}");
            assert_eq!(x, r);
        }
        assert!(advance(&mut r, Advance::Decay, now, scale1));
        // Fracturing and a field are left alone by every mode.
        for p in [
            Phase::Fracturing {
                fired_at: 5,
                by: Some(1),
            },
            Phase::Field { until: 99 },
        ] {
            let mut x = sample();
            x.phase = p;
            for m in [Advance::Mid, Advance::Ready, Advance::Stop, Advance::Decay] {
                let mut y = x.clone();
                assert!(!advance(&mut y, m, now, scale1), "{p:?} {m:?}");
                assert_eq!(y, x);
            }
        }
    }

    #[test]
    fn advance_words_parse() {
        assert_eq!(Advance::parse("mid"), Some(Advance::Mid));
        assert_eq!(Advance::parse("stop"), Some(Advance::Stop));
        assert_eq!(Advance::parse("ready"), Some(Advance::Ready));
        assert_eq!(Advance::parse("decay"), Some(Advance::Decay));
        assert_eq!(Advance::parse("later"), None);
    }
}
