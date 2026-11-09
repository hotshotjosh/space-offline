//! **BOT-UPWELL-HOME (#212): a bot corporation's director anchors an Upwell
//! structure its builders made, and lives in it.** Spec
//! `parity-structure-services.md` §5; the anchor route is
//! `tools/client-ci/scenarios/fci_upwell_anchor.py`'s
//! (`structureDeployment.Anchor`, as the client's `ConfirmDeployment` sends it).
//!
//! The pass is a state machine that is read from the world every turn, so a
//! restart or a refusal loses nothing. The brain starts it from two gates
//! ([`due`] docked, [`in_space`] on the way), each paced by [`Mind`]:
//!
//! 1. **Hull.** The CEO with no home and no hull buys a Raitaru or an Athanor
//!    from the cheapest ask in the station it stands in ([`Step::Buy`]), with
//!    what [`crate::acquire::spendable`] allows. Nothing is granted: no
//!    `/upwell`, no GM kit (owner rule "nothing from nowhere"). The ask is a
//!    builder's: the NPC markets list no category-65 hull.
//! 2. **Hold.** The hull (a Medium Upwell hull, far bigger than a frigate's cargo) does not go in a frigate's hold.
//!    The pass boards an owned hull whose cargo takes it ([`Step::Board`]),
//!    and waits when there is none ([`Hold::NoHauler`]); then the hull goes
//!    from the hangar into the cargo ([`Step::Load`]); the server anchors only
//!    from the ship's hold.
//! 3. **Place.** The server refuses a structure within 1,000 km of a station
//!    or planet and 4,000 km of a moon, so no spot beside an undock is
//!    clear. The ship warps to a *planet* at range 0 ([`Step::WarpPlanet`]):
//!    the server lands it at the client's own warp-in point
//!    ([`eve_server::warp_in::warp_in_point`], about 4,700 km above the
//!    surface), and the pass anchors there at offset (0, 0). Each planet's
//!    warp-in is checked with the server's own rule
//!    ([`eve_server::structure::find_deployment_conflict`]) before the warp
//!    ([`survey`]); the planet is `corp % planets`, the next on a refusal.
//! 4. **Wait** out the world's anchor time: the structure is not in
//!    `GetMyDockableStructures` until it is online. The bot goes back to its
//!    role meanwhile ([`Mind::active`] is false).
//! 5. **Dock** with `structureDocking.Dock` ([`Step::Dock`]), approaching first.
//! 6. **Control.** Docked in the home it takes the controls once and lets them
//!    go ([`Step::Take`], [`Step::Release`]); whether it is at the controls is
//!    read off the session's `shipid` each turn, then it is [`Step::Done`].
//!
//! **[A]** the director pays for the hull from its own wallet; a reimbursement
//! from the corporation's Master Wallet is a follow-up. Falsifier: bot corps
//! whose directors never hold the ISK, in a soak's `upwell home:` notes.
//! **[A]** a ship told to warp to a planet at range 0 lands at that planet's
//! `warp_in_point` and stays within [`ARRIVED`] of it. Falsifier: a soak where
//! the pass warps to one planet [`MAX_WARPS`] times and logs "skipping".
//! **[A]** a bot that is the CEO may anchor (the server also lets directors and
//! station managers); a director who is not the CEO is not asked. Falsifier: a
//! soak whose home is never anchored for want of a CEO.
//! **[I]** the reinforcement weekday is [`REINFORCE_WEEKDAY`], one the server on
//! base accepts (`0..=6`); the real UI sends 255, which a fix in review will
//! accept. Falsifier: that fix landing with 255 refused for 0..=6.

use crate::brain::{Act, Memory, Observation};
use crate::verbs::{FLAG_CARGO, ItemRow, StructureRow};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::structure::{self, DeployType, PointKind, SpaceBall};
use eve_server::structure_space::{ATHANOR, RAITARU};
use std::time::{Duration, Instant};

/// The home hulls a bot corp may anchor: the two Medium Upwell structures
/// builders can make (Raitaru, Athanor).
pub const HULLS: [i64; 2] = [RAITARU, ATHANOR];
/// The reinforcement weekday sent with `Anchor` **[I]** (see the module note).
pub const REINFORCE_WEEKDAY: i64 = 3;
/// The reinforcement hour (UTC), `0..=23`.
pub const REINFORCE_HOUR: i64 = 12;
/// The yaw sent with `Anchor`, radians.
pub const YAW: f64 = 0.5;
/// The range the warp to the home lands at, metres; inside the dock range.
pub const WARP_RANGE: f64 = 1_000.0;
/// How near a planet's warp-in point the ship counts as having arrived,
/// metres **[A]**.
pub const ARRIVED: f64 = 500_000.0;
/// Warps to one planet before the pass tries the next **[A]**.
pub const MAX_WARPS: u32 = 3;
/// Refusals of no known kind before the pass stops.
pub const MAX_OTHER: u32 = 5;
/// `ReleaseControl` tries before the pass stops asking.
pub const MAX_RELEASE: u32 = 3;
/// What the hull costs beyond the ask: the broker's fee and a little slack.
pub const BUY_MARGIN: f64 = 1.05;
/// The docked gate: how often a bot with nothing under way looks.
pub const RECHECK: Duration = Duration::from_secs(600);
/// The gate while a pass is under way.
pub const ACTIVE_GAP: Duration = Duration::from_secs(5);
/// The gate for a bot that is not its corporation's CEO.
pub const NOT_CEO_RECHECK: Duration = Duration::from_secs(3_600);
/// `categoryCelestial`, `categoryStation`, `categoryStructure`: the categories
/// the server measures a placement against.
const PLACEMENT_CATEGORIES: [i64; 3] = [2, 3, 65];
const GROUP_PLANET: i64 = 7;

// ---- the pure planner ------------------------------------------------------------------------

/// Where the director's hull is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hull {
    None,
    /// In the station hangar (item id).
    Hangar(i64),
    /// In the ship's cargo (item id).
    Cargo(i64),
}

/// Whether the hull in the hangar goes in a hold the pilot can fly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The active ship's cargo takes it (or the type data is not known).
    Fits,
    /// An owned ship in the hangar, whose cargo takes it.
    Hauler(i64),
    /// No owned ship does.
    NoHauler,
}

/// The cheapest ask for a home hull where the director is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ask {
    pub type_id: i64,
    pub price: f64,
    pub station: i64,
}

/// The corporation's home structure as the corporation tab lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Home {
    pub id: i64,
    pub system: i64,
}

/// How far the controls have got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Control {
    #[default]
    Not,
    Held,
    Done,
}

/// A planet of the system: whether its warp-in is a place the server would
/// let the hull be anchored, and whether the ship is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Planet {
    pub id: i64,
    /// The warp-in point exists and [`structure::find_deployment_conflict`]
    /// finds nothing there.
    pub clear: bool,
    /// The ship is within [`ARRIVED`] of the warp-in point.
    pub here: bool,
}

