//! Moon mining extractions (#53, pure core): the client's model, with no world.
//!
//! Spec: `parity-moon-drills.md` §2 and §L2.1 (the lane specs). This file holds what
//! can be decided without a `Registry`: the client's constants, the extraction
//! record, the duration and yield rules, the stage, the row text and the
//! `KeyVal` the client reads. The world part (start, cancel, tick, pushes,
//! notifications) comes in a later lane, once structures exist.
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

use eve_marshal::PyValue;

use crate::moon::Share;
use crate::pyutil::keyval;

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
