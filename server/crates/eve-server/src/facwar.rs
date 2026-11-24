//! Factional warfare: the warzones, their systems, who holds each, and the
//! victory points that move them.
//!
//! W12 packet CP18 (`docs/content/plan.md` §6.2; spec `docs/content/R-FW.md`
//! §2-§3) retired lane B1's `WARZONES` and `HQ_SYSTEMS` consts in favour of
//! the authored table `content/facwar/warzones.tsv` ([`WARZONES`]).
//!
//! W17 packet CP53b (spec R-FW §2, §3, §5, §6; C2 §5; lane note
//! `docs/lanes/cp53b.md`) adds the rest:
//!
//! - **The warzone systems** ([`Facwar`], the world slot [`SLOT`]), built
//!   from content only (CA11): every low-sec system whose `faction_id` is a
//!   militia of a warzone, plus each warzone's HQs ([`warzone_systems`]).
//!   **[A]** (the fallback only): R-FW §2 found the client's own list
//!   (`warzonespositiondata`, the visible rows with a `territorySprite`, 163
//!   systems). FW-3 extracts it to `facwar/systems.tsv` ([`SYSTEMS_FILE`]);
//!   when the bundle has it, that list is the warzone and the rule is not
//!   used (the rule gave 595 systems, 433 of them ordinary low sec the client
//!   never draws as territory). With no such file the rule stands, which is
//!   what keeps fixtures working. *Falsified by* the FW dashboard map drawing
//!   a territory system this rule leaves out, or the reverse.
//! - **Who holds each** ([`occupation`]): the `fwvp` delta ([`KIND`]),
//!   `system_id -> score occupier taken_at credited`, read lazily; a system
//!   with no row is held by its seed occupier, its own `faction_id` (R-FW §2
//!   [A]), at score 0.
//! - **Victory points** ([`tick`], the liveness pattern): the stage pulls
//!   every completed FW complex out of `population`'s `site` slots
//!   ([`FW_ARCHETYPES`]) and moves its system's score by [`vp_for`]. A
//!   complex belongs to the militia its dungeon names (R-FW §6 [I]), so
//!   completing the occupier's raises the score and completing the
//!   attacker's lowers it. `OnVictoryPointStateUpdated(ssid, VPState)` [V]
//!   goes to the characters in the system.
//! - **The flip** ([A], [`VP_THRESHOLD`]): a system whose score reaches the
//!   threshold changes hands at once (occupier and attacker swap, the score
//!   goes to 0, `dateTaken` is now). R-FW §5 rule 4 had it wait for the
//!   rollover; the plan's row asks for the flip on crossing, which is also
//!   the shorter loop for a few players. *Falsified by* players asking for a
//!   window to defend a vulnerable system. Every system whose
//!   `OccupationState` moved (the system, and neighbours whose adjacency
//!   moved) is told with `OnWarzoneOccupationStateUpdated(ssid, state)` [V].
//!
//! **Not here.** Kills do not move VP (R-FW §5 names complexes only), so no
//! `kill_hooks` row (battlefield scoreboards have their own, `fw_scoreboard`).
//! The completion scan skips battlefields (archetype 68): `fw_scoreboard`
//! credits a won one. The bunker vulnerable/non-vulnerable dialogs are later
//! work (lane note, Not done).

use crate::celestial::{Band, Universe};
use crate::content::{Boxed, Content, Sources, TableSpec};
use crate::damage::BlueTime;
use crate::pyutil::new_style;
use crate::service::Registry;
use crate::world_ext::SlotSpec;
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The authored file, relative to `--authored`.
pub const FILE: &str = "facwar/warzones.tsv";

/// Its header, frozen in `docs/content/schema.md` §4.
pub const HEADER: &str = "warzone_id\tfaction_a\tfaction_b\thq_a\thq_b\twhy";

/// The shipped copy, so the binary always has the defaults (plan §1.4).
const SHIPPED: &str = include_str!("../../../../content/facwar/warzones.tsv");

/// The extracted territory list, relative to the bundle: one row per system
/// the client draws as warzone territory (`schema.md` §2, FW-3).
pub const SYSTEMS_FILE: &str = "facwar/systems.tsv";

/// Its header.
pub const SYSTEMS_HEADER: &str = "#system_id\twarzone_id";

/// The most one kill pays by victim hull group (`appConst.facwarMaxLPPayout`,
/// #133), extracted by `content_facwar.py`; optional, like [`SYSTEMS_FILE`].
pub const MAX_LP_FILE: &str = "facwar/max_lp.tsv";

/// Its header.
pub const MAX_LP_HEADER: &str = "#group_id\tmax_lp";

/// The battlefield scoreboards' win thresholds (`fwScoreboard`'s
/// `attackerWinsThreshold` and `occupierWinsThreshold`, #8), extracted by
/// `content_facwar.py`; optional, like [`MAX_LP_FILE`].
pub const SCOREBOARDS_FILE: &str = "facwar/scoreboards.tsv";

/// Its header.
pub const SCOREBOARDS_HEADER: &str = "#type_id\tattacker_wins\toccupier_wins";

/// One row of [`SCOREBOARDS_FILE`]: the type and its two optional, signed
/// thresholds (an empty field is `None`).
pub type ScoreboardRow = (i64, Option<i64>, Option<i64>);

/// Parse one data line of [`SCOREBOARDS_FILE`].
///
/// # Errors
///
/// A line that is not three TAB fields, a type that is not an integer, or a
/// threshold that is neither empty nor an integer.
pub fn parse_scoreboard_line(line: &str) -> Result<ScoreboardRow, String> {
    let bad = || format!("{SCOREBOARDS_FILE}: bad row {line:?}");
    let f: Vec<&str> = line.split('\t').collect();
    let [t, a, o] = f[..] else {
        return Err(bad());
    };
    let opt = |s: &str| -> Result<Option<i64>, String> {
        if s.is_empty() {
            Ok(None)
        } else {
            s.parse().map(Some).map_err(|_| bad())
        }
    };
    Ok((t.parse().map_err(|_| bad())?, opt(a)?, opt(o)?))
}

/// The capture points' attributes (`fwCapturePoint`, #75), extracted by
/// `content_facwar.py`; optional, like [`MAX_LP_FILE`].
pub const CAPTURE_POINTS_FILE: &str = "facwar/capture_points.tsv";

/// Its header.
pub const CAPTURE_POINTS_HEADER: &str =
    "#type_id\tseconds\trange\tpoint_value\tlooping\tcapturing\tblocking";

/// One capture point type's attributes as the bundle states them; an empty
/// field is `None` (the server then uses its assumed value for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapturePointRow {
    /// `numberOfSeconds`.
    pub seconds: Option<i64>,
    /// `range`, metres.
    pub range: Option<i64>,
    /// `pointValue`.
    pub point_value: Option<i64>,
    /// `looping`.
    pub looping: Option<bool>,
    /// `capturingTypeListID`, a `types/typelists.tsv` id.
    pub capturing: Option<i64>,
    /// `blockingTypeListID`, likewise.
    pub blocking: Option<i64>,
}

/// Parse one data line of [`CAPTURE_POINTS_FILE`].
///
/// # Errors
///
/// A line that is not seven TAB fields, a type that is not an integer, a
/// field that is neither empty nor an integer, or a `looping` that is not
/// empty, `0` or `1`.
pub fn parse_capture_point_line(line: &str) -> Result<(i64, CapturePointRow), String> {
    let bad = || format!("{CAPTURE_POINTS_FILE}: bad row {line:?}");
    let f: Vec<&str> = line.split('\t').collect();
    let [t, secs, range, value, looping, cap, block] = f[..] else {
        return Err(bad());
    };
    let opt = |s: &str| -> Result<Option<i64>, String> {
        if s.is_empty() {
            Ok(None)
        } else {
            s.parse().map(Some).map_err(|_| bad())
        }
    };
    let looping = match looping {
        "" => None,
        "0" => Some(false),
        "1" => Some(true),
        _ => return Err(bad()),
    };
    Ok((
        t.parse().map_err(|_| bad())?,
        CapturePointRow {
            seconds: opt(secs)?,
            range: opt(range)?,
            point_value: opt(value)?,
            looping,
            capturing: opt(cap)?,
            blocking: opt(block)?,
        },
    ))
}

/// A row with a non-positive `seconds` or `range` loses that field (the
/// server then uses its assumed value), with a note saying so.
pub fn sanitized_capture_row(mut row: CapturePointRow) -> (CapturePointRow, Vec<String>) {
    let mut notes = Vec::new();
    if let Some(v) = row.seconds.filter(|v| *v <= 0) {
        notes.push(format!(
            "seconds {v} is not positive, the assumed value stands"
        ));
        row.seconds = None;
    }
    if let Some(v) = row.range.filter(|v| *v <= 0) {
        notes.push(format!(
            "range {v} is not positive, the assumed value stands"
        ));
        row.range = None;
    }
    (row, notes)
}