/// Everything the planner reads. All of it is read off the world each turn
/// except what the [`Mind`] keeps ([`Facts::control`], [`Facts::tried`],
/// [`Facts::warps_to`], [`Facts::others`], [`Facts::release_fails`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub home: Option<Home>,
    /// The home is in `GetMyDockableStructures` of the pilot's system.
    pub dockable: bool,
    pub docked_in_home: bool,
    /// The session's ship is the home: the pilot sits at its controls.
    pub at_controls: bool,
    /// Docked in a station.
    pub docked_station: bool,
    /// Docked in an Upwell structure (the session keeps its system).
    pub docked_structure: bool,
    /// In the system, and docked nowhere.
    pub in_space: bool,
    /// The ballpark is bound (an ego ball exists).
    pub bound: bool,
    pub system: Option<i64>,
    pub corp: i64,
    pub hull: Hull,
    pub hold: Hold,
    pub ask: Option<Ask>,
    /// The market could not be read.
    pub market_error: Option<String>,
    /// The pilot is the corporation's CEO; `None` when not asked.
    pub ceo: Option<bool>,
    pub isk: f64,
    /// The system's planets, in id order.
    pub planets: Vec<Planet>,
    /// Surface distance to the home in the park.
    pub gap: Option<f64>,
    pub control: Control,
    pub tried: Vec<i64>,
    pub warps_to: Option<(i64, u32)>,
    pub others: u32,
    pub release_fails: u32,
    /// The ship the pilot flew before [`Step::Board`], to fly again.
    pub reboard: Option<i64>,
    /// The session's active ship.
    pub ship: Option<i64>,
    pub dock_range: f64,
    /// Below this a warp is refused ([`crate::MIN_WARP_DISTANCE`]).
    pub warp_min: f64,
    /// The ship is in warp (or about to be).
    pub warping: bool,
}

impl Facts {
    /// **The planet to anchor at**: the first, starting at `corp % planets`
    /// and going round, whose warp-in is clear and which has not been tried.
    pub fn next_planet(&self) -> Option<&Planet> {
        let n = self.planets.len();
        if n == 0 {
            return None;
        }
        let start = (self.corp.unsigned_abs() % n as u64) as usize;
        (0..n)
            .map(|i| &self.planets[(start + i) % n])
            .find(|p| p.clear && !self.tried.contains(&p.id))
    }
}

/// One move of the pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing to do this turn, and why.
    Wait(&'static str),
    /// Under way: in warp.
    Warping,
    Buy {
        type_id: i64,
        price: f64,
        station: i64,
    },
    /// The market could not be read.
    MarketDown(String),
    /// Board an owned hull whose hold takes the structure.
    Board {
        ship: i64,
    },
    Load {
        item: i64,
    },
    Undock,
    UndockStructure,
    Bind {
        system: i64,
    },
    /// Warp to a planet at range 0: the server lands the ship at its warp-in.
    WarpPlanet {
        id: i64,
    },
    /// Give a planet up (it was warped to [`MAX_WARPS`] times).
    Skip {
        id: i64,
    },
    /// Anchor at offset (0, 0), where the ship is.
    Anchor {
        item: i64,
    },
    Warp {
        id: i64,
    },
    Approach {
        id: i64,
    },
    Dock {
        id: i64,
    },
    Take {
        id: i64,
    },
    Release,
    Done,
    GiveUp(String),
}

/// **The next move**, from the facts alone.
pub fn plan(f: &Facts) -> Step {
    // Refused over and over, whatever the step: stop, home or not.
    if f.others >= MAX_OTHER {
        return Step::GiveUp(format!("{} refusals of no known kind", f.others));
    }
    if let Some(home) = f.home {
        // The hull is anchored: the ship flown before the hauler is flown again
        // where it stands in the hangar.
        if f.docked_station
            && let Some(ship) = f.reboard
        {
            return Step::Board { ship };
        }
        if f.docked_in_home {
            if f.at_controls {
                return if f.release_fails >= MAX_RELEASE {
                    Step::GiveUp(format!(
                        "the controls would not release after {} tries",
                        f.release_fails
                    ))
                } else {
                    Step::Release
                };
            }
            return match f.control {
                Control::Not => Step::Take { id: home.id },
                // Controlled and let go: leave the structure, so the bot is
                // not stuck docked in it (the driver reads a structure dock
                // as being in space).
                Control::Held | Control::Done => Step::UndockStructure,
            };
        }
        if matches!(f.control, Control::Held | Control::Done) && !f.docked_station {
            return Step::Done;
        }
        if Some(home.system) != f.system {
            return Step::Wait("home is in another system");
        }
        if !f.dockable {
            return Step::Wait("home is still anchoring");
        }
        if f.docked_station {
            return Step::Undock;
        }
        if f.docked_structure {
            return Step::UndockStructure;
        }
        if !f.in_space {
            return Step::Wait("docked elsewhere");
        }
        if !f.bound {
            return Step::Bind {
                system: home.system,
            };
        }
        if f.warping {
            return Step::Warping;
        }
        return match f.gap {
            None => Step::Wait("home is not in the park"),
            Some(g) if g < f.dock_range => Step::Dock { id: home.id },
            Some(g) if g >= f.warp_min => Step::Warp { id: home.id },
            Some(_) => Step::Approach { id: home.id },
        };
    }
    // No home yet.
    match f.hull {
        Hull::Cargo(item) => {
            if f.docked_station {
                return Step::Undock;
            }
            if f.docked_structure {
                return Step::UndockStructure;
            }
            if !f.in_space {
                return Step::Wait("docked elsewhere");
            }
            if !f.bound {
                return match f.system {
                    Some(system) => Step::Bind { system },
                    None => Step::Wait("no system"),
                };
            }
            if f.warping {
                return Step::Warping;
            }
            match f.next_planet() {
                None => Step::GiveUp(format!(
                    "no planet warp-in is clear ({} planets, {} tried)",
                    f.planets.len(),
                    f.tried.len()
                )),
                Some(p) if p.here => Step::Anchor { item },
                Some(p) => match f.warps_to {
                    Some((id, n)) if id == p.id && n >= MAX_WARPS => Step::Skip { id: p.id },
                    _ => Step::WarpPlanet { id: p.id },
                },
            }
        }
        Hull::Hangar(item) => {
            if !f.docked_station {
                return Step::Wait("hull is in a hangar, not here");
            }
            match f.hold {
                Hold::Fits => Step::Load { item },
                Hold::Hauler(ship) => Step::Board { ship },
                Hold::NoHauler => Step::Wait("no hauler for the hull"),
            }
        }
        Hull::None => {
            if !f.docked_station {
                return Step::Wait("no hull and not at a station");
            }
            if f.ceo == Some(false) {
                return Step::Wait("not the corporation's CEO");
            }
            if let Some(why) = &f.market_error {
                return Step::MarketDown(why.clone());
            }
            match f.ask {
                None => Step::Wait("no builder ask for a home hull here"),
                Some(a) if f.isk < a.price * BUY_MARGIN => Step::Wait("short of ISK for the hull"),
                Some(a) => Step::Buy {
                    type_id: a.type_id,
                    price: a.price,
                    station: a.station,
                },
            }
        }
    }
}