/// This module's registration row, one line in `TABLES` in
/// `content_tables.rs` (coordinator-only).
pub const WARZONES: TableSpec = TableSpec {
    name: "facwar",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    let file = src.authored(FILE, SHIPPED, HEADER)?;
    let (rows, _) = parse(&file);
    let mut table = Table::new(rows).map_err(|e| format!("{FILE}: {e}"))?;
    if let Some(b) = src.bundle {
        for line in b.data_lines(MAX_LP_FILE, MAX_LP_HEADER)? {
            let mut f = line.split('\t').map(str::parse::<i64>);
            match (f.next(), f.next()) {
                (Some(Ok(g)), Some(Ok(m))) if m >= 0 => {
                    table.max_lp.insert(g, m);
                }
                _ => return Err(format!("{MAX_LP_FILE}: bad row {line:?}")),
            }
        }
        for line in b.data_lines(SCOREBOARDS_FILE, SCOREBOARDS_HEADER)? {
            let (t, a, o) = parse_scoreboard_line(&line)?;
            table.scoreboards.insert(t, (a, o));
        }
        for line in b.data_lines(CAPTURE_POINTS_FILE, CAPTURE_POINTS_HEADER)? {
            let (t, row) = parse_capture_point_line(&line)?;
            let (row, notes) = sanitized_capture_row(row);
            for n in notes {
                eprintln!("[content] {CAPTURE_POINTS_FILE}: type {t}: {n}");
            }
            table.capture_points.insert(t, row);
        }
        for line in b.data_lines(SYSTEMS_FILE, SYSTEMS_HEADER)? {
            let mut f = line.split('\t').map(str::parse::<i64>);
            match (f.next(), f.next()) {
                (Some(Ok(s)), Some(Ok(w))) => {
                    table.systems.insert(s, w);
                }
                _ => return Err(format!("{SYSTEMS_FILE}: bad row {line:?}")),
            }
        }
    }
    Ok(Box::new(table))
}

/// Every well-formed row, and one line per row that was skipped.
fn parse(file: &crate::authored::File) -> (Vec<Warzone>, Vec<String>) {
    file.parse_rows_logged(|r| {
        Ok(Warzone {
            id: r.i64(0)?,
            factions: [r.i64(1)?, r.i64(2)?],
            hqs: [r.i64(3)?, r.i64(4)?],
        })
    })
}

fn boxed_fixture() -> Boxed {
    Box::new(Table::fixture())
}

/// One row of `warzones.tsv`: a warzone, its two militias, and each
/// militia's HQ system (`hqs[i]` belongs to `factions[i]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warzone {
    /// The client's warzone id, the key of `warzonespositiondata`.
    pub id: i64,
    /// Side A and side B.
    pub factions: [i64; 2],
    /// Side A's HQ system, then side B's.
    pub hqs: [i64; 2],
}

/// Every warzone, ascending by id. Never empty (see [`Table::new`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    warzones: Vec<Warzone>,
    /// `system -> warzone` from [`SYSTEMS_FILE`]; empty without a bundle
    /// file, and then [`warzone_systems`] falls back to its rule.
    systems: BTreeMap<i64, i64>,
    /// `hull group -> the most a kill pays` from [`MAX_LP_FILE`]; empty
    /// without one, and then every kill is capped by the unknown-group figure.
    max_lp: BTreeMap<i64, i64>,
    /// `scoreboard type -> (attacker_wins, occupier_wins)` from
    /// [`SCOREBOARDS_FILE`]; empty without one.
    scoreboards: BTreeMap<i64, (Option<i64>, Option<i64>)>,
    /// `capture point type -> attributes` from [`CAPTURE_POINTS_FILE`]; empty
    /// without one.
    capture_points: BTreeMap<i64, CapturePointRow>,
}

impl Table {
    /// The rows, checked and sorted.
    ///
    /// # Errors
    ///
    /// No rows at all (`GetAllWarzones` would be `{}`, an `IndexError` at
    /// `fwWarzoneSvc.GetWarzoneIdForFaction:207`), a duplicate warzone id, or
    /// a faction that fights in two warzones or against itself (the client
    /// maps each faction to one warzone, `:203-206`).
    pub fn new(mut warzones: Vec<Warzone>) -> Result<Table, String> {
        if warzones.is_empty() {
            return Err("no warzones: GetAllWarzones may never be empty".to_owned());
        }
        warzones.sort_by_key(|w| w.id);
        if let Some(w) = warzones.windows(2).find(|p| p[0].id == p[1].id) {
            return Err(format!("warzone {} is listed twice", w[0].id));
        }
        let mut seen: Vec<i64> = Vec::new();
        for w in &warzones {
            for f in w.factions {
                if seen.contains(&f) {
                    return Err(format!("faction {f} fights in two warzones (or itself)"));
                }
                seen.push(f);
            }
        }
        Ok(Table {
            warzones,
            systems: BTreeMap::new(),
            max_lp: BTreeMap::new(),
            scoreboards: BTreeMap::new(),
            capture_points: BTreeMap::new(),
        })
    }

    /// Invented rows (ids from `schema.md` §1.1's unused ranges), never a
    /// client value.
    pub fn fixture() -> Table {
        Table::new(vec![
            Warzone {
                id: 98,
                factions: [599_001, 599_002],
                hqs: [39_000_001, 39_000_002],
            },
            Warzone {
                id: 99,
                factions: [599_003, 599_004],
                hqs: [39_000_003, 39_000_004],
            },
        ])
        .expect("the fixture is well-formed")
    }

    /// The same table with a listed territory (`system -> warzone`).
    pub fn with_systems(mut self, systems: BTreeMap<i64, i64>) -> Table {
        self.systems = systems;
        self
    }

    /// The same table with per-group kill caps (`group -> LP`).
    pub fn with_max_lp(mut self, max_lp: BTreeMap<i64, i64>) -> Table {
        self.max_lp = max_lp;
        self
    }

    /// The same table with scoreboard thresholds (`type -> (attacker, occupier)`).
    pub fn with_scoreboards(
        mut self,
        scoreboards: BTreeMap<i64, (Option<i64>, Option<i64>)>,
    ) -> Table {
        self.scoreboards = scoreboards;
        self
    }

    /// A scoreboard type's raw `(attackerWinsThreshold, occupierWinsThreshold)`,
    /// if the bundle lists the type. Either may be `None`: the component
    /// leaves it out.
    pub fn scoreboard_of(&self, type_id: i64) -> Option<(Option<i64>, Option<i64>)> {
        self.scoreboards.get(&type_id).copied()
    }

    /// The same table with capture point attributes (`type -> row`).
    pub fn with_capture_points(mut self, rows: BTreeMap<i64, CapturePointRow>) -> Table {
        self.capture_points = rows;
        self
    }

    /// The capture point types the bundle lists, ascending.
    pub fn capture_point_types(&self) -> Vec<i64> {
        self.capture_points.keys().copied().collect()
    }

    /// A capture point type's attributes, if the bundle lists the type.
    pub fn capture_point_of(&self, type_id: i64) -> Option<CapturePointRow> {
        self.capture_points.get(&type_id).copied()
    }

    /// The most a kill of a hull of `group` pays, if the bundle says.
    pub fn max_lp_of(&self, group: i64) -> Option<i64> {
        self.max_lp.get(&group).copied()
    }

    /// The client's territory list, `system -> warzone`; empty when the
    /// bundle has none.
    pub fn listed_systems(&self) -> &BTreeMap<i64, i64> {
        &self.systems
    }

    /// Every warzone, ascending by id.
    pub fn warzones(&self) -> &[Warzone] {
        &self.warzones
    }

    /// The warzone `faction` fights in, if any.
    pub fn warzone_of(&self, faction: i64) -> Option<&Warzone> {
        self.warzones.iter().find(|w| w.factions.contains(&faction))
    }

    /// `faction`'s HQ system, if it is a militia.
    pub fn hq_of(&self, faction: i64) -> Option<i64> {
        let w = self.warzone_of(faction)?;
        let side = w.factions.iter().position(|f| *f == faction)?;
        Some(w.hqs[side])
    }

    /// `GetAllWarzones`: `{warzoneID: [factionA, factionB]}`
    /// (`fwWarzoneSvc.GetWarzoneIdForFaction:203-207`).
    pub fn all_warzones(&self) -> PyValue {
        PyValue::Dict(
            self.warzones
                .iter()
                .map(|w| {
                    (
                        PyValue::List(w.factions.iter().map(|f| PyValue::Int(*f)).collect()),
                        PyValue::Int(w.id),
                    )
                })
                .collect(),
        )
    }

    /// `GetHQSystemIDs`: `{factionID: solarSystemID}` for every warzone
    /// faction (`fwWarzoneSvc.GetFactionHQSystem:211-212`).
    pub fn hq_system_ids(&self) -> PyValue {
        PyValue::Dict(
            self.warzones
                .iter()
                .flat_map(|w| {
                    w.factions
                        .iter()
                        .zip(w.hqs)
                        .map(|(f, hq)| (PyValue::Int(hq), PyValue::Int(*f)))
                })
                .collect(),
        )
    }
}

/// `factionwarfare/const:1-3` [V]: a system's distance from the front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjacency {
    /// `'FL'`, frontline.
    Frontline,
    /// `'SL'`, command operations.
    Secondline,
    /// `'BL'`, rearguard.
    Backline,
}

impl Adjacency {
    /// The wire string, one of `ADJACENCY_STATES`.
    pub fn as_str(self) -> &'static str {
        match self {
            Adjacency::Frontline => "FL",
            Adjacency::Secondline => "SL",
            Adjacency::Backline => "BL",
        }
    }
}

/// The client's class for a system's occupation (`occupationState:10-27`).
pub const OCCUPATION_STATE: &str = "factionwarfare.occupationState.OccupationState";

/// One system's `OccupationState`, a new-style object whose `__init__` never
/// runs (R-FW §3), so the keys are the **underscored** attribute names and
/// the caller upholds the invariants: `attacker` is the other militia,
/// `pirate` is not `None`. `date_taken` is a FILETIME, or `None` for never
/// (`mapData:127` sorts on it).
///
/// ```
/// use eve_server::facwar::{occupation_state, Adjacency};
/// let v = occupation_state(599_001, 599_002, Adjacency::Backline, 599_009, None);
/// assert!(matches!(v, eve_marshal::PyValue::ObjectEx { .. }));
/// ```
pub fn occupation_state(
    occupier: i64,
    attacker: i64,
    adjacency: Adjacency,
    pirate: i64,
    date_taken: Option<i64>,
) -> PyValue {
    new_style(
        OCCUPATION_STATE,
        vec![
            ("_occupierID", PyValue::Int(occupier)),
            ("_attackerID", PyValue::Int(attacker)),
            ("_adjacencyState", crate::pyutil::bstr(adjacency.as_str())),
            ("_pirateFactionID", PyValue::Int(pirate)),
            ("_dateTaken", date_taken.map_or(PyValue::None, PyValue::Int)),
        ],
    )
}

// ---------------------------------------------------------------------------
// W17 CP53b: the warzone systems, occupation and victory points.

/// `appConst.dunArchetypesFactionalWarfare` [V] (`appConst:614`, R-FW §6):
/// the four complexes (33-36) and battlefields (68). `population` keeps a
/// row of these archetypes to the warzone systems ([`militias_by_system`]).
pub const FW_ARCHETYPES: [i64; 5] = [33, 34, 35, 36, 68];

/// The delta kind this module owns (`delta_kinds::KINDS`, `("fwvp",
/// "facwar")`): `system_id -> score occupier taken_at credited`, where
/// `taken_at` is a FILETIME or empty for never, and `credited` is the
/// completion time of the newest complex already counted. Never expires.
pub const KIND: &str = "fwvp";

/// `factionwarfare/const.VICTORY_POINTS_THRESHOLD` [V] (`const:19`): every
/// system's threshold (R-FW §5 rule 1 [I]: constant, per system).
pub const VP_THRESHOLD: i64 = 75_000;

/// `factionwarfare/const.MAX_VICTORY_POINTS_OVER_THRESHOLD` [V] (`const:20`):
/// the score's cap above [`VP_THRESHOLD`] ([I] from the name).
pub const MAX_OVER_THRESHOLD: i64 = 500;

/// **[A] The victory points one completed complex is worth**, by archetype
/// (R-FW §5 rule 2): Novice, Small, Medium, Large, Battlefield, rising with
/// the size. The per-site value is server-only, so nothing in the client
/// falsifies it; humans who call a capture too slow or too quick do.
pub const VP_BY_ARCHETYPE: [(i64, i64); 5] = [
    (33, 2_500),
    (34, 5_000),
    (35, 7_500),
    (36, 10_000),
    (68, 15_000),
];

/// [A] How often the stage reads the `site` slots for new completions.
/// A complex stays completed for its row's `respawn_s` (far longer), so no
/// completion is missed; falsified by a VP update a pilot notices is late.
pub const SCAN_EVERY: BlueTime = 10 * 10_000_000;

/// The client's class for a system's victory points (`victoryPointState:5-15`).
pub const VICTORY_POINT_STATE: &str = "factionwarfare.victoryPointState.VictoryPointState";

/// The points a completed complex of `archetype` is worth ([`VP_BY_ARCHETYPE`]).
pub fn vp_for(archetype: i64) -> Option<i64> {
    VP_BY_ARCHETYPE
        .iter()
        .find(|(a, _)| *a == archetype)
        .map(|(_, v)| *v)
}

/// One system's `VictoryPointState` (R-FW §3 [V]): new-style, underscored
/// keys, `_vpScore >= 0` and `_vpThreshold > 0`, which the caller upholds.
///
/// ```
/// let v = eve_server::facwar::victory_point_state(0);
/// assert!(matches!(v, eve_marshal::PyValue::ObjectEx { .. }));
/// ```
pub fn victory_point_state(score: i64) -> PyValue {
    new_style(
        VICTORY_POINT_STATE,
        vec![
            ("_vpScore", PyValue::Int(score.max(0))),
            ("_vpThreshold", PyValue::Int(VP_THRESHOLD)),
        ],
    )
}

/// A warzone system, as content seeds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FwSystem {
    pub warzone: i64,
    /// [A] R-FW §2: the system's own `faction_id`.
    pub seed_occupier: i64,
    /// The warzone's other militia.
    pub seed_attacker: i64,
}

/// The warzone systems from content: `system -> FwSystem` (module header,
/// [A]). Empty without a universe or a warzone table.
pub fn warzone_systems(c: &Content) -> BTreeMap<i64, FwSystem> {
    let (Some(table), Some(u)) = (c.try_get::<Table>(), c.try_get::<Universe>()) else {
        return BTreeMap::new();
    };
    if !table.systems.is_empty() {
        return listed_systems(table, u);
    }
    let mut out = BTreeMap::new();
    for s in u.systems() {
        let Some(f) = s.faction_id else { continue };
        let Some(w) = table.warzone_of(f) else {
            continue;
        };
        let is_hq = w.hqs.contains(&s.id);
        if s.band() != Band::Low && !is_hq {
            continue;
        }
        let other = if w.factions[0] == f {
            w.factions[1]
        } else {
            w.factions[0]
        };
        out.insert(
            s.id,
            FwSystem {
                warzone: w.id,
                seed_occupier: f,
                seed_attacker: other,
            },
        );
    }
    out
}

/// The listed territory as `FwSystem`s. A system the universe lacks, a warzone
/// the table lacks, or one whose `faction_id` is not a militia of that
/// warzone is logged and skipped; the seed occupier is the system's own
/// `faction_id` (R-FW §2 [A]).
fn listed_systems(table: &Table, u: &Universe) -> BTreeMap<i64, FwSystem> {
    let mut out = BTreeMap::new();
    for (&id, &zone) in &table.systems {
        let (Some(w), Some(s)) = (table.warzones().iter().find(|w| w.id == zone), u.system(id))
        else {
            crate::outln!(
                "[facwar] {SYSTEMS_FILE}: system {id} or warzone {zone} unknown, skipped"
            );
            continue;
        };
        let Some(f) = s.faction_id.filter(|f| w.factions.contains(f)) else {
            crate::outln!("[facwar] system {id} is held by no militia of warzone {zone}, skipped");
            continue;
        };
        let other = if w.factions[0] == f {
            w.factions[1]
        } else {
            w.factions[0]
        };
        out.insert(
            id,
            FwSystem {
                warzone: zone,
                seed_occupier: f,
                seed_attacker: other,
            },
        );
    }
    out
}

/// Every warzone system's two militias, side A first: what
/// `population::Prepared::with_warzones` keeps the FW rows to.
pub fn militias_by_system(c: &Content) -> BTreeMap<i64, [i64; 2]> {
    let Some(table) = c.try_get::<Table>() else {
        return BTreeMap::new();
    };
    warzone_systems(c)
        .into_iter()
        .filter_map(|(s, fw)| {
            let w = table.warzones().iter().find(|w| w.id == fw.warzone)?;
            Some((s, w.factions))
        })
        .collect()
}

/// The registration row in `world_ext::SLOTS` (coordinator-only). Its
/// `make` reads content only, never deltas (CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "facwar",
    make: |c| Box::new(Facwar::build(c)) as Boxed,
};

/// The world slot: the warzone systems, each warzone's pirate, and the gate
/// graph between warzone systems (read from the catalogue on first use).
#[derive(Debug, Clone, Default)]
pub struct Facwar {
    systems: BTreeMap<i64, FwSystem>,
    /// `warzone -> its pirate faction` (`_pirateFactionID`).
    pirates: BTreeMap<i64, i64>,
    /// `warzone -> its systems`, ascending.
    by_warzone: BTreeMap<i64, Vec<i64>>,
    neighbours: Option<BTreeMap<i64, Vec<i64>>>,
    next_scan: BlueTime,
    /// The capture points (`fw_capture`, #75).
    capture: crate::fw_capture::State,
}

impl Facwar {
    /// The capture points' state.
    pub fn capture(&self) -> &crate::fw_capture::State {
        &self.capture
    }

    pub(crate) fn capture_mut(&mut self) -> &mut crate::fw_capture::State {
        &mut self.capture
    }

    /// From content: the systems, and each warzone's pirate. **[I]** a
    /// warzone's pirate is the rat faction (`npc_data::region_pirate`, CP20)
    /// of its first system that has one (R-FW §2 names Angels for the
    /// Amarr-Minmatar zone and Guristas for Caldari-Gallente, the rats of
    /// those regions); **[A]** with none, side B's militia, so the
    /// invariant "not `None`" holds. Falsified by a pirate insurgency
    /// panel naming the wrong pirate.
    pub fn build(c: &Content) -> Facwar {
        let systems = warzone_systems(c);
        let mut by_warzone: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for (s, fw) in &systems {
            by_warzone.entry(fw.warzone).or_default().push(*s);
        }
        let npc = c.try_get::<crate::npc_data::Table>();
        let u = c.try_get::<Universe>();
        let mut pirates = BTreeMap::new();
        if let Some(t) = c.try_get::<Table>() {
            for w in t.warzones() {
                let found = by_warzone.get(&w.id).into_iter().flatten().find_map(|s| {
                    let region = u?.system(*s)?.region_id;
                    npc?.region_pirate(region)
                });
                pirates.insert(w.id, found.unwrap_or(w.factions[1]));
            }
        }
        Facwar {
            systems,
            pirates,
            by_warzone,
            neighbours: None,
            next_scan: BlueTime::MIN,
            capture: crate::fw_capture::State::default(),
        }
    }

    /// The warzone system `system`, if it is one.
    pub fn system(&self, system: i64) -> Option<&FwSystem> {
        self.systems.get(&system)
    }

    /// Every warzone system, ascending.
    pub fn systems(&self) -> impl Iterator<Item = (i64, &FwSystem)> + '_ {
        self.systems.iter().map(|(s, fw)| (*s, fw))
    }
}

/// One system's occupation, from its `fwvp` row or the seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occupation {
    pub occupier: i64,
    pub attacker: i64,
    pub score: i64,
    /// FILETIME; `None` for never taken.
    pub taken_at: Option<i64>,
    /// The completion time of the newest complex already counted.
    pub credited: BlueTime,
}