/// **Which planets the hull may be anchored at**, from the system's balls.
/// `planets` is `(id, type, position, radius)`; `balls` is every celestial
/// and structure the server measures a placement against. Each planet's
/// warp-in is where a ship told to warp to it at range 0 lands.
pub fn survey(
    ego: [f64; 3],
    planets: &[(i64, i64, [f64; 3], f64)],
    balls: &[SpaceBall],
    subject: &DeployType,
) -> Vec<Planet> {
    planets
        .iter()
        .map(|&(id, type_id, position, radius)| {
            let c = eve_server::celestial::Celestial {
                id,
                type_id,
                radius,
                position,
                kind: eve_server::celestial::Kind::Planet,
            };
            let at = eve_server::warp_in::warp_in_point(&c);
            Planet {
                id,
                clear: at.is_some_and(|p| {
                    structure::find_deployment_conflict(subject, p, balls, &[]).is_none()
                }),
                here: at.is_some_and(|p| structure::distance(ego, p) < ARRIVED),
            }
        })
        .collect()
}

/// **Whether the hull in the hangar goes in a hold the pilot can fly** (pure
/// over the rows): the active ship's cargo capacity against the hull's volume,
/// both from the type data the server uses; else the lowest-id owned ship in
/// the hangar whose cargo takes it and that was not refused (`refused`).
pub fn hold_for(
    content: Option<&Content>,
    active: Option<i64>,
    hull_type: i64,
    hangar: &[ItemRow],
    me: i64,
    refused: &[i64],
) -> Hold {
    let Some(content) = content else {
        return Hold::Fits;
    };
    let Some(volume) = eve_server::item::find_in(content, hull_type).map(|t| t.volume) else {
        return Hold::Fits;
    };
    let takes = |ship_type: i64| {
        eve_server::item::hold_capacity_in(content, ship_type, FLAG_CARGO) >= volume
    };
    let active_type =
        active.and_then(|s| hangar.iter().find(|r| r.item_id == s).map(|r| r.type_id));
    if active_type.is_some_and(takes) {
        return Hold::Fits;
    }
    hangar
        .iter()
        .filter(|r| {
            r.owner_id == me
                && r.category_id == crate::acquire::CATEGORY_SHIP
                && r.singleton
                && Some(r.item_id) != active
                && !refused.contains(&r.item_id)
                && takes(r.type_id)
        })
        .map(|r| r.item_id)
        .min()
        .map_or(Hold::NoHauler, Hold::Hauler)
}

/// **A step was refused**: count it, stop carrying the pass on in space (the
/// role resumes and the docked gate looks again after [`RECHECK`]), and never
/// try a refused hauler (or the ship to board again) twice. The pass gives up
/// after [`MAX_OTHER`] in a row; any success resets the count. A refused
/// `UndockStructure` keeps the pass active: the bot is still inside the
/// structure, where the role cannot run, so the pass retries the undock and
/// [`MAX_OTHER`] still bounds it.
pub fn refuse(mind: &mut Mind, step: &Step) {
    mind.others += 1;
    if !matches!(step, Step::UndockStructure) {
        mind.active = false;
    }
    if let Step::Board { ship } = step {
        if mind.prior_ship == Some(*ship) {
            mind.prior_ship = None;
        } else {
            mind.refused.push(*ship);
        }
    }
}

/// What the refused `Anchor` means for the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The spot is bad (something too near): try the next planet.
    Placement,
    /// The pilot may not manage structures: not now.
    Role,
    /// Anything else: the item, the ship, the name.
    Other,
}

/// Read the server's refusal text (`structure_space::anchor`,
/// `service/structures.rs::manager_corp`).
pub fn classify(text: &str) -> Refusal {
    let t = text.to_ascii_lowercase();
    if t.contains("station manager or director") || t.contains("player corporation") {
        Refusal::Role
    } else if t.contains("cannot be anchored within")
        || t.contains("too far")
        || t.contains("too close")
        || t.contains("within 250 km")
    {
        Refusal::Placement
    } else {
        Refusal::Other
    }
}

/// The home hull among item rows owned by `owner`: `(item id, type id)`.
pub fn hull_in(rows: &[ItemRow], owner: i64) -> Option<(i64, i64)> {
    rows.iter()
        .find(|r| r.owner_id == owner && HULLS.contains(&r.type_id))
        .map(|r| (r.item_id, r.type_id))
}

/// The corporation's home among its structures: the first home hull, lowest id.
pub fn pick_home(rows: &[StructureRow]) -> Option<Home> {
    rows.iter()
        .filter(|r| HULLS.contains(&r.type_id))
        .min_by_key(|r| r.structure_id)
        .map(|r| Home {
            id: r.structure_id,
            system: r.system,
        })
}

/// The structure's name: three characters or more are needed.
pub fn home_name(corp: i64) -> String {
    format!("Home {corp}")
}

// ---- the gate ----------------------------------------------------------------------------------

/// What the pass keeps between turns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    pub control: Control,
    /// Planets given up: the server refused the spot, or the warps did not land.
    pub tried: Vec<i64>,
    /// The planet warped to and how many times.
    pub warps_to: Option<(i64, u32)>,
    /// Refusals of no known kind since the last success.
    pub others: u32,
    pub release_fails: u32,
    /// Hauler ids the server refused to let the pilot board.
    pub refused: Vec<i64>,
    /// The ship flown before the first [`Step::Board`], to board again once
    /// the hull is anchored and the pilot stands at its station.
    pub prior_ship: Option<i64>,
    /// Why the pass stopped for good.
    pub gave_up: Option<String>,
    /// The home is docked in, controlled and released.
    pub finished: bool,
    /// A pass is under way and a bot in space must carry on with it.
    pub active: bool,
    /// The pilot is not the CEO (or may not manage structures): look again late.
    pub not_ceo: bool,
    /// When the gate last fired.
    pub checked: Option<Instant>,
}

/// **Is a turn due?** (pure) Not for a pass that is over; an hour for a
/// bot that may not anchor; seconds for a pass under way; else [`RECHECK`].
pub fn due_at(mind: &Mind, now: Instant) -> bool {
    if mind.gave_up.is_some() || mind.finished {
        return false;
    }
    let gap = if mind.active {
        ACTIVE_GAP
    } else if mind.not_ceo {
        NOT_CEO_RECHECK
    } else {
        RECHECK
    };
    mind.checked
        .is_none_or(|t| now.saturating_duration_since(t) >= gap)
}