/// `system`'s occupation, `None` outside the warzones. A row whose occupier
/// is not one of the warzone's militias is ignored (the seed holds).
pub fn occupation(reg: &Registry, system: i64) -> Option<Occupation> {
    let fw = *reg.ext::<Facwar>().system(system)?;
    let seed = Occupation {
        occupier: fw.seed_occupier,
        attacker: fw.seed_attacker,
        score: 0,
        taken_at: None,
        credited: BlueTime::MIN,
    };
    let Some(v) = reg.deltas().of(KIND).get(&vec![system.to_string()]) else {
        return Some(seed);
    };
    let int = |i: usize| v.get(i).and_then(|s| s.parse::<i64>().ok());
    let (Some(score), Some(occupier)) = (int(0), int(1)) else {
        return Some(seed);
    };
    let pair = [fw.seed_occupier, fw.seed_attacker];
    if !pair.contains(&occupier) {
        return Some(seed);
    }
    let attacker = if occupier == pair[0] {
        pair[1]
    } else {
        pair[0]
    };
    Some(Occupation {
        occupier,
        attacker,
        score: score.clamp(0, VP_THRESHOLD + MAX_OVER_THRESHOLD),
        taken_at: int(2),
        credited: int(3).unwrap_or(BlueTime::MIN),
    })
}

fn store(reg: &mut Registry, system: i64, o: &Occupation) {
    let value = vec![
        o.score.to_string(),
        o.occupier.to_string(),
        o.taken_at.map_or_else(String::new, |t| t.to_string()),
        o.credited.to_string(),
    ];
    reg.deltas_mut().put(KIND, vec![system.to_string()], value);
}

/// How many warzone systems `faction` holds (`GetStats_FactionInfo`'s
/// `systemsCount`, R-FW §1 #20).
pub fn systems_held(reg: &Registry, faction: i64) -> usize {
    let ids: Vec<i64> = reg.ext::<Facwar>().systems.keys().copied().collect();
    ids.into_iter()
        .filter(|s| occupation(reg, *s).is_some_and(|o| o.occupier == faction))
        .count()
}

/// `facwarSolarSystemUpgradeThresholds` [V] (`appConst`, read with
/// `tools/dis.py`): the LP pool each upgrade level starts at.
pub const UPGRADE_THRESHOLDS: [i64; 7] = [0, 40_000, 60_000, 90_000, 140_000, 200_000, 300_000];

/// `facwarCommon.GetLPUpgradeLevel` [V]: the pool at or past the last
/// threshold is `facWarSolarSystemMaxLevel` (5), an empty pool is 0, else
/// the level below the first threshold the pool is under.
pub fn upgrade_level(lps: i64) -> i64 {
    if lps >= UPGRADE_THRESHOLDS[UPGRADE_THRESHOLDS.len() - 1] {
        return 5;
    }
    if lps <= 0 {
        return 0;
    }
    UPGRADE_THRESHOLDS
        .iter()
        .position(|t| lps < *t)
        .map_or(0, |i| i as i64 - 1)
}

/// `map.GetFacWarZoneInfo(factionID)`: `factionPoints` (the systems the
/// faction holds) over `maxWarZonePoints` (the warzone's systems), which
/// `facwarCommon.GetDonationTax` divides, so it is never 0; and
/// `systemUpgradeLevel`, `{system: level}` over the systems it holds, from
/// each iHub's LP pool. Both points are **[A]**: the ratio is what the tax
/// reads, and it is falsified by a donation tax that disagrees with the
/// share of the zone held.
pub fn zone_info(reg: &Registry, faction: i64) -> PyValue {
    let held: Vec<i64> = reg
        .ext::<Facwar>()
        .systems()
        .filter(|(s, _)| occupation(reg, *s).is_some_and(|o| o.occupier == faction))
        .map(|(s, _)| s)
        .collect();
    let zone = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.warzone_of(faction))
        .map(|w| w.id);
    let total = reg
        .ext::<Facwar>()
        .systems()
        .filter(|(_, fw)| Some(fw.warzone) == zone)
        .count();
    let levels = held
        .iter()
        .map(|s| {
            let lps = crate::service::facwar_mgr::lp_pool(reg, *s);
            (PyValue::Int(upgrade_level(lps)), PyValue::Int(*s))
        })
        .collect();
    crate::pyutil::keyval(vec![
        ("factionID", PyValue::Int(faction)),
        ("factionPoints", PyValue::Int(held.len() as i64)),
        ("maxWarZonePoints", PyValue::Int(total.max(1) as i64)),
        ("systemUpgradeLevel", PyValue::Dict(levels)),
    ])
}

/// The gate graph between warzone systems, built once from the catalogue
/// (which does not change while the server runs).
fn neighbours(reg: &mut Registry) -> &BTreeMap<i64, Vec<i64>> {
    if reg.ext::<Facwar>().neighbours.is_none() {
        let ids: Vec<i64> = reg.ext::<Facwar>().systems.keys().copied().collect();
        let cat = reg.celestials();
        let graph: BTreeMap<i64, Vec<i64>> = ids
            .iter()
            .map(|s| {
                let mut out: Vec<i64> = cat
                    .served_in(*s)
                    .filter(|c| c.kind == crate::celestial::Kind::Gate)
                    .filter(|c| cat.system_of(c.id) == Some(*s))
                    .filter_map(|c| cat.jump_of(c.id))
                    .map(|(_, to)| to)
                    .filter(|to| to != s && ids.binary_search(to).is_ok())
                    .collect();
                out.sort_unstable();
                out.dedup();
                (*s, out)
            })
            .collect();
        reg.ext_mut::<Facwar>().neighbours = Some(graph);
    }
    reg.ext::<Facwar>()
        .neighbours
        .as_ref()
        .expect("built above")
}

/// **[A]** R-FW §2: `FL` when a gate neighbour is held by the other
/// militia, `SL` when a neighbour is `FL`, else `BL`. Falsified by the
/// dashboard's front lines disagreeing with the occupation on screen.
pub fn adjacency(reg: &mut Registry, system: i64) -> Adjacency {
    let graph = neighbours(reg).clone();
    adjacency_in(reg, &graph, system)
}

fn adjacency_in(reg: &Registry, graph: &BTreeMap<i64, Vec<i64>>, system: i64) -> Adjacency {
    let held = |s: i64| occupation(reg, s).map(|o| o.occupier);
    let front = |s: i64| {
        let mine = held(s);
        graph
            .get(&s)
            .into_iter()
            .flatten()
            .any(|n| held(*n).is_some() && held(*n) != mine)
    };
    if front(system) {
        Adjacency::Frontline
    } else if graph.get(&system).into_iter().flatten().any(|n| front(*n)) {
        Adjacency::Secondline
    } else {
        Adjacency::Backline
    }
}

fn state_in(reg: &Registry, graph: &BTreeMap<i64, Vec<i64>>, system: i64) -> Option<PyValue> {
    let o = occupation(reg, system)?;
    let fw = reg.ext::<Facwar>().system(system)?;
    let pirate = reg.ext::<Facwar>().pirates.get(&fw.warzone).copied()?;
    Some(occupation_state(
        o.occupier,
        o.attacker,
        adjacency_in(reg, graph, system),
        pirate,
        o.taken_at,
    ))
}

/// `system`'s `OccupationState`, `None` outside the warzones
/// (`GetLocalOccupationState`'s honest "not a warzone system").
pub fn local_state(reg: &mut Registry, system: i64) -> Option<PyValue> {
    let graph = neighbours(reg).clone();
    state_in(reg, &graph, system)
}

/// `GetAllWarzonesOccupationStates`: `{warzoneID: {ssid: OccupationState}}`
/// (R-FW §3). **Every warzone key is present**, with or without systems:
/// `mapData.GetWarzoneSystems:59` and `:123` subscript it unguarded, so a
/// missing key is a `KeyError` (R-FW §0 item 2).
pub fn all_occupation_states(reg: &mut Registry) -> PyValue {
    let graph = neighbours(reg).clone();
    let zones: Vec<i64> = reg
        .content()
        .get::<Table>()
        .warzones()
        .iter()
        .map(|w| w.id)
        .collect();
    let mut out = Vec::new();
    for w in zones {
        let systems = reg
            .ext::<Facwar>()
            .by_warzone
            .get(&w)
            .cloned()
            .unwrap_or_default();
        let inner = systems
            .into_iter()
            .filter_map(|s| Some((state_in(reg, &graph, s)?, PyValue::Int(s))))
            .collect();
        out.push((PyValue::Dict(inner), PyValue::Int(w)));
    }
    PyValue::Dict(out)
}

/// `system`'s `VictoryPointState`, `None` outside the warzones.
pub fn vp_state(reg: &Registry, system: i64) -> Option<PyValue> {
    occupation(reg, system).map(|o| victory_point_state(o.score))
}

/// `GetAllVictoryPointStates`: `{ssid: VictoryPointState}`, every warzone
/// system (`fwVictoryPointSvc:59` reads it with `.get`).
pub fn all_vp_states(reg: &Registry) -> PyValue {
    let ids: Vec<i64> = reg.ext::<Facwar>().systems.keys().copied().collect();
    PyValue::Dict(
        ids.into_iter()
            .filter_map(|s| Some((vp_state(reg, s)?, PyValue::Int(s))))
            .collect(),
    )
}

// ---- pushes ----

/// `OnVictoryPointStateUpdated(ssid, VPState)` [V] (arity 2,
/// `fwVictoryPointSvc:62`), to the characters in the system.
#[derive(Debug, Clone, PartialEq)]
pub struct VictoryPointStateUpdated {
    pub to: Vec<i64>,
    pub system: i64,
    pub score: i64,
}

impl crate::notify::Push for VictoryPointStateUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnVictoryPointStateUpdated",
            vec![PyValue::Int(self.system), victory_point_state(self.score)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnWarzoneOccupationStateUpdated(ssid, OccupationState)` [V] (arity 2,
/// `fwWarzoneSvc:115`), to the characters in the system.
#[derive(Debug, Clone, PartialEq)]
pub struct WarzoneOccupationStateUpdated {
    pub to: Vec<i64>,
    pub system: i64,
    pub state: PyValue,
}

impl crate::notify::Push for WarzoneOccupationStateUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnWarzoneOccupationStateUpdated",
            vec![PyValue::Int(self.system), self.state.clone()],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Every character whose `session.solarsystemid2` is `system`: the roster's
/// system in space, else the character's station system.
pub fn in_system(reg: &Registry, system: i64) -> Vec<i64> {
    reg.config()
        .characters
        .iter()
        .map(|c| c.character_id)
        .filter(|c| reg.session_system(*c) == Some(system))
        .collect()
}

// ---- the stage ----

/// **The stage** (`tick::STAGES`): every [`SCAN_EVERY`], each warzone system
/// with a `site` delta is read through `population::sites`, and every FW
/// complex completed after the system's `credited` mark is counted
/// ([`credit`]). Pulling from the slots needs no hook in `site_spawn`.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    if !reg.ext::<Facwar>().systems.is_empty() {
        crate::fw_capture::tick(reg, ft);
        crate::fw_scoreboard::tick(reg, ft);
    }
    if reg.ext::<Facwar>().systems.is_empty() || ft < reg.ext::<Facwar>().next_scan {
        return;
    }
    reg.ext_mut::<Facwar>().next_scan = ft.saturating_add(SCAN_EVERY);
    let fw = reg.ext::<Facwar>();
    let with_sites: BTreeSet<i64> = reg
        .deltas()
        .of(crate::population::KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .filter(|s| fw.systems.contains_key(s))
        .collect();
    for system in with_sites {
        let done: Vec<(BlueTime, i64, i64)> = crate::population::sites(reg, system, ft)
            .sites
            .into_iter()
            .filter(|s| s.state == crate::population::State::Done)
            // A won battlefield is credited by `fw_scoreboard::win`, not here.
            .filter(|s| FW_ARCHETYPES.contains(&s.archetype))
            .filter(|s| s.archetype != crate::fw_scoreboard::BATTLEFIELD)
            .map(|s| (s.at, s.archetype, s.dungeon_id))
            .collect();
        if done.is_empty() {
            continue;
        }
        let owners: Vec<(BlueTime, i64, Option<i64>)> = {
            let table = reg.content().try_get::<crate::dungeon_data::Table>();
            done.into_iter()
                .map(|(at, a, d)| {
                    let owner = table.and_then(|t| t.dungeons.get(&d)?.faction);
                    (at, a, owner)
                })
                .collect()
        };
        credit(reg, system, &owners, ft);
    }
}

/// Count each completed complex `(completed_at, archetype, owner)` newer
/// than the system's `credited` mark (R-FW §5 rule 2, §6 [I]): the
/// occupier's complex raises the score, the attacker's lowers it (floored
/// at 0), anyone else's counts for nothing. Then the push, and the flip
/// when the score reaches [`VP_THRESHOLD`]. Returns the new occupation.
pub fn credit(
    reg: &mut Registry,
    system: i64,
    done: &[(BlueTime, i64, Option<i64>)],
    now: BlueTime,
) -> Option<Occupation> {
    let mut o = occupation(reg, system)?;
    let before = o;
    for (at, archetype, owner) in done {
        if *at <= before.credited {
            continue;
        }
        o.credited = o.credited.max(*at);
        let Some(points) = vp_for(*archetype) else {
            continue;
        };
        if *owner == Some(o.occupier) {
            o.score += points;
        } else if *owner == Some(o.attacker) {
            o.score -= points;
        }
        o.score = o.score.clamp(0, VP_THRESHOLD + MAX_OVER_THRESHOLD);
    }
    if o == before {
        return Some(o);
    }
    if o.score >= VP_THRESHOLD {
        return Some(flip(reg, system, o, now));
    }
    store(reg, system, &o);
    if o.score != before.score {
        let to = in_system(reg, system);
        reg.push(VictoryPointStateUpdated {
            to,
            system,
            score: o.score,
        });
    }
    Some(o)
}

/// Move `system`'s score by `delta` (a capture point's victory points,
/// #75): positive for the attacker, negative for the occupier, clamped like
/// [`credit`], with the same push and flip. `None` outside the warzones.
pub fn add_vp(reg: &mut Registry, system: i64, delta: i64, now: BlueTime) -> Option<Occupation> {
    let before = occupation(reg, system)?;
    let mut o = before;
    o.score = (o.score + delta).clamp(0, VP_THRESHOLD + MAX_OVER_THRESHOLD);
    if o.score >= VP_THRESHOLD {
        return Some(flip(reg, system, o, now));
    }
    if o.score != before.score {
        store(reg, system, &o);
        let to = in_system(reg, system);
        reg.push(VictoryPointStateUpdated {
            to,
            system,
            score: o.score,
        });
    }
    Some(o)
}

/// The flip (module header, [A]): occupier and attacker swap, the score
/// goes to 0, `dateTaken` is `now`. Every system whose `OccupationState`
/// changed (this one, and any within two gates whose adjacency moved) is
/// told, then this system's VP.
fn flip(reg: &mut Registry, system: i64, mut o: Occupation, now: BlueTime) -> Occupation {
    let graph = neighbours(reg).clone();
    let mut near: BTreeSet<i64> = BTreeSet::new();
    for n in graph.get(&system).into_iter().flatten() {
        near.insert(*n);
        near.extend(graph.get(n).into_iter().flatten().copied());
    }
    near.remove(&system);
    let was: BTreeMap<i64, Adjacency> = near
        .iter()
        .map(|s| (*s, adjacency_in(reg, &graph, *s)))
        .collect();
    std::mem::swap(&mut o.occupier, &mut o.attacker);
    o.score = 0;
    o.taken_at = Some(now);
    store(reg, system, &o);
    let mut told = vec![system];
    told.extend(
        was.into_iter()
            .filter(|(s, a)| adjacency_in(reg, &graph, *s) != *a)
            .map(|(s, _)| s),
    );
    for s in told {
        let Some(state) = state_in(reg, &graph, s) else {
            continue;
        };
        let to = in_system(reg, s);
        reg.push(WarzoneOccupationStateUpdated {
            to,
            system: s,
            state,
        });
    }
    let to = in_system(reg, system);
    reg.push(VictoryPointStateUpdated {
        to,
        system,
        score: o.score,
    });
    o
}

#[cfg(test)]
pub(crate) mod tests {
    //! `pub(crate)`: the battlefield tests (`fw_scoreboard`, `fw_capture`) reuse
    //! this module's invented warzone.
    use super::*;
    use crate::pyutil::dict_get;

    fn wz(id: i64, a: i64, b: i64) -> Warzone {
        Warzone {
            id,
            factions: [a, b],
            hqs: [a + 1_000, b + 1_000],
        }
    }

    #[test]
    fn an_empty_or_ambiguous_table_is_refused() {
        assert!(Table::new(vec![]).is_err(), "{{}} is a client IndexError");
        assert!(Table::new(vec![wz(1, 10, 11), wz(1, 12, 13)]).is_err());
        assert!(Table::new(vec![wz(1, 10, 11), wz(2, 11, 13)]).is_err());
        assert!(Table::new(vec![wz(1, 10, 10)]).is_err());
        let t = Table::new(vec![wz(2, 12, 13), wz(1, 10, 11)]).unwrap();
        assert_eq!(t.warzones()[0].id, 1, "sorted by id");
    }

    #[test]
    fn every_warzone_faction_maps_to_its_zone_and_hq() {
        let t = Table::fixture();
        for w in t.warzones() {
            for (side, f) in w.factions.iter().enumerate() {
                assert_eq!(t.warzone_of(*f).map(|x| x.id), Some(w.id));
                assert_eq!(t.hq_of(*f), Some(w.hqs[side]));
            }
        }
        assert_eq!(t.warzone_of(1), None);
    }

    /// R-FW §3 [V]: the state is new-style and its keys are the underscored
    /// attributes, `__init__` never running.
    #[test]
    fn occupation_state_keys_are_underscored() {
        let v = occupation_state(599_001, 599_002, Adjacency::Frontline, 599_009, None);
        let PyValue::ObjectEx { header, .. } = &v else {
            panic!("{v:?}")
        };
        let PyValue::Tuple(parts) = &**header else {
            panic!("{header:?}")
        };
        assert_eq!(
            parts[0],
            PyValue::Tuple(vec![PyValue::Global(OCCUPATION_STATE.as_bytes().to_vec())])
        );
        for key in [
            "_occupierID",
            "_attackerID",
            "_adjacencyState",
            "_pirateFactionID",
            "_dateTaken",
        ] {
            assert!(dict_get(&parts[1], key.as_bytes()).is_some(), "{key}");
        }
        assert_eq!(
            dict_get(&parts[1], b"_adjacencyState"),
            Some(&crate::pyutil::bstr("FL"))
        );
        assert_eq!(dict_get(&parts[1], b"_dateTaken"), Some(&PyValue::None));
    }

    /// The shipped file parses and passes [`Table::new`]; its values are not
    /// asserted (plan §1.4).
    #[test]
    fn the_shipped_file_loads() {
        let dir = crate::authored::Dir::compiled_only();
        let file = dir.load(FILE, SHIPPED, HEADER).expect("the header");
        assert!(file.skipped.is_empty(), "{:?}", file.skipped);
        assert!(parse(&file).1.is_empty(), "every row parses");
        let src = Sources::new(None, &dir);
        let t = boxed_load(&src).expect("warzones.tsv loads");
        assert!(t.downcast_ref::<Table>().is_some());
    }

    #[test]
    fn a_scoreboard_row_parses_and_a_bad_one_is_an_error() {
        assert_eq!(
            parse_scoreboard_line("990001\t30\t-40"),
            Ok((990_001, Some(30), Some(-40)))
        );
        assert_eq!(
            parse_scoreboard_line("990002\t\t25"),
            Ok((990_002, None, Some(25)))
        );
        assert_eq!(
            parse_scoreboard_line("990003\t\t"),
            Ok((990_003, None, None))
        );
        for bad in ["990001\t30", "x\t1\t2", "990001\t1.5\t2", "990001\t1\t2\t3"] {
            assert!(parse_scoreboard_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scoreboard_of_answers_from_the_table_and_an_absent_file_loads() {
        let t = Table::fixture().with_scoreboards([(990_001, (Some(30), None))].into());
        assert_eq!(t.scoreboard_of(990_001), Some((Some(30), None)));
        assert_eq!(t.scoreboard_of(990_002), None);
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = boxed_load(&src).expect("no bundle: no scoreboards, still loads");
        let loaded = loaded.downcast_ref::<Table>().expect("a Table");
        assert_eq!(loaded.scoreboard_of(990_001), None);
    }

    #[test]
    fn a_capture_point_row_parses_and_a_bad_one_is_an_error() {
        assert_eq!(
            parse_capture_point_line("990001\t45\t7000\t2\t1\t990101\t990102"),
            Ok((
                990_001,
                CapturePointRow {
                    seconds: Some(45),
                    range: Some(7000),
                    point_value: Some(2),
                    looping: Some(true),
                    capturing: Some(990_101),
                    blocking: Some(990_102),
                }
            ))
        );
        assert_eq!(
            parse_capture_point_line("990002\t\t\t\t0\t\t"),
            Ok((
                990_002,
                CapturePointRow {
                    looping: Some(false),
                    ..CapturePointRow::default()
                }
            ))
        );
        for bad in [
            "990001\t1\t2\t3\t1\t4",
            "x\t1\t2\t3\t1\t4\t5",
            "990001\t1.5\t2\t3\t1\t4\t5",
            "990001\t1\t2\t3\t2\t4\t5",
            "990001\t1\t2\t3\ttrue\t4\t5",
        ] {
            assert!(parse_capture_point_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_non_positive_seconds_or_range_falls_back_with_a_note() {
        let row = CapturePointRow {
            seconds: Some(0),
            range: Some(-5),
            point_value: Some(2),
            ..CapturePointRow::default()
        };
        let (got, notes) = sanitized_capture_row(row);
        assert_eq!(
            (got.seconds, got.range, got.point_value),
            (None, None, Some(2))
        );
        assert_eq!(notes.len(), 2);
        let ok = CapturePointRow {
            seconds: Some(1),
            range: Some(1),
            ..CapturePointRow::default()
        };
        assert_eq!(sanitized_capture_row(ok), (ok, vec![]));
    }

    #[test]
    fn capture_point_of_answers_from_the_table_and_an_absent_file_loads() {
        let row = CapturePointRow {
            seconds: Some(45),
            ..CapturePointRow::default()
        };
        let t = Table::fixture().with_capture_points([(990_001, row)].into());
        assert_eq!(t.capture_point_of(990_001), Some(row));
        assert_eq!(t.capture_point_of(990_002), None);
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = boxed_load(&src).expect("no bundle: no capture points, still loads");
        let loaded = loaded.downcast_ref::<Table>().expect("a Table");
        assert_eq!(loaded.capture_point_of(990_001), None);
    }

    // ---- W17 CP53b ----

    use crate::celestial::{Catalogue, Universe};
    use crate::dungeon_data::{self, Dungeon, Entry};
    use crate::model::{Character, SandboxConfig};
    use crate::population;
    use std::sync::Arc;

    /// Invented militias A and B of one warzone, and an outsider C.
    pub(crate) const A: i64 = 599_201;
    pub(crate) const B: i64 = 599_202;
    pub(crate) const C: i64 = 599_203;
    const ZONE: i64 = 97;
    /// Two warzone systems joined by a gate (S1 held by A, S2 by B), a
    /// low-sec system outside the warzone, and a high-sec one of A's.
    pub(crate) const S1: i64 = 39_000_201;
    pub(crate) const S2: i64 = 39_000_202;
    const LOW: i64 = 39_000_203;
    const HIGH: i64 = 39_000_204;
    const THIRD: i64 = 39_000_205;
    pub(crate) const ME: i64 = 90_000_201;
    pub(crate) const YOU: i64 = 90_000_202;
    pub(crate) const FW: i64 = FW_ARCHETYPES[0];
    /// The militia corporations of A and B (invented).
    pub(crate) const CORP_A: i64 = 1_999_201;
    pub(crate) const CORP_B: i64 = 1_999_202;

    fn universe() -> Universe {
        let row = |id: i64, faction: Option<i64>, sec: &str| {
            let f = faction.map_or_else(String::new, |f| f.to_string());
            format!(
                "{id}\t19000201\t29000201\t{f}\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}"
            )
        };
        let systems = [
            row(S1, Some(A), "0.3"),
            row(S2, Some(B), "0.3"),
            row(LOW, None, "0.3"),
            row(HIGH, Some(A), "0.9"),
            row(THIRD, Some(C), "0.3"),
        ];
        Universe::parse(&systems, &[], &[]).expect("the fixture universe")
    }

    /// FW complexes owned by A, by B and by the outsider C, all of the
    /// one fixture archetype, plus their population row.
    pub(crate) fn dungeons() -> dungeon_data::Table {
        let d = |id: i64, faction: i64| Dungeon {
            id,
            archetype: FW,
            faction: Some(faction),
            difficulty: Some(4),
            entry_type: Some(Entry::Anomaly.type_id()),
            ..Dungeon::default()
        };
        dungeon_data::Table {
            dungeons: [d(999_301, A), d(999_302, B), d(999_303, C)]
                .into_iter()
                .map(|d| (d.id, d))
                .collect(),
            ..dungeon_data::Table::default()
        }
    }

    fn population_rows() -> Vec<population::Row> {
        let row = population::Row {
            key: "fx_fw_low".to_owned(),
            band: crate::celestial::Band::Low,
            archetypes: vec![FW],
            entry: Entry::Anomaly,
            count: 8.0,
            difficulties: population::Diffs::In(vec![4]),
            respawn_s: 600,
            ttl_s: 172_800,
            pochven: false,
        };
        vec![row]
    }

    fn pilot(id: i64, system: i64) -> Character {
        Character {
            character_id: id,
            ship_id: id + 100,
            solar_system_id: Some(system),
            ..Character::default()
        }
    }

    pub(crate) fn world() -> Registry {
        world_with(|_, _, _, _| ())
    }

    /// [`world`] with `extra` applied to the npc table, the dungeons, the
    /// population rows and the scoreboard thresholds before they are shared.
    pub(crate) fn world_with(
        extra: impl FnOnce(
            &mut crate::npc_data::Table,
            &mut dungeon_data::Table,
            &mut Vec<population::Row>,
            &mut BTreeMap<i64, (Option<i64>, Option<i64>)>,
        ),
    ) -> Registry {
        world_with_points(BTreeMap::new(), extra)
    }

    /// [`world_with`] with capture point rows in the warzone table.
    pub(crate) fn world_with_points(
        points: BTreeMap<i64, CapturePointRow>,
        extra: impl FnOnce(
            &mut crate::npc_data::Table,
            &mut dungeon_data::Table,
            &mut Vec<population::Row>,
            &mut BTreeMap<i64, (Option<i64>, Option<i64>)>,
        ),
    ) -> Registry {
        let mut c = Content::fixture();
        let zone = Warzone {
            id: ZONE,
            factions: [A, B],
            hqs: [S1, S2],
        };
        let mut npc = crate::npc_data::Table::default();
        for (f, corp) in [(A, CORP_A), (B, CORP_B)] {
            npc.factions.insert(
                f,
                crate::npc_data::Faction {
                    faction_id: f,
                    corporation_id: corp + 10,
                    militia_corporation_id: Some(corp),
                    ..crate::npc_data::Faction::default()
                },
            );
        }
        let (mut dun, mut rows) = (dungeons(), population_rows());
        let mut scoreboards = BTreeMap::new();
        extra(&mut npc, &mut dun, &mut rows, &mut scoreboards);
        c.insert(
            "facwar",
            Table::new(vec![zone])
                .expect("ok")
                .with_scoreboards(scoreboards)
                .with_capture_points(points),
        );
        c.insert("universe", universe());
        c.insert("dungeon_data", dun);
        c.insert("population", population::Table::new(rows).expect("rows"));
        c.insert("npc_data", npc);
        let mut text = String::new();
        for (n, s) in [S1, S2, LOW, HIGH].iter().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\n",
                40_000_200 + n
            );
        }
        text += &format!("gate\t50000201\t990012\t5000.0\t1.0e9\t0.0\t0.0\t{S1}\t50000202\t{S2}\n");
        text += &format!("gate\t50000202\t990012\t5000.0\t1.0e9\t0.0\t0.0\t{S2}\t50000201\t{S1}\n");
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![pilot(ME, S1), pilot(YOU, S2)],
            celestials: Catalogue::parse(&text),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        reg.deltas_mut().set_world(crate::delta::World {
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        reg
    }

    /// Every push named `event`, as `(args, audience)`.
    pub(crate) fn pushes(reg: &mut Registry, event: &str) -> Vec<(Vec<PyValue>, Option<Vec<i64>>)> {
        reg.take_pending()
            .into_iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                (name == event).then(|| (args, p.audience()))
            })
            .collect()
    }

    fn key(v: &PyValue, name: &str) -> PyValue {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("{v:?}")
        };
        let PyValue::Tuple(parts) = &**header else {
            panic!("{header:?}")
        };
        dict_get(&parts[1], name.as_bytes())
            .cloned()
            .unwrap_or_else(|| panic!("{name} on {v:?}"))
    }

    pub(crate) fn fw_sites(reg: &Registry, system: i64) -> Vec<population::Site> {
        population::sites(reg, system, reg.world().birth)
            .sites
            .into_iter()
            .filter(|s| FW_ARCHETYPES.contains(&s.archetype))
            .collect()
    }

    fn listed(systems: &[(i64, i64)]) -> Content {
        let zone = Warzone {
            id: ZONE,
            factions: [A, B],
            hqs: [S1, S2],
        };
        let table = Table::new(vec![zone])
            .expect("ok")
            .with_systems(systems.iter().copied().collect());
        let mut c = Content::fixture();
        c.insert("facwar", table);
        c.insert("universe", universe());
        c
    }

    /// A listed table wins over the rule: HIGH (a high-sec A system the rule
    /// would not take unless an HQ) and S2 are unlisted, so only S1 counts,
    /// and the rule's own pick S2 is gone.
    #[test]
    fn a_listed_table_wins_over_the_rule() {
        let c = listed(&[(S1, ZONE)]);
        let got = warzone_systems(&c);
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), vec![S1]);
        assert_eq!(got[&S1].seed_occupier, A);
        assert_eq!(got[&S1].seed_attacker, B);
        assert_eq!(
            militias_by_system(&c).keys().copied().collect::<Vec<_>>(),
            vec![S1]
        );
    }

    /// A listed system whose own faction is not a militia of its warzone
    /// (THIRD is the outsider's, LOW has none) is skipped.
    #[test]
    fn a_system_of_a_third_faction_is_skipped() {
        let c = listed(&[(S1, ZONE), (THIRD, ZONE), (LOW, ZONE), (S2, 12_345)]);
        let got = warzone_systems(&c);
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), vec![S1]);
    }

    /// No listed systems: the old rule, which the fixtures rely on.
    #[test]
    fn empty_table_falls_back() {
        let c = listed(&[]);
        let got: Vec<i64> = warzone_systems(&c).into_keys().collect();
        assert_eq!(got, vec![S1, S2]);
    }

    /// The warzone systems are the low-sec systems of a warzone militia,
    /// plus the HQs; an FW row lives only there, and draws only the
    /// complexes its two militias own (never the outsider's).
    #[test]
    fn fw_complexes_live_only_in_warzone_systems_and_belong_to_its_militias() {
        let reg = world();
        let ids: Vec<i64> = reg.ext::<Facwar>().systems().map(|(s, _)| s).collect();
        assert_eq!(ids, vec![S1, S2]);
        assert_eq!(
            reg.ext::<Facwar>().system(S1).map(|f| f.seed_occupier),
            Some(A)
        );
        assert!(
            fw_sites(&reg, LOW).is_empty(),
            "low sec outside the warzone"
        );
        assert!(fw_sites(&reg, HIGH).is_empty());
        let mut owners = BTreeSet::new();
        for s in [S1, S2] {
            let here = fw_sites(&reg, s);
            assert!(!here.is_empty(), "{s} holds complexes");
            for site in here {
                let d = &dungeons().dungeons[&site.dungeon_id];
                owners.insert(d.faction.expect("owned"));
            }
        }
        assert!(
            !owners.contains(&C),
            "the outsider's complex is never drawn"
        );
        assert!(owners.iter().all(|f| [A, B].contains(f)));
    }

    /// **The plan's test**: completing a fixture FW complex owned by the
    /// occupier raises the system's VP by [`vp_for`] its archetype, and
    /// `OnVictoryPointStateUpdated(ssid, VPState)` [V arity 2] goes to the
    /// characters in that system only. Counted once.
    #[test]
    fn completing_a_fixture_fw_complex_raises_the_systems_vp_and_tells_the_system() {
        let mut reg = world();
        let now = reg.world().birth;
        // A complex its system's occupier owns, in S1 (held by A) or S2
        // (held by B), whichever the seeded draw put one in.
        let Some((sys, occupier, target, other)) = [(S1, A, YOU), (S2, B, ME)]
            .into_iter()
            .find_map(|(sys, occupier, other)| {
                let site = fw_sites(&reg, sys)
                    .into_iter()
                    .find(|s| dungeons().dungeons[&s.dungeon_id].faction == Some(occupier))?;
                Some((sys, occupier, site, other))
            })
        else {
            panic!("a warzone system holds a complex of its occupier")
        };
        let me = if other == YOU { ME } else { YOU };
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), now);
        assert_eq!(occupation(&reg, sys).map(|o| o.score), Some(0));
        assert!(population::complete(&mut reg, sys, target.site_id, now + 1));
        tick(&mut reg, Instant::now(), now + SCAN_EVERY);
        let points = vp_for(FW).expect("an FW archetype");
        let o = occupation(&reg, sys).expect("a warzone system");
        assert_eq!((o.occupier, o.score), (occupier, points));
        let told = pushes(&mut reg, "OnVictoryPointStateUpdated");
        assert_eq!(told.len(), 1, "{told:?}");
        let (args, audience) = &told[0];
        assert_eq!(args.len(), 2, "fwVictoryPointSvc:62 unpacks two");
        assert_eq!(args[0], PyValue::Int(sys));
        assert_eq!(key(&args[1], "_vpScore"), PyValue::Int(points));
        assert_eq!(key(&args[1], "_vpThreshold"), PyValue::Int(VP_THRESHOLD));
        assert_eq!(audience.as_deref(), Some(&[me][..]), "the system only");
        tick(&mut reg, Instant::now(), now + 2 * SCAN_EVERY);
        assert_eq!(occupation(&reg, sys).map(|o| o.score), Some(points), "once");
        assert!(pushes(&mut reg, "OnVictoryPointStateUpdated").is_empty());
    }

    /// A won battlefield is credited by `fw_scoreboard::win` alone: the
    /// completion scan skips archetype 68, so the system's VP moves once.
    #[test]
    fn a_completed_battlefield_is_not_credited_again_by_the_scan() {
        let bf = crate::fw_scoreboard::tests::battlefield(false);
        let (mut reg, now) = (bf.reg, bf.now);
        let vp = vp_for(crate::fw_scoreboard::BATTLEFIELD).expect("worth points");
        // The fixture picks an occupier-owned battlefield: were the scan not to
        // skip it, it would raise the score by `vp`, so a flat score proves the
        // exclusion (an attacker-owned one would floor at 0 and prove nothing).
        let dungeon = population::sites(&reg, bf.system, now)
            .sites
            .iter()
            .find(|s| s.site_id == bf.site)
            .expect("the site")
            .dungeon_id;
        let owner = reg
            .content()
            .get::<dungeon_data::Table>()
            .dungeons
            .get(&dungeon)
            .and_then(|d| d.faction);
        assert_eq!(owner, occupation(&reg, bf.system).map(|o| o.occupier));
        // The slot completes the way a win completes it, without the win's VP.
        assert!(population::complete(&mut reg, bf.system, bf.site, now + 1));
        assert_eq!(occupation(&reg, bf.system).map(|o| o.score), Some(0));
        tick(&mut reg, Instant::now(), now + SCAN_EVERY);
        assert_eq!(
            occupation(&reg, bf.system).map(|o| o.score),
            Some(0),
            "the scan did not credit the battlefield"
        );
        // The win's own credit is still paid, once.
        let mut bf2 = crate::fw_scoreboard::tests::battlefield(false);
        crate::fw_scoreboard::credit(&mut bf2.reg, bf2.site, B, 1_000, bf2.now + 1);
        tick(&mut bf2.reg, Instant::now(), bf2.now + SCAN_EVERY);
        assert_eq!(occupation(&bf2.reg, bf2.system).map(|o| o.score), Some(vp));
    }

    /// **Interim rule [A]** (3707965 FW complexes carry no static spawns): a
    /// complex with no squad and no capture-point object is taken when its
    /// capture cycle ends, so the slot completes and the attacker's enlisted
    /// pilot on the grid is paid militia LP. Invented rows.
    #[test]
    fn an_empty_complex_is_taken_by_holding_it_and_pays_militia_lp() {
        let mut reg = world();
        let now = reg.world().birth;
        let site = fw_sites(&reg, S1)
            .into_iter()
            .find(|s| dungeons().dungeons[&s.dungeon_id].faction == Some(A))
            .expect("a complex of S1's occupier");
        assert!(crate::site::instantiate(&mut reg, &site));
        // B attacks S1; ME is B's enlisted pilot, on the complex's grid.
        crate::service::facwar_mgr::enlist(&mut reg, ME, B, now).expect("enlists");
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ME,
                ME + 100,
                S1,
                crate::space_state::Motion::arrived(site.anchor, [0.0; 3]),
            ));
        crate::site_spawn::tick(&mut reg, Instant::now(), now);
        assert!(crate::site_spawn::is_empty_pocket(&reg, site.site_id));
        assert_eq!(crate::loyalty::balance(&reg, ME, CORP_B), 0);
        let cycle = crate::fw_capture::assumed().seconds * 10_000_000;
        let step = crate::fw_capture::CHECK_EVERY;
        let mut t = now;
        while t < now + 2 * cycle {
            crate::fw_capture::tick(&mut reg, t);
            t += step;
        }
        assert!(crate::fw_capture::is_captured(&reg, site.site_id));
        assert!(
            crate::loyalty::balance(&reg, ME, CORP_B) > 0,
            "the capture paid LP"
        );
        let paid = crate::loyalty::balance(&reg, ME, CORP_B);
        crate::fw_capture::tick(&mut reg, t + cycle);
        assert_eq!(crate::loyalty::balance(&reg, ME, CORP_B), paid, "once");
    }

    /// The attacker's own complex lowers the score, floored at 0; an
    /// outsider's counts for nothing.
    #[test]
    fn the_attackers_complex_lowers_the_score_and_an_outsiders_is_ignored() {
        let mut reg = world();
        let p = vp_for(FW).expect("FW");
        credit(&mut reg, S1, &[(10, FW, Some(A)), (11, FW, Some(A))], 20);
        assert_eq!(occupation(&reg, S1).map(|o| o.score), Some(2 * p));
        credit(&mut reg, S1, &[(12, FW, Some(B)), (13, FW, Some(C))], 20);
        assert_eq!(occupation(&reg, S1).map(|o| o.score), Some(p));
        credit(&mut reg, S1, &[(14, FW, Some(B)), (15, FW, Some(B))], 20);
        assert_eq!(occupation(&reg, S1).map(|o| o.score), Some(0), "floored");
        credit(&mut reg, S1, &[(9, FW, Some(A))], 20);
        assert_eq!(
            occupation(&reg, S1).map(|o| o.score),
            Some(0),
            "already counted"
        );
    }

    /// **The plan's test**: crossing [`VP_THRESHOLD`] flips `occupierID`
    /// and pushes `OnWarzoneOccupationStateUpdated(ssid, state)` [V arity
    /// 2] to the system, and to a neighbour whose adjacency moved.
    #[test]
    fn crossing_the_threshold_flips_the_occupier_and_pushes_the_occupation() {
        let mut reg = world();
        assert_eq!(adjacency(&mut reg, S1), Adjacency::Frontline);
        assert_eq!(adjacency(&mut reg, S2), Adjacency::Frontline);
        let p = vp_for(FW).expect("FW");
        let n = (VP_THRESHOLD + p - 1) / p;
        let done: Vec<(BlueTime, i64, Option<i64>)> =
            (0..n).map(|i| (100 + i, FW, Some(A))).collect();
        let _ = reg.take_pending();
        let now = 777_000;
        let o = credit(&mut reg, S1, &done, now).expect("a warzone system");
        assert_eq!((o.occupier, o.attacker, o.score), (B, A, 0));
        assert_eq!(o.taken_at, Some(now));
        assert_eq!(systems_held(&reg, B), 2);
        assert_eq!(systems_held(&reg, A), 0);
        let pending = reg.take_pending();
        let occupation_pushes: Vec<(Vec<PyValue>, Option<Vec<i64>>)> = pending
            .iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                (name == "OnWarzoneOccupationStateUpdated").then(|| (args, p.audience()))
            })
            .collect();
        let mine = occupation_pushes
            .iter()
            .find(|(a, _)| a[0] == PyValue::Int(S1))
            .expect("the flipped system is told");
        assert_eq!(mine.0.len(), 2, "fwWarzoneSvc:115 unpacks two");
        assert_eq!(key(&mine.0[1], "_occupierID"), PyValue::Int(B));
        assert_eq!(key(&mine.0[1], "_attackerID"), PyValue::Int(A));
        assert_eq!(key(&mine.0[1], "_dateTaken"), PyValue::Int(now));
        assert_eq!(mine.1.as_deref(), Some(&[ME][..]));
        let theirs = occupation_pushes
            .iter()
            .find(|(a, _)| a[0] == PyValue::Int(S2))
            .expect("S2 lost its front line");
        assert_eq!(theirs.1.as_deref(), Some(&[YOU][..]));
        assert_eq!(adjacency(&mut reg, S2), Adjacency::Backline);
        let vp: Vec<_> = pending
            .iter()
            .map(|(_, p)| p.to_push())
            .filter(|(name, _)| *name == "OnVictoryPointStateUpdated")
            .collect();
        assert_eq!(vp.len(), 1);
        assert_eq!(key(&vp[0].1[1], "_vpScore"), PyValue::Int(0));
    }

    /// A capture point's points (#75): the attacker's raise the score, the
    /// occupier's lower it (floored at 0), the push goes to the system, and
    /// reaching the threshold flips the system.
    #[test]
    fn capture_point_vp_moves_the_score_pushes_and_flips() {
        let mut reg = world();
        let now = reg.world().birth;
        let _ = reg.take_pending();
        let o = add_vp(&mut reg, S1, 4, now).expect("a warzone system");
        assert_eq!((o.occupier, o.attacker, o.score), (A, B, 4));
        assert_eq!(pushes(&mut reg, "OnVictoryPointStateUpdated").len(), 1);
        assert_eq!(add_vp(&mut reg, S1, -10, now).map(|o| o.score), Some(0));
        let _ = reg.take_pending();
        assert!(add_vp(&mut reg, S1, 0, now).is_some());
        assert!(pushes(&mut reg, "OnVictoryPointStateUpdated").is_empty());
        let o = add_vp(&mut reg, S1, VP_THRESHOLD, now).expect("flips");
        assert_eq!((o.occupier, o.attacker, o.score), (B, A, 0));
        assert!(add_vp(&mut reg, LOW, 1, now).is_none(), "not a warzone");
    }

    /// **The plan's test**: the `fwvp` delta round-trips. A world rebuilt
    /// from the rows reads the same occupation, and does not count a
    /// complex twice.
    #[test]
    fn the_fwvp_delta_round_trips() {
        let mut reg = world();
        let p = vp_for(FW).expect("FW");
        credit(&mut reg, S1, &[(10, FW, Some(A))], 20);
        let n = (VP_THRESHOLD + p - 1) / p;
        let done: Vec<_> = (0..n).map(|i| (100 + i, FW, Some(B))).collect();
        credit(&mut reg, S2, &done, 555);
        let mut again = world();
        for (k, v) in reg.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        for s in [S1, S2] {
            assert_eq!(occupation(&again, s), occupation(&reg, s), "{s}");
        }
        assert_eq!(occupation(&again, S2).map(|o| o.occupier), Some(A));
        credit(&mut again, S1, &[(10, FW, Some(A))], 30);
        assert_eq!(occupation(&again, S1).map(|o| o.score), Some(p));
    }

    /// The service arms answer from the occupation: every warzone key, each
    /// warzone system's state under its warzone, and `(ssid, VPState)` for
    /// the caller's own system.
    #[test]
    fn the_arms_answer_from_the_occupation() {
        let mut reg = world();
        let all = all_occupation_states(&mut reg);
        let kv = all.dict_as_kv().expect("a dict");
        assert_eq!(kv.len(), 1);
        let inner = kv[0].1.dict_as_kv().expect("a dict");
        let ids: Vec<&PyValue> = inner.iter().map(|(k, _)| *k).collect();
        assert_eq!(ids, vec![&PyValue::Int(S1), &PyValue::Int(S2)]);
        assert_eq!(key(inner[0].1, "_occupierID"), PyValue::Int(A));
        assert_eq!(
            key(inner[0].1, "_adjacencyState"),
            crate::pyutil::bstr("FL")
        );
        assert!(matches!(
            key(inner[0].1, "_pirateFactionID"),
            PyValue::Int(_)
        ));
        assert!(local_state(&mut reg, LOW).is_none(), "not a warzone system");
        assert!(vp_state(&reg, LOW).is_none());
        let vps = all_vp_states(&reg);
        assert_eq!(vps.dict_as_kv().map(|v| v.len()), Some(2));
    }
    /// `GetLPUpgradeLevel` [V]: 0 for an empty pool, the level below the
    /// first threshold the pool is under, 5 from the last threshold on.
    #[test]
    fn the_upgrade_level_follows_the_client_thresholds() {
        let cases = [
            (0, 0),
            (1, 0),
            (39_999, 0),
            (40_000, 1),
            (89_999, 2),
            (90_000, 3),
            (199_999, 4),
            (299_999, 5),
            (300_000, 5),
            (900_000, 5),
        ];
        for (lps, level) in cases {
            assert_eq!(upgrade_level(lps), level, "{lps}");
        }
    }

    /// `map.GetFacWarZoneInfo`: what `GetDonationTax` divides is nonzero,
    /// the points are the held systems of the zone's, and a held system's
    /// level comes from its pool.
    #[test]
    fn the_zone_info_carries_the_points_and_the_levels() {
        let mut reg = world();
        let held = systems_held(&reg, A) as i64;
        let total = reg.ext::<Facwar>().systems().count() as i64;
        assert!(held > 0 && total >= held);
        let held_system = reg
            .ext::<Facwar>()
            .systems()
            .map(|(s, _)| s)
            .find(|s| occupation(&reg, *s).is_some_and(|o| o.occupier == A))
            .expect("A holds one");
        reg.deltas_mut().put(
            crate::service::facwar_mgr::LP_KIND,
            vec![held_system.to_string()],
            vec!["60000".to_owned()],
        );
        let info = zone_info(&reg, A);
        let get = |k: &str| match &info {
            PyValue::Instance { state, .. } => dict_get(state, k.as_bytes()).cloned(),
            other => panic!("{other:?}"),
        };
        assert_eq!(get("factionPoints"), Some(PyValue::Int(held)));
        assert_eq!(get("maxWarZonePoints"), Some(PyValue::Int(total)));
        let Some(PyValue::Dict(levels)) = get("systemUpgradeLevel") else {
            panic!("a dict")
        };
        assert_eq!(levels.len() as i64, held);
        assert!(
            levels
                .iter()
                .any(|(v, k)| *k == PyValue::Int(held_system) && *v == PyValue::Int(2))
        );
        // No warzone, no systems: still a nonzero divisor.
        let none = zone_info(&reg, 1);
        let PyValue::Instance { state, .. } = &none else {
            panic!("a KeyVal")
        };
        assert_eq!(dict_get(state, b"maxWarZonePoints"), Some(&PyValue::Int(1)));
    }
}