/// The brain's docked hook: one [`Act::UpwellHome`] when [`due_at`] says so.
pub fn due(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !o.ceo && !m.upwell_home.active {
        return None;
    }
    gate(m, o)
}

/// The brain's in-space hook: a pass under way goes on in space.
pub fn in_space(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !m.upwell_home.active {
        return None;
    }
    gate(m, o)
}

fn gate(m: &mut Memory, o: &Observation) -> Option<Vec<Act>> {
    if !due_at(&m.upwell_home, o.now) {
        return None;
    }
    m.upwell_home.checked = Some(o.now);
    let mut acts = vec![Act::UpwellHome];
    if m.upwell_home.active {
        acts.push(Act::Wait(ACTIVE_GAP));
    }
    Some(acts)
}

// ---- the executor ----------------------------------------------------------------------------

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// The type's group and category from the loaded type data.
fn class_of(content: &Content, type_id: i64) -> Option<(i64, i64)> {
    eve_server::typedata::installed_in(content)
        .table()
        .get(type_id)
        .map(|r| (r.group_id, r.category_id))
}

/// The system's planets and the balls a placement is measured against, read
/// from the park (the server serves the same rows it checks against).
fn read_planets(w: &crate::World, content: &Content, hull_type: i64) -> Vec<Planet> {
    let Some(ego) = w.ego_ball() else {
        return Vec::new();
    };
    let mut planets = Vec::new();
    let mut balls = Vec::new();
    for b in w.balls() {
        if Some(b.ball_id) == w.ego || crate::park::is_rock(b.ball_id) {
            continue;
        }
        let Some(type_id) = b.type_id() else { continue };
        let Some((group_id, category_id)) = class_of(content, type_id) else {
            continue;
        };
        if !PLACEMENT_CATEGORIES.contains(&category_id) {
            continue;
        }
        balls.push(SpaceBall {
            item_id: b.ball_id,
            type_id,
            group_id,
            category_id,
            pos: b.motion.position,
            radius: b.radius,
            kind: PointKind::RealBall,
        });
        if group_id == GROUP_PLANET {
            planets.push((b.ball_id, type_id, b.motion.position, b.radius));
        }
    }
    planets.sort_by_key(|p| p.0);
    let subject = DeployType {
        type_id: hull_type,
        group_id: eve_server::structure_profile::group_of(hull_type),
        category_id: eve_server::structure_profile::CATEGORY_STRUCTURE,
        radius: eve_server::structure_space::radius(content, hull_type),
    };
    survey(ego.motion.position, &planets, &balls, &subject)
}

/// Am I my corporation's CEO? `CanLeaveCurrentCorporation` says
/// `CrpCantQuitCEO` for exactly the CEO (as `corpmind::read_ceo`).
fn read_ceo(conn: &mut Conn, corp: i64) -> Result<bool> {
    let a = crate::corp_ops::corp_call(conn, corp, "CanLeaveCurrentCorporation", vec![])?;
    Ok(match a.value {
        Some(PyValue::Tuple(items)) => items.get(1).is_some_and(|v| match v {
            PyValue::Unicode(s) => s == "CrpCantQuitCEO",
            PyValue::Bytes(b) => b == b"CrpCantQuitCEO",
            _ => false,
        }),
        _ => false,
    })
}

/// Read the world into [`Facts`].
fn read_facts(conn: &mut Conn, content: Option<&Content>, mind: &Mind) -> Result<Facts> {
    // Take in what the server pushed since the last turn (the park).
    soft(conn.pump(Duration::from_millis(100)))?.ok();
    let me = conn
        .session()
        .char_id()
        .ok_or_else(|| Error::NotSent("upwell home: no character".to_owned()))?;
    let (structure_here, station, system, in_system, ship_id, corp) = {
        let s = conn.session();
        (
            s.get_i64("structureid"),
            s.station_id(),
            s.solar_system_id2(),
            s.solar_system_id().is_some(),
            s.ship_id(),
            s.get_i64("corpid").unwrap_or(0),
        )
    };
    let docked_station = station.is_some();
    let docked_structure = structure_here.is_some() && !docked_station;
    // A structure keeps the session's system: docked there is not in space.
    let in_space = in_system && !docked_station && structure_here.is_none();
    let home = soft(conn.corp_structures())?
        .ok()
        .and_then(|rows| pick_home(&rows));
    let dockable = match (home, system) {
        (Some(h), Some(s)) if h.system == s => soft(conn.dockable_structures(s))?
            .map(|ids| ids.contains(&h.id))
            .unwrap_or(false),
        _ => false,
    };
    let docked_in_home = home.is_some_and(|h| structure_here == Some(h.id));
    let at_controls = docked_in_home && home.is_some_and(|h| ship_id == Some(h.id));
    let (bound, gap, warping) = {
        let w = conn.world_now();
        (
            w.ego.is_some(),
            home.and_then(|h| w.ego.and_then(|e| w.surface_distance(e, h.id))),
            w.ego_ball().is_some_and(|b| b.warp_commanded()),
        )
    };
    let mut hull = Hull::None;
    let mut hull_type = 0;
    let mut hold = Hold::Fits;
    if home.is_none() {
        let cargo = soft(conn.cargo())?.unwrap_or_default();
        if let Some((item, ty)) = hull_in(&cargo, me) {
            hull = Hull::Cargo(item);
            hull_type = ty;
        } else if docked_station {
            let rows = soft(conn.hangar_items())?.unwrap_or_default();
            if let Some((item, ty)) = hull_in(&rows, me) {
                hull = Hull::Hangar(item);
                hull_type = ty;
                hold = hold_for(content, ship_id, ty, &rows, me, &mind.refused);
            }
        }
    }
    let planets = match content {
        Some(c) if matches!(hull, Hull::Cargo(_)) && in_space => {
            read_planets(conn.world_now(), c, hull_type)
        }
        _ => Vec::new(),
    };
    let mut reboard = None;
    if home.is_some()
        && docked_station
        && let Some(prior) = mind.prior_ship
        && ship_id != Some(prior)
    {
        let rows = soft(conn.hangar_items())?.unwrap_or_default();
        if rows.iter().any(|r| r.item_id == prior && r.owner_id == me) {
            reboard = Some(prior);
        }
    }
    let mut ask = None;
    let mut market_error = None;
    let mut ceo = None;
    let mut isk = 0.0;
    if home.is_none()
        && hull == Hull::None
        && let Some(station) = station
    {
        ceo = Some(soft(read_ceo(conn, corp))?.unwrap_or(false));
        if ceo == Some(true) {
            for t in HULLS {
                match soft(conn.market_orders(t))? {
                    Ok(book) => {
                        if let Some(a) = book
                            .asks
                            .iter()
                            .filter(|o| o.station_id == station)
                            .min_by(|a, b| a.price.total_cmp(&b.price))
                            && ask.is_none_or(|c: Ask| a.price < c.price)
                        {
                            ask = Some(Ask {
                                type_id: t,
                                price: a.price,
                                station: a.station_id,
                            });
                        }
                    }
                    Err(why) => market_error = Some(format!("market {t}: {why}")),
                }
            }
            isk = soft(crate::acquire::spendable(conn))?.unwrap_or(0.0);
        }
    }
    Ok(Facts {
        home,
        dockable,
        docked_in_home,
        at_controls,
        docked_station,
        docked_structure,
        in_space,
        bound,
        system,
        corp,
        hull,
        hold,
        ask,
        market_error,
        ceo,
        isk,
        planets,
        gap,
        control: mind.control,
        tried: mind.tried.clone(),
        warps_to: mind.warps_to,
        others: mind.others,
        release_fails: mind.release_fails,
        reboard,
        ship: ship_id,
        dock_range: crate::DOCK_RANGE,
        warp_min: crate::MIN_WARP_DISTANCE,
        warping,
    })
}

/// **One turn of the pass**: read the world, make the one move [`plan`] names,
/// and say what happened. A refusal is a note, not an error; only a broken
/// connection is an `Err`.
pub fn turn(conn: &mut Conn, content: Option<&Content>, mind: &mut Mind) -> Result<String> {
    if let Some(why) = &mind.gave_up {
        mind.active = false;
        return Ok(format!("upwell home: stopped ({why})"));
    }
    let facts = read_facts(conn, content, mind)?;
    let step = plan(&facts);
    mind.active = !matches!(
        step,
        Step::Wait(_) | Step::Done | Step::GiveUp(_) | Step::MarketDown(_)
    );
    mind.not_ceo = matches!(step, Step::Wait("not the corporation's CEO"));
    let note = match step.clone() {
        Step::Wait(why) => format!("waiting ({why})"),
        Step::Warping => "waiting (warping)".to_owned(),
        Step::Done => {
            mind.finished = true;
            "done: docked, controlled and released".to_owned()
        }
        Step::GiveUp(why) => {
            mind.gave_up = Some(why.clone());
            format!("giving up ({why})")
        }
        Step::MarketDown(why) => format!("waiting (the market: {why})"),
        Step::Buy {
            type_id,
            price,
            station,
        } => match soft(conn.buy(station, type_id, price, 1, 0, crate::pricing::BROKER_FEE))? {
            Ok(_) => {
                mind.others = 0;
                format!("bought hull {type_id} at {price:.0} ISK")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("buy hull {type_id}: {why}")
            }
        },
        Step::Board { ship } => match soft(conn.board(ship))? {
            Ok(_) => {
                mind.others = 0;
                if mind.prior_ship == Some(ship) {
                    mind.prior_ship = None;
                } else if mind.prior_ship.is_none() {
                    mind.prior_ship = facts.ship;
                }
                format!("boarded {ship}")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("board {ship}: {why}")
            }
        },
        Step::Load { item } => match soft(conn.load_cargo(item, None))? {
            Ok(_) => {
                mind.others = 0;
                format!("hull {item} is in the hold")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("load hull {item}: {why}")
            }
        },
        Step::Undock => match soft(conn.undock_repaired())? {
            Ok(_) => {
                mind.others = 0;
                "undocked".to_owned()
            }
            Err(why) => {
                refuse(mind, &step);
                format!("undock: {why}")
            }
        },
        Step::UndockStructure => match soft(conn.undock_structure())? {
            Ok(_) => {
                mind.others = 0;
                "left the structure".to_owned()
            }
            Err(why) => {
                refuse(mind, &step);
                format!("undock from the structure: {why}")
            }
        },
        Step::Bind { system } => match soft(conn.enter_space(system))? {
            Ok(_) => {
                mind.others = 0;
                format!("bound system {system}")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("bind {system}: {why}")
            }
        },
        Step::WarpPlanet { id } => {
            mind.warps_to = Some(match mind.warps_to {
                Some((p, n)) if p == id => (p, n + 1),
                _ => (id, 1),
            });
            match soft(conn.warp_to_item(id, 0.0))? {
                Ok(_) => {
                    mind.others = 0;
                    format!("warping to planet {id}")
                }
                Err(why) => {
                    refuse(mind, &step);
                    format!("warp to planet {id}: {why}")
                }
            }
        }
        Step::Skip { id } => {
            mind.tried.push(id);
            mind.warps_to = None;
            format!("skipping planet {id}: {MAX_WARPS} warps did not land at its warp-in")
        }
        Step::Anchor { item } => {
            let corp = facts.corp;
            let here = facts.planets.iter().find(|p| p.here).map(|p| p.id);
            match soft(conn.anchor_structure(
                item,
                0.0,
                0.0,
                YAW,
                &home_name(corp),
                REINFORCE_WEEKDAY,
                REINFORCE_HOUR,
            ))? {
                Ok(_) => {
                    mind.others = 0;
                    format!("anchored hull {item} at planet {here:?}")
                }
                Err(why) => match classify(&why) {
                    Refusal::Placement => {
                        mind.tried.extend(here);
                        mind.warps_to = None;
                        format!("anchor refused for the spot at planet {here:?}: {why}")
                    }
                    Refusal::Role => {
                        mind.not_ceo = true;
                        mind.active = false;
                        format!("anchor: {why}")
                    }
                    Refusal::Other => {
                        refuse(mind, &step);
                        format!("anchor: {why}")
                    }
                },
            }
        }
        Step::Warp { id } => match soft(conn.warp_to_item(id, WARP_RANGE))? {
            Ok(_) => {
                mind.others = 0;
                format!("warping to home {id}")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("warp to {id}: {why}")
            }
        },
        Step::Approach { id } => match soft(conn.follow(id, 0.0))? {
            Ok(_) => {
                mind.others = 0;
                format!("approaching home {id}")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("approach {id}: {why}")
            }
        },
        Step::Dock { id } => match soft(conn.dock_structure(id))? {
            Ok(_) => {
                mind.others = 0;
                format!("docked in home {id}")
            }
            Err(why) => {
                refuse(mind, &step);
                format!("dock {id}: {why}")
            }
        },
        Step::Take { id } => match soft(conn.take_control(id))? {
            Ok(_) => {
                mind.control = Control::Held;
                format!("took the controls of {id}")
            }
            Err(why) => {
                // Control past docking depends on the owner's profile (#41):
                // a refusal ends the controls, not the home.
                mind.control = Control::Done;
                format!("controls of {id} refused: {why}")
            }
        },
        Step::Release => match soft(conn.release_control())? {
            Ok(_) => {
                mind.control = Control::Done;
                mind.release_fails = 0;
                "released the controls".to_owned()
            }
            Err(why) => {
                mind.release_fails += 1;
                format!("release: {why}")
            }
        },
    };
    if mind.gave_up.is_some() {
        mind.active = false;
    }
    Ok(format!("upwell home: {note}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verbs::FLAG_CARGO;

    // Invented ids throughout.
    const HOME: Home = Home {
        id: 9_100_001,
        system: 9_200_001,
    };
    const ITEM: i64 = 9_300_001;
    const ME: i64 = 9_400_001;
    /// An invented hauler hull with a big hold, and the hull's volume.
    const HAULER: i64 = 9_500_901;
    const VOLUME: f64 = 7_000.0;

    fn base() -> Facts {
        Facts {
            home: None,
            dockable: false,
            docked_in_home: false,
            at_controls: false,
            docked_station: true,
            docked_structure: false,
            in_space: false,
            bound: false,
            system: Some(HOME.system),
            corp: 9_600_005,
            hull: Hull::None,
            hold: Hold::Fits,
            ask: None,
            market_error: None,
            ceo: Some(true),
            isk: 0.0,
            planets: Vec::new(),
            gap: None,
            control: Control::Not,
            tried: Vec::new(),
            warps_to: None,
            others: 0,
            release_fails: 0,
            reboard: None,
            ship: None,
            dock_range: 2_500.0,
            warp_min: 150_000.0,
            warping: false,
        }
    }

    /// In space, bound, the hull in the hold, three planets.
    fn flying() -> Facts {
        let mut f = base();
        f.hull = Hull::Cargo(ITEM);
        f.docked_station = false;
        f.in_space = true;
        f.bound = true;
        f.planets = (1..=3)
            .map(|i| Planet {
                id: 9_700_000 + i,
                clear: true,
                here: false,
            })
            .collect();
        f
    }

    fn ask(price: f64) -> Ask {
        Ask {
            type_id: HULLS[0],
            price,
            station: 9_500_001,
        }
    }

    #[test]
    fn no_hull_and_no_ask_waits_and_never_buys_from_nowhere() {
        let f = base();
        assert!(matches!(plan(&f), Step::Wait(_)));
    }

    #[test]
    fn a_hull_is_bought_only_with_the_isk_for_it() {
        let mut f = base();
        f.ask = Some(ask(1_000.0));
        f.isk = 1_000.0;
        assert!(matches!(plan(&f), Step::Wait(_)), "no margin for the fee");
        f.isk = 1_000.0 * BUY_MARGIN;
        assert_eq!(
            plan(&f),
            Step::Buy {
                type_id: HULLS[0],
                price: 1_000.0,
                station: 9_500_001
            }
        );
    }

    #[test]
    fn only_the_ceo_buys_and_a_market_that_cannot_be_read_is_said() {
        let mut f = base();
        f.ask = Some(ask(1.0));
        f.isk = 1e9;
        f.ceo = Some(false);
        assert_eq!(plan(&f), Step::Wait("not the corporation's CEO"));
        f.ceo = Some(true);
        f.market_error = Some("no market".to_owned());
        assert_eq!(plan(&f), Step::MarketDown("no market".to_owned()));
    }

    #[test]
    fn a_hull_in_the_hangar_is_loaded_only_where_it_fits() {
        let mut f = base();
        f.hull = Hull::Hangar(ITEM);
        assert_eq!(plan(&f), Step::Load { item: ITEM });
        f.hold = Hold::Hauler(HAULER);
        assert_eq!(plan(&f), Step::Board { ship: HAULER });
        f.hold = Hold::NoHauler;
        assert_eq!(plan(&f), Step::Wait("no hauler for the hull"));
    }

    #[test]
    fn a_refused_load_is_counted_and_only_many_end_the_pass() {
        let mut f = base();
        f.hull = Hull::Hangar(ITEM);
        f.others = MAX_OTHER - 1;
        assert_eq!(plan(&f), Step::Load { item: ITEM });
        f.others = MAX_OTHER;
        assert!(matches!(plan(&f), Step::GiveUp(_)));
    }

    #[test]
    fn the_hull_in_the_hold_undocks_binds_warps_to_a_planet_then_anchors_there() {
        let mut f = flying();
        f.docked_station = true;
        f.in_space = false;
        f.bound = false;
        assert_eq!(plan(&f), Step::Undock);
        f.docked_station = false;
        f.in_space = true;
        assert_eq!(
            plan(&f),
            Step::Bind {
                system: HOME.system
            }
        );
        f.bound = true;
        let first = f.next_planet().expect("a planet").id;
        assert_eq!(plan(&f), Step::WarpPlanet { id: first });
        f.warping = true;
        assert_eq!(plan(&f), Step::Warping, "no anchor while warping");
        f.warping = false;
        for p in &mut f.planets {
            p.here = p.id == first;
        }
        assert_eq!(plan(&f), Step::Anchor { item: ITEM });
    }

    #[test]
    fn the_planet_is_the_corps_and_a_tried_or_blocked_one_is_passed() {
        let mut f = flying();
        let ids: Vec<i64> = f.planets.iter().map(|p| p.id).collect();
        let at = (f.corp.unsigned_abs() % 3) as usize;
        assert_eq!(f.next_planet().map(|p| p.id), Some(ids[at]));
        f.tried.push(ids[at]);
        assert_eq!(f.next_planet().map(|p| p.id), Some(ids[(at + 1) % 3]));
        f.planets[(at + 1) % 3].clear = false;
        assert_eq!(f.next_planet().map(|p| p.id), Some(ids[(at + 2) % 3]));
        f.tried.push(ids[(at + 2) % 3]);
        assert!(matches!(plan(&f), Step::GiveUp(_)), "all tried or blocked");
    }

    #[test]
    fn a_planet_the_warps_do_not_land_at_is_skipped() {
        let mut f = flying();
        let first = f.next_planet().expect("a planet").id;
        f.warps_to = Some((first, MAX_WARPS - 1));
        assert_eq!(plan(&f), Step::WarpPlanet { id: first });
        f.warps_to = Some((first, MAX_WARPS));
        assert_eq!(plan(&f), Step::Skip { id: first });
        f.warps_to = Some((first + 100, MAX_WARPS));
        assert_eq!(plan(&f), Step::WarpPlanet { id: first });
    }

    #[test]
    fn docked_in_a_structure_is_not_in_space_and_undocks_from_it() {
        // Finding 5: the session keeps its system, so a pilot docked in a
        // structure reads as in the system; `in_space` says no.
        let mut f = flying();
        f.in_space = false;
        f.docked_structure = true;
        assert_eq!(plan(&f), Step::UndockStructure);
        f.hull = Hull::None;
        f.home = Some(HOME);
        f.dockable = true;
        assert_eq!(plan(&f), Step::UndockStructure);
    }

    #[test]
    fn the_refusals_are_read_by_what_they_mean() {
        assert_eq!(
            classify("A structure cannot be anchored within 1000 km of that."),
            Refusal::Placement
        );
        assert_eq!(
            classify("That is too far from your ship."),
            Refusal::Placement
        );
        assert_eq!(
            classify("Too close to the moon and too far from its mining beacon."),
            Refusal::Placement
        );
        assert_eq!(
            classify("You need the station manager or director role to do that."),
            Refusal::Role
        );
        assert_eq!(
            classify("That structure is not in your ship's hold."),
            Refusal::Other
        );
    }

    #[test]
    fn an_anchoring_home_is_waited_for_not_docked() {
        let mut f = base();
        f.home = Some(HOME);
        f.dockable = false;
        f.docked_station = false;
        f.in_space = true;
        f.bound = true;
        f.gap = Some(0.0);
        assert!(matches!(plan(&f), Step::Wait(_)));
        f.dockable = true;
        assert_eq!(plan(&f), Step::Dock { id: HOME.id });
    }

    #[test]
    fn far_from_the_home_it_approaches_and_inside_range_it_docks() {
        let mut f = base();
        f.home = Some(HOME);
        f.dockable = true;
        f.docked_station = false;
        f.in_space = true;
        f.bound = true;
        f.gap = Some(3_000.0);
        assert_eq!(plan(&f), Step::Approach { id: HOME.id });
        f.gap = Some(150_000.0);
        assert_eq!(plan(&f), Step::Warp { id: HOME.id });
        f.warping = true;
        assert_eq!(plan(&f), Step::Warping, "no second warp");
        f.warping = false;
        f.gap = Some(2_499.0);
        assert_eq!(plan(&f), Step::Dock { id: HOME.id });
        f.gap = None;
        assert!(matches!(plan(&f), Step::Wait(_)));
    }

    #[test]
    fn a_home_elsewhere_is_not_flown_to() {
        let mut f = base();
        f.home = Some(Home {
            system: HOME.system + 1,
            ..HOME
        });
        f.dockable = true;
        assert!(matches!(plan(&f), Step::Wait(_)));
    }

    #[test]
    fn docked_in_the_home_it_takes_the_controls_once_and_releases_them() {
        let mut f = base();
        f.home = Some(HOME);
        f.docked_station = false;
        f.docked_in_home = true;
        assert_eq!(plan(&f), Step::Take { id: HOME.id });
        // Held, and the session's ship is the structure: let go.
        f.control = Control::Held;
        f.at_controls = true;
        assert_eq!(plan(&f), Step::Release);
        // Let go (the session says so): done.
        f.at_controls = false;
        f.control = Control::Done;
        // Let go and still docked in the home: leave it, so the bot is not
        // stuck docked (review r2 finding C); outside, the pass is over.
        assert_eq!(plan(&f), Step::UndockStructure);
        f.docked_in_home = false;
        f.docked_structure = false;
        f.in_space = true;
        assert_eq!(plan(&f), Step::Done);
    }

    #[test]
    fn at_the_controls_it_lets_go_whatever_the_memory_says() {
        // Finding 6: a restart forgets `Held`; the session does not.
        let mut f = base();
        f.home = Some(HOME);
        f.docked_station = false;
        f.docked_in_home = true;
        f.at_controls = true;
        for c in [Control::Not, Control::Held, Control::Done] {
            f.control = c;
            assert_eq!(plan(&f), Step::Release, "{c:?}");
        }
        // And the memory's `Held` with the session saying no is not a hold.
        f.at_controls = false;
        f.control = Control::Held;
        assert_eq!(plan(&f), Step::UndockStructure);
    }

    #[test]
    fn a_release_that_keeps_failing_is_not_asked_forever() {
        let mut f = base();
        f.home = Some(HOME);
        f.docked_station = false;
        f.docked_in_home = true;
        f.at_controls = true;
        f.release_fails = MAX_RELEASE - 1;
        assert_eq!(plan(&f), Step::Release);
        f.release_fails = MAX_RELEASE;
        assert!(matches!(plan(&f), Step::GiveUp(_)));
    }

    #[test]
    fn a_docked_pilot_leaves_the_station_for_a_home_it_can_dock_at() {
        let mut f = base();
        f.home = Some(HOME);
        f.dockable = true;
        assert_eq!(plan(&f), Step::Undock);
    }

    #[test]
    fn only_the_pilots_own_home_hull_counts() {
        let row = |item_id, type_id, owner_id| ItemRow {
            item_id,
            type_id,
            owner_id,
            location_id: 1,
            flag_id: FLAG_CARGO,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: false,
        };
        let rows = [
            row(1, 9_999_999, ME),
            row(2, HULLS[1], ME + 1),
            row(3, HULLS[1], ME),
        ];
        assert_eq!(hull_in(&rows, ME), Some((3, HULLS[1])));
        assert_eq!(hull_in(&rows[..2], ME), None);
    }

    #[test]
    fn the_home_is_the_lowest_id_home_hull() {
        let row = |structure_id, type_id| StructureRow {
            structure_id,
            type_id,
            system: HOME.system,
            state: 0,
        };
        let rows = [row(30, HULLS[0]), row(10, 9_999_999), row(20, HULLS[1])];
        assert_eq!(
            pick_home(&rows),
            Some(Home {
                id: 20,
                system: HOME.system
            })
        );
        assert_eq!(pick_home(&rows[1..2]), None);
    }

    #[test]
    fn the_name_is_long_enough_for_the_server() {
        assert!(eve_server::structure::valid_name(&home_name(1)));
    }

    #[test]
    fn the_reinforcement_time_is_one_the_server_takes() {
        assert_eq!(
            eve_server::structure::check_reinforce_time(REINFORCE_WEEKDAY, REINFORCE_HOUR),
            Ok(())
        );
    }

    // ---- the hold, the survey and the gate ----------------------------------------------------

    /// Fixture tables and two invented ships: a hauler with a big hold and
    /// the hull type with an invented volume (above the frigate hold, below the hauler).
    fn content() -> Content {
        let mut c = Content::fixture();
        let rows = format!(
            "{}type\t{HAULER}\t513\t6\t1\t1\t1000000.0\t20000.0\t1000.0\t100.0\t0\t\t\tInvented Hauler\n\
             type\t{RAITARU}\t{}\t{}\t1\t1\t{VOLUME}\t0.0\t1000.0\t1000.0\t0\t\t\tInvented Hull Row\n",
            eve_server::typedata::FIXTURE,
            eve_server::structure_profile::group_of(RAITARU),
            eve_server::structure_profile::CATEGORY_STRUCTURE,
        );
        let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        c
    }

    fn ship(item_id: i64, type_id: i64, owner_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id,
            location_id: 1,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: crate::acquire::CATEGORY_SHIP,
            singleton: true,
        }
    }

    #[test]
    fn a_hull_too_big_for_the_frigate_does_not_go_in_a_frigates_hold_but_a_hauler_takes_it() {
        // Finding 3. 990000001 is the fixture frigate (130 m3 of cargo).
        let c = content();
        let frigate = 990_000_001;
        let rows = [ship(11, frigate, ME), ship(12, HAULER, ME)];
        assert_eq!(
            hold_for(Some(&c), Some(11), RAITARU, &rows, ME, &[]),
            Hold::Hauler(12)
        );
        assert_eq!(
            hold_for(Some(&c), Some(12), RAITARU, &rows, ME, &[]),
            Hold::Fits
        );
        // Somebody else's hauler is not mine to board.
        let theirs = [ship(11, frigate, ME), ship(12, HAULER, ME + 1)];
        assert_eq!(
            hold_for(Some(&c), Some(11), RAITARU, &theirs, ME, &[]),
            Hold::NoHauler
        );
        // No type data: try the load.
        assert_eq!(
            hold_for(None, Some(11), RAITARU, &rows, ME, &[]),
            Hold::Fits
        );
    }

    #[test]
    fn a_planet_whose_warp_in_is_near_a_station_or_moon_is_not_clear() {
        // Finding 1. Invented geometry; the warp-in is the server's own.
        let planet = (9_800_001, 9_800_100, [2.0e9, 0.0, 1.0e9], 6.0e6);
        let c = eve_server::celestial::Celestial {
            id: planet.0,
            type_id: planet.1,
            radius: planet.3,
            position: planet.2,
            kind: eve_server::celestial::Kind::Planet,
        };
        let at = eve_server::warp_in::warp_in_point(&c).expect("a planet has a warp-in");
        let subject = DeployType {
            type_id: RAITARU,
            group_id: eve_server::structure_profile::group_of(RAITARU),
            category_id: eve_server::structure_profile::CATEGORY_STRUCTURE,
            radius: 1_000.0,
        };
        let ball = |group_id, category_id, pos| SpaceBall {
            item_id: 9_800_500,
            type_id: 9_800_501,
            group_id,
            category_id,
            pos,
            radius: 0.0,
            kind: PointKind::RealBall,
        };
        let near = |d: f64| [at[0] + d, at[1], at[2]];
        let ego = [0.0, 0.0, 0.0];
        let seen = |balls: &[SpaceBall]| survey(ego, &[planet], balls, &subject)[0];
        assert!(seen(&[]).clear);
        assert!(!seen(&[ball(15, 3, near(500_000.0))]).clear, "a station");
        assert!(!seen(&[ball(8, 2, near(3_000_000.0))]).clear, "a moon");
        assert!(seen(&[ball(15, 3, near(2_000_000.0))]).clear, "far enough");
        assert!(!seen(&[]).here);
        assert!(survey(at, &[planet], &[], &subject)[0].here);
    }

    #[test]
    fn the_gate_is_paced_and_stops_when_the_pass_is_over() {
        let t0 = Instant::now();
        let later = |s: u64| t0 + Duration::from_secs(s);
        let mut mind = Mind::default();
        assert!(due_at(&mind, t0), "never looked");
        mind.checked = Some(t0);
        assert!(!due_at(&mind, later(599)));
        assert!(due_at(&mind, later(600)));
        mind.active = true;
        assert!(!due_at(&mind, later(4)));
        assert!(due_at(&mind, later(5)), "a pass under way is minded often");
        mind.active = false;
        mind.not_ceo = true;
        assert!(!due_at(&mind, later(3_599)));
        assert!(due_at(&mind, later(3_600)));
        mind.not_ceo = false;
        mind.finished = true;
        assert!(!due_at(&mind, later(1_000_000)));
        mind.finished = false;
        mind.gave_up = Some("x".to_owned());
        assert!(!due_at(&mind, later(1_000_000)));
    }

    // ---- review r2 ----------------------------------------------------------------------------

    #[test]
    fn a_refused_step_does_not_keep_the_pass_active_and_many_end_it() {
        // Finding A. Every kind of step, not only Load and Anchor.
        let steps = [
            Step::Board { ship: HAULER },
            Step::Undock,
            Step::Bind { system: 1 },
            Step::Approach { id: 1 },
            Step::Dock { id: 1 },
            Step::Warp { id: 1 },
            Step::Buy {
                type_id: 1,
                price: 1.0,
                station: 1,
            },
        ];
        for step in &steps {
            let mut mind = Mind {
                active: true,
                ..Mind::default()
            };
            refuse(&mut mind, step);
            assert!(!mind.active, "{step:?} kept the pass active");
            assert_eq!(mind.others, 1, "{step:?}");
        }
        // A refused hauler is not boarded again.
        let mut mind = Mind::default();
        refuse(&mut mind, &Step::Board { ship: 12 });
        assert_eq!(mind.refused, vec![12]);
        let c = content();
        let rows = [ship(11, 990_000_001, ME), ship(12, HAULER, ME)];
        assert_eq!(
            hold_for(Some(&c), Some(11), RAITARU, &rows, ME, &mind.refused),
            Hold::NoHauler
        );
        // And the planner stops after MAX_OTHER, with a home too.
        let mut f = base();
        f.home = Some(HOME);
        f.others = MAX_OTHER;
        assert!(matches!(plan(&f), Step::GiveUp(_)));
    }

    #[test]
    fn a_refused_structure_undock_keeps_the_pass_active_until_max_other() {
        // Landing edit H: a bot refused the undock is still inside the
        // structure, so the pass stays on; the count still ends it.
        let mut mind = Mind {
            active: true,
            ..Mind::default()
        };
        refuse(&mut mind, &Step::UndockStructure);
        assert!(mind.active, "a refused UndockStructure dropped the pass");
        assert_eq!(mind.others, 1);
        for _ in 1..MAX_OTHER {
            refuse(&mut mind, &Step::UndockStructure);
        }
        assert_eq!(mind.others, MAX_OTHER);
        let mut f = base();
        f.home = Some(HOME);
        f.others = mind.others;
        assert!(matches!(plan(&f), Step::GiveUp(_)));
    }

    #[test]
    fn the_flown_ship_is_boarded_again_once_the_hull_is_anchored() {
        // Finding D.
        let mut f = base();
        f.home = Some(HOME);
        f.system = Some(HOME.system);
        f.dockable = true;
        assert_eq!(plan(&f), Step::Undock);
        f.reboard = Some(11);
        assert_eq!(plan(&f), Step::Board { ship: 11 });
        // A refusal of that board forgets the ship instead of asking again.
        let mut mind = Mind {
            prior_ship: Some(11),
            ..Mind::default()
        };
        refuse(&mut mind, &Step::Board { ship: 11 });
        assert_eq!(mind.prior_ship, None);
        assert!(mind.refused.is_empty());
    }
}
